//! Caption pages: the words in short groups of one or two lines, laid out in the
//! caption band, with the frames each word is active on.
//!
//! A new page starts after a sentence ends, at a pause of half a second or more, after
//! a clause once the page has three words, at eight words, and whenever the next word
//! would need a third line. A sentence or clause shorter than half a second that the
//! next word follows straight on (the end of a sentence from before a cut, say) stays
//! on the next page instead of flashing by. Lines are balanced (two lines of similar
//! width rather than a full line and a lone word). A page shows from its first word
//! until the next page starts, or until 0.4 s after its last word if the speaker
//! pauses.

use std::ops::Range;
use std::sync::Arc;

use crate::easing::frames_for;
use crate::error::OverlayError;
use crate::font::Metrics;
use crate::layout::{
    Frame, Placement, balanced_lines, even_ceil, even_floor, greedy_lines, line_width,
};
use crate::safe_zone::{BOTTOM, TOP_BAR};
use crate::shape::{Shape, Shaper};
use crate::text::{ends_clause, ends_sentence};
use crate::timing::OutputWord;

/// Height of capital letters in captions, as a fraction of the frame's reference width
/// (63 px on a 1080×1920 short).
const CAP_HEIGHT: f64 = 0.058;
/// Distance between baselines, in capital heights.
const LINE_SPACING: f32 = 1.5;
/// Lines per page.
const MAX_LINES: usize = 2;
/// Words per page.
const MAX_WORDS: usize = 8;
/// A clause end starts a new page once the page has this many words.
const CLAUSE_WORDS: usize = 3;
/// A pause this long starts a new page.
const PAGE_PAUSE_SECONDS: f64 = 0.5;
/// A sentence or clause shorter than this does not end a page on its own.
const MIN_PAGE_SECONDS: f64 = 0.5;
/// How long a page stays after its last word when the speaker pauses.
const HOLD_SECONDS: f64 = 0.4;
/// A word stays active through a gap this short, so the highlight does not flicker
/// between words.
const BRIDGE_SECONDS: f64 = 0.25;
/// Space between words, in ems, at least. Both words' outlines are added to it, so the
/// gap between the outlines is a normal space.
const WORD_SPACE_EM: f32 = 0.25;
/// Distance kept from the safe zones and the band's sides, as a fraction of the
/// reference width.
pub(crate) const MARGIN: f64 = 0.02;
/// Room kept around words for the pop, in ems.
const POP_ROOM_EM: f32 = 0.1;
/// How much the active word grows when it pops, and how long the pop takes. It
/// overshoots by about a tenth of the growth on the way (ease-out-back).
pub(crate) const POP_SCALE: f64 = 1.10;
pub(crate) const POP_SECONDS: f64 = 0.18;

/// How text is drawn: its size, outline and colours.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextStyle {
    /// Pixels per font unit.
    pub(crate) scale: f32,
    /// Height of capitals, in pixels.
    pub(crate) cap: f32,
    /// Outline width outside the letters, in pixels.
    pub(crate) stroke: f32,
    /// Fill of ordinary words, RGBA.
    pub(crate) fill: [u8; 4],
    /// Fill of the active word and keywords, RGBA.
    pub(crate) accent: [u8; 4],
    /// Outline colour, RGBA.
    pub(crate) outline: [u8; 4],
}

/// A word laid out in a picture.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlacedWord {
    pub(crate) text: Arc<str>,
    pub(crate) shape: Arc<Shape>,
    pub(crate) keyword: bool,
    /// Left end of its baseline in the picture, in whole pixels.
    pub(crate) x: f32,
    pub(crate) baseline: f32,
    /// Advance width in pixels, `fit` included.
    pub(crate) width: f32,
    /// Extra scale for a word wider than a line, at most 1.
    pub(crate) fit: f32,
    /// Output frames on which it is the word being spoken.
    pub(crate) active: Range<u64>,
}

/// One page of captions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Page {
    /// Output frames it is shown on.
    pub(crate) frames: Range<u64>,
    pub(crate) words: Vec<PlacedWord>,
}

impl Page {
    /// The word being spoken on `frame`, if any.
    pub(crate) fn active_at(&self, frame: u64) -> Option<(usize, &PlacedWord)> {
        self.words
            .iter()
            .enumerate()
            .find(|(_, word)| word.active.contains(&frame))
    }
}

/// Every caption page, and the band they are drawn in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CaptionLayout {
    /// Where the band goes on the output frame.
    pub(crate) band: Placement,
    pub(crate) style: TextStyle,
    /// Pages in output order, not overlapping.
    pub(crate) pages: Vec<Page>,
    /// Pop length, in frames.
    pub(crate) pop_frames: u64,
}

/// What the layout of one word needs.
struct Measured {
    shape: Arc<Shape>,
    width: f32,
    fit: f32,
}

