//! Reading what a source clip contains, with `ffprobe`'s JSON output.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

use crate::tools::{Ffmpeg, ToolError};

/// Why a source clip could not be probed.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// ffprobe could not read the file.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// ffprobe printed JSON this code does not understand.
    #[error("cannot read ffprobe's output: {0}")]
    Json(#[from] serde_json::Error),
    /// The file has no video stream (cover art does not count).
    #[error("has no video stream")]
    NoVideo,
    /// The video stream has no usable frame size.
    #[error("video stream has no frame size")]
    NoSize,
}

/// How a high-dynamic-range source encodes brightness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrTransfer {
    /// Perceptual quantizer, SMPTE ST 2084 (`smpte2084`): HDR10, many Android phones.
    Pq,
    /// Hybrid log-gamma, ARIB STD-B67 (`arib-std-b67`): iPhone HDR video.
    Hlg,
}

impl HdrTransfer {
    /// The transfer's name in ffmpeg and zscale.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Pq => "smpte2084",
            Self::Hlg => "arib-std-b67",
        }
    }
}

/// The colour tags of a video stream, as ffprobe names them. `None` when untagged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColourTags {
    /// `tv` (limited) or `pc` (full).
    pub range: Option<String>,
    /// Matrix coefficients, like `bt709` or `bt2020nc`.
    pub matrix: Option<String>,
    /// Transfer characteristics, like `bt709` or `arib-std-b67`.
    pub transfer: Option<String>,
    /// Colour primaries, like `bt709` or `bt2020`.
    pub primaries: Option<String>,
}

/// What the render needs to know about one source clip.
///
/// Times are seconds of source time: 0 is the start of the file (the earliest stream),
/// which is where players start counting and what `-copyts -start_at_zero` gives.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    /// Stored frame width in pixels, before rotation.
    pub width: u32,
    /// Stored frame height in pixels, before rotation.
    pub height: u32,
    /// Clockwise quarter turns players apply for display, 0 to 3.
    pub quarter_turns: u8,
    /// Sample (pixel) aspect ratio as `(num, den)`; `(1, 1)` for square pixels.
    pub sar: (u32, u32),
    /// Video codec, like `h264` or `hevc`.
    pub codec: String,
    /// Decoded pixel format, like `yuv420p` or `yuv420p10le`.
    pub pix_fmt: String,
    /// Colour tags of the video stream.
    pub colour: ColourTags,
    /// Nominal frame rate (`r_frame_rate`), if known.
    pub frame_rate: Option<(u32, u32)>,
    /// Average frame rate (`avg_frame_rate`), if known. Differs from `frame_rate` for
    /// variable-frame-rate clips.
    pub avg_frame_rate: Option<(u32, u32)>,
    /// When the first video frame is shown.
    pub video_start: f64,
    /// When the video ends, if the file says.
    pub video_end: Option<f64>,
    /// Whether the file has an audio stream.
    pub has_audio: bool,
}

impl SourceInfo {
    /// Frame size in pixels after rotation: the frames the crop is applied to, since
    /// ffmpeg rotates frames upright as it decodes them. Pixels may not be square; see
    /// [`SourceInfo::display_sar`].
    pub fn display_size(&self) -> (u32, u32) {
        if self.quarter_turns % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Sample aspect ratio of the upright frame (inverted by a quarter turn).
    pub fn display_sar(&self) -> (u32, u32) {
        if self.quarter_turns % 2 == 1 {
            (self.sar.1, self.sar.0)
        } else {
            self.sar
        }
    }

    /// The HDR transfer, if the clip is tagged PQ or HLG.
    pub fn hdr(&self) -> Option<HdrTransfer> {
        match self.colour.transfer.as_deref() {
            Some("smpte2084") => Some(HdrTransfer::Pq),
            Some("arib-std-b67") => Some(HdrTransfer::Hlg),
            _ => None,
        }
    }

    /// Whether frames come at a variable rate: the average rate differs from the
    /// nominal one.
    pub fn is_variable_frame_rate(&self) -> bool {
        match (self.frame_rate, self.avg_frame_rate) {
            (Some((a, b)), Some((c, d))) => {
                u64::from(a) * u64::from(d) != u64::from(c) * u64::from(b)
            }
            _ => false,
        }
    }
}

/// Probes the clip at `path`. Only local files are read (`file:` protocol), so a path
/// in a plan can never make ffmpeg open a network address.
pub fn probe(tools: &Ffmpeg, path: &Path) -> Result<SourceInfo, ProbeError> {
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([file_url(path)])
    .collect();
    let json = tools.run_ffprobe(&args)?;
    parse(&json)
}

/// `path` as an ffmpeg `file:` URL, so it is never read as an option, a protocol
/// (`http:`, `concat:`) or a device.
pub(crate) fn file_url(path: &Path) -> OsString {
    let mut url = OsString::from("file:");
    url.push(path.as_os_str());
    url
}

#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    streams: Vec<Stream>,
    #[serde(default)]
    format: Format,
}

