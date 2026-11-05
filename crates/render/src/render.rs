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
//! Captions arrive as the caption band: for every output frame, its RGBA picture is
//! drawn here and piped to the encoder by a thread of its own, next to the video. The
//! hook's few animated pictures go in a scratch file. Both are placed with `overlay`,
//! the hook only on its frames (PRD §8).
//!
//! The result is checked (frame count, loudness) before it replaces the output file,
//! so a failed render never leaves a broken video behind. The subtitles (SRT) are
//! written with it.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;

use crate::error::RenderError;
use sabicut_overlay::Graphics;

use crate::graph::{
    AUDIO_CLEANUP, AUDIO_FILTERS, CaptionBand, Graph, HDR_FILTERS, Inputs, OVERLAY_FILTERS,
    Overlay, OverlaySource, SAMPLE_RATE, VIDEO_FILTERS, ZOOM_FILTERS, audio_chain, overlay_chain,
    seek_before, silence, video_chain,
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
/// middle of a shorter video. Late enough to miss a blink on the first frame, and
/// inside a hook of the usual 2 s, after its entrance.
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
    /// Pictures drawn over the video, in order, on top of the project's captions and
    /// hook.
    pub overlays: Vec<Overlay>,
    /// The subtitles (SRT) to write, anywhere. They are written only when the project's
    /// graphics have word timings for every source the cuts use, after the video is in
    /// place.
    pub subtitles: Option<PathBuf>,
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
    /// The subtitles written, and how many cues they have.
    pub subtitles: Option<(PathBuf, usize)>,
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
    let outputs = std::iter::once(&options.output)
        .chain(&options.cover)
        .chain(&options.subtitles);
    for path in outputs {
        refuse_to_overwrite_a_source(project, path)?;
    }
    // Check the subtitles can be written, and the cover time, before the long part.
    if let Some(path) = &options.subtitles {
        check_writable(path)?;
    }
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
            let hook = cover_hook(project, &work, frame)?;
            let fps = project.timeline().fps();
            extract_frame(tools, &encoded, frame, fps, path, hook.as_ref())?;
            Some((path.clone(), frame))
        }
        _ => None,
    };
    fs::rename(&encoded, &options.output).map_err(RenderError::io(&options.output))?;
    // After the video is in place, so a problem with the subtitles never costs it.
    let subtitles = write_subtitles(project, options)?;
    Ok(RenderReport {
        output: options.output.clone(),
        frames,
        cover,
        subtitles,
        loudness_before,
        loudness,
    })
}

/// Writes the subtitles, if asked and the project has word timings: to a temporary file
/// next to them, then renamed over them, so a failure never leaves half a file and the
/// rename never crosses file systems.
fn write_subtitles(
    project: &Project,
    options: &RenderOptions,
) -> Result<Option<(PathBuf, usize)>, RenderError> {
    let (Some(path), Some(graphics)) = (&options.subtitles, project.graphics()) else {
        return Ok(None);
    };
    let Some(cues) = graphics.cues() else {
        return Ok(None);
    };
    let text = sabicut_overlay::srt::to_srt(&cues, project.timeline().fps());
    let temporary = temporary_path(path);
    let written = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        // It may not exist; either way the write failed.
        let _ = fs::remove_file(&temporary);
        return Err(RenderError::io(path)(error));
    }
    Ok(Some((path.clone(), cues.len())))
}

/// Creates the folder `path` goes in and checks a file can be written there, by
/// creating and removing the temporary file the real write uses.
fn check_writable(path: &Path) -> Result<(), RenderError> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(RenderError::io(dir))?;
    }
    let temporary = temporary_path(path);
    File::create(&temporary).map_err(RenderError::io(path))?;
    // It was just created; if it cannot be removed, the real write will say why.
    let _ = fs::remove_file(&temporary);
    Ok(())
}

/// A temporary file next to `path`: its name with `.tmp-<process id>` added.
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp-{}", std::process::id()));
    PathBuf::from(name)
}

