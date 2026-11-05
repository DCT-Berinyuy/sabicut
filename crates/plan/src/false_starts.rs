//! False starts: a phrase that the speaker breaks off and then says again from its start.

use std::ops::Range;

use sabicut_core::Word;

use crate::grid::{Grid, micros};
use crate::options::FalseStarts;
use crate::text::Ending;

/// A broken-off phrase, as word indexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fragment {
    pub words: Range<usize>,
    /// Whether a frame boundary fits in the pauses on both sides, so that it can be cut
    /// without cutting into a word.
    pub cuttable: bool,
}

/// The words of one source, ready for comparing.
pub(crate) struct Words<'a> {
    pub words: &'a [Word],
    /// Each word normalized (see [`crate::text::normalize`]).
    pub norms: &'a [String],
}

impl Words<'_> {
    fn len(&self) -> usize {
        self.words.len()
    }

    /// The pause after word `i`, in microseconds; `i64::MAX` after the last word.
    fn gap(&self, i: usize) -> i64 {
        match (self.words.get(i), self.words.get(i + 1)) {
            (Some(word), Some(next)) => micros(next.start()) - micros(word.end()),
            _ => i64::MAX,
        }
    }

    fn ending(&self, i: usize) -> Ending {
        self.words
            .get(i)
            .map_or(Ending::Open, |word| Ending::of(word.text()))
    }

    fn norm(&self, i: usize) -> &str {
        self.norms.get(i).map_or("", String::as_str)
    }
}

/// Finds the false starts in `words`, in order. `max_pause` is the dead-air threshold in
/// microseconds: a longer pause ends a phrase.
pub(crate) fn find(
    words: &Words<'_>,
    rule: &FalseStarts,
    max_pause: i64,
    grid: Grid,
) -> Vec<Fragment> {
    let finder = Finder {
        words,
        rule,
        min_pause: micros(rule.min_pause),
        max_pause,
    };
    let mut fragments = Vec::new();
    let mut i = 0;
    while i < words.len() {
        match finder.fragment_at(i) {
            Some(len) => {
                let range = i..i + len;
                let cuttable = (i == 0 || pause_fits_a_frame(words, i - 1, grid))
                    && pause_fits_a_frame(words, i + len - 1, grid);
                fragments.push(Fragment {
                    words: range,
                    cuttable,
                });
                // The restart may itself be broken off and said again.
                i += len;
            }
            None => i += 1,
        }
    }
    fragments
}

/// Whether a frame boundary lies strictly inside the pause after word `i`.
fn pause_fits_a_frame(words: &Words<'_>, i: usize, grid: Grid) -> bool {
    match (words.words.get(i), words.words.get(i + 1)) {
        (Some(word), Some(next)) => grid.fits_between(micros(word.end()), micros(next.start())),
        _ => false,
    }
}

struct Finder<'w, 'a> {
    words: &'w Words<'a>,
    rule: &'w FalseStarts,
    min_pause: i64,
    max_pause: i64,
}

impl Finder<'_, '_> {
    /// Whether a phrase starts at word `i`: at the start, after punctuation, or after a
    /// pause.
    fn phrase_starts(&self, i: usize) -> bool {
        i == 0
            || self.words.gap(i - 1) >= self.min_pause
            || self.words.ending(i - 1) != Ending::Open
    }

    /// The length of the false start that begins at word `i`, if there is one. The
    /// shortest fragment wins, so the restart is checked as a fragment of its own next.
    fn fragment_at(&self, i: usize) -> Option<usize> {
        if !self.phrase_starts(i) || self.words.norm(i).is_empty() {
            return None;
        }
        for len in 1..=self.rule.max_words {
            let last = i + len - 1;
            let restart = i + len;
            if restart + len > self.words.len() {
                break;
            }
            // A fragment is part of one phrase: no sentence end or long pause inside it.
            if len > 1 && self.breaks_phrase(last - 1) {
                break;
            }
            if len < self.rule.min_words || !self.breaks_off_after(last) {
                continue;
            }
            let Some(shortened) = self.repeats(i, restart, len) else {
                continue;
            };
            // A clause said twice with the same punctuation is a repetition on purpose
            // ("Oh dear, oh dear, I shall be late"), not a phrase broken off.
            if !shortened
                && self.words.ending(last) == Ending::Clause
                && self.words.ending(restart + len - 1) == Ending::Clause
            {
                continue;
            }
            if shortened || self.goes_on(restart + len - 1) {
                return Some(len);
            }
        }
        None
    }

