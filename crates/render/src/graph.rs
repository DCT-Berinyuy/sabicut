//! ffmpeg filter graphs built from a plan's timeline. `render` and `preview` share
//! [`video_chain`], so a preview still and the rendered frame at the same time come out
//! of the same filters.
//!
//! **Inputs.** Each cut reads its source through its own ffmpeg input, opened with
//! `-noaccurate_seek -ss` about a second before the cut, so only the cuts are decoded
//! and nothing is buffered for a later cut (the memory trap of `split` + `trim` +
//! `concat` on one input). Global `-copyts -start_at_zero` keeps the source's own
//! timestamps, with 0 at the start of the file, so seeking does not move the frame
//! grid.
//!
//! **Frame-exact video.** `fps=fps=<meta.fps>:start_time=0` puts a source of any frame
//! rate, constant or variable, on the frame grid that `sabicut-core` snaps cuts to
//! (boundary `k` at `k / fps` seconds): output frame `k` shows the last source frame
//! whose time rounds to `k` or earlier. With `start_time=0` the grid is anchored at
//! source time 0 whatever the seek point was, and slots before the first decoded frame
//! repeat that frame. `trim` then keeps exactly the cut's frames.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use sabicut_core::Segment;

use crate::geometry::{CropRect, crop_window};
use crate::probe::{HdrTransfer, SourceInfo, file_url};
use crate::zoom::ZoomCurve;

/// Inputs are opened this many seconds before the first frame they must deliver.
/// Seeking lands on the keyframe at or before that point, so this only guards against
/// containers whose seeking is imprecise.
pub(crate) const PREROLL_SECONDS: f64 = 1.0;

/// Decoder threads for a source up to 1080p. ffmpeg's default (one per CPU, plus one)
/// keeps a decoded frame in flight per thread; two still decode 1080p far faster than
/// x264 encodes, and keep the decoder's memory down (PRD §8: under 800 MB in all).
const DECODER_THREADS: u32 = 2;
/// Sources larger than this many pixels (4K, say) get one decoder thread: each frame in
/// flight costs 12 to 25 MB, and decoding is not their bottleneck (tone mapping is).
const LARGE_FRAME_PIXELS: u64 = 1920 * 1088;

/// Output audio sample rate in Hz (PRD F5: AAC 48 kHz).
pub const SAMPLE_RATE: u32 = 48_000;

/// Length of the fade in and fade out at every cut, in samples: 10 ms at 48 kHz.
pub const FADE_SAMPLES: u64 = 480;

/// Audio clean-up after the cuts are joined (PRD F5): FFT denoise, an 80 Hz
/// high-pass against rumble, and gentle compression for speech. Loudness is
/// normalized afterwards with two-pass `loudnorm`.
pub(crate) const AUDIO_CLEANUP: &str = "afftdn=nr=12:nf=-40:tn=1,highpass=f=80,acompressor=threshold=-20dB:ratio=3:attack=5:release=120";

/// Collects ffmpeg input arguments; an input's index is the order it was added in.
#[derive(Debug, Default)]
pub(crate) struct Inputs {
    args: Vec<OsString>,
    count: usize,
}

impl Inputs {
    /// Adds a local media file, opened at the keyframe at or before `seek` seconds of
    /// source time. Returns its input index.
    pub(crate) fn add_seeked(&mut self, path: &Path, seek: f64, threads: u32) -> usize {
        self.args
            .extend(["-protocol_whitelist".into(), "file".into()]);
        self.args.push("-noaccurate_seek".into());
        self.args
            .extend(["-threads".into(), threads.to_string().into()]);
        if seek > 0.0 {
            self.args
                .extend(["-ss".into(), format!("{seek:.6}").into()]);
        }
        self.add(file_url(path))
    }

    /// Adds a local file, read from the start. Returns its input index.
    pub(crate) fn add_file(&mut self, path: &Path) -> usize {
        self.args
            .extend(["-protocol_whitelist".into(), "file".into()]);
        self.add(file_url(path))
    }

