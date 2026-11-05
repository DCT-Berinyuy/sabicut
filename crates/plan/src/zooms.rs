//! Punch-in zooms on keywords, and an optional zoom on every second cut.

use sabicut_core::Word;

use crate::grid::{Grid, micros};
use crate::options::Zooms;
use crate::sections::Section;
use crate::text::normalize;

/// A zoom to write: it starts at frame boundary `frame` of its source.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ZoomDraft {
    pub frame: i64,
    pub scale: f64,
    /// The keyword that triggered it, if any.
    pub keyword: Option<String>,
}

/// A keyword, as the words it matches (normalized) and as given.
#[derive(Clone, Debug)]
pub(crate) struct Keyword {
    tokens: Vec<String>,
    text: String,
}

/// Where a keyword is said: the words it covers, and which keyword it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mention {
    pub first: usize,
    pub last: usize,
    pub keyword: usize,
}

pub(crate) fn keywords(list: &[String]) -> Vec<Keyword> {
    list.iter()
        .filter_map(|text| {
            let norm = normalize(text);
            (!norm.is_empty()).then(|| Keyword {
                tokens: norm.split(' ').map(str::to_owned).collect(),
                text: text.trim().to_owned(),
            })
        })
        .collect()
}

/// Every place where a keyword is said, in order. Where keywords overlap, the one with
/// more words wins.
pub(crate) fn mentions(norms: &[String], keywords: &[Keyword]) -> Vec<Mention> {
    let tokens: Vec<(&str, usize)> = norms
        .iter()
        .enumerate()
        .flat_map(|(i, norm)| {
            norm.split(' ')
                .filter(|t| !t.is_empty())
                .map(move |t| (t, i))
        })
        .collect();
    let mut found = Vec::new();
    let mut p = 0;
    while p < tokens.len() {
        let best = keywords
            .iter()
            .enumerate()
            .filter(|(_, keyword)| {
                let n = keyword.tokens.len();
                tokens.get(p..p + n).is_some_and(|window| {
                    window
                        .iter()
                        .zip(&keyword.tokens)
                        .all(|((token, _), wanted)| token == wanted)
                })
            })
            .max_by_key(|(_, keyword)| keyword.tokens.len());
        match best {
            Some((index, keyword)) => {
                let n = keyword.tokens.len();
                let first = tokens.get(p).map_or(0, |&(_, word)| word);
                let last = tokens.get(p + n - 1).map_or(first, |&(_, word)| word);
                found.push(Mention {
                    first,
                    last,
                    keyword: index,
                });
                p += n;
            }
            None => p += 1,
        }
    }
    found
}

/// The zooms of cut number `index` (counting from 0 in output order), which keeps
/// `section` of a source with these words and keyword mentions.
pub(crate) fn for_cut(
    index: usize,
    section: &Section,
    words: &[Word],
    mentions: &[Mention],
    keywords: &[Keyword],
    settings: &Zooms,
    grid: Grid,
) -> Vec<ZoomDraft> {
    let base = match settings.alternate {
        Some(scale) if index % 2 == 1 => scale,
        _ => 1.0,
    };
    let mut zooms = Vec::new();
    if !same(base, 1.0) {
        push(&mut zooms, section.start, base, None);
    }
    let Some(scale) = settings.keyword_scale else {
        return zooms;
    };
    let mut current = base;
    let mut back_at: Option<i64> = None;
    let inside = mentions.iter().filter(|mention| {
        section.words.contains(&mention.first) && section.words.contains(&mention.last)
    });
    for mention in inside {
        let (Some(first), Some(last)) = (words.get(mention.first), words.get(mention.last)) else {
            continue;
        };
        let frame = grid.nearest(micros(first.start())).max(section.start);
        if frame >= section.end {
            continue;
        }
        // A zoom back that is due before this keyword happens first; one that is not
        // yet due is dropped, so the zoom holds through this keyword too.
        if let Some(back) = back_at.take()
            && back < frame
            && push(&mut zooms, back, base, None)
        {
            current = base;
        }
        if !same(current, scale) {
            let text = keywords.get(mention.keyword).map(|k| k.text.clone());
            if push(&mut zooms, frame, scale, text) {
                current = scale;
            }
        }
        if let Some(hold) = settings.hold
            && same(current, scale)
        {
            back_at = Some(grid.nearest(micros(last.end() + hold)));
        }
    }
    if let Some(back) = back_at
        && back < section.end
    {
        push(&mut zooms, back, base, None);
    }
    zooms
}

