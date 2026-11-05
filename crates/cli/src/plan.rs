//! `sabicut plan`: turn the word timings of one or more clips into an edit plan
//! (plan.toml) that cuts pauses and false starts and zooms in on keywords.

use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use sabicut_plan::{
    CaptionStyle, DeadAir, FalseStarts, HookText, Options, PlanError, Reason, Report, SourceInput,
    Zooms, source_id,
};
use sabicut_render::{Ffmpeg, ProbeError, ToolError, probe};

use crate::files::{self, FileError};
use crate::fix;
use crate::{CliError, count};

#[derive(Debug, Args)]
pub(crate) struct PlanArgs {
    /// Video files, in the order they play. Run `sabicut analyze` on each one first
    #[arg(required = true, value_name = "MEDIA")]
    media: Vec<PathBuf>,
    /// Where to write the plan, or "-" for standard output
    #[arg(short, long, value_name = "FILE", default_value = "plan.toml")]
    out: PathBuf,
    /// Replace the plan if it exists. Plans are made to be edited by hand, so `plan`
    /// does not overwrite one unless asked
    #[arg(long)]
    force: bool,
    /// Word timings of each media file, in the same order, separated by commas or given
    /// once per file [default: <name>.words.json next to each media file]
    #[arg(long, value_name = "FILE", value_delimiter = ',')]
    words: Vec<PathBuf>,
    /// Transcript fixes (fixes.toml): applied to the words before planning, and named in
    /// the plan's captions
    #[arg(long, value_name = "FILE")]
    fixes: Option<PathBuf>,
    /// Names and terms, separated by commas. Each gets a punch-in zoom where it is said,
    /// and captions draw it in the active colour
    #[arg(long, value_delimiter = ',', value_name = "WORDS")]
    keywords: Vec<String>,
    /// Hook text shown over the opening seconds
    #[arg(long, value_name = "TEXT")]
    hook: Option<String>,
    /// How long the hook shows, in seconds
    #[arg(long, value_name = "SECS", default_value_t = HookText::DEFAULT_DUR, value_parser = positive_seconds)]
    hook_dur: f64,
    /// Centre of the 9:16 crop, from 0 (left edge) to 1 (right edge): once for every
    /// clip, or once per media file [default: 0.5]
    #[arg(long, value_name = "X", value_delimiter = ',', value_parser = unit)]
    crop_x: Vec<f64>,
    /// Output frame rate; cuts land on its frame boundaries
    #[arg(long, value_name = "FPS", default_value_t = Options::default().fps,
          value_parser = clap::value_parser!(u32).range(1..=240))]
    fps: u32,
    /// Pauses longer than this are cut, in seconds
    #[arg(long, value_name = "SECS", default_value_t = DeadAir::default().max_pause, value_parser = seconds)]
    max_pause: f64,
    /// Silence kept ahead of speech at a cut, in seconds
    #[arg(long, value_name = "SECS", default_value_t = DeadAir::default().pad_before, value_parser = seconds)]
    pad_before: f64,
    /// Silence kept after speech at a cut, in seconds
    #[arg(long, value_name = "SECS", default_value_t = DeadAir::default().pad_after, value_parser = seconds)]
    pad_after: f64,
    /// Only cut a pause if that removes at least this much, in seconds
    #[arg(long, value_name = "SECS", default_value_t = DeadAir::default().min_cut, value_parser = seconds)]
    min_cut: f64,
    /// Drop kept sections shorter than this, in seconds
    #[arg(long, value_name = "SECS", default_value_t = DeadAir::default().min_keep, value_parser = seconds)]
    min_keep: f64,
    /// How far speech may run past its words to the edge of its speech segment, in seconds
    #[arg(long, value_name = "SECS", default_value_t = DeadAir::default().vad_reach,
          value_parser = seconds, hide = true)]
    vad_reach: f64,
    /// Keep false starts (phrases broken off and said again)
    #[arg(long)]
    no_false_starts: bool,
    /// The fewest repeated words that make a false start
    #[arg(long, value_name = "N", default_value_t = 2,
          value_parser = clap::value_parser!(u16).range(1..=32))]
    false_start_words: u16,
    /// Scale of the punch-in zoom on keywords, from 1 to 4
    #[arg(long, value_name = "SCALE", default_value_t = 1.25, value_parser = scale)]
    zoom: f64,
    /// No zooms on keywords
    #[arg(long)]
    no_zoom: bool,
    /// Zoom back out this long after each keyword, in seconds [default: hold the zoom to
    /// the end of the cut]
    #[arg(long, value_name = "SECS", value_parser = seconds)]
    zoom_hold: Option<f64>,
    /// Zoom every second cut to this scale for its whole length, which hides jump cuts
    #[arg(long, value_name = "SCALE", value_parser = scale)]
    cut_zoom: Option<f64>,
    /// Leave captions out of the plan
    #[arg(long)]
    no_captions: bool,
    /// Caption font file, relative to the plan [default: the bundled Anton font]
    #[arg(long, value_name = "FILE")]
    font: Option<String>,
    /// Colour of the word being spoken and of keywords, as #RRGGBB [default: #FFC400]
    #[arg(long, value_name = "COLOUR", value_parser = colour)]
    active: Option<String>,
    /// Outline width around caption text, in pixels [default: 10]
    #[arg(long, value_name = "PX", value_parser = clap::value_parser!(u32).range(0..=64))]
    stroke: Option<u32>,
    /// Warn when the output is longer than this, in seconds
    #[arg(long, value_name = "SECS", default_value_t = 60.0, value_parser = seconds)]
    target: f64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    File(#[from] FileError),
    #[error(transparent)]
    Fix(#[from] fix::Error),
    #[error(
        "{} already exists. Plans are made to be edited by hand, so `sabicut plan` does not \
         replace one: pass --force to replace it, or -o to write another file",
        .0.display()
    )]
    Exists(PathBuf),
    #[error("{}: {source}", .path.display())]
    Media {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "{}: no word timings for {}. Run `sabicut analyze {}` first, or name the file with --words",
        .words.display(), .media.display(), .media.display()
    )]
    NoWords { media: PathBuf, words: PathBuf },
    #[error("--{flag}: {given} given for {media} media files; give it {hint}")]
    Count {
        flag: &'static str,
        given: usize,
        media: usize,
        hint: &'static str,
    },
    #[error("{}: the plan can only hold paths written in UTF-8", .0.display())]
    NotUtf8(PathBuf),
    #[error("cannot read where each clip's video ends: {0}")]
    Tools(#[source] ToolError),
    #[error("{}: cannot read where its video ends: {source}", .path.display())]
    Probe {
        path: PathBuf,
        source: Box<ProbeError>,
    },
    #[error(transparent)]
    Plan(#[from] PlanError),
}

pub(crate) fn run(args: &PlanArgs, out: &mut impl Write) -> Result<(), CliError> {
    let to_stdout = args.out.as_os_str() == "-";
    check_counts(args)?;
    if !to_stdout {
        if args.out.exists() && !args.force {
            return Err(Error::Exists(args.out.clone()).into());
        }
        files::check_folder(&args.out).map_err(Error::from)?;
    }
    let plan_folder = match args.out.parent() {
        Some(parent) if !to_stdout && !parent.as_os_str().is_empty() => parent.to_owned(),
        _ => PathBuf::from("."),
    };
    let fixes = match &args.fixes {
        Some(path) => Some((path, fix::load_fixes(path)?)),
        None => None,
    };

    let mut clips = Vec::with_capacity(args.media.len());
    for (index, media) in args.media.iter().enumerate() {
        fs::metadata(media).map_err(|source| Error::Media {
            path: media.clone(),
            source,
        })?;
        let words = args
            .words
            .get(index)
            .cloned()
            .unwrap_or_else(|| media.with_extension("words.json"));
        if !words.exists() {
            return Err(Error::NoWords {
                media: media.clone(),
                words,
            }
            .into());
        }
        let mut transcript = fix::load_words(&words)?;
        if let Some((path, fixes)) = &fixes {
            transcript = fix::apply(&transcript, fixes, path)?.into_transcript();
        }
        clips.push(SourceInput {
            id: source_id(index),
            path: relative(&plan_folder, media)?,
            words_path: relative(&plan_folder, &words)?,
            transcript,
            crop_x: crop_x(args, index),
            video_end: None,
        });
    }
    let sources = with_video_ends(&args.media, clips)?;

    let options = options(args, &plan_folder)?;
    let planned = sabicut_plan::plan(&sources, &options).map_err(Error::from)?;
    let destination = if to_stdout {
        out.write_all(planned.toml().as_bytes())?;
        "standard output".to_owned()
    } else {
        files::write_atomic(&args.out, planned.toml()).map_err(Error::from)?;
        args.out.display().to_string()
    };
    let summary = summary(&destination, &sources, planned.report());
    if to_stdout {
        eprint!("{summary}");
    } else {
        out.write_all(summary.as_bytes())?;
    }
    Ok(())
}

/// The clips, each with where its video ends, read with ffprobe. Phones often stop the
/// video a little before the audio, and no cut may go past the video's last frame. A
/// file without video, such as the benchmark's WAV readings (docs/bench/plan.md), has no
/// frames to run past, so its cuts run to the end of its audio as before.
fn with_video_ends(media: &[PathBuf], clips: Vec<SourceInput>) -> Result<Vec<SourceInput>, Error> {
    let tools = Ffmpeg::from_env().map_err(Error::Tools)?;
    media
        .iter()
        .zip(clips)
        .map(|(path, clip)| {
            let video_end = match probe(&tools, path) {
                Ok(info) => info.video_end,
                Err(ProbeError::NoVideo) => None,
                Err(source) => {
                    return Err(Error::Probe {
                        path: path.clone(),
                        source: Box::new(source),
                    });
                }
            };
            Ok(SourceInput { video_end, ..clip })
        })
        .collect()
}

fn check_counts(args: &PlanArgs) -> Result<(), Error> {
    let media = args.media.len();
    if !args.words.is_empty() && args.words.len() != media {
        return Err(Error::Count {
            flag: "words",
            given: args.words.len(),
            media,
            hint: "once per media file",
        });
    }
    if args.crop_x.len() > 1 && args.crop_x.len() != media {
        return Err(Error::Count {
            flag: "crop-x",
            given: args.crop_x.len(),
            media,
            hint: "once for all of them, or once per media file",
        });
    }
    Ok(())
}

fn crop_x(args: &PlanArgs, index: usize) -> f64 {
    match args.crop_x.as_slice() {
        [] => 0.5,
        [all] => *all,
        each => each.get(index).copied().unwrap_or(0.5),
    }
}

fn options(args: &PlanArgs, plan_folder: &Path) -> Result<Options, Error> {
    let fixes = match &args.fixes {
        Some(path) => Some(relative(plan_folder, path)?),
        None => None,
    };
    Ok(Options {
        fps: args.fps,
        dead_air: DeadAir {
            max_pause: args.max_pause,
            pad_before: args.pad_before,
            pad_after: args.pad_after,
            vad_reach: args.vad_reach,
            min_cut: args.min_cut,
            min_keep: args.min_keep,
        },
        false_starts: (!args.no_false_starts).then(|| {
            let min_words = usize::from(args.false_start_words);
            FalseStarts {
                min_words,
                max_words: FalseStarts::default().max_words.max(min_words),
                ..FalseStarts::default()
            }
        }),
        zooms: Zooms {
            keyword_scale: (!args.no_zoom).then_some(args.zoom),
            hold: args.zoom_hold,
            alternate: args.cut_zoom,
        },
        keywords: args
            .keywords
            .iter()
            .map(|keyword| keyword.trim())
            .filter(|keyword| !keyword.is_empty())
            .map(str::to_owned)
            .collect(),
        hook: args.hook.as_ref().map(|text| HookText {
            text: text.clone(),
            dur: args.hook_dur,
        }),
        captions: (!args.no_captions).then(|| CaptionStyle {
            fixes,
            font: args.font.clone(),
            active: args.active.clone(),
            stroke: args.stroke,
        }),
        target: Some(args.target),
        ..Options::default()
    })
}

/// `path` relative to `folder`, as the plan writes it. Both must exist.
fn relative(folder: &Path, path: &Path) -> Result<String, Error> {
    let canonical = |p: &Path| {
        fs::canonicalize(p).map_err(|source| Error::Media {
            path: p.to_owned(),
            source,
        })
    };
    let (folder_abs, path_abs) = (canonical(folder)?, canonical(path)?);
    let common = folder_abs
        .components()
        .zip(path_abs.components())
        .take_while(|(a, b)| a == b)
        .count();
    let relative: PathBuf = if common == 0 {
        path_abs.clone()
    } else {
        folder_abs
            .components()
            .skip(common)
            .map(|_| Path::new(".."))
            .chain(
                path_abs
                    .components()
                    .skip(common)
                    .map(|c| Path::new(c.as_os_str())),
            )
            .collect()
    };
    relative
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::NotUtf8(path.to_owned()))
}

