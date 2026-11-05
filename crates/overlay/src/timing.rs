//! Word timings placed on the output timeline.
//!
//! A word is snapped to the frame grid the way `sabicut-core` snaps cut points (nearest
//! frame boundary at `meta.fps`, halves up, from the exact decimal time), then laid
//! over every cut of its source on the [`Timeline`]. Words in removed sections are
//! dropped; a word that straddles a cut's `in` or `out` is clamped to the cut. A word
//! split by a removed section whose two parts end up next to each other on the output
//! is shown once, over both parts.

use std::collections::BTreeMap;

use sabicut_core::{Rational, Timeline, Transcript};

use crate::text::keyword_flags;

/// Word timings for a plan's sources, by source id, with fixes already applied.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WordTimings {
    by_source: BTreeMap<String, Transcript>,
}

impl WordTimings {
    /// No word timings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the word timings of source `id`.
    pub fn insert(&mut self, id: impl Into<String>, transcript: Transcript) {
        self.by_source.insert(id.into(), transcript);
    }

    /// The word timings of source `id`, if known.
    pub fn get(&self, id: &str) -> Option<&Transcript> {
        self.by_source.get(id)
    }

    /// Whether no source has word timings.
    pub fn is_empty(&self) -> bool {
        self.by_source.is_empty()
    }
}

/// A word on the output timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputWord {
    /// The word as written, with its punctuation.
    pub text: String,
    /// The first output frame it is spoken on.
    pub start: u64,
    /// The output frame after the last one it is spoken on; always after `start`.
    pub end: u64,
    /// Whether it is (part of) one of the caption keywords.
    pub keyword: bool,
}

/// Places the words of every cut's source on the output timeline, in output order.
///
/// Returns `None` if a source that a cut uses has no word timings.
pub fn place_words(
    timeline: &Timeline,
    words: &WordTimings,
    keywords: &[String],
) -> Option<Vec<OutputWord>> {
    let fps = timeline.fps();
    let mut flags: BTreeMap<&str, Vec<bool>> = BTreeMap::new();
    let mut placed: Vec<OutputWord> = Vec::new();
    // The source and index of the word placed last, to join its two parts.
    let mut last: Option<(&str, usize)> = None;
    for segment in timeline.segments() {
        let source = segment.source();
        let transcript = words.get(source)?;
        let keyword = flags.entry(source).or_insert_with(|| {
            keyword_flags(transcript.words().iter().map(|w| w.text()), keywords)
        });
        let (cut_in, cut_out) = (segment.source_in(), segment.source_out());
        for (index, word) in transcript.words().iter().enumerate() {
            let Some(start) = boundary(word.start(), fps) else {
                continue;
            };
            if start >= cut_out {
                break;
            }
            // A word of zero length still gets one frame.
            let end = boundary(word.end(), fps)
                .unwrap_or(start)
                .max(start.saturating_add(1));
            let (from, to) = (start.max(cut_in), end.min(cut_out));
            if from >= to {
                continue;
            }
            let output_start = segment.output_start() + (from - cut_in);
            let output_end = segment.output_start() + (to - cut_in);
            if let Some(previous) = placed.last_mut()
                && last == Some((source, index))
                && previous.end == output_start
            {
                previous.end = output_end;
                continue;
            }
            placed.push(OutputWord {
                text: word.text().to_owned(),
                start: output_start,
                end: output_end,
                keyword: keyword.get(index).copied().unwrap_or(false),
            });
            last = Some((source, index));
        }
    }
    Some(placed)
}

/// The frame boundary nearest to `seconds` at `fps` (halves up), as `sabicut-core`
/// snaps cut points: from the time read to the microsecond, in exact arithmetic.
pub(crate) fn boundary(seconds: f64, fps: u32) -> Option<u64> {
    let time = Rational::from_secs_f64(seconds)?;
    let (numer, denom) = (i128::from(time.numer()), i128::from(time.denom()));
    if denom <= 0 {
        return None;
    }
    let frame = (2 * numer * i128::from(fps) + denom).div_euclid(2 * denom);
    u64::try_from(frame).ok()
}

