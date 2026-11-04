//! `sabicut render`: the plan's cuts as a vertical MP4, and a cover JPG (PRD F5).
//!
//! Three steps, each streaming:
//!
//! 1. **Audio.** Every cut's audio, trimmed on the cut's frame boundaries with a 10 ms
//!    fade at both ends, joined, then denoised, high-passed and compressed, into a
//!    float WAV in a scratch directory next to the output.
//! 2. **Loudness.** `loudnorm` measures that WAV (first pass).
//! 3. **Video.** Every cut is decoded by its own ffmpeg, one after another (frame-exact
//!    trim, crop, tone mapping, scale, zoom), and its raw frames are piped into a
//!    single encoding ffmpeg, which adds the overlays, normalizes the WAV to −14 LUFS
//!    (second pass) and encodes H.264 and AAC. Memory therefore holds one decoder and
//!    the encoder, however many cuts the plan has.
//!
//! The result is checked (frame count, loudness) before it replaces the output file,
//! so a failed render never leaves a broken video behind.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;

use crate::error::RenderError;
use crate::graph::{
    AUDIO_CLEANUP, AUDIO_FILTERS, Graph, HDR_FILTERS, Inputs, Overlay, OverlaySource, SAMPLE_RATE,
    VIDEO_FILTERS, ZOOM_FILTERS, audio_chain, overlay_chain, seek_before, silence, video_chain,
};
use crate::loudness::{self, Loudness};
use crate::probe::file_url;
use crate::project::Project;
use crate::tools::Ffmpeg;
use crate::work::WorkDir;

/// x264 quality: constant rate factor 20 (PRD F5).
pub const CRF: u32 = 20;
/// Most frames x264 may look ahead (see [`Preset::lookahead`]).
pub const MAX_LOOKAHEAD: u32 = 20;
/// AAC bit rate (PRD F5).
pub const AUDIO_BITRATE: &str = "160k";
/// Where the cover is taken when no time is given, in seconds: one second in, or the
/// middle of a shorter video. Late enough to miss a blink on the first frame; inside
/// the hook (the first 2 s) once hooks are drawn.
pub const DEFAULT_COVER_SECONDS: f64 = 1.0;

/// x264's speed preset: slower presets compress better. PRD §8: `medium` for personal
/// use, `veryfast` for the hub, where throughput matters more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preset {
    /// `ultrafast`
    Ultrafast,
    /// `superfast`
    Superfast,
    /// `veryfast`: the hub default.
    Veryfast,
    /// `faster`
    Faster,
    /// `fast`
    Fast,
    /// `medium`: the default.
    #[default]
    Medium,
    /// `slow`
    Slow,
    /// `slower`
    Slower,
    /// `veryslow`
    Veryslow,
}

impl Preset {
    /// Every preset, fastest first.
    pub const ALL: [Self; 9] = [
        Self::Ultrafast,
        Self::Superfast,
        Self::Veryfast,
        Self::Faster,
        Self::Fast,
        Self::Medium,
        Self::Slow,
        Self::Slower,
        Self::Veryslow,
    ];

    /// The x264 lookahead to use instead of the preset's own, if any.
    ///
    /// At 1080×1920 each lookahead frame costs about 5 MB, so `medium`'s 40 frames take
    /// about 200 MB. Capping it at [`MAX_LOOKAHEAD`] frames saves about 100 MB, which
    /// keeps a render under the PRD's 800 MB. On real footage at CRF 20 the cap gave a
    /// file 6% smaller with SSIM 0.9787 instead of 0.9791. Faster presets already look
    /// ahead 20 frames or fewer and keep their own value.
    pub fn lookahead(self) -> Option<u32> {
        match self {
            Self::Fast | Self::Medium | Self::Slow | Self::Slower | Self::Veryslow => {
                Some(MAX_LOOKAHEAD)
            }
            Self::Ultrafast | Self::Superfast | Self::Veryfast | Self::Faster => None,
        }
    }

    /// The preset's name in x264.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ultrafast => "ultrafast",
            Self::Superfast => "superfast",
            Self::Veryfast => "veryfast",
            Self::Faster => "faster",
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
            Self::Slower => "slower",
            Self::Veryslow => "veryslow",
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An unknown preset name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown preset {0:?}; use one of ultrafast, superfast, veryfast, faster, fast, medium, slow, slower, veryslow"
)]
pub struct UnknownPreset(String);

impl FromStr for Preset {
    type Err = UnknownPreset;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.name() == name)
            .ok_or_else(|| UnknownPreset(name.to_owned()))
    }
}

