//! Sabicut's rules engine (PRD feature F3): from the word timings of one or more clips
//! to an edit plan (`plan.toml`) that keeps the speech and cuts the rest.
//!
//! - **Pauses** longer than a threshold are cut, keeping some silence around the speech.
//!   Speech runs to the edges of its voice-activity segment, which finds where a voice
//!   starts and stops better than the speech model's word times do.
//! - **False starts**, a phrase broken off and said again ("So today we… So today we're
//!   going to…"), are cut, keeping the last take.
//! - **Every cut lands outside every word**, on a frame boundary of the output rate.
//! - **No cut runs past the end of a clip's video** ([`SourceInput::video_end`]), which
//!   may come before the end of its audio.
//! - **Keywords** get a punch-in zoom where they are said.
//!
//! The result is a [`sabicut_core::Plan`] that passes validation, the same plan as TOML
//! text with comments for hand-editing, and a [`Report`] of what was cut and why.
//! Nothing here reads or writes files; `sabicut plan` does that.
//!
//! ```
//! use sabicut_core::{SpeechSegment, Transcript, Word};
//! use sabicut_plan::{Options, SourceInput, plan};
//!
//! let transcript = Transcript::new(
//!     "clip.mp4", "en", "ggml-small-q5_1.bin", 9.0,
//!     vec![SpeechSegment::new(1.0, 2.6), SpeechSegment::new(5.0, 6.5)],
//!     vec![
//!         Word::new("Rust", 1.0, 1.4, 0.9), Word::new("is", 1.4, 1.6, 0.9),
//!         Word::new("fast.", 1.6, 2.6, 0.9), Word::new("Really.", 5.0, 6.5, 0.9),
//!     ],
//! )?;
//! let source = SourceInput {
//!     id: "a".into(),
//!     path: "clip.mp4".into(),
//!     words_path: "clip.words.json".into(),
//!     transcript,
//!     crop_x: 0.5,
//!     video_end: Some(9.0), // where ffprobe says the video ends
//! };
//! let options = Options { keywords: vec!["Rust".into()], ..Options::default() };
//! let planned = plan(&[source], &options)?;
//!
//! // The 2.4 s pause is cut down to 0.25 s of silence around the speech.
//! assert_eq!(planned.plan().cuts().len(), 2);
//! assert_eq!(planned.report().mid_word, 0);
//! assert!(planned.toml().contains("zoom = [{ at = 1.000, scale = 1.25 }]"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]
#![warn(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod check;
mod false_starts;
mod grid;
mod options;
mod sections;
mod text;
mod write;
mod zooms;

use std::ops::Range;
use std::path::Path;

use sabicut_core::{Plan, Timeline, Transcript, ValidationErrors};

pub use options::{CaptionStyle, DeadAir, FalseStarts, HookText, Options, Zooms};
pub use sections::Reason;

use grid::{Grid, micros};
use sections::{Section, SourceCuts};
use zooms::{Mention, ZoomDraft};

/// One clip to plan.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInput {
    /// Id that cuts use (`[[source]] id`): 1 to 64 letters, digits, `_` or `-`.
    pub id: String,
    /// Path of the clip as written in the plan, relative to the plan file.
    pub path: String,
    /// Path of its word timings as written in the plan, relative to the plan file.
    pub words_path: String,
    /// Its word timings, with any fixes already applied.
    pub transcript: Transcript,
    /// Horizontal centre of the 9:16 crop, from 0 to 1.
    pub crop_x: f64,
    /// Where the clip's video ends, in seconds from the start of the file, if known. No
    /// cut ends after it: phones often stop the video a little before the audio, and the
    /// word timings only say where the audio ends. `sabicut plan` reads it with ffprobe;
    /// `None` lets cuts run to the end of the audio.
    pub video_end: Option<f64>,
}

impl SourceInput {
    /// The last frame boundary that the clip's cuts can reach on `grid`.
    pub(crate) fn end_of_media(&self, grid: Grid) -> i64 {
        sections::end_of_media(self.transcript.duration(), self.video_end, grid)
    }
}

