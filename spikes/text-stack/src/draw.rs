//! What every binary does the same way: the font, the text and the drawing.

// Not every binary uses every item.
#![allow(dead_code)]

use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, Pixmap, Stroke, Transform};

/// A French caption line: accents, a clause and a no-break space before the `!`.
pub const TEXT: &str = "Été comme hiver, le moulin tourne\u{a0}!";
/// The caption font Sabicut bundles.
pub const ANTON: &[u8] = include_bytes!("../../../crates/overlay/fonts/Anton-Regular.ttf");
/// Pixels per em: caption size on a 1080×1920 short.
pub const SIZE: f32 = 73.0;
/// Where the line starts: left edge and baseline, in pixels.
pub const ORIGIN: (f32, f32) = (20.0, 150.0);

/// A picture wide enough for the line.
pub fn canvas() -> Pixmap {
    Pixmap::new(1400, 240).expect("canvas")
}

/// Draws `path` the way captions are drawn: a 10 px black outline, then a gold fill.
pub fn outline_and_fill(pixmap: &mut Pixmap, path: &Path) {
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(0, 0, 0, 255);
    let stroke = Stroke {
        width: 20.0,
        line_join: LineJoin::Round,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), None);
    paint.set_color_rgba8(255, 196, 0, 255);
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// Prints a checksum of the picture and the font, so nothing is optimized away.
pub fn finish(pixmap: &Pixmap) {
    let ink: u64 = pixmap.data().iter().map(|&b| u64::from(b)).sum();
    let font: u64 = ANTON.iter().map(|&b| u64::from(b)).sum();
    println!("ink {ink} font {font}");
}