/// The time of output frame boundary `frame` in whole milliseconds, rounded to the
/// nearest (halves up).
pub(crate) fn millis(frame: u64, fps: u32) -> u64 {
    let fps = u128::from(fps.max(1));
    let millis = (2 * u128::from(frame) * 1000 + fps) / (2 * fps);
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sabicut_core::{Plan, SpeechSegment, Word};

    fn timeline(cuts: &str) -> Timeline {
        let plan = Plan::from_toml_str(&format!(
            "schema = 1\n[meta]\nfps = 30\nout = {{ w = 1080, h = 1920 }}\n\
             [[source]]\nid = \"a\"\npath = \"a.mp4\"\ncrop_x = 0.5\n{cuts}"
        ))
        .unwrap();
        Timeline::from_plan(&plan).unwrap()
    }

    fn transcript(words: &[(&str, f64, f64)]) -> Transcript {
        let words = words
            .iter()
            .map(|&(text, start, end)| Word::new(text, start, end, 0.9))
            .collect();
        Transcript::new(
            "a.mp4",
            "en",
            "test",
            60.0,
            vec![SpeechSegment::new(0.0, 60.0)],
            words,
        )
        .unwrap()
    }

    fn timings(words: &[(&str, f64, f64)]) -> WordTimings {
        let mut timings = WordTimings::new();
        timings.insert("a", transcript(words));
        timings
    }

    fn spans(words: &[OutputWord]) -> Vec<(&str, u64, u64)> {
        words
            .iter()
            .map(|w| (w.text.as_str(), w.start, w.end))
            .collect()
    }

    #[test]
    fn words_snap_like_cut_points() {
        assert_eq!(boundary(3.12, 30), Some(94)); // 93.6
        assert_eq!(boundary(0.05, 30), Some(2)); // exactly 1.5, halves up
        assert_eq!(boundary(-1.0, 30), None);
        // The same frame Timeline::map_source_secs gives for a time inside a cut.
        let timeline = timeline("[[cut]]\nsrc = \"a\"\nin = 2.0\nout = 9.0");
        for t in [2.0, 3.12, 4.05, 8.983] {
            let mapped = timeline.map_source_secs("a", t).unwrap().output_frame();
            assert_eq!(boundary(t, 30).unwrap() - 60, mapped, "{t}");
        }
    }

    #[test]
    fn removed_words_are_dropped_and_straddling_words_clamped() {
        // Cuts keep 1.0-2.0 s and 3.0-4.0 s.
        let timeline = timeline(
            "[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 2.0\n[[cut]]\nsrc = \"a\"\nin = 3.0\nout = 4.0",
        );
        let words = timings(&[
            ("before", 0.2, 0.5),
            ("straddles", 0.9, 1.2),
            ("kept", 1.3, 1.6),
            ("tail", 1.9, 2.4),
            ("removed", 2.5, 2.8),
            ("head", 2.9, 3.1),
            ("last", 3.9, 4.5),
        ]);
        let placed = place_words(&timeline, &words, &[]).unwrap();
        assert_eq!(
            spans(&placed),
            [
                ("straddles", 0, 6),
                ("kept", 9, 18),
                ("tail", 27, 30),
                ("head", 30, 33),
                ("last", 57, 60),
            ]
        );
    }

    #[test]
    fn a_word_split_by_a_removed_section_is_shown_once() {
        let timeline = timeline(
            "[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 2.0\n[[cut]]\nsrc = \"a\"\nin = 2.2\nout = 3.0",
        );
        let placed = place_words(&timeline, &timings(&[("Supabase", 1.8, 2.5)]), &[]).unwrap();
        assert_eq!(spans(&placed), [("Supabase", 24, 39)]);
    }

    #[test]
    fn a_reused_range_shows_its_words_again() {
        let timeline = timeline(
            "[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 2.0\n[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 2.0",
        );
        let placed = place_words(&timeline, &timings(&[("again", 1.5, 1.8)]), &[]).unwrap();
        assert_eq!(spans(&placed), [("again", 15, 24), ("again", 45, 54)]);
    }

    #[test]
    fn zero_length_words_get_a_frame_and_keywords_are_marked() {
        let timeline = timeline("[[cut]]\nsrc = \"a\"\nin = 0.0\nout = 5.0");
        let words = timings(&[("I", 1.0, 1.0), ("love", 1.0, 1.4), ("Rust.", 1.4, 1.9)]);
        let placed = place_words(&timeline, &words, &["rust".to_owned()]).unwrap();
        assert_eq!(
            spans(&placed),
            [("I", 30, 31), ("love", 30, 42), ("Rust.", 42, 57)]
        );
        let keywords: Vec<bool> = placed.iter().map(|w| w.keyword).collect();
        assert_eq!(keywords, [false, false, true]);
    }

    #[test]
    fn a_source_without_words_gives_none() {
        let timeline = timeline("[[cut]]\nsrc = \"a\"\nin = 0.0\nout = 5.0");
        assert_eq!(place_words(&timeline, &WordTimings::new(), &[]), None);
    }

    #[test]
    fn frames_become_milliseconds() {
        assert_eq!(millis(0, 30), 0);
        assert_eq!(millis(1, 30), 33);
        assert_eq!(millis(2, 30), 67);
        assert_eq!(millis(1800, 30), 60_000);
        assert_eq!(millis(1, 24), 42);
    }
}