/// The id of source number `index` (from 0): "a" to "z", then "aa", "ab", and so on.
pub fn source_id(index: usize) -> String {
    let mut n = index;
    let mut letters = Vec::new();
    loop {
        letters.push(b'a' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_default()
}

/// A plan, the TOML text it was read from, and what the rules did.
#[derive(Clone, Debug)]
pub struct Planned {
    plan: Plan,
    toml: String,
    report: Report,
}

impl Planned {
    /// The plan. It passes validation.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The plan as `plan.toml` text, with comments for hand-editing.
    pub fn toml(&self) -> &str {
        &self.toml
    }

    /// What the rules kept and cut.
    pub fn report(&self) -> &Report {
        &self.report
    }
}

/// What the rules kept and cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Output frame rate.
    pub fps: u32,
    /// Length of the output, in frames.
    pub output_frames: u64,
    /// Each source, in order.
    pub sources: Vec<SourceReport>,
    /// Every range that was cut, in source order.
    pub removals: Vec<Removal>,
    /// False starts that were found but kept, because there is no pause to cut at.
    pub kept_false_starts: Vec<Span>,
    /// How many zooms the plan has.
    pub zooms: usize,
    /// How many cut points the plan has: each cut's `in` and `out`.
    pub cut_points: usize,
    /// Cut points that lie inside a word, its start and end included. The start of a
    /// clip and the end of its media do not count.
    pub mid_word: usize,
    /// Things worth a look.
    pub warnings: Vec<String>,
}

impl Report {
    /// Length of the output, in seconds.
    pub fn output_seconds(&self) -> f64 {
        self.output_frames as f64 / f64::from(self.fps.max(1))
    }

    /// Length of all the sources, in seconds.
    pub fn input_seconds(&self) -> f64 {
        // Folded from +0.0: an empty float sum is -0.0, which prints as "-0.0".
        self.sources
            .iter()
            .fold(0.0, |total, source| total + source.duration)
    }

    /// How much was cut for `reason`, in seconds.
    pub fn removed(&self, reason: Reason) -> f64 {
        self.removals
            .iter()
            .filter(|removal| removal.reason == reason)
            .fold(0.0, |total, removal| total + (removal.end - removal.start))
    }
}

/// What the rules did with one source.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceReport {
    /// The source's id.
    pub id: String,
    /// Length of its media, in seconds.
    pub duration: f64,
    /// How much of it is kept, in seconds.
    pub kept: f64,
    /// How many cuts it has.
    pub cuts: usize,
    /// How many words it has.
    pub words: usize,
}

/// A range of a source that was cut, in seconds of source time on its frame grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Removal {
    /// The source's id.
    pub source: String,
    /// Where the cut range starts.
    pub start: f64,
    /// Where it ends.
    pub end: f64,
    /// Why it was cut.
    pub reason: Reason,
    /// The words in it, if any.
    pub text: String,
}

/// Some words of a source, from the start of the first to the end of the last.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// The source's id.
    pub source: String,
    /// Where the first word starts, in seconds.
    pub start: f64,
    /// Where the last word ends, in seconds.
    pub end: f64,
    /// The words.
    pub text: String,
}

/// Why a plan could not be made.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// An option is out of range.
    #[error("{name} {message}")]
    Option {
        /// The option.
        name: &'static str,
        /// What is wrong with it.
        message: String,
    },
    /// A source cannot go in a plan.
    #[error("source {id:?}: {message}")]
    Source {
        /// The source's id.
        id: String,
        /// What is wrong with it.
        message: String,
    },
    /// A source's word timings break the words.json rules.
    #[error("source {id:?}: the word timings are not valid:\n{errors}")]
    Transcript {
        /// The source's id.
        id: String,
        /// Every problem.
        errors: ValidationErrors,
    },
    /// There is nothing to keep: no source has words, or every section is too short.
    #[error("there is nothing to keep: no clip has speech that lasts long enough")]
    NothingToKeep,
    /// The plan came out invalid. This is a bug.
    #[error("the plan came out invalid, which is a bug in sabicut plan:\n{0}")]
    Internal(String),
}

/// A cut of the plan: a kept section of source number `source`, and its zooms.
#[derive(Clone, Debug)]
pub(crate) struct CutDraft {
    pub source: usize,
    pub section: Section,
    pub zooms: Vec<ZoomDraft>,
}

