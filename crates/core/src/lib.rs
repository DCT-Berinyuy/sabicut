//! Sabicut's core: the `plan.toml` schema v1, its validation, exact rational time,
//! and the output timeline derived from a plan's cuts. It also defines the word
//! timings file (`words.json`, [`Transcript`]) and the transcript fixes
//! (`fixes.toml`, [`Fixes`], [`apply_fixes`]), with no dependency on a speech model.
//!
//! ```
//! use sabicut_core::{Plan, Timeline};
//!
//! let plan = Plan::from_toml_str(
//!     r#"
//!     schema = 1
//!
//!     [meta]
//!     fps = 30
//!     out = { w = 1080, h = 1920 }
//!
//!     [[source]]
//!     id = "a"
//!     path = "raw/IMG_0412.mov"
//!     crop_x = 0.42
//!
//!     [[cut]]
//!     src = "a"
//!     in = 3.120
//!     out = 9.880
//!     zoom = [{ at = 5.4, scale = 1.25 }]
//!     "#,
//! )?;
//!
//! let timeline = Timeline::from_plan(&plan)?;
//! // `in` and `out` snap to frames 94 and 296 of the source: exactly 202 frames.
//! assert_eq!(timeline.total_frames(), 202);
//! // The zoom at 5.4 s of source time starts at frame 68 of the output.
//! assert_eq!(timeline.segments()[0].zooms()[0].output_frame(), 68);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]
#![warn(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod fixes;
mod plan;
mod rational;
mod schema_gen;
mod timeline;
mod validate;
mod words;

pub use plan::{
    Captions, Cut, Hook, Meta, OutputSize, ParseError, Plan, SCHEMA_VERSION, Source, Zoom,
};
pub use rational::Rational;
pub use timeline::{Landing, Segment, SourceMapping, Timeline, ZoomEvent};
pub use validate::{ValidationError, ValidationErrors};

pub use fixes::{
    FIXES_SCHEMA_VERSION, Fix, FixChange, FixError, Fixed, Fixes, FixesParseError, apply_fixes,
};
pub use words::{SpeechSegment, Transcript, TranscriptParseError, WORDS_SCHEMA_VERSION, Word};

/// The JSON Schema of `plan.toml` schema v1, pretty-printed with a trailing newline.
///
/// It is generated from the [`Plan`] types and is exactly the content of the
/// repository's `schema/v1.json`. TOML has no null, so optional keys are described by
/// their value type and are simply not listed as required.
pub fn json_schema() -> String {
    schema_gen::pretty::<Plan>()
}
