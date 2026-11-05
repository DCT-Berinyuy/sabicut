//! Pauses ("dead air"): which ranges of a source are kept, on its frame grid.

use std::ops::Range;

use sabicut_core::{SpeechSegment, Transcript, Word};

use crate::false_starts::{self, Words};
use crate::grid::{Grid, micros};
use crate::options::Options;

/// A kept range of a source: frame boundaries `start..end`, and the words in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Section {
    pub start: i64,
    pub end: i64,
    pub words: Range<usize>,
}

/// Why a range of a source was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A pause longer than the threshold, or silence before the first word or after the
    /// last.
    Pause,
    /// A false start: a phrase that is said again right after.
    FalseStart,
    /// A kept section that was too short, such as a lone "um".
    Short,
}

/// A cut range of a source: frame boundaries `start..end`, and the words in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    pub start: i64,
    pub end: i64,
    pub words: Range<usize>,
    pub reason: Reason,
}

/// What the rules keep and cut in one source.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceCuts {
    /// Kept ranges, in time order.
    pub sections: Vec<Section>,
    /// Cut ranges, in time order, from the start of the media to its end.
    pub gaps: Vec<Gap>,
    /// False starts that were found but kept, because no pause fits a frame boundary.
    pub kept_false_starts: Vec<Range<usize>>,
}

/// Plans one source. `norms` are its words, normalized.
pub(crate) fn plan_source(
    transcript: &Transcript,
    norms: &[String],
    options: &Options,
    grid: Grid,
) -> SourceCuts {
    let words = transcript.words();
    let air = &options.dead_air;
    let max_pause = micros(air.max_pause);
    let fragments = options
        .false_starts
        .as_ref()
        .map(|rule| false_starts::find(&Words { words, norms }, rule, max_pause, grid))
        .unwrap_or_default();
    let mut removed = vec![false; words.len()];
    for fragment in fragments.iter().filter(|fragment| fragment.cuttable) {
        for flag in removed
            .iter_mut()
            .take(fragment.words.end)
            .skip(fragment.words.start)
        {
            *flag = true;
        }
    }

    let edges = Edges {
        words,
        speech: transcript.speech(),
        end_of_media: grid.at_or_before(micros(transcript.duration())),
        grid,
        pad_before: micros(air.pad_before),
        pad_after: micros(air.pad_after),
        reach: micros(air.vad_reach),
    };
    let runs = runs(words, &removed, max_pause);
    let groups = merge_short_cuts(runs, &edges, grid.frames_for(air.min_cut));
    let min_keep = grid.frames_for(air.min_keep);
    let mut sections = Vec::new();
    let mut short = Vec::new();
    for group in groups {
        let first = group.start;
        let last = group.end.saturating_sub(1);
        let start = edges
            .start(first)
            .unwrap_or_else(|| edges.fallback(first, true));
        let end = edges
            .end(last)
            .unwrap_or_else(|| edges.fallback(last, false));
        if end - start < min_keep.max(1) {
            short.push(group);
        } else {
            sections.push(Section {
                start,
                end,
                words: group,
            });
        }
    }

    let gaps = gaps(&sections, words.len(), edges.end_of_media, |words| {
        if words.clone().any(|i| removed.get(i) == Some(&true)) {
            Reason::FalseStart
        } else if short
            .iter()
            .any(|group| group.clone().any(|i| words.contains(&i)))
        {
            Reason::Short
        } else {
            Reason::Pause
        }
    });
    SourceCuts {
        sections,
        gaps,
        kept_false_starts: fragments
            .into_iter()
            .filter(|fragment| !fragment.cuttable)
            .map(|fragment| fragment.words)
            .collect(),
    }
}

/// Runs of kept words, split at pauses longer than `max_pause` and at removed words.
fn runs(words: &[Word], removed: &[bool], max_pause: i64) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut previous: Option<&Word> = None;
    for (i, word) in words.iter().enumerate() {
        if removed.get(i) == Some(&true) {
            previous = None;
            continue;
        }
        let joined = previous.is_some_and(|p| micros(word.start()) - micros(p.end()) <= max_pause);
        match runs.last_mut() {
            Some(run) if joined => run.end = i + 1,
            _ => runs.push(i..i + 1),
        }
        previous = Some(word);
    }
    runs
}

