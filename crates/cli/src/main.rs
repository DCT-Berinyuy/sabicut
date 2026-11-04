//! `sabicut`: validate edit plans and inspect the output timeline they produce,
//! transcribe speech to word timings, apply transcript fixes, and preview and render
//! plans.

mod analyze;
mod files;
mod fix;
mod render;

use std::fmt::Display;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use sabicut_core::{Fixes, ParseError, Plan, Timeline, Transcript, ValidationErrors};

/// Plans are small text files; anything bigger is almost certainly the wrong file.
const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;

/// Sabicut, an offline video studio for communities.
///
/// This version works with edit plans (plan.toml): it validates them, prints their
/// JSON Schema, and prints the output timeline they produce. It also transcribes
/// speech to word timings (words.json), applies transcript fixes (fixes.toml), writes
/// preview stills, and renders plans to a vertical MP4 with ffmpeg (captions come
/// later).
#[derive(Debug, Parser)]
#[command(name = "sabicut", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check a plan.toml against schema v1 and report every problem
    Validate {
        /// Path to the plan file
        plan: PathBuf,
    },
    /// Print the JSON Schema of a file format (plan.toml by default)
    Schema {
        /// Which file format
        #[arg(value_enum, default_value_t = SchemaFormat::Plan)]
        format: SchemaFormat,
    },
    /// Print the output timeline derived from a plan's cuts
    Timeline {
        /// Path to the plan file
        plan: PathBuf,
    },
    /// Transcribe the speech in a video or audio file to word timings (words.json)
    Analyze(analyze::AnalyzeArgs),
    /// Apply transcript fixes (fixes.toml) to word timings and show what changes
    Fix(fix::FixArgs),
    /// Write PNG stills at chosen output times, and a contact sheet of them
    Preview(render::PreviewArgs),
    /// Render a plan to a vertical MP4 and a cover JPG with ffmpeg
    Render(render::RenderArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SchemaFormat {
    /// Edit plans, plan.toml (schema v1)
    Plan,
    /// Word timings, words.json (schema v1)
    Words,
    /// Transcript fixes, fixes.toml (schema v1)
    Fixes,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("{}: file is larger than {} MiB, so it is not a plan", .path.display(), MAX_PLAN_BYTES / 1024 / 1024)]
    TooLarge { path: PathBuf },
    #[error("{}: {source}", .path.display())]
    Parse { path: PathBuf, source: ParseError },
    #[error("{}: {}\n{}", .path.display(), count(.errors.as_slice().len(), "problem"), bullets(.errors))]
    Invalid {
        path: PathBuf,
        errors: ValidationErrors,
    },
    #[error(transparent)]
    Tool(#[from] sabicut_render::ToolError),
    #[error(transparent)]
    Render(#[from] sabicut_render::RenderError),
    #[error("cannot write output: {0}")]
    Write(#[from] io::Error),
    #[error(transparent)]
    Analyze(#[from] analyze::Error),
    #[error(transparent)]
    Fix(#[from] fix::Error),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        // The reader went away (for example `sabicut schema | head`); nothing to report.
        Err(CliError::Write(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            // If stderr is gone too there is nobody left to tell.
            let _ = writeln!(io::stderr(), "error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &Command) -> Result<(), CliError> {
    let mut out = io::stdout().lock();
    match command {
        Command::Validate { plan } => {
            let timeline = load(plan)?;
            writeln!(
                out,
                "ok: {} is a valid plan ({}, {} = {:.3} s at {} fps)",
                plan.display(),
                count(timeline.segments().len(), "cut"),
                count(timeline.total_frames(), "frame"),
                timeline.duration().to_f64(),
                timeline.fps(),
            )?;
        }
        Command::Schema { format } => {
            let schema = match format {
                SchemaFormat::Plan => sabicut_core::json_schema(),
                SchemaFormat::Words => Transcript::json_schema(),
                SchemaFormat::Fixes => Fixes::json_schema(),
            };
            out.write_all(schema.as_bytes())?;
        }
        Command::Timeline { plan } => {
            let timeline = load(plan)?;
            write_timeline(&mut out, plan, &timeline)?;
        }
        Command::Analyze(args) => analyze::run(args, &mut out)?,
        Command::Fix(args) => fix::run(args, &mut out)?,
        Command::Preview(args) => render::preview(args, &mut out)?,
        Command::Render(args) => render::render(args, &mut out)?,
    }
    out.flush()?;
    Ok(())
}

/// Reads, parses, and validates a plan file.
fn load(path: &Path) -> Result<Timeline, CliError> {
    load_plan(path).map(|(_, timeline)| timeline)
}

/// Reads, parses, and validates a plan file, keeping the plan.
fn load_plan(path: &Path) -> Result<(Plan, Timeline), CliError> {
    let read_error = |source| CliError::Read {
        path: path.to_owned(),
        source,
    };
    let mut text = String::new();
    File::open(path)
        .map_err(read_error)?
        .take(MAX_PLAN_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(read_error)?;
    if text.len() > MAX_PLAN_BYTES {
        return Err(CliError::TooLarge {
            path: path.to_owned(),
        });
    }
    let plan = Plan::from_toml_str(&text).map_err(|source| CliError::Parse {
        path: path.to_owned(),
        source,
    })?;
    let timeline = Timeline::from_plan(&plan).map_err(|errors| CliError::Invalid {
        path: path.to_owned(),
        errors,
    })?;
    Ok((plan, timeline))
}

fn write_timeline(out: &mut impl Write, path: &Path, timeline: &Timeline) -> io::Result<()> {
    let secs = |frame: u64| timeline.frame_time(frame).to_f64();
    let total = timeline.total_frames();
    writeln!(
        out,
        "{}: {}, {} ({:.3} s) at {} fps",
        path.display(),
        count(timeline.segments().len(), "cut"),
        count(total, "frame"),
        secs(total),
        timeline.fps(),
    )?;
    writeln!(out)?;

    let id_width = timeline
        .segments()
        .iter()
        .map(|segment| segment.source().len())
        .chain(["source".len()])
        .max()
        .unwrap_or_default();
    writeln!(
        out,
        "cut  {:<id_width$}  {:>21}  {:>21}  {:>6}  output frames",
        "source", "source time (s)", "output time (s)", "frames",
    )?;
    for (i, segment) in timeline.segments().iter().enumerate() {
        writeln!(
            out,
            "{i:>3}  {:<id_width$}  {:>9.3} - {:>9.3}  {:>9.3} - {:>9.3}  {:>6}  {}..{}",
            segment.source(),
            secs(segment.source_in()),
            secs(segment.source_out()),
            secs(segment.output_start()),
            secs(segment.output_end()),
            segment.frames(),
            segment.output_start(),
            segment.output_end(),
        )?;
        for zoom in segment.zooms() {
            writeln!(
                out,
                "     zoom x{:.2} at source {:.3} s -> output {:.3} s (frame {})",
                zoom.scale(),
                secs(zoom.source_frame()),
                secs(zoom.output_frame()),
                zoom.output_frame(),
            )?;
        }
    }
    if let Some(frames) = timeline.hook_frames() {
        writeln!(out)?;
        writeln!(
            out,
            "hook: output 0.000 - {:.3} s (frames 0..{frames})",
            secs(frames)
        )?;
    }
    Ok(())
}

/// "1 cut", "3 cuts".
fn count<T: Display + PartialEq + From<u8>>(n: T, noun: &str) -> String {
    if n == T::from(1) {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn bullets(errors: &ValidationErrors) -> String {
    errors
        .iter()
        .map(|error| format!("  - {error}"))
        .collect::<Vec<_>>()
        .join("\n")
}
