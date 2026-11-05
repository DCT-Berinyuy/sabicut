//! cosmic-text (harfrust shaping, swash outlines) with tiny-skia, set up as captions
//! would need it: one embedded font, no system fonts.

#[path = "../draw.rs"]
mod draw;

use cosmic_text::{
    Attrs, Buffer, Command, Family, FontSystem, Metrics, Shaping, SwashCache, fontdb,
};
use tiny_skia::PathBuilder;

fn main() {
    let mut db = fontdb::Database::new();
    db.load_font_data(draw::ANTON.to_vec());
    let mut fonts = FontSystem::new_with_locale_and_db("fr-FR".to_owned(), db);
    let mut buffer = Buffer::new(&mut fonts, Metrics::new(draw::SIZE, draw::SIZE * 1.2));
    let attrs = Attrs::new().family(Family::Name("Anton"));
    buffer.set_text(draw::TEXT, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut fonts, false);
    let mut cache = SwashCache::new();
    let mut builder = PathBuilder::new();
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical = glyph.physical(draw::ORIGIN, 1.0);
            let (x, y) = (physical.x as f32, physical.y as f32);
            let Some(commands) = cache.get_outline_commands(&mut fonts, physical.cache_key) else {
                continue;
            };
            for command in commands {
                match *command {
                    Command::MoveTo(p) => builder.move_to(x + p.x, y - p.y),
                    Command::LineTo(p) => builder.line_to(x + p.x, y - p.y),
                    Command::QuadTo(c, p) => builder.quad_to(x + c.x, y - c.y, x + p.x, y - p.y),
                    Command::CurveTo(c1, c2, p) => {
                        builder.cubic_to(x + c1.x, y - c1.y, x + c2.x, y - c2.y, x + p.x, y - p.y)
                    }
                    Command::Close => builder.close(),
                }
            }
        }
    }
    let mut pixmap = draw::canvas();
    draw::outline_and_fill(&mut pixmap, &builder.finish().expect("path"));
    draw::finish(&pixmap);
}