/// Plans the clips in `sources`, in order: cuts follow the order of the sources, and
/// within each source the order of time.
pub fn plan(sources: &[SourceInput], options: &Options) -> Result<Planned, PlanError> {
    options
        .check()
        .map_err(|(name, message)| PlanError::Option { name, message })?;
    check_sources(sources)?;
    let grid = Grid::new(options.fps);
    let keywords = zooms::keywords(&options.keywords);

    let mut plans = Vec::with_capacity(sources.len());
    let mut mentions: Vec<Vec<Mention>> = Vec::with_capacity(sources.len());
    for source in sources {
        let norms: Vec<String> = source
            .transcript
            .words()
            .iter()
            .map(|word| text::normalize(word.text()))
            .collect();
        plans.push(sections::plan_source(
            &source.transcript,
            source.end_of_media(grid),
            &norms,
            options,
            grid,
        ));
        mentions.push(zooms::mentions(&norms, &keywords));
    }

    let mut cuts: Vec<CutDraft> = Vec::new();
    for (index, (plan, source)) in plans.iter().zip(sources).enumerate() {
        let said = mentions.get(index).map(Vec::as_slice).unwrap_or_default();
        for section in &plan.sections {
            let zooms = zooms::for_cut(
                cuts.len(),
                section,
                source.transcript.words(),
                said,
                &keywords,
                &options.zooms,
                grid,
            );
            cuts.push(CutDraft {
                source: index,
                section: section.clone(),
                zooms,
            });
        }
    }
    if cuts.is_empty() {
        return Err(PlanError::NothingToKeep);
    }

    let output_frames: i64 = cuts
        .iter()
        .map(|cut| cut.section.end - cut.section.start)
        .sum();
    let mut warnings = Vec::new();
    let hook = options.hook.as_ref().map(|hook| {
        let frames = grid.nearest(micros(hook.dur));
        let dur = if frames > output_frames {
            warnings.push(format!(
                "the hook was meant to last {} s, but the output is only {:.3} s long, so the hook lasts the whole output",
                hook.dur,
                grid.seconds(output_frames)
            ));
            grid::millis_text(grid.millis(output_frames))
        } else if frames < 1 {
            grid::millis_text(grid.millis(1))
        } else {
            text::toml_float(hook.dur)
        };
        (hook.text.trim(), dur)
    });

    let toml = write::render(&write::Document {
        options,
        sources,
        plans: &plans,
        cuts: &cuts,
        hook,
        grid,
    });
    let plan =
        Plan::from_toml_str(&toml).map_err(|error| PlanError::Internal(error.to_string()))?;
    let timeline =
        Timeline::from_plan(&plan).map_err(|errors| PlanError::Internal(errors.to_string()))?;
    let (cut_points, mid_word) = check::mid_word(&timeline, sources, grid);

    let report = report(
        sources,
        &plans,
        &cuts,
        grid,
        options,
        ReportTotals {
            output_frames: timeline.total_frames(),
            cut_points,
            mid_word,
        },
        warnings,
    );
    Ok(Planned { plan, toml, report })
}

/// Rejects sources that cannot go in a plan.
fn check_sources(sources: &[SourceInput]) -> Result<(), PlanError> {
    if sources.is_empty() {
        return Err(PlanError::NothingToKeep);
    }
    for (i, source) in sources.iter().enumerate() {
        let problem = |message: String| PlanError::Source {
            id: source.id.clone(),
            message,
        };
        let id_ok = (1..=64).contains(&source.id.len())
            && source
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !id_ok {
            return Err(problem(
                "the id must be 1 to 64 letters, digits, '_' or '-'".to_owned(),
            ));
        }
        if sources.iter().take(i).any(|other| other.id == source.id) {
            return Err(problem("the id is used twice".to_owned()));
        }
        for (what, path) in [("path", &source.path), ("words_path", &source.words_path)] {
            if path.is_empty() || path.chars().any(char::is_control) {
                return Err(problem(format!("{what} must be a file path, got {path:?}")));
            }
        }
        if !(source.crop_x.is_finite() && (0.0..=1.0).contains(&source.crop_x)) {
            return Err(problem(format!(
                "crop_x must be between 0 and 1, got {}",
                source.crop_x
            )));
        }
        if let Some(end) = source.video_end
            && !(end.is_finite() && end >= 0.0)
        {
            return Err(problem(format!(
                "video_end must be a time of 0 s or more, got {end}"
            )));
        }
        source
            .transcript
            .validate()
            .map_err(|errors| PlanError::Transcript {
                id: source.id.clone(),
                errors,
            })?;
    }
    Ok(())
}

