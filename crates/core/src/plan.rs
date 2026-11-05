//! The `plan.toml` document, schema v1. These types mirror the file one to one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::validate::{self, ValidationErrors};

/// The plan format version this build reads.
pub const SCHEMA_VERSION: u32 = 1;

pub(crate) const MAX_FPS: u32 = 240;
pub(crate) const MAX_DIMENSION: u32 = 8192;
pub(crate) const MAX_SECONDS: f64 = 86_400.0;
pub(crate) const MIN_ZOOM: f64 = 1.0;
pub(crate) const MAX_ZOOM: f64 = 4.0;
pub(crate) const MAX_STROKE: u32 = 64;
pub(crate) const MAX_ID_LEN: usize = 64;

/// # Sabicut edit plan (schema v1)
///
/// A hand-editable description of one short video: the source clips, the ranges kept
/// from them, and how the result is captioned. Output timing is derived from the cuts
/// and never stored. Relative paths are resolved from the plan file's directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Plan format version. Always 1 for this schema.
    #[schemars(extend("const" = 1))]
    schema: u32,
    /// Output video settings.
    meta: Meta,
    /// Source clips. Cuts refer to them by id.
    #[serde(rename = "source")]
    #[schemars(length(min = 1))]
    sources: Vec<Source>,
    /// Ranges kept from the sources, in output order.
    #[serde(rename = "cut")]
    #[schemars(length(min = 1))]
    cuts: Vec<Cut>,
    /// Hook text shown over the opening seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hook")]
    hook: Option<Hook>,
    /// Word-by-word caption settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Captions")]
    captions: Option<Captions>,
}

/// Output video settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// Output frame rate in frames per second. Every cut is trimmed to a whole number
    /// of frames at this rate.
    #[schemars(range(min = 1, max = MAX_FPS))]
    fps: u32,
    /// Output frame size in pixels.
    out: OutputSize,
}

/// Output frame size in pixels. Both sides must be even.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputSize {
    /// Width in pixels.
    #[schemars(range(min = 2, max = MAX_DIMENSION), extend("multipleOf" = 2))]
    w: u32,
    /// Height in pixels.
    #[schemars(range(min = 2, max = MAX_DIMENSION), extend("multipleOf" = 2))]
    h: u32,
}

/// One source clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Short name that cuts use to refer to this clip: 1 to 64 letters, digits,
    /// underscores, or hyphens.
    #[schemars(pattern(r"^[A-Za-z0-9_-]{1,64}$"))]
    id: String,
    /// Path to the clip file.
    #[schemars(length(min = 1))]
    path: String,
    /// Horizontal centre of the 9:16 crop window, from 0 (left edge) to 1 (right edge).
    #[schemars(range(min = 0.0, max = 1.0))]
    crop_x: f64,
    /// Word timings file (words.json) of this clip, for the captions. When the plan has
    /// [captions] and more than one source, every source that a cut uses needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1))]
    words: Option<String>,
}

/// A range of one source clip that is kept in the output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    /// Id of the source clip.
    #[schemars(pattern(r"^[A-Za-z0-9_-]{1,64}$"))]
    src: String,
    /// Start of the range, in seconds of source time.
    #[serde(rename = "in")]
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    in_point: f64,
    /// End of the range, in seconds of source time. Must be greater than in.
    #[serde(rename = "out")]
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    out_point: f64,
    /// Punch-in zooms within this cut, in increasing order of at, at least 1 frame apart.
    #[serde(rename = "zoom", default, skip_serializing_if = "Vec::is_empty")]
    zooms: Vec<Zoom>,
}

/// A punch-in zoom.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Zoom {
    /// When the zoom starts, in seconds of source time (not output time). Must be
    /// inside the cut (in <= at < out) and at least 1 frame before out.
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    at: f64,
    /// Zoom factor, from 1 (no zoom) to 4.
    #[schemars(range(min = MIN_ZOOM, max = MAX_ZOOM))]
    scale: f64,
}

/// Hook text shown over the opening seconds of the output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    /// The hook line, for example "Stop leaking your API keys".
    #[schemars(pattern(r"\S"))]
    text: String,
    /// How long the hook stays on screen, in seconds from the start of the output.
    /// At most the length of the output.
    #[schemars(range(max = MAX_SECONDS), extend("exclusiveMinimum" = 0))]
    dur: f64,
}

/// Word-by-word caption settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Captions {
    /// Word timings file (words.json) of the only source, when it has no words of its
    /// own. Ignored when the plan has more than one source: each source then names its
    /// own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1))]
    words: Option<String>,
    /// Replacement rules applied to the words (fixes.toml).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1))]
    fixes: Option<String>,
    /// Words that are always drawn in the active colour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(inner(pattern(r"\S")))]
    keywords: Vec<String>,
    /// Caption font file. Without it, captions use the Anton font that sabicut bundles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1))]
    font: Option<String>,
    /// Colour of the word being spoken and of keywords, as #RRGGBB. Gold (#FFC400) by
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        with = "String",
        pattern(r"^#[0-9A-Fa-f]{6}$"),
        extend("default" = Captions::DEFAULT_ACTIVE)
    )]
    active: Option<String>,
    /// Outline width around caption text, in output pixels. 10 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        with = "u32",
        range(max = MAX_STROKE),
        extend("default" = Captions::DEFAULT_STROKE)
    )]
    stroke: Option<u32>,
}

