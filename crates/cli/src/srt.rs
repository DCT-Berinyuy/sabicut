//! `sabicut srt`: write a plan's subtitles (SRT) without rendering, for example after
//! fixing the transcript.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use sabicut_overlay::{place_words, srt};

use crate::files::{self, FileError};
use crate::render::plan_dir;
use crate::{CliError, count, load_plan};

#[derive(Debug, Args)]
pub(crate) struct SrtArgs {
    /// Path to the plan file
    plan: PathBuf,
    /// Where to write the subtitles, or "-" for standard output [default: the plan's
    /// path, ending in .srt]
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    File(#[from] FileError),
    #[error(
        "{}: no word timings for source `{source_id}`; set `words` on its [[source]] \
         (or in [captions] when the plan has one source)",
        .plan.display()
    )]
    NoWords { plan: PathBuf, source_id: String },
}

pub(crate) fn run(args: &SrtArgs, out: &mut impl Write) -> Result<(), CliError> {
    let (plan, timeline) = load_plan(&args.plan)?;
    let words = sabicut_overlay::load_word_timings(&plan, &timeline, &plan_dir(&args.plan))?;
    let placed = place_words(&timeline, &words, &[]).ok_or_else(|| {
        let source_id = timeline
            .segments()
            .iter()
            .map(|segment| segment.source())
            .find(|id| words.get(id).is_none())
            .unwrap_or_default()
            .to_owned();
        Error::NoWords {
            plan: args.plan.clone(),
            source_id,
        }
    })?;
    let fps = timeline.fps();
    let cues = srt::cues(&placed, fps, timeline.total_frames());
    let text = srt::to_srt(&cues, fps);
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.plan.with_extension("srt"));
    if output == Path::new("-") {
        out.write_all(text.as_bytes())?;
        return Ok(());
    }
    files::write_atomic(&output, &text).map_err(Error::from)?;
    writeln!(
        out,
        "ok: wrote {} ({}, {})",
        output.display(),
        count(cues.len(), "cue"),
        count(placed.len(), "word")
    )?;
    Ok(())
}