struct ReportTotals {
    output_frames: u64,
    cut_points: usize,
    mid_word: usize,
}

fn report(
    sources: &[SourceInput],
    plans: &[SourceCuts],
    cuts: &[CutDraft],
    grid: Grid,
    options: &Options,
    totals: ReportTotals,
    mut warnings: Vec<String>,
) -> Report {
    let mut per_source = Vec::with_capacity(sources.len());
    let mut removals = Vec::new();
    let mut kept_false_starts = Vec::new();
    for (index, (source, plan)) in sources.iter().zip(plans).enumerate() {
        let words = source.transcript.words();
        let text = |range: &Range<usize>| -> String {
            let list: Vec<&str> = words
                .get(range.clone())
                .unwrap_or_default()
                .iter()
                .map(|word| word.text())
                .collect();
            list.join(" ")
        };
        let kept: i64 = plan
            .sections
            .iter()
            .map(|section| section.end - section.start)
            .sum();
        per_source.push(SourceReport {
            id: source.id.clone(),
            duration: source.transcript.duration(),
            kept: grid.seconds(kept),
            cuts: cuts.iter().filter(|cut| cut.source == index).count(),
            words: words.len(),
        });
        if words.is_empty() {
            warnings.push(format!(
                "source {} ({}) has no words, so it has no cuts",
                source.id, source.path
            ));
        }
        warnings.extend(speech_after_video(source, grid));
        let file_name = Path::new(&source.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if file_name
            .as_deref()
            .is_some_and(|name| name != source.transcript.media())
        {
            warnings.push(format!(
                "source {}: {} holds the words of {:?}, not of {}",
                source.id,
                source.words_path,
                source.transcript.media(),
                source.path
            ));
        }
        for gap in &plan.gaps {
            removals.push(Removal {
                source: source.id.clone(),
                start: grid.seconds(gap.start),
                end: grid.seconds(gap.end),
                reason: gap.reason,
                text: text(&gap.words),
            });
        }
        for range in &plan.kept_false_starts {
            let first = words.get(range.start);
            let last = words.get(range.end.saturating_sub(1));
            if let (Some(first), Some(last)) = (first, last) {
                kept_false_starts.push(Span {
                    source: source.id.clone(),
                    start: first.start(),
                    end: last.end(),
                    text: text(range),
                });
            }
        }
    }
    if totals.mid_word > 0 {
        warnings.push(format!(
            "{} of {} cut points lie inside a word; please report this",
            totals.mid_word, totals.cut_points
        ));
    }
    let output = grid.seconds(i64::try_from(totals.output_frames).unwrap_or(i64::MAX));
    if let Some(target) = options.target
        && output > target
    {
        warnings.push(format!(
            "the output is {output:.1} s, longer than the {target} s target"
        ));
    }
    Report {
        fps: options.fps,
        output_frames: totals.output_frames,
        sources: per_source,
        removals,
        kept_false_starts,
        zooms: cuts.iter().map(|cut| cut.zooms.len()).sum(),
        cut_points: totals.cut_points,
        mid_word: totals.mid_word,
        warnings,
    }
}

/// A warning when the clip's video ends before its speech does, so that no cut can keep
/// the rest of the speech.
fn speech_after_video(source: &SourceInput, grid: Grid) -> Option<String> {
    let video_end = source.video_end?;
    let last = source.transcript.words().last()?;
    let end = source.end_of_media(grid);
    let audio_end = sections::end_of_media(source.transcript.duration(), None, grid);
    (end < audio_end && grid.is_before(end, micros(last.end()))).then(|| {
        format!(
            "source {}: the video ends at {video_end:.3} s, before the speech does ({:.3} s), so the speech after it is cut",
            source.id,
            last.end()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ids_count_in_letters() {
        let ids: Vec<String> = [0, 1, 25, 26, 27, 51, 52, 701, 702]
            .into_iter()
            .map(source_id)
            .collect();
        assert_eq!(ids, ["a", "b", "z", "aa", "ab", "az", "ba", "zz", "aaa"]);
    }
}