    /// Adds a local file of raw RGBA pictures (straight alpha), `width` × `height`, at
    /// `fps`. Returns its input index.
    pub(crate) fn add_rgba_file(
        &mut self,
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
    ) -> usize {
        self.args
            .extend(["-protocol_whitelist".into(), "file".into()]);
        self.rgba_format(width, height, fps);
        self.add(file_url(path))
    }

    /// Adds the caption band, raw RGBA pictures that `render` writes to the encoder's
    /// [`BAND_PIPE`]. Returns its input index.
    pub(crate) fn add_band_pipe(&mut self, band: CaptionBand, fps: u32) -> usize {
        self.args
            .extend(["-protocol_whitelist".into(), "pipe".into()]);
        self.rgba_format(band.width, band.height, fps);
        // A few band pictures (0.7 MB each on a 1080×1920 short) may wait between the
        // pipe and overlay.
        self.args
            .extend(["-thread_queue_size".into(), BAND_QUEUE.to_string().into()]);
        self.add(BAND_PIPE.into())
    }

    /// Adds the input an overlay is drawn from. Returns its input index.
    pub(crate) fn add_overlay(&mut self, overlay: &Overlay, fps: u32) -> usize {
        match &overlay.source {
            OverlaySource::File(path) => self.add_file(path),
            OverlaySource::Rgba {
                path,
                width,
                height,
            } => self.add_rgba_file(path, *width, *height, fps),
            OverlaySource::CaptionBand(band) => self.add_band_pipe(*band, fps),
        }
    }

    fn rgba_format(&mut self, width: u32, height: u32, fps: u32) {
        self.args.extend([
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-s".into(),
            format!("{width}x{height}").into(),
            "-framerate".into(),
            fps.to_string().into(),
        ]);
    }

    fn add(&mut self, url: OsString) -> usize {
        self.args.extend(["-i".into(), url]);
        self.count += 1;
        self.count - 1
    }

    /// The arguments, in order.
    pub(crate) fn args(&self) -> &[OsString] {
        &self.args
    }
}

/// A filter graph under construction: one chain per line.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    text: String,
}

impl Graph {
    /// Appends `[in1][in2]filters[out1][out2]`.
    pub(crate) fn chain(&mut self, inputs: &[&str], filters: &str, outputs: &[&str]) {
        if !self.text.is_empty() {
            self.text.push_str(";\n");
        }
        // Writing to a String cannot fail.
        for input in inputs {
            let _ = write!(self.text, "[{input}]");
        }
        self.text.push_str(filters);
        for output in outputs {
            let _ = write!(self.text, "[{output}]");
        }
    }

    /// The graph text, ready for `-/filter_complex <file>`.
    pub(crate) fn finish(mut self) -> String {
        self.text.push('\n');
        self.text
    }
}

/// How one source's frames become output frames: its crop window, colour handling and
/// the output size and rate.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VideoSpec {
    fps: u32,
    out: (u32, u32),
    crop: CropRect,
    /// `scale`'s name for the source's YUV matrix, like `bt709`.
    matrix: &'static str,
    /// `tv` (limited) or `pc` (full).
    range: &'static str,
    hdr: Option<Hdr>,
    decoder_threads: u32,
}

/// The colour description of an HDR source, for `zscale`.
#[derive(Clone, Debug, PartialEq)]
struct Hdr {
    transfer: HdrTransfer,
    matrix: String,
    primaries: String,
}

