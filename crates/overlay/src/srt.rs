//! Subtitles: the words as SubRip (SRT) cues on the output timeline, one phrase per
//! cue, without styling.
//!
//! A cue ends after a sentence, at a pause of 0.6 s or more, after a clause once it
//! has a full line of text, when it would last more than 7 s, and when the next word
//! would not fit in two lines of 42 characters. A phrase shorter than 0.8 s that the
//! next one follows straight on (the end of a sentence from before a cut, say) runs on
//! into the next cue instead of flashing by. Cues are timed from the first frame of
//! their first word to the last frame of their last word, kept on screen at least
//! 0.8 s when the next cue leaves room, and never overlap. Lines are balanced.

use std::fmt::Write as _;

use crate::easing::frames_for;
use crate::layout::{balanced_lines, greedy_lines};
use crate::text::{ends_clause, ends_sentence};
use crate::timing::{OutputWord, millis};

/// Characters per line.
pub const MAX_LINE_CHARS: usize = 42;
/// Lines per cue.
pub const MAX_LINES: usize = 2;
/// The longest a cue lasts, in seconds.
pub const MAX_CUE_SECONDS: f64 = 7.0;
/// A pause this long, in seconds, ends a cue.
pub const CUE_PAUSE_SECONDS: f64 = 0.6;
/// The shortest a cue stays on screen when the next cue leaves room, in seconds.
pub const MIN_CUE_SECONDS: f64 = 0.8;

/// One subtitle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    /// The first output frame it shows on.
    pub start: u64,
    /// The output frame after the last one it shows on.
    pub end: u64,
    /// Its text, one or two lines.
    pub lines: Vec<String>,
}

/// Groups `words` (on an output of `total_frames` at `fps`) into cues.
pub fn cues(words: &[OutputWord], fps: u32, total_frames: u64) -> Vec<Cue> {
    let groups = group(words, fps);
    let min_frames = frames_for(MIN_CUE_SECONDS, fps);
    let mut cues: Vec<Cue> = Vec::with_capacity(groups.len());
    for (index, range) in groups.iter().enumerate() {
        let group = words.get(range.clone()).unwrap_or_default();
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let next = groups
            .get(index + 1)
            .and_then(|next| words.get(next.start))
            .map_or(total_frames, |word| word.start);
        // A word shorter than a frame still gets one, which the next word may share, so
        // the next cue's start can come before this cue's last frame: it wins.
        let end = last
            .end
            .max(first.start.saturating_add(min_frames))
            .min(next)
            .min(total_frames);
        if end <= first.start {
            continue;
        }
        cues.push(Cue {
            start: first.start,
            end,
            lines: lines(group),
        });
    }
    cues
}

/// The cues as an SRT file: numbered from 1, `HH:MM:SS,mmm` times, a blank line after
/// each cue.
pub fn to_srt(cues: &[Cue], fps: u32) -> String {
    let mut text = String::new();
    for (index, cue) in cues.iter().enumerate() {
        // Writing to a String cannot fail.
        let _ = writeln!(
            text,
            "{}\n{} --> {}",
            index + 1,
            timestamp(cue.start, fps),
            timestamp(cue.end, fps)
        );
        for line in &cue.lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push('\n');
    }
    text
}

