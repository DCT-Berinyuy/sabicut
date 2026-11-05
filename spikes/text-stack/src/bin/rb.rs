//! rustybuzz (shaping and outlines through its ttf-parser) with tiny-skia, as
//! sabicut-overlay draws captions.

#[path = "../draw.rs"]
mod draw;

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use rustybuzz::{Face, UnicodeBuffer};
use tiny_skia::PathBuilder;

/// Appends a glyph's outline in font units (y up) to a path in pixels (y down).
struct Outline<'b> {
    builder: &'b mut PathBuilder,
    x: f32,
    y: f32,
    scale: f32,
}

impl Outline<'_> {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y - y * self.scale)
    }
}

impl OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.builder.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.builder.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let ((x1, y1), (x, y)) = (self.at(x1, y1), self.at(x, y));
        self.builder.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let ((x1, y1), (x2, y2), (x, y)) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.builder.close();
    }
}

fn main() {
    let face = Face::from_slice(draw::ANTON, 0).expect("font");
    let mut text = UnicodeBuffer::new();
    text.push_str(draw::TEXT);
    let glyphs = rustybuzz::shape(&face, &[], text);
    let scale = draw::SIZE / face.units_per_em() as f32;
    let mut builder = PathBuilder::new();
    let (mut pen, baseline) = draw::ORIGIN;
    for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
        let mut outline = Outline {
            builder: &mut builder,
            x: pen + position.x_offset as f32 * scale,
            y: baseline - position.y_offset as f32 * scale,
            scale,
        };
        let _ = face.outline_glyph(GlyphId(info.glyph_id as u16), &mut outline);
        pen += position.x_advance as f32 * scale;
    }
    let mut pixmap = draw::canvas();
    draw::outline_and_fill(&mut pixmap, &builder.finish().expect("path"));
    draw::finish(&pixmap);
}