/// What to render, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    /// The MP4 to write. Replaced only when the render succeeds.
    pub output: PathBuf,
    /// The cover JPG to write, if any.
    pub cover: Option<PathBuf>,
    /// When the cover is taken, in seconds of output time. Defaults to
    /// [`DEFAULT_COVER_SECONDS`], or the middle of a shorter video.
    pub cover_at: Option<f64>,
    /// x264 speed preset.
    pub preset: Preset,
    /// Pictures drawn over the video, in order (none until captions arrive in F6).
    pub overlays: Vec<Overlay>,
}

/// A step of the render, for progress reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Another heavy job (a transcription or a render) is running on this machine;
    /// this render waits for it.
    Waiting,
    /// The one-job-at-a-time lock could not be taken; the render runs anyway.
    LockUnavailable(String),
    /// Cutting and cleaning up the audio.
    Audio,
    /// Measuring loudness.
    Loudness,
    /// Encoding: `done` of `total` frames written.
    Video {
        /// Frames written so far.
        done: u64,
        /// Frames in the output.
        total: u64,
    },
    /// Checking the result.
    Check,
    /// Writing the cover.
    Cover,
}

/// What a render produced.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderReport {
    /// The MP4 written.
    pub output: PathBuf,
    /// Frames in it (checked against the timeline).
    pub frames: u64,
    /// The cover written, and the output frame it shows.
    pub cover: Option<(PathBuf, u64)>,
    /// Loudness of the cut audio before normalization.
    pub loudness_before: Loudness,
    /// Loudness of the output's audio, measured on the encoded file.
    pub loudness: Loudness,
}

/// Renders `project` to `options.output` (and the cover, if asked), reporting
/// progress to `progress`.
pub fn render(
    tools: &Ffmpeg,
    project: &Project,
    options: &RenderOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<RenderReport, RenderError> {
    check_tools(tools, project, options)?;
    for path in std::iter::once(&options.output).chain(&options.cover) {
        refuse_to_overwrite_a_source(project, path)?;
    }
    // Check the cover time before the long part.
    let cover_frame = match &options.cover {
        Some(_) => {
            let duration = project.timeline().duration().to_f64();
            let at = options
                .cover_at
                .unwrap_or_else(|| DEFAULT_COVER_SECONDS.min(duration / 2.0));
            Some(project.frame_at(at, "--cover-at")?)
        }
        None => None,
    };
    // One heavy job at a time on this machine (PRD H4): wait while a transcription or
    // another render runs. If the lock cannot be taken at all, render anyway.
    let _lock = match sabicut_core::host::acquire_job_lock(|| progress(Progress::Waiting)) {
        Ok(lock) => Some(lock),
        Err(error) => {
            progress(Progress::LockUnavailable(error.to_string()));
            None
        }
    };
    let parent = parent_dir(&options.output);
    fs::create_dir_all(&parent).map_err(RenderError::io(&parent))?;
    let work = WorkDir::new(&parent)?;

    progress(Progress::Audio);
    let wav = work.file("audio.wav");
    render_audio(tools, project, &work, &wav)?;

    progress(Progress::Loudness);
    let loudness_before = measure_loudness(tools, &wav)?;

    let encoded = work.file("output.mp4");
    let total = project.timeline().total_frames();
    progress(Progress::Video { done: 0, total });
    render_video(
        tools,
        project,
        options,
        &work,
        &wav,
        &loudness_before,
        &encoded,
        progress,
    )?;

    progress(Progress::Check);
    let frames = count_frames(tools, &encoded)?;
    if frames != total {
        return Err(RenderError::FrameCount {
            got: frames,
            want: total,
        });
    }
    let loudness = measure_loudness(tools, &encoded)?;

    // The cover comes from the finished file, before it replaces the output, so a
    // failure leaves neither file half-done.
    let cover = match (&options.cover, cover_frame) {
        (Some(path), Some(frame)) => {
            progress(Progress::Cover);
            if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                fs::create_dir_all(dir).map_err(RenderError::io(dir))?;
            }
            extract_frame(tools, &encoded, frame, project.timeline().fps(), path)?;
            Some((path.clone(), frame))
        }
        _ => None,
    };
    fs::rename(&encoded, &options.output).map_err(RenderError::io(&options.output))?;
    Ok(RenderReport {
        output: options.output.clone(),
        frames,
        cover,
        loudness_before,
        loudness,
    })
}