/// `HH:MM:SS,mmm` for output frame boundary `frame`.
fn timestamp(frame: u64, fps: u32) -> String {
    let millis = millis(frame, fps);
    let (seconds, millis) = (millis / 1000, millis % 1000);
    let (minutes, seconds) = (seconds / 60, seconds % 60);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn chars(word: &OutputWord) -> f32 {
    word.text.chars().count() as f32
}

/// Splits the words into cues, as ranges of indexes.
fn group(words: &[OutputWord], fps: u32) -> Vec<std::ops::Range<usize>> {
    let pause = frames_for(CUE_PAUSE_SECONDS, fps);
    let longest = frames_for(MAX_CUE_SECONDS, fps);
    let shortest = frames_for(MIN_CUE_SECONDS, fps);
    let lengths: Vec<f32> = words.iter().map(chars).collect();
    let fits = |range: std::ops::Range<usize>| {
        lengths
            .get(range)
            .and_then(|l| balanced_lines(l, 1.0, MAX_LINE_CHARS as f32, MAX_LINES))
            .is_some()
    };
    let mut groups = Vec::new();
    let mut start = 0;
    for (index, word) in words.iter().enumerate() {
        if index == start {
            continue;
        }
        let (Some(previous), Some(first)) = (words.get(index - 1), words.get(start)) else {
            continue;
        };
        let full_line = lengths.get(start..index).is_some_and(|l| {
            l.iter().sum::<f32>() + (l.len() as f32 - 1.0) >= MAX_LINE_CHARS as f32
        });
        // Too short to read on its own, unless a pause ends it anyway.
        let readable = previous.end.saturating_sub(first.start) >= shortest;
        let breaks = word.start.saturating_sub(previous.end) >= pause
            || (readable && ends_sentence(&previous.text))
            || (readable && ends_clause(&previous.text) && full_line)
            || word.end.saturating_sub(first.start) > longest
            || !fits(start..index + 1);
        if breaks {
            groups.push(start..index);
            start = index;
        }
    }
    if start < words.len() {
        groups.push(start..words.len());
    }
    groups
}

/// The cue's words in balanced lines of at most 42 characters (a single longer word
/// gets a line of its own).
fn lines(words: &[OutputWord]) -> Vec<String> {
    let lengths: Vec<f32> = words.iter().map(chars).collect();
    let ranges = balanced_lines(&lengths, 1.0, MAX_LINE_CHARS as f32, MAX_LINES)
        .unwrap_or_else(|| greedy_lines(&lengths, 1.0, MAX_LINE_CHARS as f32));
    ranges
        .into_iter()
        .map(|range| {
            words
                .get(range)
                .unwrap_or_default()
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: u64, end: u64) -> OutputWord {
        OutputWord {
            text: text.to_owned(),
            start,
            end,
            keyword: false,
        }
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0, 30), "00:00:00,000");
        assert_eq!(timestamp(37, 30), "00:00:01,233");
        assert_eq!(timestamp(30 * 3723, 30), "01:02:03,000");
    }

    fn spans(cues: Vec<Cue>) -> Vec<(u64, u64, Vec<String>)> {
        cues.into_iter()
            .map(|cue| (cue.start, cue.end, cue.lines))
            .collect()
    }

    #[test]
    fn cues_break_at_sentences_and_pauses_and_last_long_enough() {
        let words = [
            word("Hello", 0, 12),
            word("there.", 12, 30),
            word("How", 32, 36),
            word("are", 36, 39),
            // A pause of 18 frames (0.6 s).
            word("you?", 57, 63),
        ];
        assert_eq!(
            spans(cues(&words, 30, 120)),
            [
                (0, 30, vec!["Hello there.".to_owned()]),
                (32, 56, vec!["How are".to_owned()]),
                (57, 81, vec!["you?".to_owned()]),
            ]
        );
    }

    #[test]
    fn a_fragment_too_short_to_read_runs_on_into_the_next_cue() {
        // The end of a sentence from before the cut, then the next one straight away.
        let words = [
            word("it?\"", 4, 6),
            word("\"And", 6, 12),
            word("what", 12, 18),
            word("now?\"", 18, 40),
        ];
        assert_eq!(
            spans(cues(&words, 30, 100)),
            [(4, 40, vec!["it?\" \"And what now?\"".to_owned()])]
        );
    }

    #[test]
    fn cues_never_overlap_when_words_share_a_frame() {
        // "là." is shorter than a frame: it gets frame 30, which "Banc." starts on too.
        let words = [
            word("Assis", 0, 20),
            word("là.", 30, 31),
            word("Banc.", 30, 52),
        ];
        let cues = cues(&words, 30, 100);
        assert_eq!(cues.len(), 2, "{cues:?}");
        assert_eq!((cues[0].end, cues[1].start), (30, 30));
    }

    #[test]
    fn long_phrases_wrap_into_two_balanced_lines_then_a_new_cue() {
        let text = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen";
        let words: Vec<OutputWord> = text
            .split(' ')
            .enumerate()
            .map(|(i, w)| word(w, i as u64 * 6, i as u64 * 6 + 6))
            .collect();
        let cues = cues(&words, 30, 1000);
        for cue in &cues {
            assert!(cue.lines.len() <= 2, "{cue:?}");
            for line in &cue.lines {
                assert!(line.chars().count() <= MAX_LINE_CHARS, "{line:?}");
            }
        }
        assert_eq!(
            cues[0].lines,
            [
                "one two three four five six seven eight",
                "nine ten eleven twelve thirteen fourteen"
            ]
        );
        assert_eq!(cues[1].lines, ["fifteen sixteen"]);
    }

    #[test]
    fn srt_text() {
        let cues = [
            Cue {
                start: 0,
                end: 45,
                lines: vec!["Été comme hiver,".to_owned(), "le moulin\u{a0}!".to_owned()],
            },
            Cue {
                start: 45,
                end: 60,
                lines: vec!["Fin.".to_owned()],
            },
        ];
        assert_eq!(
            to_srt(&cues, 30),
            "1\n00:00:00,000 --> 00:00:01,500\nÉté comme hiver,\nle moulin\u{a0}!\n\n\
             2\n00:00:01,500 --> 00:00:02,000\nFin.\n\n"
        );
        assert_eq!(to_srt(&[], 30), "");
    }
}
