//! `sabicut render` and `sabicut preview`.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use sabicut_core::Plan;
use sabicut_render::{
    Ffmpeg, OpenError, Preset, PreviewOptions, ProbedSource, Progress, Project, RenderOptions,
};

use crate::{CliError, count, load_plan};

#[derive(Debug, Args)]
pub(crate) struct RenderArgs {
    /// Path to the plan file
    plan: PathBuf,
    /// Where to write the video [default: the plan's path, ending in .mp4]
    #[arg(short, long, value_name = "MP4")]
    output: Option<PathBuf>,
    /// Where to write the cover [default: the video's path, ending in .jpg]
    #[arg(long, value_name = "JPG")]
    cover: Option<PathBuf>,
    /// Output time of the cover frame, in seconds [default: 1, or the middle of a
    /// shorter video]
    #[arg(long, value_name = "SECONDS")]
    cover_at: Option<f64>,
    /// Do not write a cover
    #[arg(long, conflicts_with_all = ["cover", "cover_at"])]
    no_cover: bool,
    /// x264 speed preset: medium for personal use, veryfast on a hub
    #[arg(long, default_value_t = Preset::Medium, value_name = "PRESET")]
    preset: Preset,
}

#[derive(Debug, Args)]
pub(crate) struct PreviewArgs {
    /// Path to the plan file
    plan: PathBuf,
    /// Output times of the stills, in seconds, separated by commas: --at 1.5,8,22
    #[arg(long, required = true, value_delimiter = ',', num_args = 1.., value_name = "SECONDS")]
    at: Vec<f64>,
    /// Directory for the stills and the contact sheet [default: <plan>-preview next to
    /// the plan]
    #[arg(short, long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
    /// Shade the areas TikTok, Reels and Shorts cover with their buttons and captions
    #[arg(long)]
    safe_zones: bool,
}

pub(crate) fn render(args: &RenderArgs, out: &mut impl Write) -> Result<(), CliError> {
    let (plan, _) = load_plan(&args.plan)?;
    let tools = Ffmpeg::from_env()?;
    let project = open(&tools, &plan, &args.plan)?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.plan.with_extension("mp4"));
    let cover = (!args.no_cover).then(|| {
        args.cover
            .clone()
            .unwrap_or_else(|| output.with_extension("jpg"))
    });
    let mut log = io::stderr().lock();
    describe_sources(&mut log, &project)?;
    let options = RenderOptions {
        output,
        cover,
        cover_at: args.cover_at,
        preset: args.preset,
        overlays: Vec::new(),
    };
    let mut reporter = Reporter::new(log);
    let report = sabicut_render::render(&tools, &project, &options, &mut |progress| {
        reporter.report(progress);
    })?;
    reporter.finish();

    let timeline = project.timeline();
    let (w, h) = project.out();
    let loudness = if report.loudness.is_silent() {
        "silent audio".to_owned()
    } else {
        format!("{:.1} LUFS", report.loudness.integrated)
    };
    writeln!(
        out,
        "ok: wrote {} ({} = {:.3} s at {} fps, {w}x{h}, {loudness})",
        report.output.display(),
        count(report.frames, "frame"),
        timeline.duration().to_f64(),
        timeline.fps(),
    )?;
    if let Some((path, frame)) = &report.cover {
        writeln!(
            out,
            "ok: wrote {} (cover: frame {frame}, {:.3} s)",
            path.display(),
            timeline.frame_time(*frame).to_f64(),
        )?;
    }
    note_overlays(out, &plan)?;
    Ok(())
}

pub(crate) fn preview(args: &PreviewArgs, out: &mut impl Write) -> Result<(), CliError> {
    let (plan, _) = load_plan(&args.plan)?;
    let tools = Ffmpeg::from_env()?;
    let project = open(&tools, &plan, &args.plan)?;
    let out_dir = args.out_dir.clone().unwrap_or_else(|| {
        let stem = args.plan.file_stem().unwrap_or_default().to_string_lossy();
        plan_dir(&args.plan).join(format!("{stem}-preview"))
    });
    let options = PreviewOptions {
        times: args.at.clone(),
        out_dir,
        safe_zones: args.safe_zones,
    };
    let report = sabicut_render::preview(&tools, &project, &options)?;
    let timeline = project.timeline();
    for still in &report.stills {
        writeln!(
            out,
            "ok: wrote {} (frame {} at {:.3} s)",
            still.path.display(),
            still.frame,
            timeline.frame_time(still.frame).to_f64(),
        )?;
    }
    writeln!(
        out,
        "ok: wrote {} ({})",
        report.contact_sheet.display(),
        count(report.stills.len(), "still"),
    )?;
    note_overlays(out, &plan)?;
    Ok(())
}

/// The directory relative plan paths are resolved from.
fn plan_dir(plan: &Path) -> PathBuf {
    match plan.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_owned(),
        _ => PathBuf::from("."),
    }
}

