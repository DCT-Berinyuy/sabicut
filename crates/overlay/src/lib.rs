//! Sabicut's overlays (PRD F6): word-by-word captions and the hook text, drawn as RGBA
//! pictures for `sabicut-render` to place over the video, and subtitles (SRT).
//!
//! Everything is derived from the plan, its output [`Timeline`](sabicut_core::Timeline)
//! and the word timings, with fixes applied. Words are shaped with rustybuzz (kerning,
//! accents) and their outlines drawn with tiny-skia, so drawing is deterministic: the
//! same plan, timeline, words and frame give the same pixels every time.
//!
//! - **Captions** show the words in short pages of one or two lines inside the *caption
//!   band*, a strip of the frame that the apps' interface leaves clear
//!   ([`safe_zone`]). The word being spoken is drawn in the plan's `active` colour and
//!   pops (an ease-out-back scale), keywords are always in that colour, and every word
//!   has a thick outline.
//! - **The hook** is large text under the top bar during the hook's frames, with an
//!   entrance animation. It always sits above the caption band, so the two never
//!   overlap.
//! - **Subtitles** are phrase-level SRT cues on the output timeline ([`srt`]).
//!
//! Only the band and the hook's box are drawn, never whole frames (PRD §8).
//!
//! ```no_run
//! use std::path::Path;
//! use sabicut_core::{Plan, Timeline};
//! use sabicut_overlay::Graphics;
//!
//! let plan = Plan::from_toml_str(&std::fs::read_to_string("short/plan.toml")?)?;
//! let timeline = Timeline::from_plan(&plan)?;
//! let graphics = Graphics::load(&plan, &timeline, Path::new("short"))?.expect("captions");
//! let mut painter = graphics.painter();
//! if let Some(band) = graphics.band() {
//!     // Frame 30 of the caption band, as straight RGBA for ffmpeg.
//!     let rgba = painter.band_rgba(30);
//!     assert_eq!(rgba.len(), band.width as usize * band.height as usize * 4);
//! }
//! // Subtitles for the whole output.
//! std::fs::write("short/plan.srt", graphics.subtitles().unwrap_or_default())?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]
#![warn(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod captions;
pub mod easing;
mod error;
mod font;
mod graphics;
mod hook;
mod layout;
mod load;
mod paint;
pub mod safe_zone;
mod shape;
pub mod srt;
mod text;
mod timing;

pub use error::OverlayError;
pub use font::{BUNDLED_FONT, BUNDLED_FONT_NAME, Font, FontError};
pub use graphics::{Graphics, Hook};
pub use layout::Placement;
pub use load::load_word_timings;
pub use paint::{Painter, straight_rgba};
pub use timing::{OutputWord, WordTimings, place_words};
/// The picture type frames are drawn into (premultiplied RGBA), from tiny-skia.
pub use tiny_skia::Pixmap;
