//! End-to-end tests with the real ffmpeg, on synthetic clips generated with lavfi at
//! test time (no media is committed).
//!
//! The clips carry a grey patch in the middle whose brightness encodes the source
//! frame number (`16 + 7 × (N mod 32)`), so the frame shown by every output frame can
//! be read back and compared with what the timeline says, without fonts or OCR.
//!
//! These tests need `ffmpeg` and `ffprobe` (set `SABICUT_FFMPEG` / `SABICUT_FFPROBE`
//! to use other builds). Each takes a global lock, so only one ffmpeg job runs at a
//! time and the suite fits on a small machine.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use sabicut_core::{Plan, SpeechSegment, Timeline, Transcript, Word};
use sabicut_overlay::{Font, Graphics, WordTimings};
use sabicut_render::{
    CONTACT_SHEET_NAME, Ffmpeg, HdrTransfer, OpenError, Preset, PreviewOptions, Project,
    RenderError, RenderOptions, RenderReport, ToolError, preview, render,
};
use serde_json::Value;

static FFMPEG_JOBS: Mutex<()> = Mutex::new(());

/// Runs one ffmpeg-heavy test at a time.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    FFMPEG_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tools() -> &'static Ffmpeg {
    static TOOLS: OnceLock<Ffmpeg> = OnceLock::new();
    TOOLS.get_or_init(|| {
        Ffmpeg::from_env().expect("these tests need ffmpeg and ffprobe on PATH (or SABICUT_FFMPEG)")
    })
}

