//! End-to-end tests of `sabicut plan`. `plan` reads where each clip's video ends with
//! ffprobe, so these tests make small clips with ffmpeg (set `SABICUT_FFMPEG` and
//! `SABICUT_FFPROBE` to use other builds).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sabicut_core::{Plan, SpeechSegment, Timeline, Transcript, Word};

fn sabicut_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sabicut"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// A fresh scratch folder for one test.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn ffmpeg(args: &[&str]) {
    let program = std::env::var("SABICUT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
    let output = Command::new(program)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ffmpeg {args:?} failed:\n{}",
        stderr(&output)
    );
}

/// Makes the clip `dir/name`, a small silent video, and writes its `<stem>.words.json`
/// with these phrases (see [`words`]). The video lasts a second longer than the audio,
/// so its end limits no cut.
fn clip(dir: &Path, name: &str, phrases: &[(f64, &str)]) {
    let duration = words(dir, name, phrases);
    let video = format!("testsrc2=s=64x36:r=30:d={}", duration + 1.0);
    let path = dir.join(name);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &video,
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        path.to_str().unwrap(),
    ]);
}

/// Writes `<stem>.words.json` for the clip `dir/name` with these phrases: `(start,
/// words)`, each word 0.3 s long, one speech segment each, and the audio lasting 1 s
/// past the last word. Returns the audio's length.
fn words(dir: &Path, name: &str, phrases: &[(f64, &str)]) -> f64 {
    let mut speech = Vec::new();
    let mut words = Vec::new();
    let mut end = 0.0;
    for &(start, text) in phrases {
        let mut t = start;
        for word in text.split(' ') {
            words.push(Word::new(word, t, t + 0.3, 0.9));
            t += 0.3;
        }
        speech.push(SpeechSegment::new(start - 0.03, t));
        end = t;
    }
    let duration = end + 1.0;
    let transcript = Transcript::new(name, "en", "test", duration, speech, words).unwrap();
    let words_file = dir.join(Path::new(name).with_extension("words.json"));
    fs::write(words_file, transcript.to_json().unwrap()).unwrap();
    duration
}

fn read_plan(path: &Path) -> Plan {
    let plan = Plan::from_toml_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(Timeline::from_plan(&plan).is_ok());
    plan
}

