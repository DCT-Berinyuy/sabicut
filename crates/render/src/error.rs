//! Errors from rendering and previewing.

use std::io;
use std::path::PathBuf;

use crate::probe::ProbeError;
use crate::tools::ToolError;

/// Why a render or preview failed.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// ffmpeg or ffprobe is missing, lacks something, or failed.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// A source clip could not be read.
    #[error("source `{id}` ({}): {source}", .path.display())]
    Probe {
        /// The source's id in the plan.
        id: String,
        /// The clip's path.
        path: PathBuf,
        /// What went wrong.
        source: Box<ProbeError>,
    },
    /// A cut names a source the plan does not declare. Validation rules this out.
    #[error("cut[{cut}] uses source `{id}`, which the plan does not declare")]
    UnknownSource {
        /// Index of the cut in the plan.
        cut: usize,
        /// The source id it names.
        id: String,
    },
    /// A cut reaches past the end of its source clip.
    #[error(
        "cut[{cut}] ends at {end:.3} s, but source `{id}` ends at {video_end:.3} s; move cut[{cut}].out back"
    )]
    CutPastEnd {
        /// Index of the cut in the plan.
        cut: usize,
        /// The source's id.
        id: String,
        /// Where the cut ends, in seconds of source time.
        end: f64,
        /// Where the source's video ends.
        video_end: f64,
    },
    /// An output file is one of the source clips.
    #[error("{} is source `{id}`; writing there would destroy it, so choose another output path", .path.display())]
    WouldOverwriteSource {
        /// The output path.
        path: PathBuf,
        /// The source's id.
        id: String,
    },
    /// A requested time is not on the output.
    #[error("{what} {time} s is not within the output (0 to {duration:.3} s)")]
    TimeOutOfRange {
        /// Which option it came from, like `--at`.
        what: String,
        /// The requested time.
        time: f64,
        /// Length of the output.
        duration: f64,
    },
    /// Preview was asked for no stills.
    #[error("no preview times given")]
    NoTimes,
    /// A file or directory could not be written.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// ffmpeg's loudness measurement could not be read.
    #[error("cannot read the loudness measurement from ffmpeg's output")]
    Loudness,
    /// A cut's decoder delivered the wrong number of frames.
    #[error("cut[{cut}] delivered {got} frames instead of {want}")]
    CutFrames {
        /// Index of the cut in the plan.
        cut: usize,
        /// Frames delivered.
        got: u64,
        /// Frames in the cut.
        want: u64,
    },
    /// ffmpeg's stdin or stdout could not be connected.
    #[error("cannot connect to ffmpeg's input or output")]
    Pipe,
    /// The rendered file does not have the timeline's frame count.
    #[error("the rendered video has {got} frames, but the timeline has {want}")]
    FrameCount {
        /// Frames in the file.
        got: u64,
        /// Frames in the timeline.
        want: u64,
    },
}

impl RenderError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}
