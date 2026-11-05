//! Errors from loading and laying out captions, the hook and subtitles.

use std::io;
use std::path::PathBuf;

use sabicut_core::{FixError, FixesParseError, TranscriptParseError, ValidationErrors};

use crate::font::FontError;

/// Why captions, the hook or subtitles could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    /// A words.json, fixes.toml or font file could not be read.
    #[error("{}: {source}", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// A file is too big to be what it should be.
    #[error("{}: file is larger than {} MiB, so it is not {kind}", .path.display(), .limit / 1024 / 1024)]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The size limit in bytes.
        limit: usize,
        /// What it should be, like "a words.json".
        kind: &'static str,
    },
    /// A words.json file could not be parsed.
    #[error("{}: {source}", .path.display())]
    Words {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: TranscriptParseError,
    },
    /// A words.json file breaks schema v1's rules.
    #[error("{}: {}", .path.display(), problems(.errors))]
    InvalidWords {
        /// The file.
        path: PathBuf,
        /// Every problem.
        errors: ValidationErrors,
    },
    /// The fixes.toml file could not be parsed.
    #[error("{}: {source}", .path.display())]
    Fixes {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: FixesParseError,
    },
    /// The fixes could not be applied.
    #[error("{}: {source}", .path.display())]
    ApplyFixes {
        /// The fixes.toml file.
        path: PathBuf,
        /// What went wrong.
        source: FixError,
    },
    /// The caption font cannot be used.
    #[error("{}: {source}", .path.display())]
    Font {
        /// The font file, or the name of the bundled font.
        path: PathBuf,
        /// What is wrong with it.
        source: FontError,
    },
    /// Captions are on, but a source that a cut uses has no word timings. Validation
    /// rules this out for plans; it can still happen with word timings built by hand.
    #[error(
        "captions need word timings for source `{source_id}`: set `words` on its [[source]]{}",
        if *.single_source { " or in [captions]" } else { " (with 2 or more sources, each one needs its own)" }
    )]
    NoWords {
        /// The source's id.
        source_id: String,
        /// Whether the plan has only one source, so `captions.words` would do.
        single_source: bool,
    },
    /// `captions.active` is not a `#RRGGBB` colour. Validation rules this out.
    #[error("captions.active must be a #RRGGBB colour like \"#FFC400\", got {0:?}")]
    Colour(String),
    /// The hook text does not fit in the space above the captions, even small.
    #[error(
        "hook.text is too long to fit above the captions ({words} words); shorten it to a line or two"
    )]
    HookTooLong {
        /// How many words the hook has.
        words: usize,
    },
    /// The output frame is too small to draw text on.
    #[error("the output ({width}x{height}) is too small to draw captions or a hook on")]
    TooSmall {
        /// Output width.
        width: u32,
        /// Output height.
        height: u32,
    },
}

impl OverlayError {
    pub(crate) fn read(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Read { path, source }
    }
}

/// "1 problem" or "3 problems", then one problem per line.
fn problems(errors: &ValidationErrors) -> String {
    let count = errors.as_slice().len();
    let mut text = if count == 1 {
        "1 problem".to_owned()
    } else {
        format!("{count} problems")
    };
    for error in errors {
        text.push_str("\n  - ");
        text.push_str(&error.to_string());
    }
    text
}
