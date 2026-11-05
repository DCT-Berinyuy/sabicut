//! Drawing frames: the caption band and the hook, as tiny-skia pictures and as the
//! straight (not premultiplied) RGBA that ffmpeg's `overlay` expects.
//!
//! Each word is drawn once per colour and size into a small picture (outline, then
//! fill, anti-aliased) and kept while its caption page shows, so a frame is a few
//! copies. A frame that looks like the one before is not drawn again.

use std::collections::HashMap;
use std::sync::Arc;

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Pixmap, PixmapPaint, Stroke, Transform,
};

use crate::captions::{POP_SCALE, PlacedWord, TextStyle};
use crate::easing::{ease_out_back, frame_progress};
use crate::graphics::Graphics;
use crate::hook::{HookPose, thousandths};
use crate::shape::Shape;

/// Word pictures kept at most; the cache starts again when it is full. One page needs
/// about 50 (each word in its colour, and the active word at each step of its pop), and
/// the cache also starts again at every new page, so this is only a safety net.
const MAX_RASTERS: usize = 256;

/// Draws a plan's caption band and hook, frame by frame. Keeps the word pictures it
/// has drawn, so drawing consecutive frames is fast.
pub struct Painter<'g> {
    graphics: &'g Graphics,
    rasters: HashMap<RasterKey, Option<Arc<Raster>>>,
    /// The caption page the cached word pictures belong to.
    rasters_page: Option<usize>,
    band: Layer<BandState>,
    hook: Layer<HookPose>,
}

/// A picture the size of the band or the hook, what it shows, and the same picture as
/// straight RGBA.
struct Layer<S> {
    pixmap: Option<Pixmap>,
    state: Option<Option<S>>,
    rgba: Vec<u8>,
    rgba_state: Option<Option<S>>,
}

impl<S> Layer<S> {
    fn new(width: u32, height: u32) -> Self {
        Self {
            pixmap: Pixmap::new(width, height),
            state: None,
            rgba: Vec::new(),
            rgba_state: None,
        }
    }
}

