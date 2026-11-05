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

/// How far past the video's end, in microseconds, a frame boundary still counts as
/// within it. ffprobe prints times rounded to the microsecond, and the video's end is
/// worked out from up to three of them: the video's start, the file's, and the video's
/// duration.
const VIDEO_END_SLACK_US: i64 = 2;

/// The last frame boundary that a source's cuts can reach: where its audio ends (the
/// duration of its word timings) or, if its video ends sooner, where the video does.
pub(crate) fn end_of_media(audio: f64, video: Option<f64>, grid: Grid) -> i64 {
    let audio_end = grid.at_or_before(micros(audio));
    match video {
        Some(video) => {
            // Capped to the audio first, which also keeps the arithmetic in range.
            let video = micros(video.max(0.0).min(audio));
            audio_end.min(grid.at_or_before(video + VIDEO_END_SLACK_US))
        }
        None => audio_end,
    }
}

/// Plans one source whose cuts end by frame boundary `end_of_media` ([`end_of_media`]).
/// `norms` are its words, normalized.
pub(crate) fn plan_source(
    transcript: &Transcript,
    end_of_media: i64,
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
        end_of_media,
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
        // The media may end (with the video) before the speech does: no section runs
        // past it, and one that this leaves too short is dropped like any other.
        let end = edges
            .end(last)
            .unwrap_or_else(|| edges.fallback(last, false))
            .min(edges.end_of_media);
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
        plan_with_video(transcript, None)
    }

    /// Plans `transcript` at 30 fps, for a clip whose video ends at `video_end`.
    fn plan_with_video(transcript: &Transcript, video_end: Option<f64>) -> SourceCuts {
        let norms: Vec<String> = transcript
            .words()
            .iter()
            .map(|w| crate::text::normalize(w.text()))
            .collect();
        let grid = Grid::new(30);
        let end = end_of_media(transcript.duration(), video_end, grid);
        plan_source(transcript, end, &norms, &Options::default(), grid)
    }

    fn kept(cuts: &SourceCuts) -> Vec<(i64, i64)> {
        cuts.sections.iter().map(|s| (s.start, s.end)).collect()
    }

    #[test]
    fn the_media_ends_with_its_audio_or_its_video_if_that_ends_sooner() {
        let grid = Grid::new(30);
        assert_eq!(end_of_media(4.0, None, grid), 120);
        assert_eq!(end_of_media(4.0, Some(3.7), grid), 111);
        assert_eq!(end_of_media(4.0, Some(3.73), grid), 111);
        assert_eq!(end_of_media(4.0, Some(5.0), grid), 120);
        assert_eq!(end_of_media(4.0, Some(0.0), grid), 0);
        // 301 frames at 30 fps last 10.0333… s, which ffprobe prints as 10.033333.
        assert_eq!(end_of_media(11.0, Some(10.033333), grid), 301);
    }

    #[test]
    fn the_last_cut_stops_at_the_end_of_the_video() {
        // The speech runs on to 3.95 s of 4 s of audio, but the video ends at 3.7 s.
        let t = transcript(
            4.0,
            &[(2.5, 3.98)],
            &[
                ("Ship", 2.6, 2.9),
                ("it", 2.9, 3.1),
                ("tonight.", 3.1, 3.95),
            ],
        );
        assert_eq!(kept(&plan(&t)), [(71, 120)]);
        let cuts = plan_with_video(&t, Some(3.7));
        assert_eq!(kept(&cuts), [(71, 111)]);
        let gaps: Vec<(i64, i64, Reason)> = cuts
            .gaps
            .iter()
            .map(|g| (g.start, g.end, g.reason))
            .collect();
        assert_eq!(gaps, [(0, 71, Reason::Pause)]);
    }

    #[test]
    fn a_section_that_the_end_of_the_video_leaves_too_short_is_dropped() {
        // The video ends at 3.7 s: "Bye." keeps only 3.367 - 3.7 s, under the 0.5 s
        // minimum, and "Thanks." comes after the end of the video.
        let t = transcript(
            5.0,
            &[(1.0, 2.0), (3.5, 3.8), (4.3, 4.6)],
            &[
                ("Hello.", 1.0, 2.0),
                ("Bye.", 3.5, 3.8),
                ("Thanks.", 4.3, 4.6),
            ],
        );
        assert_eq!(kept(&plan(&t)), [(26, 63), (101, 117), (125, 141)]);
        let cuts = plan_with_video(&t, Some(3.7));
        assert_eq!(kept(&cuts), [(26, 63)]);
        let gaps: Vec<(i64, i64, Reason, Range<usize>)> = cuts
            .gaps
            .iter()
            .map(|g| (g.start, g.end, g.reason, g.words.clone()))
            .collect();
        assert_eq!(
            gaps,
            [(0, 26, Reason::Pause, 0..0), (63, 111, Reason::Short, 1..3)]
        );
    }

    #[test]
    fn a_video_longer_than_its_audio_changes_nothing() {
        let t = transcript(
            4.0,
            &[(1.0, 3.98)],
            &[("Hello", 1.0, 1.5), ("there.", 1.5, 3.95)],
        );
        let longer = plan_with_video(&t, Some(4.5));
        assert_eq!(kept(&longer), kept(&plan(&t)));
        assert_eq!(longer.gaps, plan(&t).gaps);
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