/// Joins runs that only a pause separates when cutting it would remove less than
/// `min_cut` frames, or when no frame boundary fits in it.
fn merge_short_cuts(runs: Vec<Range<usize>>, edges: &Edges<'_>, min_cut: i64) -> Vec<Range<usize>> {
    let mut groups: Vec<Range<usize>> = Vec::with_capacity(runs.len());
    for run in runs {
        if let Some(group) = groups.last_mut()
            && group.end == run.start
        {
            let removes = match (
                edges.end(group.end.saturating_sub(1)),
                edges.start(run.start),
            ) {
                (Some(end), Some(start)) => start - end,
                _ => 0,
            };
            if removes < min_cut.max(1) {
                group.end = run.end;
                continue;
            }
        }
        groups.push(run);
    }
    groups
}

/// The cut ranges around `sections`, from frame 0 to `end_of_media`, each with the words
/// in it and the reason that `reason` gives for those words.
fn gaps(
    sections: &[Section],
    word_count: usize,
    end_of_media: i64,
    reason: impl Fn(&Range<usize>) -> Reason,
) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut push = |start: i64, end: i64, words: Range<usize>| {
        if end > start || !words.is_empty() {
            let reason = reason(&words);
            gaps.push(Gap {
                start,
                end: end.max(start),
                words,
                reason,
            });
        }
    };
    let (mut frame, mut word) = (0, 0);
    for section in sections {
        push(frame, section.start, word..section.words.start);
        frame = section.end;
        word = section.words.end;
    }
    push(frame, end_of_media, word..word_count);
    gaps
}

/// Where kept sections start and end: on frame boundaries outside every word, padded
/// around the speech.
struct Edges<'a> {
    words: &'a [Word],
    speech: &'a [SpeechSegment],
    end_of_media: i64,
    grid: Grid,
    pad_before: i64,
    pad_after: i64,
    reach: i64,
}

