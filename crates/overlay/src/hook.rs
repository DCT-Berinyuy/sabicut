//! The hook: large text under the top bar on the hook's frames (the first `dur`
//! seconds), with an entrance animation.
//!
//! The text is centred in the same column as the captions, starting just below the top
//! bar. It is set as large as fits (capitals from 8.5% down to 4.5% of the reference
//! width, at most four balanced lines) in the space between the top bar and the
//! caption band, so the hook never overlaps the captions. It enters by growing from 70%
//! to full size with an ease-out-back pop and fading in, over 0.3 s, then holds still.

use std::ops::Range;
use std::sync::Arc;

use crate::captions::{MARGIN, PlacedWord, TextStyle};
use crate::easing::{ease_out_back, ease_out_cubic, frame_progress, frames_for};
use crate::error::OverlayError;
use crate::layout::{Frame, Placement, balanced_lines, even_ceil, even_floor, line_width};
use crate::safe_zone::TOP_BAR;
use crate::shape::{Shape, Shaper};
use crate::text::{keyword_flags, split_words};

/// Largest and smallest capital height, as fractions of the reference width.
const CAP_MAX: f64 = 0.085;
const CAP_MIN: f64 = 0.045;
/// Each try is this much smaller than the one before.
const SIZE_STEP: f64 = 0.92;
/// Distance between baselines, in capital heights.
const LINE_SPACING: f32 = 1.4;
const MAX_LINES: usize = 4;
/// The hook never reaches below this fraction of the frame height.
const MAX_BOTTOM: f64 = 0.45;
/// Space between words, in ems, at least. Both words' outlines are added to it.
const WORD_SPACE_EM: f32 = 0.25;
/// Room for the entrance's overshoot, in ems.
const GROW_ROOM_EM: f32 = 0.06;
/// The entrance: how long, and the size it starts from.
const ENTRANCE_SECONDS: f64 = 0.3;
const START_SCALE: f64 = 0.7;

/// The hook, laid out in its picture.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HookLayout {
    pub(crate) placement: Placement,
    pub(crate) style: TextStyle,
    pub(crate) words: Vec<PlacedWord>,
    /// The centre of the text in the picture, which the entrance grows from.
    pub(crate) centre: (f32, f32),
    /// The hook is drawn on output frames `0..frames`.
    pub(crate) frames: u64,
    /// The first `entrance` of them are animated.
    pub(crate) entrance: u64,
}

/// How the hook looks on one frame: its scale and opacity, in thousandths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HookPose {
    pub(crate) scale: u32,
    pub(crate) opacity: u32,
}

/// What the hook says and when it shows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HookText<'a> {
    pub(crate) text: &'a str,
    pub(crate) keywords: &'a [String],
    /// The hook is shown on output frames `0..frames`.
    pub(crate) frames: u64,
    pub(crate) fps: u32,
}

/// The hook's words, shaped.
struct HookWords<'a> {
    words: Vec<&'a str>,
    flags: Vec<bool>,
    shapes: Vec<Arc<Shape>>,
}

/// The hook set at one size.
struct Fitted {
    style: TextStyle,
    room: f32,
    gap: f32,
    line_advance: f32,
    lines: Vec<Range<usize>>,
    widths: Vec<f32>,
    fits: Vec<f32>,
    height: u32,
}

impl HookLayout {
    /// Lays out the hook in the space between the top bar and `band` (or 45% of the
    /// frame height). `base` gives the colours, and the outline width that goes with
    /// the caption cap height `base.cap`; the hook's outline grows with its size.
    pub(crate) fn new(
        frame: &Frame,
        shaper: &mut Shaper<'_>,
        hook: HookText<'_>,
        base: &TextStyle,
        band: Option<Placement>,
    ) -> Result<Self, OverlayError> {
        let words = split_words(hook.text);
        let flags = keyword_flags(words.iter().copied(), hook.keywords);
        let shapes = words.iter().map(|word| shaper.shape(word)).collect();
        let words = HookWords {
            words,
            flags,
            shapes,
        };
        let margin = MARGIN * frame.reference;
        let top = even_ceil(TOP_BAR.bottom * frame.height + margin);
        let mut bottom = even_floor(MAX_BOTTOM * frame.height);
        if let Some(band) = band {
            bottom = bottom.min(even_floor(f64::from(band.y) - margin));
        }
        let (left, right) = frame.text_column();
        if bottom <= top || right <= left || base.cap <= 0.0 {
            return Err(OverlayError::TooSmall {
                width: frame.width as u32,
                height: frame.height as u32,
            });
        }
        let width = right - left;
        let min_cap = CAP_MIN * frame.reference;
        let mut cap = CAP_MAX * frame.reference;
        loop {
            let at_min = cap <= min_cap;
            let fitted = fit(shaper, &words, base, cap as f32, width, margin, at_min)
                .filter(|fitted| fitted.height <= bottom - top);
            if let Some(fitted) = fitted {
                return Ok(Self::place(&words, fitted, left, top, width, hook));
            }
            if at_min {
                return Err(OverlayError::HookTooLong {
                    words: words.words.len(),
                });
            }
            cap = (cap * SIZE_STEP).max(min_cap);
        }
    }