/// Fails if `path` is one of the project's source clips, which writing it would destroy.
fn refuse_to_overwrite_a_source(project: &Project, path: &Path) -> Result<(), RenderError> {
    // A file that does not exist yet cannot be a source.
    let Ok(target) = fs::canonicalize(path) else {
        return Ok(());
    };
    for source in project.sources() {
        if fs::canonicalize(source.path()).is_ok_and(|clip| clip == target) {
            return Err(RenderError::WouldOverwriteSource {
                path: path.to_owned(),
                id: source.id().to_owned(),
            });
        }
    }
    Ok(())
}

fn check_tools(
    tools: &Ffmpeg,
    project: &Project,
    options: &RenderOptions,
) -> Result<(), RenderError> {
    tools.require_filters(VIDEO_FILTERS, "cut and scale video")?;
    tools.require_filters(AUDIO_FILTERS, "cut, clean up and normalize audio")?;
    if project.zooms() {
        tools.require_filters(ZOOM_FILTERS, "zoom")?;
    }
    if project.tone_maps() {
        tools.require_filters(HDR_FILTERS, "tone-map HDR (PQ or HLG) video to SDR")?;
    }
    if !options.overlays.is_empty() {
        tools.require_filters(&["overlay"], "draw overlays")?;
    }
    tools.require_encoder("libx264", "encode H.264 video")?;
    tools.require_encoder("aac", "encode AAC audio")?;
    if options.cover.is_some() {
        tools.require_encoder("mjpeg", "write the cover JPG")?;
    }
    Ok(())
}

/// The directory a file goes in: its parent, or `.` for a bare file name.
pub(crate) fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_owned(),
        _ => PathBuf::from("."),
    }
}