#[derive(Default, Deserialize)]
struct Format {
    start_time: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    start_time: Option<String>,
    duration: Option<String>,
    #[serde(default)]
    disposition: BTreeMap<String, i64>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct SideData {
    side_data_type: Option<String>,
    rotation: Option<serde_json::Number>,
}

/// Parses `ffprobe -print_format json -show_format -show_streams` output.
pub(crate) fn parse(json: &[u8]) -> Result<SourceInfo, ProbeError> {
    let output: Output = serde_json::from_slice(json)?;
    let streams = &output.streams;
    // The first video stream that is not cover art, like ffmpeg's `V:0`.
    let video = streams
        .iter()
        .find(|s| {
            s.codec_type.as_deref() == Some("video")
                && s.disposition.get("attached_pic").copied().unwrap_or(0) == 0
        })
        .ok_or(ProbeError::NoVideo)?;
    let (width, height) = match (video.width, video.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => return Err(ProbeError::NoSize),
    };
    let file_start = seconds(output.format.start_time.as_deref()).unwrap_or(0.0);
    let video_start =
        seconds(video.start_time.as_deref()).map_or(0.0, |start| (start - file_start).max(0.0));
    let video_end = match seconds(video.duration.as_deref()) {
        Some(duration) => Some(video_start + duration),
        None => seconds(output.format.duration.as_deref()),
    };
    Ok(SourceInfo {
        width,
        height,
        quarter_turns: quarter_turns(video),
        sar: video
            .sample_aspect_ratio
            .as_deref()
            .and_then(ratio)
            .unwrap_or((1, 1)),
        codec: video.codec_name.clone().unwrap_or_default(),
        pix_fmt: video.pix_fmt.clone().unwrap_or_default(),
        colour: ColourTags {
            range: known(&video.color_range),
            matrix: known(&video.color_space),
            transfer: known(&video.color_transfer),
            primaries: known(&video.color_primaries),
        },
        frame_rate: video.r_frame_rate.as_deref().and_then(ratio),
        avg_frame_rate: video.avg_frame_rate.as_deref().and_then(ratio),
        video_start,
        video_end,
        has_audio: streams
            .iter()
            .any(|s| s.codec_type.as_deref() == Some("audio")),
    })
}

/// Clockwise quarter turns from the display matrix (or the old `rotate` tag).
/// ffprobe reports the matrix's counter-clockwise angle, so -90 is a clockwise quarter
/// turn. Only whether the turn is odd matters for the frame size.
fn quarter_turns(stream: &Stream) -> u8 {
    let from_matrix = stream
        .side_data_list
        .iter()
        .filter(|side| side.side_data_type.as_deref() == Some("Display Matrix"))
        .find_map(|side| side.rotation.as_ref()?.as_f64())
        .map(|degrees| -degrees);
    let degrees = from_matrix.or_else(|| stream.tags.get("rotate")?.trim().parse().ok());
    match degrees {
        Some(degrees) if degrees.is_finite() => {
            let turns = (degrees / 90.0).round().rem_euclid(4.0);
            // In 0..4 after rem_euclid, so the cast is exact.
            turns as u8
        }
        _ => 0,
    }
}

fn seconds(value: Option<&str>) -> Option<f64> {
    value?.trim().parse::<f64>().ok().filter(|s| s.is_finite())
}

/// `"30000/1001"` or `"16:9"` as a ratio with a non-zero denominator and numerator.
fn ratio(text: &str) -> Option<(u32, u32)> {
    let (num, den) = text.split_once(['/', ':'])?;
    let (num, den) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    (num > 0 && den > 0).then_some((num, den))
}

fn known(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .filter(|v| !v.is_empty() && *v != "unknown" && *v != "unspecified" && *v != "reserved")
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPHONE_HLG: &str = r#"{
        "streams": [
            {"index": 0, "codec_name": "hevc", "codec_type": "video", "width": 1920, "height": 1080,
             "pix_fmt": "yuv420p10le", "sample_aspect_ratio": "1:1", "color_range": "tv",
             "color_space": "bt2020nc", "color_transfer": "arib-std-b67", "color_primaries": "bt2020",
             "r_frame_rate": "30/1", "avg_frame_rate": "16200/541", "start_time": "0.000000",
             "duration": "18.033333", "disposition": {"default": 1, "attached_pic": 0},
             "side_data_list": [{"side_data_type": "Display Matrix", "displaymatrix": "...", "rotation": -90}]},
            {"index": 1, "codec_name": "aac", "codec_type": "audio", "start_time": "0.000000"},
            {"index": 2, "codec_type": "data", "codec_name": "none"}
        ],
        "format": {"start_time": "0.000000", "duration": "18.040000"}
    }"#;