    fn place(
        words: &HookWords<'_>,
        fitted: Fitted,
        left: u32,
        top: u32,
        width: u32,
        hook: HookText<'_>,
    ) -> Self {
        let cap = fitted.style.cap;
        let mut placed = Vec::with_capacity(words.words.len());
        for (row, line) in fitted.lines.iter().enumerate() {
            let line_px = line_width(&fitted.widths, line.clone(), fitted.gap);
            let mut x = ((width as f32 - line_px) / 2.0).round();
            let baseline = (fitted.room + cap + fitted.line_advance * row as f32).round();
            for index in line.clone() {
                let (Some(word), Some(shape), Some(fit), Some(w)) = (
                    words.words.get(index),
                    words.shapes.get(index),
                    fitted.fits.get(index),
                    fitted.widths.get(index),
                ) else {
                    continue;
                };
                placed.push(PlacedWord {
                    text: Arc::from(*word),
                    shape: Arc::clone(shape),
                    keyword: words.flags.get(index).copied().unwrap_or(false),
                    x,
                    baseline,
                    width: *w,
                    fit: *fit,
                    active: 0..0,
                });
                x = (x + w + fitted.gap).round();
            }
        }
        let block = cap + fitted.line_advance * (fitted.lines.len().max(1) as f32 - 1.0);
        Self {
            placement: Placement {
                x: left,
                y: top,
                width,
                height: fitted.height,
            },
            centre: (width as f32 / 2.0, fitted.room + block / 2.0),
            style: fitted.style,
            words: placed,
            frames: hook.frames,
            entrance: frames_for(ENTRANCE_SECONDS, hook.fps).min(hook.frames),
        }
    }

    /// How the hook looks on output frame `frame`, or `None` when it is not shown.
    pub(crate) fn pose(&self, frame: u64) -> Option<HookPose> {
        if frame >= self.frames {
            return None;
        }
        let progress = frame_progress(frame, self.entrance);
        let scale = START_SCALE + (1.0 - START_SCALE) * ease_out_back(progress);
        let opacity = ease_out_cubic((progress * 2.0).min(1.0));
        Some(HookPose {
            scale: thousandths(scale),
            opacity: thousandths(opacity),
        })
    }
}

/// The hook set with capitals `cap_px` high in a column `width` wide, if its words fit
/// in at most four lines. Words wider than a line shrink to fit only at the smallest
/// size (`at_min`).
fn fit(
    shaper: &Shaper<'_>,
    words: &HookWords<'_>,
    base: &TextStyle,
    cap_px: f32,
    width: u32,
    margin: f64,
    at_min: bool,
) -> Option<Fitted> {
    let metrics = shaper.metrics();
    let scale = cap_px / metrics.cap;
    let em = metrics.upem * scale;
    let stroke = (base.stroke * cap_px / base.cap).round();
    let room = ((metrics.ascent - metrics.cap) * scale).max(metrics.descent * scale)
        + stroke
        + GROW_ROOM_EM * em;
    let side = stroke + GROW_ROOM_EM * em + (margin / 2.0) as f32;
    let max_width = width as f32 - 2.0 * side;
    let natural: Vec<f32> = words.shapes.iter().map(|s| s.advance * scale).collect();
    if max_width <= 0.0 || (!at_min && natural.iter().any(|w| *w > max_width)) {
        return None;
    }
    let fits: Vec<f32> = natural
        .iter()
        .map(|w| if *w > max_width { max_width / w } else { 1.0 })
        .collect();
    let widths: Vec<f32> = natural.iter().zip(&fits).map(|(w, f)| w * f).collect();
    let gap = (metrics.space * scale).max(WORD_SPACE_EM * em) + 2.0 * stroke;
    let lines = balanced_lines(&widths, gap, max_width, MAX_LINES)?;
    let line_advance = (cap_px * LINE_SPACING).round();
    let block = cap_px + line_advance * (lines.len().max(1) as f32 - 1.0);
    Some(Fitted {
        style: TextStyle {
            scale,
            cap: cap_px,
            stroke,
            ..base.clone()
        },
        room,
        gap,
        line_advance,
        lines,
        widths,
        fits,
        height: even_ceil(f64::from(block + 2.0 * room)),
    })
}

/// `value` in whole thousandths.
pub(crate) fn thousandths(value: f64) -> u32 {
    let milli = (value * 1000.0).round();
    if milli.is_finite() && milli > 0.0 {
        // Positive, whole and small, so the cast is exact.
        milli.min(1e6) as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousandths_round() {
        assert_eq!(thousandths(1.0), 1000);
        assert_eq!(thousandths(0.7364), 736);
        assert_eq!(thousandths(-1.0), 0);
        assert_eq!(thousandths(f64::NAN), 0);
    }
}
