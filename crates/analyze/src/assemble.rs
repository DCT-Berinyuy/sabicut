//! Turning what Whisper heard into words.json words: annotations such as
//! `[BLANK_AUDIO]` are dropped, punctuation that Whisper wrote as a word of its own
//! joins its neighbour, every word ends inside its speech segment, and no word starts
//! before the previous one ends.

use std::ops::Range;

use sabicut_core::Word;

use crate::audio::SAMPLE_RATE;

/// Joins a word and its punctuation ("moulin" + "!") so they stay on one caption line.
const NO_BREAK_SPACE: char = '\u{a0}';

/// Punctuation that belongs to the word after it.
const OPENING: [char; 13] = [
    '«', '“', '‘', '„', '‚', '‹', '(', '[', '{', '¿', '¡', '—', '–',
];

/// A word as Whisper heard it, placed in the original audio.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HeardWord {
    /// The word as Whisper wrote it, trimmed.
    pub text: String,
    /// Index of the speech segment it belongs to.
    pub segment: usize,
    /// Start, in samples of the original audio.
    pub start: usize,
    /// End, in samples of the original audio.
    pub end: usize,
    /// Mean probability of its tokens.
    pub probability: f32,
}

/// The final words, in seconds.
pub(crate) fn finish(heard: Vec<HeardWord>, segments: &[Range<usize>]) -> Vec<Word> {
    let words: Vec<HeardWord> = heard
        .into_iter()
        .filter(|word| !is_annotation(&word.text))
        .collect();
    let mut words = join_punctuation(words);
    clamp_to_segments(&mut words, segments);
    keep_in_order(&mut words);
    words
        .into_iter()
        .map(|word| {
            Word::new(
                word.text,
                seconds(word.start),
                seconds(word.end),
                f64::from(word.probability.clamp(0.0, 1.0)),
            )
        })
        .collect()
}

pub(crate) fn seconds(samples: usize) -> f64 {
    samples as f64 / SAMPLE_RATE as f64
}

/// Whisper's notes about the audio, like `[BLANK_AUDIO]` or `[Music]`.
fn is_annotation(text: &str) -> bool {
    text.len() > 2 && text.starts_with('[') && text.ends_with(']')
}

fn is_punctuation(text: &str) -> bool {
    !text.chars().any(char::is_alphanumeric)
}

/// Attaches punctuation-only words to a neighbour: opening marks to the next word,
/// everything else to the previous one, with a no-break space as in French typography.
/// The neighbour keeps its timing.
fn join_punctuation(words: Vec<HeardWord>) -> Vec<HeardWord> {
    let mut out: Vec<HeardWord> = Vec::with_capacity(words.len());
    let mut prefix = String::new();
    for mut word in words {
        if is_punctuation(&word.text) {
            let opening = word.text.chars().all(|c| OPENING.contains(&c));
            match out.last_mut() {
                Some(previous) if !opening && prefix.is_empty() => {
                    previous.text.push(NO_BREAK_SPACE);
                    previous.text.push_str(&word.text);
                }
                _ => {
                    prefix.push_str(&word.text);
                    prefix.push(NO_BREAK_SPACE);
                }
            }
            continue;
        }
        if !prefix.is_empty() {
            word.text.insert_str(0, &prefix);
            prefix.clear();
        }
        out.push(word);
    }
    if let Some(last) = out.last_mut() {
        let rest = prefix.trim_end_matches(NO_BREAK_SPACE);
        if !rest.is_empty() {
            last.text.push(NO_BREAK_SPACE);
            last.text.push_str(rest);
        }
    }
    out
}

/// Ends every word at the end of its speech segment at the latest: Whisper's end times
/// tend to run on into the following pause.
fn clamp_to_segments(words: &mut [HeardWord], segments: &[Range<usize>]) {
    for word in words {
        if let Some(segment) = segments.get(word.segment) {
            word.start = word.start.clamp(segment.start, segment.end);
            word.end = word.end.min(segment.end);
        }
        word.end = word.end.max(word.start);
    }
}

/// Makes every word start no earlier than the previous one ends, by ending the
/// previous word earlier (or, if it started later, starting this one later).
fn keep_in_order(words: &mut [HeardWord]) {
    let mut words = words.iter_mut();
    let Some(mut previous) = words.next() else {
        return;
    };
    for word in words {
        if word.start < previous.end {
            previous.end = previous.start.max(word.start);
        }
        if word.start < previous.end {
            word.start = previous.end;
            word.end = word.end.max(word.start);
        }
        previous = word;
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Lists of one speech segment are intended.
mod tests {
    use super::*;

    const S: usize = SAMPLE_RATE;

    fn heard(text: &str, start: usize, end: usize) -> HeardWord {
        HeardWord {
            text: text.to_owned(),
            segment: 0,
            start,
            end,
            probability: 0.9,
        }
    }

    fn texts(words: &[Word]) -> Vec<&str> {
        words.iter().map(Word::text).collect()
    }

    #[test]
    fn punctuation_joins_its_neighbour_with_a_no_break_space() {
        let words = finish(
            vec![
                heard("«", 0, S / 10),
                heard("Le", S / 10, S / 5),
                heard("moulin", S / 5, S / 2),
                heard("!", S / 2, S),
                heard("»", S, S + 1),
                heard("Fin.", 2 * S, 3 * S),
            ],
            &[0..10 * S],
        );
        assert_eq!(texts(&words), ["«\u{a0}Le", "moulin\u{a0}!\u{a0}»", "Fin."]);
        // Punctuation does not change the timing of the word it joins.
        assert_eq!((words[0].start(), words[0].end()), (0.1, 0.2));
        assert_eq!((words[1].start(), words[1].end()), (0.2, 0.5));
    }

    #[test]
    fn stray_punctuation_and_annotations_are_handled() {
        let words = finish(
            vec![
                heard("!", 0, 10),
                heard("[BLANK_AUDIO]", 10, S),
                heard("Hi", S, 2 * S),
                heard("(", 2 * S, 2 * S),
            ],
            &[0..10 * S],
        );
        assert_eq!(texts(&words), ["!\u{a0}Hi\u{a0}("]);
        assert!(finish(vec![heard("…", 0, 10)], &[0..S]).is_empty());
        let bracket_word = finish(vec![heard("[]", 0, 10), heard("ok", 10, 20)], &[0..S]);
        assert_eq!(texts(&bracket_word), ["[]\u{a0}ok"]);
    }

    #[test]
    fn words_end_with_their_speech_segment() {
        let mut word = heard("pause", 2 * S, 5 * S);
        word.segment = 1;
        let words = finish(vec![word], &[0..S, S..3 * S, 4 * S..6 * S]);
        assert_eq!((words[0].start(), words[0].end()), (2.0, 3.0));
    }

    #[test]
    fn words_never_overlap() {
        let words = finish(
            vec![
                heard("a", 0, S),
                heard("b", S / 2, 2 * S),
                heard("c", S / 4, S / 2),
            ],
            &[0..10 * S],
        );
        let times: Vec<(f64, f64)> = words.iter().map(|w| (w.start(), w.end())).collect();
        assert_eq!(times, [(0.0, 0.5), (0.5, 0.5), (0.5, 0.5)]);
        for pair in words.windows(2) {
            assert!(pair[1].start() >= pair[0].end());
        }
    }
}