    #[test]
    fn iphone_hlg_portrait_clip() {
        let info = parse(IPHONE_HLG.as_bytes()).unwrap();
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.quarter_turns, 1);
        assert_eq!(info.display_size(), (1080, 1920));
        assert_eq!(info.hdr(), Some(HdrTransfer::Hlg));
        assert_eq!(info.colour.matrix.as_deref(), Some("bt2020nc"));
        assert!(info.is_variable_frame_rate());
        assert!(info.has_audio);
        assert_eq!(info.video_start, 0.0);
        assert!((info.video_end.unwrap() - 18.033333).abs() < 1e-9);
    }

    #[test]
    fn start_times_are_relative_to_the_file_start() {
        let json = r#"{
            "streams": [
                {"codec_type": "audio", "start_time": "1.476780"},
                {"codec_type": "video", "codec_name": "h264", "width": 1280, "height": 720,
                 "start_time": "1.500000", "duration": "10.000000", "color_transfer": "unknown",
                 "tags": {"rotate": "180"}}
            ],
            "format": {"start_time": "1.476780", "duration": "10.023220"}
        }"#;
        let info = parse(json.as_bytes()).unwrap();
        assert!((info.video_start - 0.02322).abs() < 1e-9);
        assert!((info.video_end.unwrap() - 10.02322).abs() < 1e-9);
        assert_eq!(info.quarter_turns, 2);
        assert_eq!(info.display_size(), (1280, 720));
        assert_eq!(info.colour.transfer, None);
        assert_eq!(info.hdr(), None);
    }

    #[test]
    fn cover_art_is_not_the_video() {
        let json = r#"{
            "streams": [
                {"codec_type": "video", "width": 600, "height": 600, "disposition": {"attached_pic": 1}},
                {"codec_type": "audio"}
            ],
            "format": {}
        }"#;
        assert!(matches!(parse(json.as_bytes()), Err(ProbeError::NoVideo)));
    }

    #[test]
    fn rotations_become_clockwise_quarter_turns() {
        let turns = |rotation: &str| {
            let json = format!(
                r#"{{"streams": [{{"codec_type": "video", "width": 4, "height": 2,
                   "side_data_list": [{{"side_data_type": "Display Matrix", "rotation": {rotation}}}]}}]}}"#
            );
            parse(json.as_bytes()).unwrap().quarter_turns
        };
        assert_eq!(turns("-90"), 1);
        assert_eq!(turns("90"), 3);
        assert_eq!(turns("180"), 2);
        assert_eq!(turns("-180.0"), 2);
        assert_eq!(turns("0"), 0);
    }

    #[test]
    fn ratios() {
        assert_eq!(ratio("30000/1001"), Some((30000, 1001)));
        assert_eq!(ratio("4:3"), Some((4, 3)));
        assert_eq!(ratio("0/0"), None);
        assert_eq!(ratio("0:1"), None);
    }

    #[test]
    fn file_urls_never_look_like_options() {
        assert_eq!(file_url(Path::new("-i.mp4")), "file:-i.mp4");
        assert_eq!(
            file_url(Path::new("/clips/http:x.mov")),
            "file:/clips/http:x.mov"
        );
    }
}
