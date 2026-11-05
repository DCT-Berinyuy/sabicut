//! Caption fonts: the bundled Anton, or a font file named by the plan.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};

use crate::error::OverlayError;

/// Anton Regular by Vernon Adams and the Anton Project Authors, embedded in the binary
/// so captions need no font file. It is licensed under the SIL Open Font License 1.1;
/// the license text is `fonts/OFL.txt` in this crate.
pub const BUNDLED_FONT: &[u8] = include_bytes!("../fonts/Anton-Regular.ttf");

/// The name of the bundled font.
pub const BUNDLED_FONT_NAME: &str = "Anton Regular";

/// Font files larger than this are not caption fonts.
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

/// A font that captions can be drawn with: an OpenType or TrueType font with outlines.
#[derive(Clone)]
pub struct Font {
    data: FontData,
    name: String,
}

#[derive(Clone)]
enum FontData {
    Bundled,
    Loaded(Arc<[u8]>),
}

/// Why a font cannot be used for captions.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FontError {
    /// The bytes are not an OpenType or TrueType font.
    #[error("not an OpenType or TrueType font")]
    NotAFont,
    /// The font has no outline for the letter "a", so it cannot draw captions (a bitmap
    /// or colour emoji font, or a font for another script).
    #[error("the font has no outline for the letter \"a\"; use a font for Latin text")]
    NoOutlines,
}

impl Font {
    /// The bundled Anton Regular.
    pub fn bundled() -> Self {
        Self {
            data: FontData::Bundled,
            name: BUNDLED_FONT_NAME.to_owned(),
        }
    }

    /// A font from its bytes; `name` is shown in messages.
    pub fn from_bytes(bytes: Vec<u8>, name: impl Into<String>) -> Result<Self, FontError> {
        let font = Self {
            data: FontData::Loaded(Arc::from(bytes)),
            name: name.into(),
        };
        font.check()?;
        Ok(font)
    }

    /// Reads a font file.
    pub fn load(path: &Path) -> Result<Self, OverlayError> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| file.take(MAX_FONT_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(OverlayError::read(path))?;
        if bytes.len() > MAX_FONT_BYTES {
            return Err(OverlayError::TooLarge {
                path: path.to_owned(),
                limit: MAX_FONT_BYTES,
                kind: "a font",
            });
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        Self::from_bytes(bytes, name).map_err(|source| OverlayError::Font {
            path: path.to_owned(),
            source,
        })
    }

    /// The font's name: [`BUNDLED_FONT_NAME`], or the file name it was loaded from.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this is the bundled Anton.
    pub fn is_bundled(&self) -> bool {
        matches!(self.data, FontData::Bundled)
    }

    /// The font file's bytes.
    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            FontData::Bundled => BUNDLED_FONT,
            FontData::Loaded(bytes) => bytes,
        }
    }

    /// The font, ready for shaping.
    pub(crate) fn face(&self) -> Result<rustybuzz::Face<'_>, FontError> {
        rustybuzz::Face::from_slice(self.bytes(), 0).ok_or(FontError::NotAFont)
    }

    fn check(&self) -> Result<(), FontError> {
        let face = self.face()?;
        let glyph = face.glyph_index('a').ok_or(FontError::NoOutlines)?;
        let mut sink = Sink;
        face.outline_glyph(glyph, &mut sink)
            .map(|_| ())
            .ok_or(FontError::NoOutlines)
    }
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("name", &self.name)
            .field("bytes", &self.bytes().len())
            .finish()
    }
}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.bytes() == other.bytes()
    }
}

/// Size measurements of a font, in font units (y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    /// Units per em.
    pub(crate) upem: f32,
    /// Height of capital letters above the baseline.
    pub(crate) cap: f32,
    /// How far accents and tall letters reach above the baseline, at least `cap`.
    pub(crate) ascent: f32,
    /// How far descenders reach below the baseline, as a positive number.
    pub(crate) descent: f32,
    /// Advance of a space.
    pub(crate) space: f32,
}

impl Metrics {
    pub(crate) fn of(face: &rustybuzz::Face<'_>) -> Self {
        // rustybuzz's own units_per_em is an i32; the font's head table holds a u16.
        let upem = f32::from(rustybuzz::ttf_parser::Face::units_per_em(face).max(1));
        let cap = face
            .capital_height()
            .filter(|&cap| cap > 0)
            .map_or(upem * 0.7, f32::from);
        let space = face
            .glyph_index(' ')
            .and_then(|glyph| face.glyph_hor_advance(glyph))
            .map_or(upem * 0.25, f32::from);
        Self {
            upem,
            cap,
            ascent: f32::from(face.ascender()).max(cap),
            descent: (-f32::from(face.descender())).max(0.0),
            space,
        }
    }
}

/// Throws an outline away; used to check that a glyph has one.
struct Sink;

impl OutlineBuilder for Sink {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

/// Glyph ids from shaping are 32-bit; fonts index glyphs with 16 bits.
pub(crate) fn glyph_id(id: u32) -> Option<GlyphId> {
    u16::try_from(id).ok().map(GlyphId)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_font_is_anton_with_outlines() {
        let font = Font::bundled();
        assert!(font.is_bundled());
        assert_eq!(font.name(), "Anton Regular");
        font.check().unwrap();
        let face = font.face().unwrap();
        // French needs these.
        for c in "éèêëàâçîïôûùüÿœæÉÈÀÇ«»’".chars() {
            assert!(face.glyph_index(c).is_some(), "Anton lacks {c:?}");
        }
        let metrics = Metrics::of(&face);
        assert!(metrics.cap > 0.0 && metrics.cap < metrics.upem);
        assert!(metrics.ascent >= metrics.cap);
        assert!(metrics.space > 0.0);
    }

    #[test]
    fn junk_is_not_a_font() {
        assert_eq!(
            Font::from_bytes(b"not a font".to_vec(), "junk.ttf").unwrap_err(),
            FontError::NotAFont
        );
    }

    #[test]
    fn a_missing_font_file_names_the_path() {
        let error = Font::load(Path::new("/nonexistent/sabicut/font.ttf")).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("/nonexistent/sabicut/font.ttf: "),
            "{error}"
        );
    }
}