/// Why a `plan.toml` could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The file has no top-level `schema` key.
    #[error("missing `schema = 1` (the plan format version) at the top of the file")]
    MissingSchema,
    /// `schema` is present but is not a whole number.
    #[error("`schema` must be a whole number, like `schema = 1`")]
    SchemaNotInteger,
    /// The file uses a format version that this build cannot read.
    #[error("schema {0} is not supported: this version of sabicut reads schema {SCHEMA_VERSION}")]
    UnsupportedSchema(i64),
    /// The text is not valid TOML, or does not have the shape of a schema v1 plan.
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

/// Reads only the version, so a plan from a newer format gets a clear error.
#[derive(Deserialize)]
struct VersionProbe {
    schema: Option<toml::Value>,
}

impl Plan {
    /// Parses a `plan.toml` document.
    ///
    /// This checks the format version and the shape of the document (unknown keys are
    /// rejected). Call [`Plan::validate`] or [`crate::Timeline::from_plan`] to check
    /// the values.
    pub fn from_toml_str(text: &str) -> Result<Self, ParseError> {
        let probe: VersionProbe = toml::from_str(text)?;
        match probe.schema {
            None => return Err(ParseError::MissingSchema),
            Some(toml::Value::Integer(version)) if version == i64::from(SCHEMA_VERSION) => {}
            Some(toml::Value::Integer(version)) => {
                return Err(ParseError::UnsupportedSchema(version));
            }
            Some(_) => return Err(ParseError::SchemaNotInteger),
        }
        Ok(toml::from_str(text)?)
    }

    /// Checks every rule of schema v1 and reports all problems at once.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        validate::check(self).map(|_| ())
    }

    /// Plan format version.
    pub fn schema(&self) -> u32 {
        self.schema
    }

    /// Output video settings.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// Source clips, in file order (`[[source]]`).
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// Cuts, in output order (`[[cut]]`).
    pub fn cuts(&self) -> &[Cut] {
        &self.cuts
    }

    /// Hook text settings, if the plan has a `[hook]` table.
    pub fn hook(&self) -> Option<&Hook> {
        self.hook.as_ref()
    }

    /// Caption settings, if the plan has a `[captions]` table.
    pub fn captions(&self) -> Option<&Captions> {
        self.captions.as_ref()
    }

    /// The word timings file (words.json) for the captions of source `id`, as written
    /// in the plan: the source's own `words`, or else `captions.words`, but only when
    /// the plan has a single source.
    ///
    /// Returns `None` when no source has this id or there are no word timings for it.
    pub fn caption_words(&self, id: &str) -> Option<&str> {
        let source = self.sources.iter().find(|source| source.id == id)?;
        source.words.as_deref().or_else(|| {
            if self.sources.len() == 1 {
                self.captions.as_ref()?.words.as_deref()
            } else {
                None
            }
        })
    }
}

impl Meta {
    /// Output frame rate in frames per second.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Output frame size in pixels.
    pub fn out(&self) -> OutputSize {
        self.out
    }
}

impl OutputSize {
    /// Width in pixels.
    pub fn w(&self) -> u32 {
        self.w
    }

    /// Height in pixels.
    pub fn h(&self) -> u32 {
        self.h
    }
}

impl Source {
    /// Short name that cuts use to refer to this clip.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Path to the clip file, as written in the plan.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Horizontal centre of the 9:16 crop window, from 0 to 1.
    pub fn crop_x(&self) -> f64 {
        self.crop_x
    }

    /// Word timings file of this clip, if the plan names one. To find the words for the
    /// captions, use [`Plan::caption_words`].
    pub fn words(&self) -> Option<&str> {
        self.words.as_deref()
    }
}

impl Cut {
    /// Id of the source clip.
    pub fn src(&self) -> &str {
        &self.src
    }

    /// Start of the range in seconds of source time (`in` in the file).
    pub fn in_point(&self) -> f64 {
        self.in_point
    }

    /// End of the range in seconds of source time (`out` in the file).
    pub fn out_point(&self) -> f64 {
        self.out_point
    }

    /// Punch-in zooms within this cut (`zoom` in the file).
    pub fn zooms(&self) -> &[Zoom] {
        &self.zooms
    }
}

impl Zoom {
    /// When the zoom starts, in seconds of source time.
    pub fn at(&self) -> f64 {
        self.at
    }

    /// Zoom factor; 1 means no zoom.
    pub fn scale(&self) -> f64 {
        self.scale
    }
}

impl Hook {
    /// The hook line.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How long the hook stays on screen, in seconds.
    pub fn dur(&self) -> f64 {
        self.dur
    }
}

impl Captions {
    /// The active-word colour when the plan does not set one: gold.
    pub const DEFAULT_ACTIVE: &'static str = "#FFC400";
    /// The outline width when the plan does not set one, in output pixels.
    pub const DEFAULT_STROKE: u32 = 10;

    /// Word timings file of the only source, if the plan names one here. To find the
    /// words for the captions, use [`Plan::caption_words`].
    pub fn words(&self) -> Option<&str> {
        self.words.as_deref()
    }

    /// Replacement rules file, if any.
    pub fn fixes(&self) -> Option<&str> {
        self.fixes.as_deref()
    }

    /// Words that are always drawn in the active colour.
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// Caption font file, or `None` for the Anton font that sabicut bundles.
    pub fn font(&self) -> Option<&str> {
        self.font.as_deref()
    }

    /// Active-word colour (`#RRGGBB`): as written in the plan, or
    /// [`Captions::DEFAULT_ACTIVE`].
    pub fn active(&self) -> &str {
        self.active.as_deref().unwrap_or(Self::DEFAULT_ACTIVE)
    }

    /// Outline width around caption text, in output pixels: as written in the plan, or
    /// [`Captions::DEFAULT_STROKE`].
    pub fn stroke(&self) -> u32 {
        self.stroke.unwrap_or(Self::DEFAULT_STROKE)
    }
}