impl VideoSpec {
    /// The spec for `info`, with the crop window centred at `crop_x`.
    pub(crate) fn new(info: &SourceInfo, crop_x: f64, fps: u32, out: (u32, u32)) -> Self {
        let (width, height) = info.display_size();
        let crop = crop_window(width, height, info.display_sar(), crop_x, out);
        let full_range =
            info.colour.range.as_deref() == Some("pc") || info.pix_fmt.starts_with("yuvj");
        let hdr = info.hdr().map(|transfer| Hdr {
            transfer,
            matrix: zscale_matrix(info.colour.matrix.as_deref()).to_owned(),
            primaries: zscale_primaries(info.colour.primaries.as_deref()).to_owned(),
        });
        Self {
            fps,
            out,
            crop,
            matrix: scale_matrix(info.colour.matrix.as_deref(), width.max(height)),
            range: if full_range { "pc" } else { "tv" },
            hdr,
            decoder_threads: if u64::from(width) * u64::from(height) > LARGE_FRAME_PIXELS {
                1
            } else {
                DECODER_THREADS
            },
        }
    }

    /// The crop window on the upright source frame.
    pub(crate) fn crop(&self) -> CropRect {
        self.crop
    }

    /// How many threads decode this source.
    pub(crate) fn decoder_threads(&self) -> u32 {
        self.decoder_threads
    }

    /// Whether the source is tone-mapped from HDR.
    pub(crate) fn is_hdr(&self) -> bool {
        self.hdr.is_some()
    }
}

/// `scale`'s `in_color_matrix` for a source's matrix tag. Untagged sources are taken to
/// be BT.709 when HD (1280 px or more on the long side) and BT.601 otherwise, as
/// players do.
fn scale_matrix(tag: Option<&str>, long_side: u32) -> &'static str {
    match tag {
        Some("bt709") => "bt709",
        Some("smpte170m" | "bt470bg") => "smpte170m",
        Some("fcc") => "fcc",
        Some("smpte240m") => "smpte240m",
        Some("bt2020nc" | "bt2020c") => "bt2020",
        _ if long_side >= 1280 => "bt709",
        _ => "smpte170m",
    }
}

fn zscale_matrix(tag: Option<&str>) -> &'static str {
    match tag {
        Some("bt2020c") => "bt2020c",
        Some("bt709") => "bt709",
        _ => "bt2020nc",
    }
}

fn zscale_primaries(tag: Option<&str>) -> &'static str {
    match tag {
        Some("bt709") => "bt709",
        Some("smpte432") => "smpte432",
        _ => "bt2020",
    }
}

/// The filters that turn the upright video of an input opened before `segment` into
/// the segment's local frames `range` (0 is the segment's first frame): frame-exact
/// selection, crop, tone mapping for HDR, scale and zoom.
///
/// Timestamps start at `range.start`, counted in frames, so the zoom expression sees
/// the same frame numbers whether it renders the whole segment or one still. The
/// chain always yields exactly `range.end - range.start` frames: if a source ends a
/// little early, its last frame is held.
pub(crate) fn video_chain(
    spec: &VideoSpec,
    curve: &ZoomCurve,
    segment: &Segment,
    range: Range<u64>,
) -> String {
    let fps = spec.fps;
    let (out_w, out_h) = spec.out;
    let CropRect { w, h, x, y } = spec.crop;
    let first = segment.source_in().saturating_add(range.start);
    let end = segment.source_in().saturating_add(range.end);
    let count = range.end.saturating_sub(range.start);
    let mut filters = vec![
        format!("fps=fps={fps}:start_time=0"),
        format!("trim=start_pts={first}:end_pts={end}"),
        format!("setpts=PTS-STARTPTS+{}", range.start),
        format!("crop={w}:{h}:{x}:{y}"),
    ];
    let (matrix, range_in) = match &spec.hdr {
        Some(hdr) => {
            filters.push(tone_map(hdr, spec.out));
            ("bt709", "tv")
        }
        None => (spec.matrix, spec.range),
    };
    let colour =
        format!("in_color_matrix={matrix}:out_color_matrix=bt709:in_range={range_in}:out_range=tv");
    if curve.is_flat() {
        filters.push(format!("scale={out_w}:{out_h}:flags=bicubic:{colour}"));
        filters.push("format=yuv420p".into());
    } else {
        // Convert colour at the crop's size, then let zoompan crop the zoomed centre and
        // scale it to the output size in one step. `it` is the input frame's time, so
        // round(it·fps) is the local frame number set by setpts above.
        filters.push(format!("scale=flags=bicubic:{colour}"));
        filters.push("format=yuv420p".into());
        filters.push(format!(
            "zoompan=z='{}':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=1:s={out_w}x{out_h}:fps={fps}",
            curve.expression(&format!("round(it*{fps})"))
        ));
    }
    // Tag the frames BT.709 (what they now are) so the encoder writes it into the
    // stream; ffmpeg 7 and later take these tags from the frames.
    filters
        .push("setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709".into());
    filters.push("setsar=1".into());
    filters.push(format!(
        "tpad=stop=-1:stop_mode=clone,trim=end_frame={count}"
    ));
    filters.join(",")
}

