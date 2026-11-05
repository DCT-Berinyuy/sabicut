//! Baseline: tiny-skia and the embedded font, but no text stack. Draws an outlined
//! rectangle where the text would go.

#[path = "../draw.rs"]
mod draw;

use tiny_skia::{PathBuilder, Rect};

fn main() {
    let mut pixmap = draw::canvas();
    let rect = Rect::from_xywh(draw::ORIGIN.0, 90.0, 1100.0, 60.0).expect("rect");
    draw::outline_and_fill(&mut pixmap, &PathBuilder::from_rect(rect));
    draw::finish(&pixmap);
}
