//! Shaping: text to glyph outlines with rustybuzz (HarfBuzz's rules: kerning,
//! accents, ligatures), as one tiny-skia path per run of text.

use std::collections::HashMap;
use std::sync::Arc;

use rustybuzz::ttf_parser::OutlineBuilder;
use rustybuzz::{Face, UnicodeBuffer};
use tiny_skia::{Path, PathBuilder};

use crate::font::{Metrics, glyph_id};

/// A run of text, shaped: its outline and how far it advances the pen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    /// The glyph outlines in font units, y up, with the pen starting at (0, 0) on the
    /// baseline. `None` when nothing has ink (spaces).
    pub(crate) path: Option<Path>,
    /// Advance width in font units.
    pub(crate) advance: f32,
}

/// Shapes text with one font, keeping every shape it made.
pub(crate) struct Shaper<'f> {
    face: Face<'f>,
    metrics: Metrics,
    shapes: HashMap<String, Arc<Shape>>,
    /// Characters the font has no glyph for, in the order first met.
    missing: Vec<char>,
}

impl<'f> Shaper<'f> {
    pub(crate) fn new(face: Face<'f>) -> Self {
        let metrics = Metrics::of(&face);
        Self {
            face,
            metrics,
            shapes: HashMap::new(),
            missing: Vec::new(),
        }
    }

    pub(crate) fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// Characters met so far that the font cannot draw.
    pub(crate) fn missing(&self) -> &[char] {
        &self.missing
    }

    /// The shape of `text`, made once.
    pub(crate) fn shape(&mut self, text: &str) -> Arc<Shape> {
        if let Some(shape) = self.shapes.get(text) {
            return Arc::clone(shape);
        }
        let shape = Arc::new(self.shape_uncached(text));
        self.shapes.insert(text.to_owned(), Arc::clone(&shape));
        shape
    }

    fn shape_uncached(&mut self, text: &str) -> Shape {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let glyphs = rustybuzz::shape(&self.face, &[], buffer);
        let mut builder = PathBuilder::new();
        let (mut pen_x, mut pen_y) = (0.0_f32, 0.0_f32);
        for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            if info.glyph_id == 0 {
                self.note_missing(text, info.cluster);
            }
            if let Some(glyph) = glyph_id(info.glyph_id) {
                let mut outline = Outline {
                    builder: &mut builder,
                    dx: pen_x + position.x_offset as f32,
                    dy: pen_y + position.y_offset as f32,
                };
                // Glyphs without ink (spaces) have no outline; that is fine.
                let _ = self.face.outline_glyph(glyph, &mut outline);
            }
            pen_x += position.x_advance as f32;
            pen_y += position.y_advance as f32;
        }
        Shape {
            path: builder.finish(),
            advance: pen_x,
        }
    }

    fn note_missing(&mut self, text: &str, cluster: u32) {
        let character = usize::try_from(cluster)
            .ok()
            .and_then(|start| text.get(start..))
            .and_then(|rest| rest.chars().next());
        if let Some(character) = character.filter(|c| !c.is_whitespace())
            && !self.missing.contains(&character)
        {
            self.missing.push(character);
        }
    }
}

/// Appends glyph outlines to a path, moved to where the glyph sits.
struct Outline<'b> {
    builder: &'b mut PathBuilder,
    dx: f32,
    dy: f32,
}

impl OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x + self.dx, y + self.dy);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x + self.dx, y + self.dy);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.builder
            .quad_to(x1 + self.dx, y1 + self.dy, x + self.dx, y + self.dy);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            x1 + self.dx,
            y1 + self.dy,
            x2 + self.dx,
            y2 + self.dy,
            x + self.dx,
            y + self.dy,
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Font;

    #[test]
    fn kerning_and_accents_are_shaped() {
        let font = Font::bundled();
        let mut shaper = Shaper::new(font.face().unwrap());
        let separate = shaper.shape("A").advance + shaper.shape("V").advance;
        let pair = shaper.shape("AV").advance;
        assert!(pair < separate, "no kerning: AV {pair} vs A + V {separate}");
        // A precomposed é and e followed by a combining acute come out the same.
        let composed = shaper.shape("été");
        let decomposed = shaper.shape("e\u{301}te\u{301}");
        assert_eq!(composed.advance, decomposed.advance);
        assert!(shaper.missing().is_empty(), "{:?}", shaper.missing());
        assert!(shaper.shape(" ").path.is_none());
    }

    #[test]
    fn french_spaces_need_no_glyph_of_their_own() {
        // Anton has no narrow no-break space (U+202F), which French puts before `!`,
        // `?` and `;`; shaping falls back to its space, so nothing shows as a box.
        let font = Font::bundled();
        let mut shaper = Shaper::new(font.face().unwrap());
        let narrow = shaper.shape("moulin\u{202f}!");
        let wide = shaper.shape("moulin\u{a0}!");
        assert!(shaper.missing().is_empty(), "{:?}", shaper.missing());
        assert!(narrow.advance > shaper.shape("moulin!").advance);
        assert!(narrow.advance <= wide.advance);
    }

    #[test]
    fn missing_characters_are_listed_once() {
        let font = Font::bundled();
        let mut shaper = Shaper::new(font.face().unwrap());
        shaper.shape("a\u{1F600}b\u{1F600}");
        assert_eq!(shaper.missing(), ['\u{1F600}']);
    }

    #[test]
    fn shapes_are_made_once() {
        let font = Font::bundled();
        let mut shaper = Shaper::new(font.face().unwrap());
        let first = shaper.shape("Sabicut");
        let second = shaper.shape("Sabicut");
        assert!(Arc::ptr_eq(&first, &second));
    }
}