impl CaptionLayout {
    pub(crate) fn new(
        frame: &Frame,
        shaper: &mut Shaper<'_>,
        words: &[OutputWord],
        style: TextStyle,
        timing: Timing,
    ) -> Result<Self, OverlayError> {
        let metrics = shaper.metrics();
        let em = metrics.upem * style.scale;
        let line_advance = (style.cap * LINE_SPACING).round();
        let band = band_placement(frame, &metrics, &style, line_advance)?;
        let margin = MARGIN * frame.reference;
        let max_width = band.width as f32 - 2.0 * (style.stroke + (margin / 2.0) as f32);
        if max_width < em {
            return Err(too_small(frame));
        }
        let geometry = PageGeometry {
            band,
            gap: (metrics.space * style.scale).max(WORD_SPACE_EM * em) + 2.0 * style.stroke,
            max_width,
            line_advance,
            cap: style.cap,
        };
        let measured = measure(shaper, words, &style, max_width);
        let widths: Vec<f32> = measured.iter().map(|m| m.width).collect();
        let groups = group_pages(words, &widths, geometry.gap, max_width, timing);
        let pages = groups
            .iter()
            .enumerate()
            .filter_map(|(index, group)| {
                let next_start = groups
                    .get(index + 1)
                    .and_then(|next| words.get(next.start))
                    .map_or(timing.total_frames, |word| word.start);
                lay_out_page(
                    words.get(group.clone())?,
                    measured.get(group.clone())?,
                    widths.get(group.clone())?,
                    geometry,
                    next_start,
                    timing,
                )
            })
            .collect();
        Ok(Self {
            band,
            style,
            pages,
            pop_frames: frames_for(POP_SECONDS, timing.fps),
        })
    }

    /// The page shown on `frame`, with its index.
    pub(crate) fn page_at(&self, frame: u64) -> Option<(usize, &Page)> {
        let after = self
            .pages
            .partition_point(|page| page.frames.start <= frame);
        let index = after.checked_sub(1)?;
        let page = self.pages.get(index)?;
        page.frames.contains(&frame).then_some((index, page))
    }
}

/// The caption cap height in pixels for a frame.
pub(crate) fn cap_height(frame: &Frame) -> f32 {
    (CAP_HEIGHT * frame.reference) as f32
}

fn too_small(frame: &Frame) -> OverlayError {
    OverlayError::TooSmall {
        width: frame.width as u32,
        height: frame.height as u32,
    }
}

/// Where the caption band goes: the text column, just above the bottom zone, tall
/// enough for two lines with their accents, descenders, outline and pop.
fn band_placement(
    frame: &Frame,
    metrics: &Metrics,
    style: &TextStyle,
    line_advance: f32,
) -> Result<Placement, OverlayError> {
    let em = metrics.upem * style.scale;
    let above = (metrics.ascent - metrics.cap) * style.scale;
    let below = metrics.descent * style.scale;
    let room = above.max(below) + style.stroke + POP_ROOM_EM * em;
    let block = style.cap + line_advance * (MAX_LINES as f32 - 1.0);
    let height = even_ceil(f64::from(block + 2.0 * room));
    let margin = MARGIN * frame.reference;
    let bottom = even_floor(BOTTOM.top * frame.height - margin);
    let top_limit = even_ceil(TOP_BAR.bottom * frame.height + margin);
    let (left, right) = frame.text_column();
    match bottom.checked_sub(height) {
        Some(y) if y >= top_limit && right > left => Ok(Placement {
            x: left,
            y,
            width: right - left,
            height,
        }),
        _ => Err(too_small(frame)),
    }
}

/// Shapes the words and measures them at the caption size, shrinking any word wider
/// than a line.
fn measure(
    shaper: &mut Shaper<'_>,
    words: &[OutputWord],
    style: &TextStyle,
    max_width: f32,
) -> Vec<Measured> {
    words
        .iter()
        .map(|word| {
            let shape = shaper.shape(&word.text);
            let natural = shape.advance * style.scale;
            let fit = if natural > max_width {
                max_width / natural
            } else {
                1.0
            };
            Measured {
                shape,
                width: natural * fit,
                fit,
            }
        })
        .collect()
}

/// Frame rate and length of the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timing {
    pub(crate) fps: u32,
    pub(crate) total_frames: u64,
}

/// Splits the words into pages, as ranges of indexes.
fn group_pages(
    words: &[OutputWord],
    widths: &[f32],
    gap: f32,
    max_width: f32,
    timing: Timing,
) -> Vec<Range<usize>> {
    let pause = frames_for(PAGE_PAUSE_SECONDS, timing.fps);
    let shortest = frames_for(MIN_PAGE_SECONDS, timing.fps);
    let fits = |range: Range<usize>| {
        widths
            .get(range)
            .and_then(|w| balanced_lines(w, gap, max_width, MAX_LINES))
            .is_some()
    };
    let mut pages = Vec::new();
    let mut start = 0;
    for (index, word) in words.iter().enumerate() {
        if index == start {
            continue;
        }
        let count = index - start;
        let breaks = match (words.get(index - 1), words.get(start)) {
            (Some(previous), Some(first)) => {
                let readable = previous.end.saturating_sub(first.start) >= shortest;
                word.start.saturating_sub(previous.end) >= pause
                    || (readable && ends_sentence(&previous.text))
                    || (readable && ends_clause(&previous.text) && count >= CLAUSE_WORDS)
                    || count >= MAX_WORDS
                    || !fits(start..index + 1)
            }
            _ => false,
        };
        if breaks {
            pages.push(start..index);
            start = index;
        }
    }
    if start < words.len() {
        pages.push(start..words.len());
    }
    pages
}