/// HDR (PQ or HLG, BT.2020) to SDR BT.709 with zscale and tonemap (Hable curve), on
/// linear light in 32-bit float RGB. The first zscale also scales the crop window to
/// the output size, so the float frames are no bigger than the output (a 4K crop would
/// be twice the pixels) and the tone mapping is faster.
fn tone_map(hdr: &Hdr, (width, height): (u32, u32)) -> String {
    format!(
        "zscale=w={width}:h={height}:f=bicubic:tin={}:min={}:pin={}:t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p",
        hdr.transfer.ffmpeg_name(),
        hdr.matrix,
        hdr.primaries,
    )
}

/// Filters needed for a video chain: always, and with zoom or HDR.
pub(crate) const VIDEO_FILTERS: &[&str] = &[
    "fps",
    "trim",
    "setpts",
    "crop",
    "scale",
    "format",
    "setparams",
    "setsar",
    "tpad",
    "concat",
];
/// Filters needed to zoom.
pub(crate) const ZOOM_FILTERS: &[&str] = &["zoompan"];
/// Filters needed to draw overlays, captions and the hook.
pub(crate) const OVERLAY_FILTERS: &[&str] = &["overlay", "scale", "format"];
/// Filters needed to tone-map HDR sources.
pub(crate) const HDR_FILTERS: &[&str] = &["zscale", "tonemap"];
/// Filters needed for the audio.
pub(crate) const AUDIO_FILTERS: &[&str] = &[
    "aresample",
    "aformat",
    "atrim",
    "asetpts",
    "apad",
    "afade",
    "anullsrc",
    "anull",
    "concat",
    "afftdn",
    "highpass",
    "acompressor",
    "loudnorm",
];

/// The audio sample at frame boundary `frame`: `frame / fps` seconds at
/// [`SAMPLE_RATE`], rounded to the nearest sample (halves up). Exact integer maths.
pub fn sample_at(frame: u64, fps: u32) -> u64 {
    let (frame, fps, rate) = (
        u128::from(frame),
        u128::from(fps.max(1)),
        u128::from(SAMPLE_RATE),
    );
    let sample = (2 * frame * rate + fps) / (2 * fps);
    u64::try_from(sample).unwrap_or(u64::MAX)
}

/// Where a segment's audio comes from on the source, and how long it is on the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AudioSpan {
    /// First source sample (at [`SAMPLE_RATE`]).
    pub(crate) source_start: u64,
    /// Number of output samples. Counted between the segment's output frame
    /// boundaries, so the audio never drifts from the video over many cuts.
    pub(crate) samples: u64,
}

impl AudioSpan {
    /// The span of `segment` at `fps`.
    pub(crate) fn new(segment: &Segment, fps: u32) -> Self {
        Self {
            source_start: sample_at(segment.source_in(), fps),
            samples: sample_at(segment.output_end(), fps)
                .saturating_sub(sample_at(segment.output_start(), fps)),
        }
    }
}