fn media_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sabicut-render-media");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fresh, empty directory for one test's outputs.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("sabicut-render-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ffmpeg(args: &[&str]) {
    let output = Command::new(tools().ffmpeg_path())
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ffmpeg {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs ffmpeg with `args`, split on whitespace (no filter here contains a space), and
/// writes `output`.
fn ffmpeg_to(args: &str, output: &Path) {
    let mut all: Vec<&str> = args.split_whitespace().collect();
    all.push(output.to_str().unwrap());
    ffmpeg(&all);
}

fn ffmpeg_stdout(args: &[&str]) -> Vec<u8> {
    let output = Command::new(tools().ffmpeg_path())
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ffmpeg {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn ffprobe_json(path: &Path, extra: &[&str]) -> Value {
    let output = Command::new(tools().ffprobe_path())
        .args(["-v", "error", "-print_format", "json"])
        .args(extra)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The frame-number patch: grey `16 + 7 × (N mod 32)`, `size` pixels square.
fn index_patch(fps: u32, seconds: u32, size: u32) -> String {
    format!(
        "nullsrc=s=16x16:r={fps}:d={seconds},geq=lum='16+7*mod(N\\,32)':cb=128:cr=128,scale={size}:{size}:flags=neighbor"
    )
}

/// Generates `name` once (per test run) with `make`, unless it is already there.
fn clip(name: &str, make: impl FnOnce(&Path)) -> PathBuf {
    let path = media_dir().join(name);
    if !path.exists() {
        let partial = media_dir().join(format!("partial-{name}"));
        make(&partial);
        std::fs::rename(&partial, &path).unwrap();
    }
    path
}

/// 12 s of 1920×1080 at 60 fps (H.264), with a 440 Hz tone whose level drifts slowly,
/// so the second loudness pass can use one constant gain.
fn landscape() -> PathBuf {
    clip("landscape-1080p60-v1.mp4", |path| {
        let args = format!(
            "-f lavfi -i testsrc2=s=1920x1080:r=60:d=12 -f lavfi -i {} \
             -f lavfi -i sine=f=440:r=48000:d=12,volume='0.2+0.05*sin(2*PI*t/3)':eval=frame,aformat=channel_layouts=stereo \
             -filter_complex [0:v][1:v]overlay=(W-w)/2:(H-h)/2,format=yuv420p[v] -map [v] -map 2:a \
             -c:v libx264 -preset ultrafast -crf 16 -g 60 -c:a aac -b:a 128k -f mp4",
            index_patch(60, 12, 480)
        );
        ffmpeg_to(&args, path);
    })
}

/// 6 s of portrait 1080×1920 at 30 fps, stored as 1920×1080 with a display rotation, as
/// phones record it. A white bar marks the top of the picture.
fn rotated_portrait() -> PathBuf {
    clip("portrait-rotated-v1.mp4", |path| {
        let stored = media_dir().join("partial-portrait-stored.mp4");
        let args = format!(
            "-f lavfi -i testsrc2=s=1080x1920:r=30:d=6 -f lavfi -i {} -f lavfi -i sine=f=660:r=48000:d=6,volume=0.2 \
             -filter_complex [0:v][1:v]overlay=(W-w)/2:(H-h)/2,drawbox=x=0:y=0:w=1080:h=120:color=white:t=fill,transpose=clock,format=yuv420p[v] \
             -map [v] -map 2:a -c:v libx264 -preset ultrafast -crf 16 -c:a aac -f mp4",
            index_patch(30, 6, 360)
        );
        ffmpeg_to(&args, &stored);
        // Players turn it back upright, as ffmpeg's autorotate does.
        ffmpeg(&[
            "-display_rotation",
            "90",
            "-i",
            stored.to_str().unwrap(),
            "-c",
            "copy",
            "-f",
            "mp4",
            path.to_str().unwrap(),
        ]);
        std::fs::remove_file(stored).unwrap();
    })
}

/// 10 s of 1280×720 at a variable frame rate: nominally 30 fps, with every frame up to
/// 6 ms early or late and every tenth frame missing. The file starts at 1.5 s, like
/// some phone clips, and its audio is 44.1 kHz mono.
fn variable_frame_rate() -> PathBuf {
    clip("vfr-720p-v1.mp4", |path| {
        let args = format!(
            "-f lavfi -i testsrc2=s=1280x720:r=30:d=10 -f lavfi -i {} -f lavfi -i sine=f=523:r=44100:d=10,volume=0.2 \
             -filter_complex [0:v][1:v]overlay=(W-w)/2:(H-h)/2,format=yuv420p,settb=1/90000,setpts='(N/30+0.006*sin(N*1.7))/TB',select='not(eq(mod(n\\,10)\\,7))'[v] \
             -map [v] -map 2:a -c:v libx264 -preset ultrafast -crf 16 -bf 0 -fps_mode passthrough \
             -enc_time_base:v 1/90000 -video_track_timescale 90000 -c:a aac -output_ts_offset 1.5 -f mp4",
            index_patch(30, 10, 360)
        );
        ffmpeg_to(&args, path);
    })
}

/// 4 s of 1280×720 at 30 fps, 10-bit HEVC tagged HLG / BT.2020 like iPhone HDR video.
fn hdr_hlg() -> PathBuf {
    clip("hdr-hlg-hevc-v1.mp4", |path| {
        let args = format!(
            "-f lavfi -i testsrc2=s=1280x720:r=30:d=4 -f lavfi -i {} -f lavfi -i sine=f=330:r=48000:d=4,volume=0.2 \
             -filter_complex [0:v][1:v]overlay=(W-w)/2:(H-h)/2,format=yuv420p10le,setparams=range=tv:color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc[v] \
             -map [v] -map 2:a -c:v libx265 -preset ultrafast -crf 22 -x265-params log-level=error -tag:v hvc1 \
             -c:a aac -f mp4",
            index_patch(30, 4, 240)
        );
        ffmpeg_to(&args, path);
    })
}

/// Writes a plan next to the outputs, with `source` as source "a".
fn plan(dir: &Path, source: &Path, crop_x: f64, cuts: &str) -> (Plan, PathBuf) {
    let text = format!(
        "schema = 1\n[meta]\nfps = 30\nout = {{ w = 1080, h = 1920 }}\n[[source]]\nid = \"a\"\npath = {:?}\ncrop_x = {crop_x}\n{cuts}",
        source.to_str().unwrap()
    );
    let path = dir.join("plan.toml");
    std::fs::write(&path, &text).unwrap();
    (Plan::from_toml_str(&text).unwrap(), path)
}

fn render_plan(dir: &Path, plan: &Plan) -> (Project, RenderReport) {
    let project = Project::open(tools(), plan, dir).unwrap();
    let options = RenderOptions {
        output: dir.join("out.mp4"),
        cover: Some(dir.join("cover.jpg")),
        cover_at: None,
        preset: Preset::Ultrafast,
        overlays: Vec::new(),
        subtitles: None,
    };
    let report = render(tools(), &project, &options, &mut |_| {}).unwrap();
    (project, report)
}

/// The frame number (mod 32) shown in the middle of every frame of `path`, in order.
/// Read as RGB, so limited-range video and full-range JPEGs read the same.
fn frame_numbers(path: &Path) -> Vec<u32> {
    let rgb = ffmpeg_stdout(&[
        "-i",
        path.to_str().unwrap(),
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-vf",
        "crop=64:64:(iw-64)/2:(ih-64)/2,scale=1:1:flags=area,format=rgb24",
        "-f",
        "rawvideo",
        "-",
    ]);
    rgb.chunks(3).map(|px| patch_number(px[0])).collect()
}

/// Patch brightness `16 + 7k` (limited range) is `7k × 255/219` in RGB.
fn patch_number(red: u8) -> u32 {
    let k = (f64::from(red) * 219.0 / (7.0 * 255.0)).round() as i64;
    k.rem_euclid(32) as u32
}

/// The frame number a 60 or 30 fps constant-rate clip starting at 0 shows on output
/// frame `slot` of a 30 fps grid.
fn cfr_number(slot: u64, source_fps: u64) -> u32 {
    ((slot * source_fps / 30) % 32) as u32
}

/// What each output frame should show: for each segment, the source frame for each
/// grid slot, given `number_at(slot)`.
fn expected_numbers(timeline: &Timeline, number_at: impl Fn(u64) -> u32) -> Vec<u32> {
    timeline
        .segments()
        .iter()
        .flat_map(|s| {
            (s.source_in()..s.source_out())
                .map(&number_at)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn assert_numbers(got: &[u32], want: &[u32]) {
    assert_eq!(got.len(), want.len(), "frame count");
    let wrong: Vec<(usize, u32, u32)> = got
        .iter()
        .zip(want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .map(|(i, (g, w))| (i, *g, *w))
        .collect();
    assert!(
        wrong.is_empty(),
        "frames showing the wrong source frame (index, got, want): {wrong:?}"
    );
}

fn stream<'a>(probe: &'a Value, kind: &str) -> &'a Value {
    probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["codec_type"] == kind)
        .unwrap()
}

fn number(value: &Value) -> f64 {
    value.as_str().unwrap().parse().unwrap()
}

/// Checks everything PRD F5 promises about a rendered file.
fn assert_output_properties(path: &Path, timeline: &Timeline) {
    let probe = ffprobe_json(path, &["-show_format", "-show_streams"]);
    let video = stream(&probe, "video");
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(
        (video["width"].as_u64(), video["height"].as_u64()),
        (Some(1080), Some(1920))
    );
    assert_eq!(video["pix_fmt"], "yuv420p");
    assert_eq!(video["r_frame_rate"], "30/1");
    assert_eq!(video["avg_frame_rate"], "30/1");
    assert_eq!(video["color_space"], "bt709");
    assert_eq!(video["color_transfer"], "bt709");
    assert_eq!(video["color_primaries"], "bt709");
    let audio = stream(&probe, "audio");
    assert_eq!(audio["codec_name"], "aac");
    assert_eq!(audio["sample_rate"], "48000");
    assert_eq!(audio["channels"].as_u64(), Some(2));
    let frame = 1.0 / 30.0;
    let want = timeline.duration().to_f64();
    for duration in [
        number(&video["duration"]),
        number(&probe["format"]["duration"]),
    ] {
        assert!(
            (duration - want).abs() <= frame + 1e-6,
            "duration {duration} vs {want}"
        );
    }
    let bitrate = number(&audio["bit_rate"]);
    assert!(
        (100_000.0..=200_000.0).contains(&bitrate),
        "audio bit rate {bitrate}"
    );
    // Every sample is there: 1600 per frame at 48 kHz and 30 fps.
    let samples = ffmpeg_stdout(&[
        "-i",
        path.to_str().unwrap(),
        "-map",
        "0:a:0",
        "-f",
        "s16le",
        "-ac",
        "1",
        "-",
    ])
    .len()
        / 2;
    let want_samples = timeline.total_frames() as usize * 1600;
    assert!(
        samples.abs_diff(want_samples) <= 1600,
        "{samples} audio samples, want {want_samples}"
    );
    assert_fast_start(path);
    assert_loudness(path);
}

/// `+faststart`: the index (moov) comes before the media data (mdat).
fn assert_fast_start(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let mut boxes = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        boxes.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        let size = match size {
            0 => bytes.len() - at,
            1 => u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize,
            n => n,
        };
        at += size;
    }
    let position = |name: &str| boxes.iter().position(|b| b == name).unwrap();
    assert!(position("moov") < position("mdat"), "boxes: {boxes:?}");
}

/// Integrated loudness by ffmpeg's EBU R128 meter, independent of loudnorm.
fn assert_loudness(path: &Path) {
    let output = Command::new(tools().ffmpeg_path())
        .args(["-nostdin", "-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-af",
            "ebur128=framelog=quiet",
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stderr);
    let summary = &log[log.rfind("Summary:").unwrap()..];
    let line = summary
        .lines()
        .find(|l| l.trim_start().starts_with("I:"))
        .unwrap();
    let lufs: f64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(
        (lufs + 14.0).abs() <= 1.0,
        "integrated loudness {lufs} LUFS, want -14 ± 1"
    );
}

/// One frame of `path` (frame number `frame`, or the first), cropped by `crop` if
/// given, shrunk to 270×480 RGB.
fn small_rgb(path: &Path, frame: Option<u64>, crop: Option<&str>) -> Vec<u8> {
    let mut chain = String::new();
    if let Some(frame) = frame {
        chain.push_str(&format!("select=eq(n\\,{frame}),"));
    }
    if let Some(crop) = crop {
        chain.push_str(&format!("crop={crop},"));
    }
    chain.push_str("scale=270:480:flags=area,format=rgb24");
    ffmpeg_stdout(&[
        "-i",
        path.to_str().unwrap(),
        "-vf",
        &chain,
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-",
    ])
}

/// Mean absolute difference between two equally sized images.
fn difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(x.abs_diff(*y)))
        .sum::<f64>()
        / a.len() as f64
}

#[test]
fn landscape_60fps_render_is_frame_exact_and_normalized() {
    let _lock = one_at_a_time();
    let dir = scratch("landscape");
    let (plan, _) = plan(
        &dir,
        &landscape(),
        0.5,
        "[[cut]]\nsrc = \"a\"\nin = 0.0\nout = 1.0\n\
         [[cut]]\nsrc = \"a\"\nin = 3.12\nout = 6.88\nzoom = [{ at = 5.4, scale = 1.5 }]\n\
         [[cut]]\nsrc = \"a\"\nin = 10.0\nout = 11.95\n",
    );
    let (project, report) = render_plan(&dir, &plan);
    let timeline = project.timeline();
    assert_eq!(timeline.total_frames(), 30 + 112 + 59);
    assert_eq!(report.frames, timeline.total_frames());
    assert!(
        (report.loudness.integrated + 14.0).abs() <= 1.0,
        "{:?}",
        report.loudness
    );

    let output = dir.join("out.mp4");
    assert_output_properties(&output, timeline);
    let want = expected_numbers(timeline, |slot| cfr_number(slot, 60));
    assert_numbers(&frame_numbers(&output), &want);

    // The default cover is frame 30 (1 s): the first frame of the second cut.
    let (cover, frame) = report.cover.clone().unwrap();
    assert_eq!(frame, 30);
    let probe = ffprobe_json(&cover, &["-show_streams"]);
    assert_eq!(stream(&probe, "video")["codec_name"], "mjpeg");
    assert_eq!(
        (
            stream(&probe, "video")["width"].as_u64(),
            stream(&probe, "video")["height"].as_u64()
        ),
        (Some(1080), Some(1920))
    );
    assert_eq!(frame_numbers(&cover), [want[30]]);

    // Preview stills are the rendered frames: same source frame, crop and zoom. 3.3 s is
    // frame 99, in the middle of the zoom's ease (frames 98 to 103).
    let times = [0.5, 1.0, 3.3, 5.0, 6.69];
    let stills_dir = dir.join("stills");
    let stills = preview(
        tools(),
        &project,
        &PreviewOptions {
            times: times.to_vec(),
            out_dir: stills_dir.clone(),
            safe_zones: false,
        },
    )
    .unwrap();
    let frames: Vec<u64> = stills.stills.iter().map(|s| s.frame).collect();
    assert_eq!(frames, [15, 30, 99, 150, 200]);
    for still in &stills.stills {
        assert_eq!(
            frame_numbers(&still.path),
            [want[still.frame as usize]],
            "still {:?}",
            still.path
        );
        let diff = difference(
            &small_rgb(&output, Some(still.frame), None),
            &small_rgb(&still.path, None, None),
        );
        assert!(
            diff < 3.0,
            "still at frame {} differs from the render by {diff}",
            still.frame
        );
    }
    let sheet = ffprobe_json(&stills.contact_sheet, &["-show_streams"]);
    assert_eq!(stills.contact_sheet, stills_dir.join(CONTACT_SHEET_NAME));
    assert_eq!(
        (
            stream(&sheet, "video")["width"].as_u64(),
            stream(&sheet, "video")["height"].as_u64()
        ),
        (Some(1896), Some(672))
    );

    // The safe-zone guide shades the top bar and leaves the middle alone.
    let guided = preview(
        tools(),
        &project,
        &PreviewOptions {
            times: vec![5.0],
            out_dir: dir.join("guided"),
            safe_zones: true,
        },
    )
    .unwrap();
    let (plain, shaded) = (&stills.stills[3].path, &guided.stills[0].path);
    let top = difference(
        &small_rgb(plain, None, Some("1080:200:0:0")),
        &small_rgb(shaded, None, Some("1080:200:0:0")),
    );
    let middle = difference(
        &small_rgb(plain, None, Some("600:600:200:700")),
        &small_rgb(shaded, None, Some("600:600:200:700")),
    );
    assert!(top > 20.0, "top bar not shaded ({top})");
    assert!(middle < 0.5, "middle changed ({middle})");
}

/// The pixels of `crop` (`w:h:x:y`) in frame `frame` of `path`, as RGB.
fn region_rgb(path: &Path, frame: u64, crop: &str) -> Vec<u8> {
    ffmpeg_stdout(&[
        "-i",
        path.to_str().unwrap(),
        "-vf",
        &format!("select=eq(n\\,{frame}),crop={crop},format=rgb24"),
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-",
    ])
}

/// How many pixels are close to the caption gold, #FFC400.
fn gold_pixels(rgb: &[u8]) -> usize {
    rgb.chunks(3)
        .filter(|p| p[0] >= 200 && (150..=225).contains(&p[1]) && p[2] <= 90)
        .count()
}

#[test]
fn captions_and_hook_are_drawn_as_in_the_preview_with_subtitles() {
    let _lock = one_at_a_time();
    let dir = scratch("captions");
    let (plan, _) = plan(
        &dir,
        &landscape(),
        0.5,
        "[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 4.0\n\
         [[cut]]\nsrc = \"a\"\nin = 6.0\nout = 9.0\n\
         [hook]\ntext = \"Stop leaking your API keys\"\ndur = 2.0\n\
         [captions]\nwords = \"talk.words.json\"\nkeywords = [\"Supabase\"]\n\
         font = \"Anton-Regular.ttf\"\nactive = \"#FFC400\"\nstroke = 10\n",
    );
    let timeline = Timeline::from_plan(&plan).unwrap();
    let words = [
        ("Rust", 1.2, 1.6),
        ("and", 1.6, 1.8),
        ("Supabase", 1.8, 2.6),
        ("together.", 2.6, 3.4),
        ("Ship", 6.2, 6.6),
        ("it", 6.6, 6.8),
        ("tonight!", 6.8, 7.8),
    ];
    let transcript = Transcript::new(
        "landscape.mp4",
        "en",
        "test",
        12.0,
        vec![SpeechSegment::new(1.0, 9.0)],
        words
            .iter()
            .map(|&(text, start, end)| Word::new(text, start, end, 0.9))
            .collect(),
    )
    .unwrap();
    let mut timings = WordTimings::new();
    timings.insert("a", transcript);
    let graphics = Graphics::new(&plan, &timeline, &timings, &Font::bundled()).unwrap();
    let band = graphics.band().unwrap();
    let hook = graphics.hook().unwrap().placement;
    let band_crop = format!("{}:{}:{}:{}", band.width, band.height, band.x, band.y);
    let hook_crop = format!("{}:{}:{}:{}", hook.width, hook.height, hook.x, hook.y);
    let plain = Project::open(tools(), &plan, &dir).unwrap();
    let project = plain.clone().with_graphics(Some(graphics.clone()));
    // The subtitles may go anywhere: here in a new folder in the system temp folder,
    // which is often on another file system (RAM) than the video.
    let elsewhere = std::env::temp_dir().join(format!("sabicut-media-{}", std::process::id()));
    let options = RenderOptions {
        output: dir.join("out.mp4"),
        cover: Some(dir.join("cover.jpg")),
        cover_at: Some(3.5),
        preset: Preset::Ultrafast,
        overlays: Vec::new(),
        subtitles: Some(elsewhere.join("subtitles/out.srt")),
    };
    let report = render(tools(), &project, &options, &mut |_| {}).unwrap();
    let output = dir.join("out.mp4");
    assert_eq!(report.frames, 180);
    assert_output_properties(&output, &timeline);
    let (srt, cues) = report.subtitles.clone().unwrap();
    assert_eq!(cues, 2);
    assert_eq!(srt, elsewhere.join("subtitles/out.srt"));
    assert_eq!(
        std::fs::read_to_string(&srt).unwrap(),
        graphics.subtitles().unwrap()
    );
    let _ = std::fs::remove_dir_all(&elsewhere);

    // "Supabase" (output frames 24 to 48) is active at 1.3 s: gold in the band.
    assert!(gold_pixels(&region_rgb(&output, 39, &band_crop)) > 2000);

    // Preview stills show the captions and hook exactly as the render does.
    let times = [0.5, 1.3, 2.5];
    let stills = preview(
        tools(),
        &project,
        &PreviewOptions {
            times: times.to_vec(),
            out_dir: dir.join("stills"),
            safe_zones: false,
        },
    )
    .unwrap();
    let plain_stills = preview(
        tools(),
        &plain,
        &PreviewOptions {
            times: times.to_vec(),
            out_dir: dir.join("plain"),
            safe_zones: false,
        },
    )
    .unwrap();
    for (still, plain) in stills.stills.iter().zip(&plain_stills.stills) {
        let diff = difference(
            &small_rgb(&output, Some(still.frame), None),
            &small_rgb(&still.path, None, None),
        );
        assert!(
            diff < 3.0,
            "still at frame {} differs by {diff}",
            still.frame
        );
        // The hook shows on its first 60 frames only; captions show in all three.
        let hook_change = difference(
            &small_rgb(&still.path, None, Some(&hook_crop)),
            &small_rgb(&plain.path, None, Some(&hook_crop)),
        );
        let band_change = difference(
            &small_rgb(&still.path, None, Some(&band_crop)),
            &small_rgb(&plain.path, None, Some(&band_crop)),
        );
        if still.frame < 60 {
            assert!(hook_change > 10.0, "no hook at frame {}", still.frame);
        } else {
            assert!(hook_change < 0.5, "hook at frame {}", still.frame);
        }
        assert!(band_change > 5.0, "no captions at frame {}", still.frame);
    }

    // The cover is taken after the hook, so the hook is drawn on it.
    let (cover, frame) = report.cover.clone().unwrap();
    assert_eq!(frame, 105);
    let cover_hook = difference(
        &small_rgb(&cover, None, Some(&hook_crop)),
        &small_rgb(&output, Some(frame), Some(&hook_crop)),
    );
    assert!(cover_hook > 10.0, "no hook on the cover ({cover_hook})");
}

#[test]
fn rotated_portrait_source_comes_out_upright() {
    let _lock = one_at_a_time();
    let dir = scratch("portrait");
    let source = rotated_portrait();
    let (plan, _) = plan(
        &dir,
        &source,
        0.2,
        "[[cut]]\nsrc = \"a\"\nin = 0.5\nout = 2.0\n[[cut]]\nsrc = \"a\"\nin = 3.0\nout = 5.5\n",
    );
    let project = Project::open(tools(), &plan, &dir).unwrap();
    let probed = &project.sources()[0];
    assert_eq!((probed.info().width, probed.info().height), (1920, 1080));
    assert_eq!(probed.info().quarter_turns % 2, 1);
    assert_eq!(probed.info().display_size(), (1080, 1920));
    // Already 9:16 once upright, so crop_x does not matter and nothing is cut off.
    assert_eq!(
        (
            probed.crop().w,
            probed.crop().h,
            probed.crop().x,
            probed.crop().y
        ),
        (1080, 1920, 0, 0)
    );

    let (project, _) = render_plan(&dir, &plan);
    let output = dir.join("out.mp4");
    assert_output_properties(&output, project.timeline());
    assert_numbers(
        &frame_numbers(&output),
        &expected_numbers(project.timeline(), |slot| cfr_number(slot, 30)),
    );
    // The white bar is across the top of the picture, not down a side.
    let white_share = |crop: &str, size: &str| {
        let rgb = ffmpeg_stdout(&[
            "-i",
            output.to_str().unwrap(),
            "-frames:v",
            "1",
            "-vf",
            &format!("crop={crop},scale={size}:flags=area,format=rgb24"),
            "-f",
            "rawvideo",
            "-",
        ]);
        let white = rgb
            .chunks(3)
            .filter(|px| px.iter().all(|&c| c > 220))
            .count();
        white as f64 / (rgb.len() / 3) as f64
    };
    assert!(
        white_share("1080:100:0:0", "54:5") > 0.95,
        "the top is not the white bar"
    );
    assert!(
        white_share("100:1920:0:0", "5:96") < 0.2,
        "the white bar runs down the left side"
    );
}

#[test]
fn variable_frame_rate_source_lands_on_the_frame_grid() {
    let _lock = one_at_a_time();
    let dir = scratch("vfr");
    let source = variable_frame_rate();
    // The first cut starts before the first video frame (the file starts with audio).
    let (plan, _) = plan(
        &dir,
        &source,
        0.5,
        "[[cut]]\nsrc = \"a\"\nin = 0.0\nout = 1.0\n\
         [[cut]]\nsrc = \"a\"\nin = 4.0\nout = 6.5\n\
         [[cut]]\nsrc = \"a\"\nin = 8.2\nout = 9.9\n",
    );
    let project = Project::open(tools(), &plan, &dir).unwrap();
    assert!(project.sources()[0].info().is_variable_frame_rate());
    assert!(project.sources()[0].info().video_start > 0.0);

    // Where ffmpeg puts each source frame: its timestamp, moved so the file starts at
    // 0 (-start_at_zero shifts by the file's start time, rounded to the stream's time
    // base), on the 30 fps grid with halves rounded away from zero (the fps filter).
    let probe = ffprobe_json(
        &source,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=start_time:packet=pts",
        ],
    );
    let start = probe["format"]["start_time"].as_str().unwrap();
    let (whole, fraction) = start.split_once('.').unwrap();
    let start_us: i128 = whole.parse::<i128>().unwrap() * 1_000_000
        + format!("{fraction:0<6}")[..6].parse::<i128>().unwrap();
    let shift = (2 * start_us * 90_000 + 1_000_000) / 2_000_000;
    let slots: Vec<i128> = probe["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let pts = i128::from(p["pts"].as_i64().unwrap()) - shift;
            (2 * pts * 30 + 90_000).div_euclid(180_000)
        })
        .collect();
    let numbers = frame_numbers(&source);
    assert_eq!(numbers.len(), slots.len());
    let number_at = |slot: u64| {
        // The last frame at or before the slot; slots before the first frame repeat it.
        let index = slots
            .iter()
            .rposition(|s| *s <= i128::from(slot))
            .unwrap_or(0);
        numbers[index]
    };

    let (project, _) = render_plan(&dir, &plan);
    let output = dir.join("out.mp4");
    assert_output_properties(&output, project.timeline());
    assert_numbers(
        &frame_numbers(&output),
        &expected_numbers(project.timeline(), number_at),
    );
}

#[test]
fn hdr_hevc_source_is_tone_mapped_to_sdr() {
    let _lock = one_at_a_time();
    let dir = scratch("hdr");
    let source = hdr_hlg();
    let (plan, _) = plan(
        &dir,
        &source,
        0.5,
        "[[cut]]\nsrc = \"a\"\nin = 0.5\nout = 1.5\nzoom = [{ at = 1.0, scale = 1.2 }]\n[[cut]]\nsrc = \"a\"\nin = 2.0\nout = 3.5\n",
    );
    let project = Project::open(tools(), &plan, &dir).unwrap();
    let info = project.sources()[0].info();
    assert_eq!(info.codec, "hevc");
    assert_eq!(info.pix_fmt, "yuv420p10le");
    assert_eq!(info.hdr(), Some(HdrTransfer::Hlg));
    assert!(project.tone_maps());

    let (project, _) = render_plan(&dir, &plan);
    let output = dir.join("out.mp4");
    assert_output_properties(&output, project.timeline());
    assert_eq!(
        frame_numbers(&output).len() as u64,
        project.timeline().total_frames()
    );
    // Tone-mapped to a normal-looking SDR picture: neither black nor washed out flat.
    let rgb = ffmpeg_stdout(&[
        "-i",
        output.to_str().unwrap(),
        "-frames:v",
        "1",
        "-vf",
        "scale=108:192:flags=area,format=gray",
        "-f",
        "rawvideo",
        "-",
    ]);
    let mean = rgb.iter().map(|&v| f64::from(v)).sum::<f64>() / rgb.len() as f64;
    let spread = rgb.iter().max().unwrap() - rgb.iter().min().unwrap();
    assert!((30.0..=220.0).contains(&mean), "mean brightness {mean}");
    assert!(spread > 100, "contrast {spread}");
}

#[test]
fn a_cut_past_the_end_of_its_source_is_rejected() {
    let _lock = one_at_a_time();
    let dir = scratch("past-end");
    let (plan, _) = plan(
        &dir,
        &landscape(),
        0.5,
        "[[cut]]\nsrc = \"a\"\nin = 10.0\nout = 13.0\n",
    );
    let error = Project::open(tools(), &plan, &dir).unwrap_err();
    assert!(
        matches!(
            error,
            OpenError::Render(RenderError::CutPastEnd { cut: 0, .. })
        ),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("cut[0] ends at 13.000 s, but source `a` ends at 12.000 s"),
        "{error}"
    );
}

/// A stand-in ffmpeg without zscale, and an ffprobe that describes an HLG clip.
#[cfg(unix)]
#[test]
fn hdr_without_zscale_fails_with_a_clear_message() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("no-zscale");
    let filters: String = [
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
        "aresample",
        "aformat",
        "atrim",
        "asetpts",
        "apad",
        "afade",
        "anullsrc",
        "anull",
        "afftdn",
        "highpass",
        "acompressor",
        "loudnorm",
        "pad",
        "xstack",
        "tonemap",
    ]
    .iter()
    .map(|f| format!(" ... {f:<16} V->V       x\\n"))
    .collect();
    let ffmpeg = dir.join("ffmpeg");
    std::fs::write(
        &ffmpeg,
        format!(
            "#!/bin/sh\ncase \"$2\" in\n-version) echo 'ffmpeg version 6.1.1-fake Copyright';;\n-filters) printf '{filters}';;\n-encoders) printf ' V....D libx264  x\\n A....D aac  x\\n V....D png  x\\n V....D mjpeg  x\\n';;\nesac\n"
        ),
    )
    .unwrap();
    let ffprobe = dir.join("ffprobe");
    std::fs::write(
        &ffprobe,
        "#!/bin/sh\necho '{\"streams\": [{\"codec_type\": \"video\", \"codec_name\": \"hevc\", \"width\": 1920, \"height\": 1080, \"color_transfer\": \"arib-std-b67\", \"duration\": \"60.0\"}], \"format\": {\"duration\": \"60.0\"}}'\n",
    )
    .unwrap();
    for script in [&ffmpeg, &ffprobe] {
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let tools = Ffmpeg::new(ffmpeg, ffprobe).unwrap();
    let (plan, _) = plan(
        &dir,
        Path::new("clip.mov"),
        0.5,
        "[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 2.0\n",
    );
    let project = Project::open(&tools, &plan, &dir).unwrap();
    let options = RenderOptions {
        output: dir.join("out.mp4"),
        cover: None,
        cover_at: None,
        preset: Preset::Veryfast,
        overlays: Vec::new(),
        subtitles: None,
    };
    let error = render(&tools, &project, &options, &mut |_| {}).unwrap_err();
    assert!(
        matches!(&error, RenderError::Tool(ToolError::MissingFilters { missing, .. }) if missing == &["zscale"]),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "ffmpeg 6.1.1-fake lacks the zscale filter needed to tone-map HDR (PQ or HLG) video to SDR; install an ffmpeg build that has it"
    );
    let still = preview(
        &tools,
        &project,
        &PreviewOptions {
            times: vec![0.5],
            out_dir: dir.join("stills"),
            safe_zones: false,
        },
    );
    assert!(matches!(
        still,
        Err(RenderError::Tool(ToolError::MissingFilters { .. }))
    ));
}