/// Arguments every ffmpeg run starts with: no keyboard, quiet, overwrite.
pub(crate) fn base_args(log_level: &str) -> Vec<OsString> {
    [
        "-nostdin",
        "-hide_banner",
        "-nostats",
        "-loglevel",
        log_level,
        "-y",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// Global arguments for runs that read the sources: keep their timestamps, counted
/// from the start of each file, so that seeking does not move the frame grid.
pub(crate) const SOURCE_TIMESTAMPS: [&str; 2] = ["-copyts", "-start_at_zero"];

fn render_audio(
    tools: &Ffmpeg,
    project: &Project,
    work: &WorkDir,
    wav: &Path,
) -> Result<(), RenderError> {
    let fps = project.timeline().fps();
    let mut inputs = Inputs::default();
    let mut graph = Graph::default();
    let mut labels = Vec::new();
    for (i, cut) in project.cuts().iter().enumerate() {
        let label = format!("a{i}");
        if cut.has_audio {
            let seek = seek_before(cut.segment.source_in(), fps);
            let input = inputs.add_seeked(&cut.path, seek, cut.spec.decoder_threads());
            graph.chain(
                &[&format!("{input}:a:0")],
                &audio_chain(cut.audio, seek),
                &[&label],
            );
        } else {
            graph.chain(&[], &silence(cut.audio.samples), &[&label]);
        }
        labels.push(label);
    }
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    graph.chain(
        &labels,
        &format!("concat=n={}:v=0:a=1,{AUDIO_CLEANUP}", labels.len()),
        &["aout"],
    );
    let script = work.write("audio.graph", &graph.finish())?;
    let mut args = base_args("error");
    args.extend(SOURCE_TIMESTAMPS.map(OsString::from));
    args.extend_from_slice(inputs.args());
    args.extend(tools.graph_script_args(&script));
    args.extend(
        [
            "-map",
            "[aout]",
            "-c:a",
            "pcm_f32le",
            "-ar",
            &SAMPLE_RATE.to_string(),
            "-f",
            "wav",
        ]
        .map(OsString::from),
    );
    args.push(wav.as_os_str().to_owned());
    tools.run_ffmpeg(&args)?;
    Ok(())
}

/// Measures the loudness of `path`'s first audio stream.
pub(crate) fn measure_loudness(tools: &Ffmpeg, path: &Path) -> Result<Loudness, RenderError> {
    let mut args = base_args("info");
    args.extend(["-protocol_whitelist", "file", "-i"].map(OsString::from));
    args.push(file_url(path));
    args.extend(["-vn", "-sn", "-dn", "-map", "0:a:0", "-af"].map(OsString::from));
    args.push(loudness::measure_filter().into());
    args.extend(["-f", "null", "-"].map(OsString::from));
    let stderr = tools.run_ffmpeg(&args)?;
    loudness::parse(&stderr).ok_or(RenderError::Loudness)
}

/// Encodes the video. Each cut is decoded by its own ffmpeg, one cut after another,
/// and its frames are piped raw into a single encoding ffmpeg. So memory holds one
/// decoder and the encoder however many cuts there are, and every cut is checked to
/// deliver exactly its number of frames.
#[allow(clippy::too_many_arguments)]
fn render_video(
    tools: &Ffmpeg,
    project: &Project,
    options: &RenderOptions,
    work: &WorkDir,
    wav: &Path,
    loudness: &Loudness,
    encoded: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), RenderError> {
    let fps = project.timeline().fps();
    let total = project.timeline().total_frames();
    let (width, height) = project.out();
    let frame_bytes = u64::from(width) * u64::from(height) * 3 / 2;

    let script = work.write("encode.graph", &encoder_graph(options, loudness))?;
    let mut args = base_args("error");
    args.extend(
        [
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-s",
            &format!("{width}x{height}"),
            "-framerate",
            &fps.to_string(),
            // Frames are 3 MB each: queue only a few between the pipe and the encoder.
            "-thread_queue_size",
            "4",
            "-i",
            "pipe:0",
        ]
        .map(OsString::from),
    );
    let mut inputs = Inputs::default();
    // Inputs 1 and up; 0 is the raw video on stdin.
    inputs.add_file(wav);
    for overlay in &options.overlays {
        match &overlay.source {
            OverlaySource::File(path) => inputs.add_file(path),
        };
    }
    args.extend_from_slice(inputs.args());
    args.extend(tools.graph_script_args(&script));
    args.extend(encoder_options(options.preset, fps));
    args.push(encoded.as_os_str().to_owned());
    let mut encoder = tools.spawn_ffmpeg(&args, Stdio::piped(), Stdio::null())?;
    let mut sink = encoder.take_stdin().ok_or(RenderError::Pipe)?;

    let mut done = 0;
    let mut buffer = vec![0_u8; 1 << 20];
    for (index, cut) in project.cuts().iter().enumerate() {
        let graph = format!(
            "[0:V:0]{}[v]\n",
            video_chain(&cut.spec, &cut.curve, &cut.segment, 0..cut.segment.frames())
        );
        let script = work.write(&format!("cut-{index}.graph"), &graph)?;
        let mut cut_inputs = Inputs::default();
        cut_inputs.add_seeked(
            &cut.path,
            seek_before(cut.segment.source_in(), fps),
            cut.spec.decoder_threads(),
        );
        let mut args = base_args("error");
        args.extend(SOURCE_TIMESTAMPS.map(OsString::from));
        args.extend_from_slice(cut_inputs.args());
        args.extend(tools.graph_script_args(&script));
        args.extend(
            [
                "-map",
                "[v]",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "pipe:1",
            ]
            .map(OsString::from),
        );
        let mut decoder = tools.spawn_ffmpeg(&args, Stdio::null(), Stdio::piped())?;
        let mut frames = decoder.take_stdout().ok_or(RenderError::Pipe)?;
        let mut copied: u64 = 0;
        loop {
            let read = match frames.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(RenderError::Pipe),
            };
            let chunk = buffer.get(..read).unwrap_or_default();
            if sink.write_all(chunk).is_err() {
                // The encoder stopped reading: it failed, and its error says why.
                drop(decoder);
                drop(sink);
                return Err(encoder
                    .finish()
                    .err()
                    .map_or(RenderError::Pipe, RenderError::from));
            }
            let before = copied / frame_bytes;
            copied += read as u64;
            if copied / frame_bytes > before {
                progress(Progress::Video {
                    done: done + copied / frame_bytes,
                    total,
                });
            }
        }
        decoder.finish()?;
        let want = cut.segment.frames();
        if copied != want * frame_bytes {
            return Err(RenderError::CutFrames {
                cut: index,
                got: copied / frame_bytes,
                want,
            });
        }
        done += want;
    }
    drop(sink);
    encoder.finish()?;
    Ok(())
}

/// The encoder's graph: raw frames on input 0 tagged BT.709, overlays on top, and the
/// audio (input 1) normalized with the second loudness pass.
fn encoder_graph(options: &RenderOptions, loudness: &Loudness) -> String {
    let mut graph = Graph::default();
    graph.chain(
        &["0:v:0"],
        "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        &["vbase"],
    );
    let video = overlay_chain(&mut graph, "vbase", &options.overlays, 2);
    graph.chain(&[&video], "null", &["vout"]);
    // loudnorm works at 192 kHz, and in dynamic mode (used when one gain cannot reach
    // the target, or the loudness range is 0) its timestamps jump by tens of
    // milliseconds partway through, which players would hear as audio drifting
    // behind the video. The audio is one continuous stream here, so stamp it again
    // from the sample count.
    graph.chain(
        &["1:a:0"],
        &format!(
            "{},aresample={SAMPLE_RATE},asetpts=N/SR/TB,aformat=sample_fmts=fltp:sample_rates={SAMPLE_RATE}:channel_layouts=stereo",
            loudness.normalize_filter()
        ),
        &["aout"],
    );
    graph.finish()
}

/// Output options: x264 CRF 20 at a constant `fps`, AAC 160k 48 kHz stereo, BT.709
/// tags, index at the front of the file.
fn encoder_options(preset: Preset, fps: u32) -> Vec<OsString> {
    let fps = fps.to_string();
    let crf = CRF.to_string();
    let rate = SAMPLE_RATE.to_string();
    let lookahead = preset
        .lookahead()
        .map(|frames| ["-rc-lookahead".to_owned(), frames.to_string()]);
    let options = [
        "-map",
        "[vout]",
        "-map",
        "[aout]",
        "-c:v",
        "libx264",
        "-preset",
        preset.name(),
        "-crf",
        &crf,
        "-pix_fmt",
        "yuv420p",
        "-profile:v",
        "high",
        "-fps_mode",
        "cfr",
        "-r",
        &fps,
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-color_range",
        "tv",
        "-c:a",
        "aac",
        "-b:a",
        AUDIO_BITRATE,
        "-ar",
        &rate,
        "-ac",
        "2",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ]
    .map(OsString::from);
    lookahead
        .into_iter()
        .flatten()
        .map(OsString::from)
        .chain(options)
        .collect()
}

/// Counts the video frames (packets) in `path`.
pub(crate) fn count_frames(tools: &Ffmpeg, path: &Path) -> Result<u64, RenderError> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
        "v:0",
        "-count_packets",
        "-show_entries",
        "stream=nb_read_packets",
        "-of",
        "csv=p=0",
    ]
    .map(OsString::from)
    .into();
    args.push(file_url(path));
    let stdout = tools.run_ffprobe(&args)?;
    let text = String::from_utf8_lossy(&stdout);
    // An empty or garbled count means the file has no readable video at all.
    Ok(text.trim().trim_end_matches(',').parse().unwrap_or(0))
}