#[test]
fn plan_writes_a_valid_plan_with_paths_relative_to_it() {
    let dir = scratch_dir("plan-single");
    fs::create_dir_all(dir.join("raw")).unwrap();
    clip(
        &dir.join("raw"),
        "IMG_0412.mov",
        &[
            (1.0, "Stop leaking your keys."),
            (4.0, "So today we..."),
            (5.5, "So today we learn Rust."),
            (9.0, "Bye."),
        ],
    );
    let output = sabicut_in(
        &dir,
        &[
            "plan",
            "raw/IMG_0412.mov",
            "--keywords",
            "Rust",
            "--hook",
            "Stop leaking keys",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let report = stdout(&output);
    assert!(report.starts_with("wrote plan.toml: 3 cuts, "), "{report}");
    assert!(report.contains("false starts cut"), "{report}");
    assert!(report.contains("\"So today we...\""), "{report}");
    assert!(
        report.contains("cut points inside a word: 0 of 6"),
        "{report}"
    );

    let plan = read_plan(&dir.join("plan.toml"));
    assert_eq!(plan.sources()[0].path(), "raw/IMG_0412.mov");
    assert_eq!(plan.sources()[0].crop_x(), 0.5);
    assert_eq!(
        plan.captions().unwrap().words(),
        Some("raw/IMG_0412.words.json")
    );
    assert_eq!(plan.captions().unwrap().keywords(), ["Rust"]);
    assert_eq!(plan.hook().unwrap().text(), "Stop leaking keys");
    assert_eq!(plan.cuts()[1].zooms().len(), 1);

    let check = sabicut_in(&dir, &["validate", "plan.toml"]);
    assert!(check.status.success(), "{}", stderr(&check));
}

#[test]
fn plan_does_not_replace_a_plan_unless_forced() {
    let dir = scratch_dir("plan-force");
    clip(&dir, "clip.mp4", &[(1.0, "Hello there.")]);
    fs::write(dir.join("plan.toml"), "# my edits\n").unwrap();

    let output = sabicut_in(&dir, &["plan", "clip.mp4"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("plan.toml already exists"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fs::read_to_string(dir.join("plan.toml")).unwrap(),
        "# my edits\n"
    );

    let forced = sabicut_in(&dir, &["plan", "clip.mp4", "--force"]);
    assert!(forced.status.success(), "{}", stderr(&forced));
    read_plan(&dir.join("plan.toml"));
}

#[test]
fn plan_says_how_to_get_missing_word_timings() {
    let dir = scratch_dir("plan-no-words");
    fs::write(dir.join("other.mov"), b"").unwrap();
    let output = sabicut_in(&dir, &["plan", "other.mov"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("Run `sabicut analyze other.mov` first"),
        "{}",
        stderr(&output)
    );
    assert!(!dir.join("plan.toml").exists());

    let missing = sabicut_in(&dir, &["plan", "nowhere.mov"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("nowhere.mov"));
}

#[test]
fn plan_names_a_clip_it_cannot_read() {
    let dir = scratch_dir("plan-unreadable");
    words(&dir, "clip.mp4", &[(1.0, "Hello there.")]);
    fs::write(dir.join("clip.mp4"), b"not a video").unwrap();
    let output = sabicut_in(&dir, &["plan", "clip.mp4"]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.starts_with("error: clip.mp4: cannot read where its video ends: "),
        "{message}"
    );
    assert!(!dir.join("plan.toml").exists());
}

/// The plan benchmark plans its WAV readings (docs/bench/plan.md). A file without video
/// has no frames to run past, so its cuts run to the end of its audio.
#[test]
fn an_audio_file_is_planned_to_the_end_of_its_audio() {
    let dir = scratch_dir("plan-audio-only");
    let duration = words(&dir, "talk.wav", &[(1.0, "Hello there.")]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("sine=f=440:r=16000:d={duration}"),
        dir.join("talk.wav").to_str().unwrap(),
    ]);
    let output = sabicut_in(&dir, &["plan", "talk.wav", "-o", "-"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let plan = Plan::from_toml_str(&stdout(&output)).unwrap();
    let cuts: Vec<(f64, f64)> = plan
        .cuts()
        .iter()
        .map(|cut| (cut.in_point(), cut.out_point()))
        .collect();
    assert_eq!(cuts, [(0.833, 1.7)]);
}

/// Speech from 0.6 s to 1.4 s and from 2.6 s to the end of the 4 s audio.
const OUTLASTING_WORDS: &str = r#"{
  "schema": 1,
  "media": "clip.mp4",
  "language": "en",
  "model": "test",
  "duration": 4.0,
  "speech": [
    {"start":0.5,"end":1.45},
    {"start":2.5,"end":3.98}
  ],
  "words": [
    {"text":"Rust","start":0.6,"end":0.9,"probability":0.9},
    {"text":"and","start":0.9,"end":1.1,"probability":0.9},
    {"text":"Supabase.","start":1.1,"end":1.4,"probability":0.9},
    {"text":"Ship","start":2.6,"end":2.9,"probability":0.9},
    {"text":"it","start":2.9,"end":3.1,"probability":0.9},
    {"text":"tonight.","start":3.1,"end":3.95,"probability":0.9}
  ]
}
"#;

/// Phones often stop the video a little before the audio. Here the speech runs on to
/// the end of the audio, 0.3 s after the video ends: the last cut stops at the end of
/// the video, so the plan renders as it is.
#[test]
fn a_clip_whose_audio_outlasts_its_video_plans_and_renders() {
    let dir = scratch_dir("plan-audio-outlasts-video");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=640x360:r=30:d=3.7",
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:r=48000:d=4,volume=0.2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        dir.join("clip.mp4").to_str().unwrap(),
    ]);
    fs::write(dir.join("clip.words.json"), OUTLASTING_WORDS).unwrap();

    let planned = sabicut_in(&dir, &["plan", "clip.mp4", "--keywords", "Supabase"]);
    assert!(planned.status.success(), "{}", stderr(&planned));
    let report = stdout(&planned);
    assert!(report.starts_with("wrote plan.toml: 2 cuts, "), "{report}");
    assert!(
        report.contains(
            "warning: source a: the video ends at 3.700 s, before the speech does (3.950 s)"
        ),
        "{report}"
    );
    assert!(
        report.contains("cut points inside a word: 0 of 4"),
        "{report}"
    );
    let plan = read_plan(&dir.join("plan.toml"));
    let ends: Vec<f64> = plan.cuts().iter().map(|cut| cut.out_point()).collect();
    assert_eq!(ends, [1.567, 3.7]);

    let rendered = sabicut_in(
        &dir,
        &["render", "plan.toml", "--preset", "ultrafast", "--no-cover"],
    );
    assert!(rendered.status.success(), "{}", stderr(&rendered));
    let out = stdout(&rendered);
    assert!(
        out.starts_with("ok: wrote plan.mp4 (76 frames = 2.533 s at 30 fps, 1080x1920, "),
        "{out}"
    );
}

#[test]
fn several_clips_name_their_own_words() {
    let dir = scratch_dir("plan-several");
    clip(
        &dir,
        "a.mp4",
        &[(1.0, "First clip here."), (4.0, "More of it.")],
    );
    clip(&dir, "b.mp4", &[(0.5, "Second clip.")]);
    fs::create_dir_all(dir.join("edit")).unwrap();
    let output = sabicut_in(
        &dir,
        &[
            "plan",
            "a.mp4",
            "b.mp4",
            "-o",
            "edit/plan.toml",
            "--crop-x",
            "0.4",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let plan = read_plan(&dir.join("edit/plan.toml"));
    let srcs: Vec<&str> = plan.cuts().iter().map(|cut| cut.src()).collect();
    assert_eq!(srcs, ["a", "a", "b"]);
    assert_eq!(plan.sources()[1].path(), "../b.mp4");
    assert_eq!(plan.sources()[1].crop_x(), 0.4);
    assert_eq!(plan.captions().unwrap().words(), None);
    assert_eq!(plan.caption_words("a"), Some("../a.words.json"));
    assert_eq!(plan.caption_words("b"), Some("../b.words.json"));

    let counts = sabicut_in(&dir, &["plan", "a.mp4", "b.mp4", "--words", "a.words.json"]);
    assert_eq!(counts.status.code(), Some(1));
    assert!(
        stderr(&counts).contains("--words: 1 given for 2 media files; give it once per media file"),
        "{}",
        stderr(&counts)
    );
}

#[test]
fn plan_can_print_to_standard_output_with_fixes_applied() {
    let dir = scratch_dir("plan-stdout");
    clip(&dir, "clip.mp4", &[(1.0, "We use Supa base daily.")]);
    fs::write(
        dir.join("fixes.toml"),
        "schema = 1\n[[fix]]\nfrom = \"Supa base\"\nto = \"Supabase\"\n",
    )
    .unwrap();
    let output = sabicut_in(
        &dir,
        &[
            "plan",
            "clip.mp4",
            "-o",
            "-",
            "--fixes",
            "fixes.toml",
            "--keywords",
            "Supabase",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).starts_with("wrote standard output: 1 cut"));
    let plan = Plan::from_toml_str(&stdout(&output)).unwrap();
    assert_eq!(plan.captions().unwrap().fixes(), Some("fixes.toml"));
    // The fixed word "Supabase" starts at 1.6 s and gets the zoom.
    let zooms = plan.cuts()[0].zooms();
    assert_eq!((zooms.len(), zooms[0].at()), (1, 1.6));
    assert!(stdout(&output).contains("[[cut]]  # \"We use Supabase daily.\""));
    assert!(!dir.join("plan.toml").exists());
}