    fn breaks_phrase(&self, k: usize) -> bool {
        self.words.ending(k) == Ending::Sentence || self.words.gap(k) > self.max_pause
    }

    /// Whether the speaker breaks off after word `k`: a dash or an ellipsis, or a pause,
    /// but not the end of a sentence (a complete sentence said twice is kept).
    fn breaks_off_after(&self, k: usize) -> bool {
        match self.words.ending(k) {
            Ending::Sentence => false,
            Ending::CutOff => true,
            Ending::Open | Ending::Clause => self.words.gap(k) >= self.min_pause,
        }
    }

    /// Whether the `len` words from `restart` repeat the `len` words from `start`. The
    /// last word of the fragment may be cut short ("tod" for "today"). Returns whether it
    /// was, or `None` when the words differ.
    fn repeats(&self, start: usize, restart: usize, len: usize) -> Option<bool> {
        for k in 0..len {
            let (said, again) = (self.words.norm(start + k), self.words.norm(restart + k));
            if said.is_empty() {
                return None;
            }
            if k + 1 < len {
                if said != again || self.breaks_phrase(restart + k) {
                    return None;
                }
            } else if said == again {
                return Some(false);
            } else if again.len() > said.len() && again.starts_with(said) {
                return Some(true);
            } else {
                return None;
            }
        }
        None
    }

