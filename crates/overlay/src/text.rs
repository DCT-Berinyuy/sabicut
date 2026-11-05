//! Text rules shared by captions, the hook and subtitles: what a word is for keyword
//! matching, where sentences and clauses end, and how the hook splits into words.

/// Symbols that belong to a word even at its edges, as in "C++", "C#", "50%" or
/// "@dct". The same set fixes.toml uses, so keywords match the words fixes produce.
const WORD_SYMBOLS: [char; 10] = ['+', '#', '%', '&', '@', '$', '€', '£', '°', '_'];

/// Closing marks that may follow the end of a sentence or clause: quotes, brackets and
/// the no-break spaces French puts before `!`, `?`, `:` and `»`.
const CLOSERS: [char; 11] = [
    '"', '\'', '’', '”', '»', ')', ']', '}', '\u{a0}', '\u{202f}', ' ',
];

fn is_edge_punctuation(c: char) -> bool {
    !c.is_alphanumeric() && !WORD_SYMBOLS.contains(&c)
}

/// The words of `text` without the punctuation at their edges, in lowercase: what
/// keywords are compared with. `«Supabase»,` gives `["supabase"]`.
pub(crate) fn match_cores(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(is_edge_punctuation))
        .filter(|core| !core.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The last character of `text` that is not a closing quote, bracket or space.
fn last_mark(text: &str) -> Option<char> {
    text.trim_end_matches(CLOSERS).chars().next_back()
}

/// Whether a word ends a sentence: `end.`, `vraiment !`, `why?"`, `so…`.
pub(crate) fn ends_sentence(text: &str) -> bool {
    matches!(last_mark(text), Some('.' | '!' | '?' | '…'))
}

/// Whether a word ends a clause: `first,`, `voici :`, `then;`.
pub(crate) fn ends_clause(text: &str) -> bool {
    matches!(last_mark(text), Some(',' | ';' | ':'))
}

/// Splits text into words at spaces, but not at no-break spaces, so French `moulin !`
/// stays one word.
pub(crate) fn split_words(text: &str) -> Vec<&str> {
    text.split(|c: char| c.is_whitespace() && c != '\u{a0}' && c != '\u{202f}')
        .filter(|word| !word.is_empty())
        .collect()
}

/// The keyword flag of each word in `words`: whether it is part of a run of words that
/// spells one of `keywords`. Matching ignores capitals and the punctuation around
/// words, and a keyword of several words matches them in a row, also across a word
/// that a fix joined (`Visual Studio`).
pub(crate) fn keyword_flags<'w>(
    words: impl IntoIterator<Item = &'w str>,
    keywords: &[String],
) -> Vec<bool> {
    // Every word core, with the index of the word it is in.
    let mut cores: Vec<(String, usize)> = Vec::new();
    let mut count = 0;
    for (index, word) in words.into_iter().enumerate() {
        cores.extend(match_cores(word).into_iter().map(|core| (core, index)));
        count = index + 1;
    }
    let mut flags = vec![false; count];
    for keyword in keywords {
        let wanted = match_cores(keyword);
        if wanted.is_empty() {
            continue;
        }
        for start in 0..cores.len() {
            let Some(run) = cores.get(start..start + wanted.len()) else {
                break;
            };
            if run
                .iter()
                .zip(&wanted)
                .all(|((core, _), want)| core == want)
            {
                for (_, index) in run {
                    if let Some(flag) = flags.get_mut(*index) {
                        *flag = true;
                    }
                }
            }
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cores_drop_edge_punctuation_and_capitals() {
        assert_eq!(match_cores("«Supabase»,"), ["supabase"]);
        assert_eq!(match_cores("C++, l'homme !"), ["c++", "l'homme"]);
        assert!(match_cores(" … ").is_empty());
    }

    #[test]
    fn sentence_and_clause_ends() {
        for end in [
            "end.",
            "vraiment\u{a0}!",
            "why?\"",
            "so…",
            "(fin.)",
            "« Oui. »",
        ] {
            assert!(ends_sentence(end), "{end}");
        }
        for not in ["Mr", "50%", "first,", "e.g"] {
            assert!(!ends_sentence(not), "{not}");
        }
        assert!(ends_clause("first,"));
        assert!(ends_clause("voici\u{a0}:"));
        assert!(!ends_clause("end."));
    }

    #[test]
    fn hook_words_keep_no_break_spaces() {
        assert_eq!(
            split_words("  Stop  leaking\u{a0}! your\tkeys "),
            ["Stop", "leaking\u{a0}!", "your", "keys"]
        );
    }

    #[test]
    fn keywords_match_whole_words_in_a_row() {
        let words = [
            "I", "love", "rust,", "and", "Visual", "Studio", "Code.", "Rusty",
        ];
        let keywords = ["Rust".to_owned(), "visual studio".to_owned()];
        assert_eq!(
            keyword_flags(words, &keywords),
            [false, false, true, false, true, true, false, false]
        );
        // A fix joined "Visual Studio" into one word.
        let joined = ["open", "Visual Studio."];
        assert_eq!(keyword_flags(joined, &keywords), [false, true]);
        assert_eq!(keyword_flags(["Été"], &["été".to_owned()]), [true]);
    }
}