/// The fixed geometry pages are laid out with.
#[derive(Clone, Copy)]
struct PageGeometry {
    band: Placement,
    gap: f32,
    max_width: f32,
    line_advance: f32,
    cap: f32,
}

/// Lays out one page's words in the band, centred, and works out when each is active.
/// `None` if the page would not be shown on any frame.
fn lay_out_page(
    words: &[OutputWord],
    measured: &[Measured],
    widths: &[f32],
    geometry: PageGeometry,
    next_start: u64,
    timing: Timing,
) -> Option<Page> {
    let (first, last) = (words.first()?, words.last()?);
    let hold = frames_for(HOLD_SECONDS, timing.fps);
    let bridge = frames_for(BRIDGE_SECONDS, timing.fps);
    let end = next_start
        .min(last.end.saturating_add(hold))
        .min(timing.total_frames);
    if end <= first.start {
        return None;
    }
    // Pages are cut so that they fit in two lines; greedy lines are a safety net.
    let lines = balanced_lines(widths, geometry.gap, geometry.max_width, MAX_LINES)
        .unwrap_or_else(|| greedy_lines(widths, geometry.gap, geometry.max_width));
    let count = lines.len().max(1) as f32;
    let block = geometry.cap + geometry.line_advance * (count - 1.0);
    let top = ((geometry.band.height as f32 - block) / 2.0).round();
    let mut placed = Vec::with_capacity(words.len());
    for (row, line) in lines.iter().enumerate() {
        let width = line_width(widths, line.clone(), geometry.gap);
        let mut x = ((geometry.band.width as f32 - width) / 2.0).round();
        let baseline = (top + geometry.cap + geometry.line_advance * row as f32).round();
        for index in line.clone() {
            let (Some(word), Some(size)) = (words.get(index), measured.get(index)) else {
                continue;
            };
            let active_end = match words.get(index + 1) {
                Some(next) if next.start.saturating_sub(word.end) <= bridge => next.start,
                _ => word.end,
            };
            let active_end = active_end.max(word.start + 1).min(end);
            placed.push(PlacedWord {
                text: Arc::from(word.text.as_str()),
                shape: Arc::clone(&size.shape),
                keyword: word.keyword,
                x,
                baseline,
                width: size.width,
                fit: size.fit,
                active: word.start..active_end,
            });
            x = (x + size.width + geometry.gap).round();
        }
    }
    Some(Page {
        frames: first.start..end,
        words: placed,
    })
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

    const TIMING: Timing = Timing {
        fps: 30,
        total_frames: 3000,
    };

    #[test]
    fn pages_break_at_sentences_pauses_clauses_and_width() {
        let words = [
            word("One", 0, 10),
            word("two.", 10, 20),
            word("Three", 20, 30),
            word("four", 30, 40),
            // A pause of 15 frames (0.5 s).
            word("five", 55, 65),
            word("six,", 65, 75),
            word("seven", 75, 85),
            word("eight,", 85, 95),
            word("nine", 95, 105),
        ];
        let widths = vec![100.0; words.len()];
        let pages = group_pages(&words, &widths, 10.0, 1000.0, TIMING);
        assert_eq!(pages, [0..2, 2..4, 4..8, 8..9]);
        // Only two words fit on each of the two lines.
        let narrow = group_pages(&words[4..], &widths[4..], 10.0, 210.0, TIMING);
        assert_eq!(narrow, [0..4, 4..5]);
    }

    #[test]
    fn a_fragment_too_short_to_read_stays_on_the_next_page() {
        let words = [
            word("it?\"", 4, 6),
            word("\"And", 6, 12),
            word("what", 12, 18),
            word("now?\"", 18, 40),
            word("Then", 40, 50),
        ];
        let pages = group_pages(&words, &[50.0; 5], 10.0, 1000.0, TIMING);
        assert_eq!(pages, [0..4, 4..5]);
    }

    #[test]
    fn pages_hold_at_most_eight_words() {
        let words: Vec<OutputWord> = (0..10).map(|i| word("a", i * 3, i * 3 + 3)).collect();
        let pages = group_pages(&words, &[10.0; 10], 1.0, 1000.0, TIMING);
        assert_eq!(pages, [0..8, 8..10]);
    }
}