/// What the band shows: a page, and its active word with its pop in thousandths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BandState {
    page: usize,
    active: Option<(usize, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RasterKey {
    text: Arc<str>,
    fill: [u8; 4],
    /// Pixels per font unit, in millionths.
    scale: u64,
    /// Outline width in hundredths of a pixel.
    stroke: u32,
}

/// One word, drawn: the picture and where its top-left corner is from the word's pen
/// position (the left end of its baseline).
struct Raster {
    pixmap: Pixmap,
    left: i32,
    top: i32,
}

impl<'g> Painter<'g> {
    pub(crate) fn new(graphics: &'g Graphics) -> Self {
        let band = graphics
            .band()
            .map_or((1, 1), |band| (band.width, band.height));
        let hook = graphics
            .hook()
            .map_or((1, 1), |hook| (hook.placement.width, hook.placement.height));
        Self {
            graphics,
            rasters: HashMap::new(),
            rasters_page: None,
            band: Layer::new(band.0, band.1),
            hook: Layer::new(hook.0, hook.1),
        }
    }

    /// The caption band on output frame `frame`, premultiplied, or `None` when no
    /// caption shows (or the plan has none).
    pub fn band(&mut self, frame: u64) -> Option<&Pixmap> {
        let state = self.band_state(frame);
        self.draw_band(state);
        state.and(self.band.pixmap.as_ref())
    }

    /// The caption band on output frame `frame` as straight RGBA, `width × height × 4`
    /// bytes: transparent when no caption shows. Empty if the plan has no captions.
    pub fn band_rgba(&mut self, frame: u64) -> &[u8] {
        if self.graphics.band().is_none() {
            return &[];
        }
        let state = self.band_state(frame);
        if self.band.rgba_state != Some(state) {
            self.draw_band(state);
            let Layer { pixmap, rgba, .. } = &mut self.band;
            match (state, pixmap.as_ref()) {
                (Some(_), Some(pixmap)) => straight_rgba(pixmap, rgba),
                (_, pixmap) => clear(rgba, pixmap),
            }
            self.band.rgba_state = Some(state);
        }
        &self.band.rgba
    }

    /// The hook on output frame `frame`, premultiplied, or `None` when it is not shown.
    pub fn hook(&mut self, frame: u64) -> Option<&Pixmap> {
        let pose = self
            .graphics
            .hook
            .as_ref()
            .and_then(|hook| hook.pose(frame));
        self.draw_hook(pose);
        pose.and(self.hook.pixmap.as_ref())
    }

    /// The hook on output frame `frame` as straight RGBA, `width × height × 4` bytes:
    /// transparent when it is not shown. Empty if the plan has no hook.
    pub fn hook_rgba(&mut self, frame: u64) -> &[u8] {
        let Some(layout) = self.graphics.hook.as_ref() else {
            return &[];
        };
        let pose = layout.pose(frame);
        if self.hook.rgba_state != Some(pose) {
            self.draw_hook(pose);
            let Layer { pixmap, rgba, .. } = &mut self.hook;
            match (pose, pixmap.as_ref()) {
                (Some(_), Some(pixmap)) => straight_rgba(pixmap, rgba),
                (_, pixmap) => clear(rgba, pixmap),
            }
            self.hook.rgba_state = Some(pose);
        }
        &self.hook.rgba
    }

    /// What the band shows on `frame`.
    fn band_state(&self, frame: u64) -> Option<BandState> {
        let captions = self.graphics.captions.as_ref()?;
        let (page, shown) = captions.page_at(frame)?;
        let active = shown.active_at(frame).map(|(index, word)| {
            let progress = frame_progress(frame - word.active.start, captions.pop_frames);
            let pop = 1.0 + (POP_SCALE - 1.0) * ease_out_back(progress);
            (index, thousandths(pop))
        });
        Some(BandState { page, active })
    }

    fn draw_band(&mut self, state: Option<BandState>) {
        if self.band.state == Some(state) {
            return;
        }
        self.band.state = Some(state);
        let graphics = self.graphics;
        let (Some(state), Some(captions)) = (state, graphics.captions.as_ref()) else {
            return;
        };
        let Some(page) = captions.pages.get(state.page) else {
            return;
        };
        // Pages show one after another, so the words of earlier pages are not needed
        // again: keep memory to one page's pictures.
        if self.rasters_page != Some(state.page) {
            self.rasters.clear();
            self.rasters_page = Some(state.page);
        }
        let Some(mut pixmap) = self.band.pixmap.take() else {
            return;
        };
        pixmap.fill(Color::TRANSPARENT);
        let style = &captions.style;
        let active = state.active.map(|(index, _)| index);
        for (index, word) in page.words.iter().enumerate() {
            if Some(index) != active {
                let fill = if word.keyword {
                    style.accent
                } else {
                    style.fill
                };
                self.draw_word(&mut pixmap, word, style, fill, 1.0, 1.0);
            }
        }
        // The active word last, on top of its neighbours if it grows into them.
        if let Some((index, pop)) = state.active
            && let Some(word) = page.words.get(index)
        {
            let pop = pop as f32 / 1000.0;
            self.draw_word(&mut pixmap, word, style, style.accent, pop, 1.0);
        }
        self.band.pixmap = Some(pixmap);
    }

    fn draw_hook(&mut self, pose: Option<HookPose>) {
        if self.hook.state == Some(pose) {
            return;
        }
        self.hook.state = Some(pose);
        let graphics = self.graphics;
        let (Some(pose), Some(layout)) = (pose, graphics.hook.as_ref()) else {
            return;
        };
        let Some(mut pixmap) = self.hook.pixmap.take() else {
            return;
        };
        pixmap.fill(Color::TRANSPARENT);
        let scale = pose.scale as f32 / 1000.0;
        let opacity = pose.opacity as f32 / 1000.0;
        let (cx, cy) = layout.centre;
        for word in &layout.words {
            let fill = if word.keyword {
                layout.style.accent
            } else {
                layout.style.fill
            };
            // Grow from the centre of the whole text, not of each word.
            let moved = PlacedWord {
                x: cx + scale * (word.x - cx),
                baseline: cy + scale * (word.baseline - cy),
                width: word.width * scale,
                fit: word.fit * scale,
                ..word.clone()
            };
            self.draw_word(&mut pixmap, &moved, &layout.style, fill, 1.0, opacity);
        }
        self.hook.pixmap = Some(pixmap);
    }

    /// Draws `word` in `fill` onto `target`, grown by `pop` about its centre and kept
    /// inside `target`. A word as wide as `target` grows only as far as it fits.
    fn draw_word(
        &mut self,
        target: &mut Pixmap,
        word: &PlacedWord,
        style: &TextStyle,
        fill: [u8; 4],
        pop: f32,
        opacity: f32,
    ) {
        let room = target.width() as f32 - 2.0 * (style.stroke + 1.0);
        let pop = if word.width > 0.0 {
            pop.min(room / word.width).max(1.0)
        } else {
            pop
        };
        let scale = style.scale * word.fit * pop;
        let key = RasterKey {
            text: Arc::clone(&word.text),
            fill,
            scale: millionths(scale),
            stroke: (style.stroke * 100.0).round().max(0.0) as u32,
        };
        if self.rasters.len() >= MAX_RASTERS && !self.rasters.contains_key(&key) {
            self.rasters.clear();
        }
        let raster = self
            .rasters
            .entry(key)
            .or_insert_with_key(|key| {
                let scale = key.scale as f32 / 1e6;
                rasterize(&word.shape, scale, style, fill).map(Arc::new)
            })
            .clone();
        let Some(raster) = raster else {
            return;
        };
        let cap = style.cap * word.fit;
        let (cx, cy) = (word.x + word.width / 2.0, word.baseline - cap / 2.0);
        let pen_x = (cx - pop * word.width / 2.0).round() as i32;
        let pen_y = (cy + pop * cap / 2.0).round() as i32;
        let x = keep_inside(pen_x + raster.left, raster.pixmap.width(), target.width());
        let y = keep_inside(pen_y + raster.top, raster.pixmap.height(), target.height());
        let paint = PixmapPaint {
            opacity,
            ..PixmapPaint::default()
        };
        target.draw_pixmap(
            x,
            y,
            raster.pixmap.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
    }
}

/// Moves a span of `size` starting at `start` inside `0..limit`, if it fits.
fn keep_inside(start: i32, size: u32, limit: u32) -> i32 {
    let room = i64::from(limit) - i64::from(size);
    if room < 0 {
        return 0;
    }
    i32::try_from(i64::from(start).clamp(0, room)).unwrap_or(0)
}

fn millionths(value: f32) -> u64 {
    let micro = (f64::from(value) * 1e6).round();
    if micro.is_finite() && micro > 0.0 {
        // Positive, whole and clamped, so the cast is exact.
        micro.min(1e15) as u64
    } else {
        0
    }
}

/// Draws a shape at `scale` pixels per font unit: the outline (twice the stroke width,
/// centred on the letters' edges, so `style.stroke` shows outside them), then the fill.
/// `None` for shapes without ink.
fn rasterize(shape: &Shape, scale: f32, style: &TextStyle, fill: [u8; 4]) -> Option<Raster> {
    let path = shape
        .path
        .clone()?
        .transform(Transform::from_row(scale, 0.0, 0.0, -scale, 0.0, 0.0))?;
    let bounds = path.bounds();
    let pad = style.stroke + 1.0;
    let left = (bounds.left() - pad).floor();
    let top = (bounds.top() - pad).floor();
    let width = ((bounds.right() + pad).ceil() - left).max(1.0);
    let height = ((bounds.bottom() + pad).ceil() - top).max(1.0);
    let mut pixmap = Pixmap::new(width as u32, height as u32)?;
    let shift = Transform::from_translate(-left, -top);
    if style.stroke > 0.0 {
        let pen = Stroke {
            width: style.stroke * 2.0,
            line_join: LineJoin::Round,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint(style.outline), &pen, shift, None);
    }
    pixmap.fill_path(&path, &paint(fill), FillRule::Winding, shift, None);
    Some(Raster {
        pixmap,
        left: left as i32,
        top: top as i32,
    })
}

fn paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    let [r, g, b, a] = rgba;
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

/// Writes `pixmap` into `rgba` as straight (not premultiplied) RGBA, 4 bytes a pixel:
/// what ffmpeg reads as `rgba`.
pub fn straight_rgba(pixmap: &Pixmap, rgba: &mut Vec<u8>) {
    rgba.clear();
    rgba.reserve(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let colour = pixel.demultiply();
        rgba.extend_from_slice(&[colour.red(), colour.green(), colour.blue(), colour.alpha()]);
    }
}

/// Makes `rgba` a transparent picture the size of `pixmap`.
fn clear(rgba: &mut Vec<u8>, pixmap: Option<&Pixmap>) {
    let len = pixmap.map_or(0, |pixmap| pixmap.data().len());
    rgba.clear();
    rgba.resize(len, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_kept_inside() {
        assert_eq!(keep_inside(-5, 10, 100), 0);
        assert_eq!(keep_inside(95, 10, 100), 90);
        assert_eq!(keep_inside(40, 10, 100), 40);
        assert_eq!(keep_inside(40, 200, 100), 0);
    }

    #[test]
    fn straight_rgba_undoes_premultiplication() {
        let mut pixmap = Pixmap::new(2, 1).unwrap();
        pixmap.fill(Color::from_rgba8(255, 196, 0, 128));
        let mut rgba = Vec::new();
        straight_rgba(&pixmap, &mut rgba);
        assert_eq!(rgba.len(), 8);
        assert_eq!(rgba[3], 128);
        assert!(
            rgba[0] >= 254 && (195..=197).contains(&rgba[1]) && rgba[2] == 0,
            "{rgba:?}"
        );
    }
}