impl Edges<'_> {
    /// The speech segment that contains time `t`.
    fn segment(&self, t: f64) -> Option<&SpeechSegment> {
        let after = self.speech.partition_point(|segment| segment.start() <= t);
        let segment = self.speech.get(after.checked_sub(1)?)?;
        (t <= segment.end()).then_some(segment)
    }

    /// The first boundary of a section that starts with word `first`: `pad_before` ahead
    /// of the speech, which starts where the word's speech segment does (at most `reach`
    /// earlier than the word), but after the word before it. `None` when no boundary
    /// fits between the two words.
    fn start(&self, first: usize) -> Option<i64> {
        let word = self.words.get(first)?;
        let start = micros(word.start());
        let speech = self
            .segment(word.start())
            .map_or(start, |segment| {
                micros(segment.start()).max(start - self.reach)
            })
            .min(start);
        let lowest = match first.checked_sub(1).and_then(|i| self.words.get(i)) {
            Some(previous) => self.grid.after(micros(previous.end())),
            None => 0,
        };
        let mut highest = self.grid.before(start);
        if first == 0 {
            // The start of the media is a clean cut even if a word starts right there.
            highest = highest.max(0);
        }
        (lowest <= highest).then(|| {
            self.grid
                .nearest(speech - self.pad_before)
                .clamp(lowest, highest)
        })
    }

    /// The last boundary of a section that ends with word `last`: `pad_after` beyond the
    /// speech, which ends where the word's speech segment does (at most `reach` later
    /// than the word), but before the next word and the end of the media.
    fn end(&self, last: usize) -> Option<i64> {
        let word = self.words.get(last)?;
        let end = micros(word.end());
        let speech = self
            .segment(word.start())
            .map_or(end, |segment| micros(segment.end()).min(end + self.reach))
            .max(end);
        let lowest = self.grid.after(end);
        let next = self.words.get(last + 1);
        let highest = match next {
            Some(next) => self.grid.before(micros(next.start())),
            None => self.end_of_media,
        };
        if lowest <= highest {
            Some(
                self.grid
                    .nearest(speech + self.pad_after)
                    .clamp(lowest, highest),
            )
        } else if next.is_none() {
            // The media ends inside or right after the last word.
            Some(self.end_of_media)
        } else {
            None
        }
    }

    /// The boundary nearest to a word's start or end, for the edge cases where no boundary
    /// fits outside the words. The plan's self-check counts these as mid-word cuts.
    fn fallback(&self, index: usize, start: bool) -> i64 {
        self.words.get(index).map_or(0, |word| {
            let t = if start { word.start() } else { word.end() };
            self.grid
                .nearest(micros(t))
                .clamp(0, self.end_of_media.max(0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(duration: f64, speech: &[(f64, f64)], words: &[(&str, f64, f64)]) -> Transcript {
        Transcript::new(
            "clip.mp4",
            "en",
            "test",
            duration,
            speech
                .iter()
                .map(|&(start, end)| SpeechSegment::new(start, end))
                .collect(),
            words
                .iter()
                .map(|&(text, start, end)| Word::new(text, start, end, 0.9))
                .collect(),
        )
        .unwrap()
    }

    fn plan(transcript: &Transcript) -> SourceCuts {
        let norms: Vec<String> = transcript
            .words()
            .iter()
            .map(|w| crate::text::normalize(w.text()))
            .collect();
        plan_source(transcript, &norms, &Options::default(), Grid::new(30))
    }

    #[test]
    fn a_long_pause_is_cut_with_padding_around_the_speech() {
        let t = transcript(
            10.0,
            &[(1.0, 2.0), (5.0, 6.0)],
            &[
                ("Hello", 1.03, 1.5),
                ("there.", 1.5, 2.0),
                ("Bye.", 5.03, 6.0),
            ],
        );
        let cuts = plan(&t);
        let frames: Vec<(i64, i64)> = cuts.sections.iter().map(|s| (s.start, s.end)).collect();
        // 1.0 - 0.15 = 0.85 s (frame 25.5 -> 26); 2.0 + 0.1 = 2.1 s (frame 63);
        // 5.0 - 0.15 = 4.85 s (frame 145.5 -> 146); 6.0 + 0.1 = 6.1 s (frame 183).
        assert_eq!(frames, [(26, 63), (146, 183)]);
        let reasons: Vec<Reason> = cuts.gaps.iter().map(|g| g.reason).collect();
        assert_eq!(reasons, [Reason::Pause, Reason::Pause, Reason::Pause]);
        assert_eq!(cuts.gaps.last().map(|g| g.end), Some(300));
    }

    #[test]
    fn a_short_pause_stays() {
        let t = transcript(
            4.0,
            &[(1.0, 3.0)],
            &[("One", 1.0, 1.5), ("two", 1.8, 2.2), ("three.", 2.2, 3.0)],
        );
        assert_eq!(plan(&t).sections.len(), 1);
    }

    #[test]
    fn a_pause_that_padding_eats_up_stays() {
        // 0.42 s between segments: padding keeps 0.25 s, so the cut would remove less
        // than 0.2 s (5 frames instead of 6).
        let t = transcript(
            4.0,
            &[(1.0, 2.0), (2.42, 3.0)],
            &[("One.", 1.0, 2.0), ("Two.", 2.42, 3.0)],
        );
        assert_eq!(plan(&t).sections.len(), 1);
    }

    #[test]
    fn a_lone_short_word_is_dropped_and_reported() {
        let t = transcript(
            12.0,
            &[(1.0, 3.0), (5.0, 5.1), (8.0, 10.0)],
            &[("Hello.", 1.0, 3.0), ("Um.", 5.0, 5.1), ("Bye.", 8.0, 10.0)],
        );
        let cuts = plan(&t);
        assert_eq!(cuts.sections.len(), 2);
        let middle = cuts.gaps.get(1).unwrap();
        assert_eq!((middle.reason, middle.words.clone()), (Reason::Short, 1..2));
    }

    #[test]
    fn the_start_of_the_media_is_a_clean_cut() {
        let t = transcript(3.0, &[(0.0, 1.0)], &[("Hi.", 0.0, 1.0)]);
        let cuts = plan(&t);
        assert_eq!(cuts.sections.first().map(|s| s.start), Some(0));
    }

    #[test]
    fn a_word_running_to_the_end_of_the_media_keeps_it() {
        let t = transcript(2.0, &[(1.0, 2.0)], &[("Bye", 1.0, 2.0)]);
        let cuts = plan(&t);
        assert_eq!(cuts.sections.first().map(|s| s.end), Some(60));
    }
}