/// The hook to draw on a cover taken after the hook has gone: its still picture, in a
/// scratch file. `None` if the plan has no hook or the cover is taken during it.
fn cover_hook(
    project: &Project,
    work: &WorkDir,
    frame: u64,
) -> Result<Option<Overlay>, RenderError> {
    let Some(graphics) = project.graphics() else {
        return Ok(None);
    };
    let Some(hook) = graphics.hook().filter(|hook| frame >= hook.frames.end) else {
        return Ok(None);
    };
    let path = work.file("cover-hook.rgba");
    let still = hook.animated.saturating_sub(1);
    fs::write(&path, graphics.painter().hook_rgba(still)).map_err(RenderError::io(&path))?;
    Ok(Some(Overlay {
        source: OverlaySource::Rgba {
            path,
            width: hook.placement.width,
            height: hook.placement.height,
        },
        x: hook.placement.x,
        y: hook.placement.y,
        frames: None,
    }))
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
    if project.draws_graphics() {
        tools.require_filters(OVERLAY_FILTERS, "draw captions and the hook")?;
    }
    if !options.overlays.is_empty() {
        tools.require_filters(OVERLAY_FILTERS, "draw overlays")?;
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
/// deliver exactly its number of frames. With captions, a second thread draws each
/// frame's caption band and pipes it to the encoder.
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
    let (width, height) = project.out();
    let frame_bytes = usize::try_from(u64::from(width) * u64::from(height) * 3 / 2)
        .map_err(|_| RenderError::Pipe)?;

    // Inputs 1 and up; 0 is the raw video on stdin. The WAV, then the overlays.
    let mut inputs = Inputs::default();
    inputs.add_file(wav);
    let drawn = graphics_overlays(project, work)?;
    let overlays: Vec<(usize, &Overlay)> = drawn
        .iter()
        .chain(&options.overlays)
        .map(|overlay| (1 + inputs.add_overlay(overlay, fps), overlay))
        .collect();
    let script = work.write("encode.graph", &encoder_graph(&overlays, loudness))?;
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
    args.extend_from_slice(inputs.args());
    args.extend(tools.graph_script_args(&script));
    args.extend(encoder_options(options.preset, fps));
    args.push(encoded.as_os_str().to_owned());

    // The caption band goes in on the encoder's standard output (see BAND_PIPE).
    let (band_reader, band) = match project.graphics().filter(|g| g.band().is_some()) {
        Some(graphics) => {
            let (reader, writer) = io::pipe().map_err(|_| RenderError::Pipe)?;
            (Some(reader), Some((writer, graphics.painter())))
        }
        None => (None, None),
    };
    let stdout = band_reader.map_or_else(Stdio::null, Stdio::from);
    let mut encoder = tools.spawn_ffmpeg(&args, Stdio::piped(), stdout)?;
    let video = encoder.take_stdin().ok_or(RenderError::Pipe)?;
    let total = project.timeline().total_frames();
    let mut feed = Feed {
        video,
        frame: vec![0; frame_bytes],
        done: 0,
        total,
    };
    // ffmpeg opens its inputs one after another and reads ahead in each while it does,
    // so each pipe gets a writer of its own: this thread writes the video, another the
    // caption band. ffmpeg's queues keep the band at most a few frames ahead.
    std::thread::scope(|scope| {
        let band_writer = band.map(|(mut pipe, mut painter)| {
            scope.spawn(move || -> io::Result<()> {
                for frame in 0..total {
                    pipe.write_all(painter.band_rgba(frame))?;
                }
                Ok(())
            })
        });
        for index in 0..project.cuts().len() {
            match feed_cut(tools, project, work, index, &mut feed, progress) {
                Ok(()) => {}
                // Returning drops the encoder, which stops it, so the band writer
                // stops too.
                Err(Stop::Failed(error)) => return Err(error),
                Err(Stop::Encoder) => {
                    // The encoder stopped reading: it failed, and its error says why.
                    drop(feed);
                    return Err(encoder
                        .finish()
                        .err()
                        .map_or(RenderError::Pipe, RenderError::from));
                }
            }
        }
        // Closing the video pipe (and the band pipe, once all its frames are written)
        // tells the encoder the video is complete.
        drop(feed);
        encoder.finish()?;
        match band_writer.map(std::thread::ScopedJoinHandle::join) {
            Some(Ok(Err(_)) | Err(_)) => Err(RenderError::Pipe),
            Some(Ok(Ok(()))) | None => Ok(()),
        }
    })
}

/// Where the video frames go: the encoder's standard input.
struct Feed {
    video: std::process::ChildStdin,
    /// One raw video frame.
    frame: Vec<u8>,
    /// Output frames written so far.
    done: u64,
    total: u64,
}

/// Why feeding the encoder stopped.
enum Stop {
    /// The encoder stopped reading: it failed.
    Encoder,
    /// Something else failed.
    Failed(RenderError),
}

impl From<RenderError> for Stop {
    fn from(error: RenderError) -> Self {
        Self::Failed(error)
    }
}

impl Feed {
    /// Writes the frame in `self.frame`.
    fn write_frame(&mut self) -> Result<(), Stop> {
        self.video
            .write_all(&self.frame)
            .map_err(|_| Stop::Encoder)?;
        self.done += 1;
        Ok(())
    }
}

/// Decodes cut `index` with its own ffmpeg and writes its frames to `feed`.
fn feed_cut(
    tools: &Ffmpeg,
    project: &Project,
    work: &WorkDir,
    index: usize,
    feed: &mut Feed,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), Stop> {
    let fps = project.timeline().fps();
    let cut = project.cuts().get(index).ok_or(RenderError::Pipe)?;
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
    let mut decoder = tools
        .spawn_ffmpeg(&args, Stdio::null(), Stdio::piped())
        .map_err(RenderError::from)?;
    let mut frames = decoder.take_stdout().ok_or(RenderError::Pipe)?;
    let mut copied: u64 = 0;
    // Whole frames go to the encoder; a short read means the decoder ended, and any
    // bytes of a partial frame mean it ended mid-frame.
    let partial = loop {
        let read = read_full(&mut frames, &mut feed.frame).map_err(|_| RenderError::Pipe)?;
        if read < feed.frame.len() {
            break read > 0;
        }
        feed.write_frame()?;
        copied += 1;
        progress(Progress::Video {
            done: feed.done,
            total: feed.total,
        });
    };
    decoder.finish().map_err(RenderError::from)?;
    let want = cut.segment.frames();
    if partial || copied != want {
        return Err(RenderError::CutFrames {
            cut: index,
            got: copied,
            want,
        }
        .into());
    }
    Ok(())
}

/// Reads until `buffer` is full or the stream ends; returns how many bytes it read.
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while let Some(rest) = buffer.get_mut(filled..).filter(|rest| !rest.is_empty()) {
        match reader.read(rest) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// The project's hook and caption band, as overlays: the hook's animated pictures are
/// written to a scratch file (its last picture is held to the end of its frames), and
/// the band is piped while the video is encoded.
fn graphics_overlays(project: &Project, work: &WorkDir) -> Result<Vec<Overlay>, RenderError> {
    let Some(graphics) = project.graphics() else {
        return Ok(Vec::new());
    };
    let mut overlays = Vec::new();
    if let Some(hook) = graphics.hook() {
        let path = work.file("hook.rgba");
        write_hook(graphics, hook.animated, &path)?;
        overlays.push(Overlay {
            source: OverlaySource::Rgba {
                path,
                width: hook.placement.width,
                height: hook.placement.height,
            },
            x: hook.placement.x,
            y: hook.placement.y,
            frames: Some(hook.frames),
        });
    }
    if let Some(band) = graphics.band() {
        overlays.push(Overlay {
            source: OverlaySource::CaptionBand(CaptionBand::new(band.width, band.height)),
            x: band.x,
            y: band.y,
            frames: None,
        });
    }
    Ok(overlays)
}

/// Writes the hook's first `frames` pictures to `path` as raw RGBA.
fn write_hook(graphics: &Graphics, frames: u64, path: &Path) -> Result<(), RenderError> {
    let error = RenderError::io(path);
    let write = || -> io::Result<()> {
        let mut file = BufWriter::new(File::create(path)?);
        let mut painter = graphics.painter();
        for frame in 0..frames.max(1) {
            file.write_all(painter.hook_rgba(frame))?;
        }
        file.flush()
    };
    write().map_err(error)
}

/// The encoder's graph: raw frames on input 0 tagged BT.709, overlays on top (each with
/// the input index of its pictures), and the audio (input 1) normalized with the second
/// loudness pass.
fn encoder_graph(overlays: &[(usize, &Overlay)], loudness: &Loudness) -> String {
    let mut graph = Graph::default();
    graph.chain(
        &["0:v:0"],
        "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        &["vbase"],
    );
    let video = overlay_chain(&mut graph, "vbase", overlays, "");
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

/// How the cover's frame becomes a JPG: full range, as JPEG expects.
const COVER_FILTERS: &str = "scale=out_range=pc,format=yuvj420p";

/// Writes output frame `frame` of the video at `path` to a JPG at `jpg`, with
/// `overlay` drawn on it if given.
fn extract_frame(
    tools: &Ffmpeg,
    path: &Path,
    frame: u64,
    fps: u32,
    jpg: &Path,
    overlay: Option<&Overlay>,
) -> Result<(), RenderError> {
    // Seek to the frame's own timestamp (rounded down to the microsecond): ffmpeg then
    // decodes from the keyframe before and outputs the first frame at or after it.
    let micros = u128::from(frame) * 1_000_000 / u128::from(fps.max(1));
    let at = format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000);
    let mut args = base_args("error");
    args.extend(["-protocol_whitelist", "file", "-ss", &at, "-i"].map(OsString::from));
    args.push(file_url(path));
    match overlay {
        None => args.extend(["-map", "0:v:0", "-vf", COVER_FILTERS].map(OsString::from)),
        Some(overlay) => {
            let mut inputs = Inputs::default();
            let input = 1 + inputs.add_overlay(overlay, fps);
            args.extend_from_slice(inputs.args());
            let mut graph = Graph::default();
            let top = overlay_chain(&mut graph, "0:v:0", &[(input, overlay)], "c");
            graph.chain(&[&top], COVER_FILTERS, &["cover"]);
            args.extend(["-filter_complex".into(), graph.finish().into()]);
            args.extend(["-map", "[cover]"].map(OsString::from));
        }
    }
    args.extend(
        [
            "-frames:v",
            "1",
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
    fn subtitles_are_written_through_a_file_next_to_them() {
        let dir = std::env::temp_dir().join(format!("sabicut-srt-{}", std::process::id()));
        let path = dir.join("new/folder/short.srt");
        check_writable(&path).unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert!(!temporary_path(&path).exists());
        assert_eq!(
            temporary_path(&path),
            dir.join(format!("new/folder/short.srt.tmp-{}", std::process::id()))
        );
        let _ = fs::remove_dir_all(&dir);
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
