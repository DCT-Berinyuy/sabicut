//! How words are compared and where phrases break.

/// Symbols that belong to a word even at its edges, as in "C++", "C#" or "50%" (the same
/// set that transcript fixes use).
const WORD_SYMBOLS: [char; 10] = ['+', '#', '%', '&', '@', '$', '€', '£', '°', '_'];

/// Closing marks that can follow a word's punctuation, as in `late."` or `moulin !»`.
const CLOSERS: [char; 8] = ['"', '\'', '”', '’', '»', ')', ']', '}'];

/// How a word ends, which tells whether a phrase breaks after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    /// No punctuation: the phrase goes on.
    Open,
    /// A comma, semicolon or colon.
    Clause,
    /// A full stop, question mark or exclamation mark.
    Sentence,
    /// A dash or an ellipsis: the speaker broke off.
    CutOff,
}

impl Ending {
    pub(crate) fn of(text: &str) -> Self {
        let text = text.trim_end_matches(|c: char| c.is_whitespace() || CLOSERS.contains(&c));
        if text.ends_with("...") || text.ends_with('…') {
            return Self::CutOff;
        }
        match text.chars().next_back() {
            Some('-' | '—' | '–') => Self::CutOff,
            Some('.' | '!' | '?') => Self::Sentence,
            Some(',' | ';' | ':') => Self::Clause,
            _ => Self::Open,
        }
    }
}

/// A word as it is compared: lowercase, without the punctuation around it, curly
/// apostrophes made straight. Pieces that a no-break space or a fix joined into one word
/// stay separated by single spaces. Empty when the word has no letters or digits.
pub(crate) fn normalize(text: &str) -> String {
    let mut out = String::new();
    for piece in text.split_whitespace() {
        let core =
            piece.trim_matches(|c: char| !(c.is_alphanumeric() || WORD_SYMBOLS.contains(&c)));
        if core.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        for c in core.chars() {
            match c {
                '’' | 'ʼ' | '‘' => out.push('\''),
                c => out.extend(c.to_lowercase()),
            }
        }
    }
    out
}

/// Text for a one-line comment or report: at most `max` characters, ending with "…" when
/// shortened, and with any control characters replaced by spaces.
pub(crate) fn snippet(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() <= max {
        return clean;
    }
    let mut short: String = clean.chars().take(max.saturating_sub(1)).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// A TOML basic string: `"..."` with quotes, backslashes and control characters escaped.
pub(crate) fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number for TOML that always reads as a float, like `1.0` or `1.25`.
pub(crate) fn toml_float(value: f64) -> String {
    let text = format!("{value}");
    if text.contains(['.', 'e', 'E']) || !value.is_finite() {
        text
    } else {
        format!("{text}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endings() {
        assert_eq!(Ending::of("we..."), Ending::CutOff);
        assert_eq!(Ending::of("we…"), Ending::CutOff);
        assert_eq!(Ending::of("tod-"), Ending::CutOff);
        assert_eq!(Ending::of("so—"), Ending::CutOff);
        assert_eq!(Ending::of("late.\""), Ending::Sentence);
        assert_eq!(Ending::of("moulin\u{a0}!\u{a0}»"), Ending::Sentence);
        assert_eq!(Ending::of("Rust?"), Ending::Sentence);
        assert_eq!(Ending::of("dear,"), Ending::Clause);
        assert_eq!(Ending::of("well-known"), Ending::Open);
        assert_eq!(Ending::of("3.5"), Ending::Open);
    }

    #[test]
    fn normalize_ignores_case_and_edge_punctuation() {
        assert_eq!(normalize("\"So,"), "so");
        assert_eq!(normalize("We’re"), "we're");
        assert_eq!(normalize("C++!"), "c++");
        assert_eq!(normalize("moulin\u{a0}!"), "moulin");
        assert_eq!(normalize("«\u{a0}Le"), "le");
        assert_eq!(normalize("New York,"), "new york");
        assert_eq!(normalize("..."), "");
        assert_eq!(normalize("Maître"), "maître");
    }

    #[test]
    fn snippets_are_short_and_on_one_line() {
        assert_eq!(snippet("short", 10), "short");
        assert_eq!(snippet("a b c d e f g h", 6), "a b c…");
        assert_eq!(snippet("tab\there", 20), "tab here");
    }

    #[test]
    fn toml_strings_are_escaped() {
        assert_eq!(toml_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(toml_string("x\u{7f}y"), "\"x\\u007Fy\"");
        assert_eq!(toml_string("é « »"), "\"é « »\"");
    }

    #[test]
    fn toml_floats_have_a_point() {
        assert_eq!(toml_float(1.0), "1.0");
        assert_eq!(toml_float(1.25), "1.25");
        assert_eq!(toml_float(2.0), "2.0");
    }
}