/// Adds a zoom if it starts at least 1 frame after the one before it.
fn push(zooms: &mut Vec<ZoomDraft>, frame: i64, scale: f64, keyword: Option<String>) -> bool {
    if zooms.last().is_some_and(|last| frame <= last.frame) {
        return false;
    }
    zooms.push(ZoomDraft {
        frame,
        scale,
        keyword,
    });
    true
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norms(text: &str) -> Vec<String> {
        text.split(' ').map(normalize).collect()
    }

    #[test]
    fn mentions_match_whole_words_and_prefer_longer_keywords() {
        let keywords = keywords(&["Rust".into(), "GitHub Actions".into(), "GitHub".into()]);
        let found = mentions(
            &norms("I love Rust, GitHub Actions and GitHub. rusty"),
            &keywords,
        );
        let spans: Vec<(usize, usize, usize)> =
            found.iter().map(|m| (m.first, m.last, m.keyword)).collect();
        assert_eq!(spans, [(2, 2, 0), (3, 4, 1), (6, 6, 2)]);
    }

    fn word_list() -> Vec<Word> {
        // "We use Rust and Rust again", 0.5 s per word from 1.0 s.
        ["We", "use", "Rust", "and", "Rust", "again."]
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let start = 1.0 + 0.5 * i as f64;
                Word::new(*text, start, start + 0.5, 0.9)
            })
            .collect()
    }

    fn zooms_with(settings: &Zooms, index: usize) -> Vec<(i64, f64)> {
        let words = word_list();
        let keywords = keywords(&["rust".into()]);
        let norms: Vec<String> = words.iter().map(|w| normalize(w.text())).collect();
        let found = mentions(&norms, &keywords);
        let section = Section {
            start: 27,
            end: 123,
            words: 0..6,
        };
        for_cut(
            index,
            &section,
            &words,
            &found,
            &keywords,
            settings,
            Grid::new(30),
        )
        .into_iter()
        .map(|z| (z.frame, z.scale))
        .collect()
    }

    #[test]
    fn a_keyword_zoom_holds_to_the_end_of_the_cut_by_default() {
        // "Rust" starts at 2.0 s (frame 60); the second "Rust" changes nothing.
        assert_eq!(zooms_with(&Zooms::default(), 0), [(60, 1.25)]);
    }

    #[test]
    fn a_hold_zooms_back_after_the_keyword() {
        let settings = Zooms {
            hold: Some(0.2),
            ..Zooms::default()
        };
        // Back 0.2 s after "Rust" ends (2.7 s, frame 81), in again at 3.0 s (frame 90),
        // back at 3.7 s (frame 111).
        assert_eq!(
            zooms_with(&settings, 0),
            [(60, 1.25), (81, 1.0), (90, 1.25), (111, 1.0)]
        );
        // A hold that runs into the next mention keeps the zoom: back at 3.0 s is not
        // before "Rust" at 3.0 s, so the zoom holds until 0.5 s after it (4.0 s).
        let long = Zooms {
            hold: Some(0.5),
            ..Zooms::default()
        };
        assert_eq!(zooms_with(&long, 0), [(60, 1.25), (120, 1.0)]);
        // A zoom back that would land on the cut's end is left out.
        let longer = Zooms {
            hold: Some(0.6),
            ..Zooms::default()
        };
        assert_eq!(zooms_with(&longer, 0), [(60, 1.25)]);
    }

    #[test]
    fn every_second_cut_can_start_zoomed_in() {
        let settings = Zooms {
            alternate: Some(1.1),
            hold: Some(0.2),
            ..Zooms::default()
        };
        assert_eq!(zooms_with(&settings, 0).first(), Some(&(60, 1.25)));
        assert_eq!(
            zooms_with(&settings, 1),
            [(27, 1.1), (60, 1.25), (81, 1.1), (90, 1.25), (111, 1.1)]
        );
    }

    #[test]
    fn no_keyword_zooms_when_turned_off() {
        let settings = Zooms {
            keyword_scale: None,
            ..Zooms::default()
        };
        assert_eq!(zooms_with(&settings, 0), []);
    }
}