/// The filters that turn the audio of an input opened at `seek` seconds into the
/// segment's samples, with a short fade at both ends against clicks. The result is
/// always exactly `span.samples` long: a source that ends early is padded with
/// silence.
pub(crate) fn audio_chain(span: AudioSpan, seek: f64) -> String {
    let rate = SAMPLE_RATE;
    let first_pts = seconds_to_samples(seek);
    let (start, count) = (span.source_start, span.samples);
    let end = start.saturating_add(count);
    format!(
        "aresample={rate}:async=1:first_pts={first_pts},aformat=sample_fmts=fltp:channel_layouts=stereo,atrim=start_pts={start}:end_pts={end},asetpts=PTS-STARTPTS,apad=whole_len={count},atrim=end_sample={count},{}",
        fades(count)
    )
}

/// A source filter producing `samples` of stereo silence, for a source with no audio.
pub(crate) fn silence(samples: u64) -> String {
    format!(
        "anullsrc=r={SAMPLE_RATE}:cl=stereo,atrim=end_sample={samples},aformat=sample_fmts=fltp:channel_layouts=stereo"
    )
}

fn fades(count: u64) -> String {
    let fade = FADE_SAMPLES.min(count / 2).max(1);
    format!(
        "afade=t=in:ss=0:ns={fade},afade=t=out:ss={}:ns={fade}",
        count.saturating_sub(fade)
    )
}

fn seconds_to_samples(seconds: f64) -> u64 {
    let samples = (seconds.max(0.0) * f64::from(SAMPLE_RATE)).round();
    if samples.is_finite() && samples < 1e18 {
        // Non-negative, whole and in range, so the cast is exact.
        samples as u64
    } else {
        0
    }
}

/// Where to open an input so that it delivers source frame `frame` (at `fps`): the
/// pre-roll before that frame's time, but not before the start of the file.
pub(crate) fn seek_before(frame: u64, fps: u32) -> f64 {
    let time = frame as f64 / f64::from(fps.max(1));
    (time - PREROLL_SECONDS).max(0.0)
}

/// A picture drawn over the joined cuts, before encoding: how the captions and the hook
/// (PRD F6) are placed, and the way to add other graphics. PRD §8 keeps this cheap:
/// captions are a caption band (756×240 RGBA on a 1080×1920 short) placed with
/// `overlay=x:y`, and other graphics are only drawn inside their time window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// Where the overlay's frames come from.
    pub source: OverlaySource,
    /// Left edge on the output frame, in pixels.
    pub x: u32,
    /// Top edge on the output frame, in pixels.
    pub y: u32,
    /// Output frames it is drawn on, as `start..end`; `None` draws it on every frame.
    pub frames: Option<Range<u64>>,
}

/// Where an overlay's frames come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlaySource {
    /// A local image or video file. An image (a PNG with transparency, say) is held for
    /// as long as the overlay is drawn; a video is drawn frame by frame from output
    /// frame 0 and its last frame is held when it ends.
    File(PathBuf),
    /// A local file of raw RGBA pictures, 8 bits a channel with straight (not
    /// premultiplied) alpha, `width` × `height`, one per output frame from frame 0. The
    /// last picture is held when the file ends.
    Rgba {
        /// The file.
        path: PathBuf,
        /// Picture width in pixels.
        width: u32,
        /// Picture height in pixels.
        height: u32,
    },
    /// The caption band: raw RGBA pictures that `render` draws from the project's
    /// captions for every output frame and pipes to ffmpeg while it encodes. Only
    /// `render` makes these.
    CaptionBand(CaptionBand),
}

impl OverlaySource {
    /// Whether the pictures are raw RGBA, which are converted to the video's colours
    /// before they are drawn.
    fn is_rgba(&self) -> bool {
        matches!(self, Self::Rgba { .. } | Self::CaptionBand(_))
    }
}

/// The size of the caption band piped to ffmpeg ([`OverlaySource::CaptionBand`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptionBand {
    width: u32,
    height: u32,
}