/// Writes output frame `frame` of the video at `path` to a JPG at `jpg`.
fn extract_frame(
    tools: &Ffmpeg,
    path: &Path,
    frame: u64,
    fps: u32,
    jpg: &Path,
) -> Result<(), RenderError> {
    // Seek to the frame's own timestamp (rounded down to the microsecond): ffmpeg then
    // decodes from the keyframe before and outputs the first frame at or after it.
    let micros = u128::from(frame) * 1_000_000 / u128::from(fps.max(1));
    let at = format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000);
    let mut args = base_args("error");
    args.extend(["-protocol_whitelist", "file", "-ss", &at, "-i"].map(OsString::from));
    args.push(file_url(path));
    args.extend(
        [
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=out_range=pc,format=yuvj420p",
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "-update",
            "1",
            "-f",
            "image2",
        ]
        .map(OsString::from),
    );
    args.push(jpg.as_os_str().to_owned());
    tools.run_ffmpeg(&args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_presets_look_ahead_at_most_20_frames() {
        let options: Vec<String> = encoder_options(Preset::Medium, 30)
            .iter()
            .map(|o| o.to_string_lossy().into_owned())
            .collect();
        assert_eq!(options[..2], ["-rc-lookahead", "20"]);
        assert!(options.contains(&"medium".to_owned()));
        let fast = encoder_options(Preset::Veryfast, 30);
        assert!(!fast.iter().any(|o| o == "-rc-lookahead"));
    }

    #[test]
    fn presets_round_trip() {
        for preset in Preset::ALL {
            assert_eq!(preset.name().parse::<Preset>(), Ok(preset));
        }
        assert_eq!(Preset::default(), Preset::Medium);
        assert!("fastest".parse::<Preset>().is_err());
    }

    #[test]
    fn bare_file_names_go_in_the_current_directory() {
        assert_eq!(parent_dir(Path::new("out.mp4")), Path::new("."));
        assert_eq!(
            parent_dir(Path::new("renders/out.mp4")),
            Path::new("renders")
        );
    }
}