fn open(tools: &Ffmpeg, plan: &Plan, path: &Path) -> Result<Project, CliError> {
    Project::open(tools, plan, &plan_dir(path)).map_err(|error| match error {
        OpenError::Invalid(errors) => CliError::Invalid {
            path: path.to_owned(),
            errors,
        },
        OpenError::Render(error) => CliError::Render(error),
    })
}

fn describe_sources(log: &mut impl Write, project: &Project) -> io::Result<()> {
    for source in project.sources() {
        writeln!(log, "{}", describe(source))?;
    }
    Ok(())
}

fn describe(source: &ProbedSource) -> String {
    let info = source.info();
    let (w, h) = info.display_size();
    let crop = source.crop();
    let mut notes = Vec::new();
    if info.quarter_turns % 2 == 1 {
        notes.push("rotated".to_owned());
    }
    if let Some(transfer) = info.hdr() {
        notes.push(format!(
            "HDR ({}), tone-mapped to SDR",
            transfer.ffmpeg_name()
        ));
    }
    if info.is_variable_frame_rate() {
        notes.push("variable frame rate".to_owned());
    }
    if !info.has_audio {
        notes.push("no audio (silent)".to_owned());
    }
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!(", {}", notes.join(", "))
    };
    format!(
        "source {}: {}, {w}x{h} {}{notes}; crop {}x{} at x {}, y {}",
        source.id(),
        source.path().display(),
        info.codec,
        crop.w,
        crop.h,
        crop.x,
        crop.y,
    )
}

/// Tells the user that hook and caption settings are not drawn yet.
fn note_overlays(out: &mut impl Write, plan: &Plan) -> io::Result<()> {
    let mut skipped = Vec::new();
    if plan.hook().is_some() {
        skipped.push("[hook]");
    }
    if plan.captions().is_some() {
        skipped.push("[captions]");
    }
    if !skipped.is_empty() {
        writeln!(
            out,
            "note: {} not drawn yet; captions and the hook come in a later version",
            skipped.join(" and "),
        )?;
    }
    Ok(())
}

/// Prints render progress on stderr: a live percentage on a terminal, otherwise a line
/// every 10%.
struct Reporter<W: Write> {
    log: W,
    live: bool,
    printed_percent: Option<u64>,
}

impl<W: Write> Reporter<W> {
    fn new(log: W) -> Self {
        let live = io::stderr().is_terminal();
        Self {
            log,
            live,
            printed_percent: None,
        }
    }

    fn report(&mut self, progress: Progress) {
        // Progress is a courtesy: if stderr is gone, keep rendering.
        let _ = match progress {
            Progress::Waiting => writeln!(
                self.log,
                "waiting for another sabicut job (a transcription or a render) to finish..."
            ),
            Progress::LockUnavailable(error) => writeln!(
                self.log,
                "note: cannot take the one-job-at-a-time lock ({error}); rendering anyway"
            ),
            Progress::Audio => writeln!(self.log, "cutting and cleaning up the audio..."),
            Progress::Loudness => writeln!(self.log, "measuring loudness..."),
            Progress::Video { done, total } => self.video(done, total),
            Progress::Check => {
                self.end_live_line();
                writeln!(self.log, "checking the result...")
            }
            Progress::Cover => writeln!(self.log, "writing the cover..."),
        };
    }

    fn video(&mut self, done: u64, total: u64) -> io::Result<()> {
        let percent = done.saturating_mul(100) / total.max(1);
        if self.live {
            write!(
                self.log,
                "\rencoding video: {percent:>3}% ({done}/{total} frames)"
            )?;
            self.printed_percent = Some(percent);
            return self.log.flush();
        }
        if self.printed_percent.is_none_or(|last| percent >= last + 10) {
            self.printed_percent = Some(percent - percent % 10);
            writeln!(
                self.log,
                "encoding video: {percent}% ({done}/{total} frames)"
            )?;
        }
        Ok(())
    }

    fn end_live_line(&mut self) {
        if self.live && self.printed_percent.is_some() {
            let _ = writeln!(self.log);
            self.printed_percent = None;
        }
    }

    fn finish(&mut self) {
        self.end_live_line();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_terminal_progress_prints_every_ten_percent() {
        let mut reporter = Reporter {
            log: Vec::new(),
            live: false,
            printed_percent: None,
        };
        for done in 0..=100 {
            reporter.report(Progress::Video { done, total: 100 });
        }
        let text = String::from_utf8(reporter.log).unwrap();
        assert_eq!(text.lines().count(), 11, "{text}");
        assert!(
            text.ends_with("encoding video: 100% (100/100 frames)\n"),
            "{text}"
        );
    }

    #[test]
    fn plan_paths_resolve_from_the_plan_directory() {
        assert_eq!(plan_dir(Path::new("plan.toml")), Path::new("."));
        assert_eq!(
            plan_dir(Path::new("shorts/a/plan.toml")),
            Path::new("shorts/a")
        );
    }
}