/// What was written and what the rules did.
fn summary(destination: &str, sources: &[SourceInput], report: &Report) -> String {
    let mut text = format!(
        "wrote {destination}: {}, {:.3} s at {} fps, from {:.3} s of {}\n",
        count(sources_cuts(report), "cut"),
        report.output_seconds(),
        report.fps,
        report.input_seconds(),
        count(sources.len(), "clip"),
    );
    let width = sources.iter().map(|s| s.path.len()).max().unwrap_or(0);
    for (source, numbers) in sources.iter().zip(&report.sources) {
        let _ = writeln!(
            text,
            "  {:<3} {:<width$}  {:>9.3} s -> {:>9.3} s in {}",
            source.id,
            source.path,
            numbers.duration,
            numbers.kept,
            count(numbers.cuts, "cut"),
        );
    }
    let false_starts: Vec<_> = report
        .removals
        .iter()
        .filter(|removal| removal.reason == Reason::FalseStart)
        .collect();
    let short = report
        .removals
        .iter()
        .filter(|removal| removal.reason == Reason::Short)
        .count();
    let _ = writeln!(
        text,
        "  cut: pauses {:.1} s, false starts {:.1} s ({}), too short {:.1} s ({})",
        report.removed(Reason::Pause),
        report.removed(Reason::FalseStart),
        false_starts.len(),
        report.removed(Reason::Short),
        short,
    );
    if !false_starts.is_empty() {
        let _ = writeln!(
            text,
            "  false starts cut (each has a \"# removed\" line in the plan, to bring it back):"
        );
        for removal in false_starts {
            let _ = writeln!(
                text,
                "    {:<3} {:>9.3} - {:>9.3}  \"{}\"",
                removal.source, removal.start, removal.end, removal.text
            );
        }
    }
    if !report.kept_false_starts.is_empty() {
        let _ = writeln!(
            text,
            "  false starts kept, because there is no pause to cut at:"
        );
        for span in &report.kept_false_starts {
            let _ = writeln!(
                text,
                "    {:<3} {:>9.3} - {:>9.3}  \"{}\"",
                span.source, span.start, span.end, span.text
            );
        }
    }
    let _ = writeln!(
        text,
        "  {}; cut points inside a word: {} of {}",
        count(report.zooms, "zoom"),
        report.mid_word,
        report.cut_points
    );
    for warning in &report.warnings {
        let _ = writeln!(text, "warning: {warning}");
    }
    text
}

fn sources_cuts(report: &Report) -> usize {
    report.sources.iter().map(|source| source.cuts).sum()
}

fn parse(text: &str) -> Result<f64, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{text:?} is not a number"))
}

fn seconds(text: &str) -> Result<f64, String> {
    let value = parse(text)?;
    if (0.0..=3600.0).contains(&value) {
        Ok(value)
    } else {
        Err(format!("must be between 0 and 3600 seconds, got {text}"))
    }
}

fn positive_seconds(text: &str) -> Result<f64, String> {
    let value = seconds(text)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(format!("must be more than 0 seconds, got {text}"))
    }
}

fn unit(text: &str) -> Result<f64, String> {
    let value = parse(text)?;
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(format!("must be between 0 and 1, got {text}"))
    }
}

fn colour(text: &str) -> Result<String, String> {
    let hex = text
        .strip_prefix('#')
        .is_some_and(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
    if hex {
        Ok(text.to_owned())
    } else {
        Err(format!("must be a colour like #FFC400, got {text:?}"))
    }
}

fn scale(text: &str) -> Result<f64, String> {
    let value = parse(text)?;
    if (1.0..=4.0).contains(&value) {
        Ok(value)
    } else {
        Err(format!("must be between 1 and 4, got {text}"))
    }
}