    /// Whether the restart goes on after its word `k`, within the same phrase.
    fn goes_on(&self, k: usize) -> bool {
        k + 1 < self.words.len() && !self.breaks_phrase(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::normalize;

    /// Words from `(text, start, end)`.
    fn words(spec: &[(&str, f64, f64)]) -> Vec<Word> {
        spec.iter()
            .map(|&(text, start, end)| Word::new(text, start, end, 0.9))
            .collect()
    }

    /// Words said one after another, `0.25` s each, with `pause` seconds after each word
    /// whose text is followed by `|` in `text`.
    fn spoken(text: &str, pause: f64) -> Vec<Word> {
        let mut t = 0.5;
        let mut out = Vec::new();
        for token in text.split(' ') {
            let (word, paused) = match token.strip_suffix('|') {
                Some(word) => (word, true),
                None => (token, false),
            };
            out.push(Word::new(word, t, t + 0.25, 0.9));
            t += 0.25 + if paused { pause } else { 0.0 };
        }
        out
    }

    fn found(words: &[Word]) -> Vec<(Range<usize>, bool)> {
        let norms: Vec<String> = words.iter().map(|w| normalize(w.text())).collect();
        let words = Words {
            words,
            norms: &norms,
        };
        find(&words, &FalseStarts::default(), 400_000, Grid::new(30))
            .into_iter()
            .map(|f| (f.words, f.cuttable))
            .collect()
    }

    #[test]
    fn a_broken_off_phrase_said_again_is_found() {
        let words = spoken(
            "So today we...| So today we're going to talk about Rust.",
            0.3,
        );
        assert_eq!(found(&words), [(0..3, true)]);
    }

    #[test]
    fn several_fragments_before_the_last_take_are_found() {
        let words = spoken(
            "So today...| So today we-| So today we're going to talk about Rust.",
            0.3,
        );
        assert_eq!(found(&words), [(0..2, true), (2..5, true)]);
    }

    #[test]
    fn a_pause_without_punctuation_breaks_a_phrase() {
        let words = spoken("I think it| I think it works.", 0.3);
        assert_eq!(found(&words), [(0..3, true)]);
    }

    #[test]
    fn a_word_cut_short_matches_the_word_said_again() {
        let words = spoken("Let me tod-| let me today show you.", 0.3);
        assert_eq!(found(&words), [(0..3, true)]);
        // Cut short and nothing after it: the longer word itself goes on.
        let words = spoken("Hello.| Go to the sto-| go to the store", 0.3);
        assert_eq!(found(&words), [(1..5, true)]);
    }

    #[test]
    fn emphasis_and_one_word_repeats_are_kept() {
        assert_eq!(found(&spoken("This is very,| very good.", 0.3)), []);
        assert_eq!(found(&spoken("No no no, it is fine.", 0.3)), []);
        assert_eq!(found(&spoken("I- I think so.", 0.3)), []);
        // Repeats from the benchmark readings, with pauses added.
        assert_eq!(
            found(&spoken("Down,| down,| down.| Would it never end?", 0.3)),
            []
        );
        assert_eq!(
            found(&spoken("Oh dear,| oh dear,| I shall be late.", 0.3)),
            []
        );
    }

    #[test]
    fn a_clause_broken_off_at_a_comma_is_found() {
        let words = spoken("So today we,| so today we're going to talk.", 0.3);
        assert_eq!(found(&words), [(0..3, true)]);
    }

    #[test]
    fn a_complete_sentence_said_twice_is_kept() {
        assert_eq!(found(&spoken("Thank you.| Thank you so much.", 0.3)), []);
        // The same phrase twice, with nothing more: nothing to prefer.
        assert_eq!(found(&spoken("Go team,| go team", 0.3)), []);
    }

    #[test]
    fn a_comma_without_a_pause_is_not_a_break() {
        assert_eq!(found(&spoken("Very good, very good indeed.", 0.3)), []);
    }

    #[test]
    fn a_repeat_in_the_middle_of_a_phrase_is_kept() {
        // "we should go." does not start a phrase, so it is not a fragment.
        let words = spoken("I said that we should go.| We should go now.", 0.3);
        assert_eq!(found(&words), []);
    }

    #[test]
    fn different_phrases_with_the_same_start_are_kept() {
        let words = spoken("I like Rust.| I like Go too.", 0.3);
        assert_eq!(found(&words), []);
        let words = spoken("So today we| so tomorrow we will see.", 0.3);
        assert_eq!(found(&words), []);
    }

    #[test]
    fn a_false_start_without_a_pause_is_found_but_not_cuttable() {
        // Whisper times words within a stretch of speech back to back, so a dash
        // with no pause leaves no silence to cut in.
        let words = words(&[
            ("So", 0.5, 0.7),
            ("today-", 0.7, 1.0),
            ("so", 1.0, 1.2),
            ("today", 1.2, 1.5),
            ("we", 1.5, 1.7),
            ("start.", 1.7, 2.0),
        ]);
        assert_eq!(found(&words), [(0..2, false)]);
    }

    #[test]
    fn a_long_pause_inside_a_fragment_splits_it() {
        let words = words(&[
            ("We", 0.5, 0.7),
            ("go", 1.5, 1.7), // 0.8 s pause: a new phrase
            ("now-", 1.7, 2.0),
            ("we", 2.3, 2.5),
            ("go", 2.5, 2.7),
            ("now", 2.7, 2.9),
            ("then.", 2.9, 3.2),
        ]);
        // "We go now-" spans a long pause, so it is not one fragment; "go now-" does not
        // repeat at "we go now", so nothing is found.
        assert_eq!(found(&words), []);
    }

    #[test]
    fn longer_fragments_are_limited() {
        let rule = FalseStarts {
            max_words: 3,
            ..FalseStarts::default()
        };
        let words = spoken("one two three four-| one two three four five.", 0.3);
        let norms: Vec<String> = words.iter().map(|w| normalize(w.text())).collect();
        let list = Words {
            words: &words,
            norms: &norms,
        };
        assert!(find(&list, &rule, 400_000, Grid::new(30)).is_empty());
        assert_eq!(found(&words), [(0..4, true)]);
    }
}
