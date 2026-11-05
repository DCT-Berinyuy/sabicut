//! Sabicut's renderer: turns a plan and its [`Timeline`](sabicut_core::Timeline) into
//! ffmpeg runs. It writes the final vertical MP4, cover and subtitles ([`render`], PRD
//! F5), and PNG stills with a contact sheet ([`preview`], PRD F4). Captions and the
//! hook (PRD F6) are drawn by `sabicut-overlay` and placed over the video by both:
//! give the project its [`Graphics`](sabicut_overlay::Graphics) with
//! [`Project::with_graphics`]. ffmpeg runs as a subprocess (PRD §8): find it with
//! [`Ffmpeg::from_env`].
//!
//! ```no_run
//! use std::path::Path;
//! use sabicut_core::Plan;
//! use sabicut_render::{Ffmpeg, Preset, Project, RenderOptions, render};
//!
//! let plan = Plan::from_toml_str(&std::fs::read_to_string("short/plan.toml")?)?;
//! let tools = Ffmpeg::from_env()?;
//! let project = Project::open(&tools, &plan, Path::new("short"))?;
//! let options = RenderOptions {
//!     output: "short/plan.mp4".into(),
//!     cover: Some("short/plan.jpg".into()),
//!     cover_at: None,
//!     preset: Preset::Medium,
//!     overlays: Vec::new(),
//!     subtitles: None,
//! };
//! let report = render(&tools, &project, &options, &mut |_progress| {})?;
//! println!("{} frames at {:.1} LUFS", report.frames, report.loudness.integrated);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]
#![warn(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod error;
mod geometry;
mod graph;
mod loudness;
mod preview;
mod probe;
mod project;
mod render;
pub mod safezone;
mod tools;
mod work;
pub mod zoom;

pub use error::RenderError;
pub use geometry::{CropRect, crop_window};
pub use graph::{CaptionBand, FADE_SAMPLES, Overlay, OverlaySource, SAMPLE_RATE, sample_at};
pub use loudness::{Loudness, TARGET_LUFS, TARGET_RANGE, TARGET_TRUE_PEAK};
pub use preview::{
    CONTACT_SHEET_NAME, PreviewOptions, PreviewReport, SHEET_CELL_WIDTH, SHEET_COLUMNS, Still,
    preview, still_name,
};
pub use probe::{ColourTags, HdrTransfer, ProbeError, SourceInfo, probe};
pub use project::{OpenError, ProbedSource, Project};
pub use render::{
    AUDIO_BITRATE, CRF, DEFAULT_COVER_SECONDS, MAX_LOOKAHEAD, Preset, Progress, RenderOptions,
    RenderReport, UnknownPreset, render,
};
pub use sabicut_core::host::{FFMPEG_ENV, FFPROBE_ENV};
pub use tools::{Ffmpeg, ToolError};