impl CaptionBand {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Where the encoder reads the caption band: its standard output, which `render`
/// connects to the reading end of a pipe (ffmpeg writes nothing there, since the video
/// goes to a file). Standard input already carries the video, and this needs no named
/// pipe or extra file descriptor, so it works the same everywhere.
pub(crate) const BAND_PIPE: &str = "pipe:1";
/// Caption band pictures that may wait in ffmpeg between the pipe and `overlay`, like
/// the video's queue of 4 frames.
const BAND_QUEUE: u32 = 4;

/// Converts raw RGBA pictures to the video's colours before `overlay` draws them:
/// BT.709, limited range, 4:2:0 with alpha. Done explicitly so every ffmpeg version
/// converts the same way (left to itself, ffmpeg 6.1 converts with BT.601).
pub(crate) const RGBA_TO_VIDEO: &str = "scale=out_color_matrix=bt709:out_range=tv,format=yuva420p";

/// Appends `overlays` over the video labelled `base` and returns the final label. Each
/// overlay comes with the input index of its pictures; `prefix` keeps the labels of
/// several chains in one graph apart.
pub(crate) fn overlay_chain(
    graph: &mut Graph,
    base: &str,
    overlays: &[(usize, &Overlay)],
    prefix: &str,
) -> String {
    let mut current = base.to_owned();
    for (k, (input, overlay)) in overlays.iter().enumerate() {
        let mut picture = format!("{input}:v:0");
        if overlay.source.is_rgba() {
            let converted = format!("{prefix}rgba{k}");
            graph.chain(&[&picture], RGBA_TO_VIDEO, &[&converted]);
            picture = converted;
        }
        let next = format!("{prefix}ov{k}");
        let enable = overlay.frames.as_ref().map_or_else(String::new, |frames| {
            format!(
                ":enable='between(n,{},{})'",
                frames.start,
                frames.end.saturating_sub(1)
            )
        });
        graph.chain(
            &[&current, &picture],
            &format!(
                "overlay=x={}:y={}:eof_action=repeat{enable}",
                overlay.x, overlay.y
            ),
            &[&next],
        );
        current = next;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::ColourTags;
    use sabicut_core::{Plan, Timeline};

    fn info(width: u32, height: u32) -> SourceInfo {
        SourceInfo {
            width,
            height,
            quarter_turns: 0,
            sar: (1, 1),
            codec: "h264".into(),
            pix_fmt: "yuv420p".into(),
            colour: ColourTags::default(),
            frame_rate: Some((60, 1)),
            avg_frame_rate: Some((60, 1)),
            video_start: 0.0,
            video_end: Some(60.0),
            has_audio: true,
        }
    }

    fn timeline(cuts: &str) -> Timeline {
        let plan = Plan::from_toml_str(&format!(
            r#"
            schema = 1
            [meta]
            fps = 30
            out = {{ w = 1080, h = 1920 }}
            [[source]]
            id = "a"
            path = "a.mp4"
            crop_x = 0.5
            {cuts}
            "#
        ))
        .unwrap();
        Timeline::from_plan(&plan).unwrap()
    }

    #[test]
    fn video_chain_without_zoom() {
        let timeline = timeline("[[cut]]\nsrc = \"a\"\nin = 3.12\nout = 9.88");
        let segment = &timeline.segments()[0];
        let spec = VideoSpec::new(&info(1920, 1080), 0.5, 30, (1080, 1920));
        let curve = ZoomCurve::new(segment, 30);
        assert_eq!(
            video_chain(&spec, &curve, segment, 0..segment.frames()),
            "fps=fps=30:start_time=0,trim=start_pts=94:end_pts=296,setpts=PTS-STARTPTS+0,crop=608:1080:656:0,\
             scale=1080:1920:flags=bicubic:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=tv,\
             format=yuv420p,setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709,\
             setsar=1,tpad=stop=-1:stop_mode=clone,trim=end_frame=202"
        );
        // A still of local frame 10 takes the same filters, trimmed to that frame.
        let still = video_chain(&spec, &curve, segment, 10..11);
        assert!(
            still.contains("trim=start_pts=104:end_pts=105,setpts=PTS-STARTPTS+10,"),
            "{still}"
        );
        assert!(still.ends_with("trim=end_frame=1"), "{still}");
    }

    #[test]
    fn video_chain_with_zoom_and_hdr() {
        let timeline = timeline(
            "[[cut]]\nsrc = \"a\"\nin = 3.12\nout = 9.88\nzoom = [{ at = 5.4, scale = 1.25 }]",
        );
        let segment = &timeline.segments()[0];
        let mut hlg = info(1920, 1080);
        hlg.colour.transfer = Some("arib-std-b67".into());
        hlg.colour.matrix = Some("bt2020nc".into());
        let spec = VideoSpec::new(&hlg, 0.5, 30, (1080, 1920));
        assert!(spec.is_hdr());
        let chain = video_chain(
            &spec,
            &ZoomCurve::new(segment, 30),
            segment,
            0..segment.frames(),
        );
        assert!(
            chain.contains("crop=608:1080:656:0,zscale=w=1080:h=1920:f=bicubic:tin=arib-std-b67:min=bt2020nc:pin=bt2020:t=linear:npl=100,"),
            "{chain}"
        );
        assert!(chain.contains("zscale=t=bt709:m=bt709:r=tv,format=yuv420p,scale=flags=bicubic:in_color_matrix=bt709:"), "{chain}");
        assert!(
            chain.contains("zoompan=z='if(lt(round(it*30),68),1,"),
            "{chain}"
        );
        assert!(
            chain.contains(":d=1:s=1080x1920:fps=30,setparams="),
            "{chain}"
        );
    }

    #[test]
    fn untagged_colour_follows_the_frame_size() {
        assert_eq!(scale_matrix(None, 1920), "bt709");
        assert_eq!(scale_matrix(None, 640), "smpte170m");
        assert_eq!(scale_matrix(Some("bt470bg"), 1920), "smpte170m");
        let mut full = info(1280, 720);
        full.pix_fmt = "yuvj420p".into();
        assert_eq!(VideoSpec::new(&full, 0.5, 30, (1080, 1920)).range, "pc");
    }

    #[test]
    fn audio_spans_follow_output_frame_boundaries() {
        assert_eq!(sample_at(94, 30), 150_400);
        // 7 fps does not divide 48 kHz: 1 frame is 6857.14 samples.
        assert_eq!(sample_at(1, 7), 6857);
        assert_eq!(sample_at(3, 7), 20_571);
        let timeline = timeline(
            "[[cut]]\nsrc = \"a\"\nin = 3.12\nout = 9.88\n[[cut]]\nsrc = \"a\"\nin = 12.0\nout = 15.5",
        );
        let spans: Vec<AudioSpan> = timeline
            .segments()
            .iter()
            .map(|s| AudioSpan::new(s, 30))
            .collect();
        assert_eq!(
            spans[0],
            AudioSpan {
                source_start: 150_400,
                samples: 202 * 1600
            }
        );
        assert_eq!(
            spans[1],
            AudioSpan {
                source_start: 576_000,
                samples: 105 * 1600
            }
        );
    }

    #[test]
    fn audio_chain_trims_pads_and_fades() {
        let chain = audio_chain(
            AudioSpan {
                source_start: 150_400,
                samples: 323_200,
            },
            2.133333,
        );
        assert_eq!(
            chain,
            "aresample=48000:async=1:first_pts=102400,aformat=sample_fmts=fltp:channel_layouts=stereo,\
             atrim=start_pts=150400:end_pts=473600,asetpts=PTS-STARTPTS,apad=whole_len=323200,\
             atrim=end_sample=323200,afade=t=in:ss=0:ns=480,afade=t=out:ss=322720:ns=480"
        );
        // A 1-frame cut at 240 fps is 200 samples: the fades take half each.
        assert!(
            audio_chain(
                AudioSpan {
                    source_start: 0,
                    samples: 200
                },
                0.0
            )
            .ends_with("afade=t=in:ss=0:ns=100,afade=t=out:ss=100:ns=100")
        );
    }

    #[test]
    fn seeking_starts_a_second_early_but_not_before_the_file() {
        assert_eq!(seek_before(94, 30), 94.0 / 30.0 - 1.0);
        assert_eq!(seek_before(15, 30), 0.0);
    }

    #[test]
    fn inputs_are_local_files_opened_before_the_cut() {
        let mut inputs = Inputs::default();
        assert_eq!(inputs.add_seeked(Path::new("/clips/a.mov"), 2.5, 2), 0);
        assert_eq!(inputs.add_seeked(Path::new("/clips/b.mov"), 0.0, 1), 1);
        assert_eq!(inputs.add_file(Path::new("/tmp/x.wav")), 2);
        let args: Vec<String> = inputs
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.join(" "),
            "-protocol_whitelist file -noaccurate_seek -threads 2 -ss 2.500000 -i file:/clips/a.mov \
             -protocol_whitelist file -noaccurate_seek -threads 1 -i file:/clips/b.mov \
             -protocol_whitelist file -i file:/tmp/x.wav"
        );
    }

    #[test]
    fn overlays_are_drawn_in_order_inside_their_windows() {
        let mut graph = Graph::default();
        let overlays = [
            Overlay {
                source: OverlaySource::File("band.png".into()),
                x: 0,
                y: 1300,
                frames: None,
            },
            Overlay {
                source: OverlaySource::Rgba {
                    path: "hook.rgba".into(),
                    width: 756,
                    height: 300,
                },
                x: 162,
                y: 262,
                frames: Some(0..60),
            },
            Overlay {
                source: OverlaySource::CaptionBand(CaptionBand::new(756, 240)),
                x: 162,
                y: 1128,
                frames: None,
            },
        ];
        let indexed: Vec<(usize, &Overlay)> =
            overlays.iter().zip(3..).map(|(o, i)| (i, o)).collect();
        let label = overlay_chain(&mut graph, "vcat", &indexed, "e");
        assert_eq!(label, "eov2");
        assert_eq!(
            graph.finish(),
            "[vcat][3:v:0]overlay=x=0:y=1300:eof_action=repeat[eov0];\n\
             [4:v:0]scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[ergba1];\n\
             [eov0][ergba1]overlay=x=162:y=262:eof_action=repeat:enable='between(n,0,59)'[eov1];\n\
             [5:v:0]scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[ergba2];\n\
             [eov1][ergba2]overlay=x=162:y=1128:eof_action=repeat[eov2]\n"
        );
    }

    #[test]
    fn rgba_inputs_are_raw_video() {
        let mut inputs = Inputs::default();
        inputs.add_file(Path::new("/w/audio.wav"));
        let hook = Overlay {
            source: OverlaySource::Rgba {
                path: "/w/hook.rgba".into(),
                width: 756,
                height: 300,
            },
            x: 162,
            y: 262,
            frames: Some(0..60),
        };
        let band = Overlay {
            source: OverlaySource::CaptionBand(CaptionBand::new(756, 240)),
            x: 162,
            y: 1128,
            frames: None,
        };
        assert_eq!(inputs.add_overlay(&hook, 30), 1);
        assert_eq!(inputs.add_overlay(&band, 30), 2);
        let args: Vec<String> = inputs
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.join(" "),
            "-protocol_whitelist file -i file:/w/audio.wav \
             -protocol_whitelist file -f rawvideo -pix_fmt rgba -s 756x300 -framerate 30 -i file:/w/hook.rgba \
             -protocol_whitelist pipe -f rawvideo -pix_fmt rgba -s 756x240 -framerate 30 -thread_queue_size 4 -i pipe:1"
        );
    }
}
