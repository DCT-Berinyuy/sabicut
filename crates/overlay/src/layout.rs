//! Geometry shared by captions and the hook: where text may go on the frame, and how
//! words wrap into lines.

use std::ops::Range;

use crate::safe_zone::TEXT_COLUMN;

/// Where a picture goes on the output frame, in pixels. Every value is even, so the
/// picture lines up with the 4:2:0 chroma of the video.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl Placement {
    /// The right edge (exclusive).
    pub fn right(&self) -> u32 {
        self.x + self.width
    }

    /// The bottom edge (exclusive).
    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }

    /// Whether the two share any pixel.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// The output frame's size, and the size text is measured against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub(crate) width: f64,
    pub(crate) height: f64,
    /// The width of the largest 9:16 area the frame holds: text sizes are fractions of
    /// it, so text looks the same on a 1080×1920 short and on other shapes.
    pub(crate) reference: f64,
}

impl Frame {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let (width, height) = (f64::from(width), f64::from(height));
        Self {
            width,
            height,
            reference: width.min(height * 9.0 / 16.0),
        }
    }

    /// The columns text may use, as `[left, right)` in pixels: centred, and clear of
    /// the action column.
    pub(crate) fn text_column(&self) -> (u32, u32) {
        (
            even_ceil(TEXT_COLUMN.0 * self.width),
            even_floor(TEXT_COLUMN.1 * self.width),
        )
    }
}

/// Slack for rounding to whole pixels: fractions of the frame like 0.85 × 1080 come out
/// a hair off (917.99999999999997), and must still round to 918.
const PIXEL_SLACK: f64 = 1e-6;

/// The largest even number at most `value` (0 for negative values).
pub(crate) fn even_floor(value: f64) -> u32 {
    to_u32(((value + PIXEL_SLACK) / 2.0).floor() * 2.0)
}

/// The smallest even number at least `value` (0 for negative values).
pub(crate) fn even_ceil(value: f64) -> u32 {
    to_u32(((value - PIXEL_SLACK) / 2.0).ceil() * 2.0)
}

fn to_u32(value: f64) -> u32 {
    if value.is_finite() && value > 0.0 {
        // Whole, positive and clamped to the range, so the cast is exact.
        value.min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

/// Breaks words of the given `widths` into lines at most `max_width` wide, `gap` apart,
/// filling each line before starting the next. A word wider than `max_width` gets a
/// line of its own.
pub(crate) fn greedy_lines(widths: &[f32], gap: f32, max_width: f32) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut width = 0.0_f32;
    for (index, &word) in widths.iter().enumerate() {
        if index > start && width + gap + word > max_width {
            lines.push(start..index);
            start = index;
            width = word;
        } else if index == start {
            width = word;
        } else {
            width += gap + word;
        }
    }
    if start < widths.len() {
        lines.push(start..widths.len());
    }
    lines
}

/// Like [`greedy_lines`], but with lines of even length: the fewest lines that fit in
/// `max_width`, made as narrow as they can be. `None` if they need more than
/// `max_lines` lines.
pub(crate) fn balanced_lines(
    widths: &[f32],
    gap: f32,
    max_width: f32,
    max_lines: usize,
) -> Option<Vec<Range<usize>>> {
    let lines = greedy_lines(widths, gap, max_width);
    if lines.len() > max_lines {
        return None;
    }
    if lines.len() <= 1 {
        return Some(lines);
    }
    // The narrowest whole-pixel width that still needs no more lines.
    let widest = widths.iter().copied().fold(0.0_f32, f32::max);
    let (mut low, mut high) = (widest.ceil(), max_width.floor().max(widest.ceil()));
    while low < high {
        let middle = ((low + high) / 2.0).floor();
        if greedy_lines(widths, gap, middle).len() <= lines.len() {
            high = middle;
        } else {
            low = middle + 1.0;
        }
    }
    Some(greedy_lines(widths, gap, high))
}

/// The width of the words in `range`, `gap` apart.
pub(crate) fn line_width(widths: &[f32], range: Range<usize>, gap: f32) -> f32 {
    let words = widths.get(range).unwrap_or_default();
    let gaps = words.len().saturating_sub(1) as f32 * gap;
    words.iter().sum::<f32>() + gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_column_on_a_short() {
        let frame = Frame::new(1080, 1920);
        assert_eq!(frame.reference, 1080.0);
        assert_eq!(frame.text_column(), (162, 918));
        assert_eq!(Frame::new(1920, 1080).reference, 607.5);
    }

    #[test]
    fn even_rounding() {
        assert_eq!((even_floor(161.9), even_ceil(161.1)), (160, 162));
        assert_eq!((even_floor(-3.0), even_ceil(f64::NAN)), (0, 0));
    }

    #[test]
    fn greedy_fills_lines() {
        let widths = [100.0, 100.0, 100.0, 300.0];
        assert_eq!(greedy_lines(&widths, 10.0, 320.0), [0..3, 3..4]);
        assert_eq!(greedy_lines(&widths, 10.0, 200.0), [0..1, 1..2, 2..3, 3..4]);
        assert_eq!(greedy_lines(&[500.0, 10.0], 10.0, 100.0), [0..1, 1..2]);
        assert!(greedy_lines(&[], 10.0, 100.0).is_empty());
    }

    #[test]
    fn balanced_lines_are_even() {
        // Greedy at 320 gives 3 + 1 words; balanced gives 2 + 2.
        let widths = [100.0, 100.0, 100.0, 100.0];
        assert_eq!(greedy_lines(&widths, 10.0, 320.0), [0..3, 3..4]);
        assert_eq!(
            balanced_lines(&widths, 10.0, 320.0, 2),
            Some(vec![0..2, 2..4])
        );
        assert_eq!(balanced_lines(&widths, 10.0, 200.0, 2), None);
        let one_word = balanced_lines(&[50.0], 10.0, 200.0, 2).unwrap();
        assert_eq!((one_word.len(), one_word.first()), (1, Some(&(0..1))));
        assert_eq!(line_width(&widths, 1..3, 10.0), 210.0);
    }

    #[test]
    fn placements_overlap_only_when_they_share_pixels() {
        let a = Placement {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = Placement {
            x: 0,
            y: 10,
            width: 10,
            height: 10,
        };
        let c = Placement {
            x: 9,
            y: 9,
            width: 4,
            height: 4,
        };
        assert!(!a.overlaps(&b));
        assert!(a.overlaps(&c) && b.overlaps(&c));
    }
}
