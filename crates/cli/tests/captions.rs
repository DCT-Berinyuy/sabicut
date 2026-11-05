//! `sabicut render` and `sabicut srt` on a plan with captions and a hook: the subtitles,
//! with fixes applied and removed words gone, and what the render reports.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// ffmpeg jobs are heavy, so the tests that render run one at a time.
static FFMPEG_JOBS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sabicut(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sabicut"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// Spoken from 0.6 s to 3.3 s of the clip. "removed" falls between the cuts, and the
/// fixes turn "rust" into "Rust".
const WORDS: &str = r#"{
  "schema": 1,
  "media": "clip.mp4",
  "language": "en",
  "model": "ggml-small-q5_1.bin",
  "duration": 4.0,
  "speech": [
    {"start":0.5,"end":3.5}
  ],
  "words": [
    {"text":"rust","start":0.6,"end":0.9,"probability":0.9},
    {"text":"and","start":0.9,"end":1.1,"probability":0.9},
    {"text":"Supabase.","start":1.1,"end":1.4,"probability":0.9},
    {"text":"removed","start":1.6,"end":2.4,"probability":0.9},
    {"text":"Ship","start":2.6,"end":2.9,"probability":0.9},
    {"text":"it!","start":2.9,"end":3.3,"probability":0.9}
  ]
}
"#;

const FIXES: &str = "schema = 1\n[[fix]]\nfrom = \"rust\"\nto = \"Rust\"\n";

/// A fresh directory with the words, fixes and a plan that cuts 0.5-1.5 s and 2.5-3.5 s
/// of `clip.mp4` (made only if `with_clip`). Returns the plan's path.
fn captioned_plan(name: &str, with_clip: bool, words_file: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    if with_clip {
        let ffmpeg = std::env::var("SABICUT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
        let status = Command::new(ffmpeg)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=4"])
            .args(["-f", "lavfi", "-i", "sine=f=440:r=48000:d=4,volume=0.2"])
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac"])
            .arg(dir.join("clip.mp4"))
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg could not make the test clip");
    }
    fs::write(dir.join("talk.words.json"), WORDS).unwrap();
    fs::write(dir.join("fixes.toml"), FIXES).unwrap();
    // No font, colour or outline: the defaults (the bundled Anton, gold, 10 px) apply.
    let plan = dir.join("plan.toml");
    fs::write(
        &plan,
        format!(
            "schema = 1\n[meta]\nfps = 30\nout = {{ w = 1080, h = 1920 }}\n\
             [[source]]\nid = \"a\"\npath = \"clip.mp4\"\ncrop_x = 0.5\n\
             [[cut]]\nsrc = \"a\"\nin = 0.5\nout = 1.5\n\
             [[cut]]\nsrc = \"a\"\nin = 2.5\nout = 3.5\n\
             [hook]\ntext = \"Ship it tonight\"\ndur = 1.0\n\
             [captions]\nwords = {words_file:?}\nfixes = \"fixes.toml\"\nkeywords = [\"Supabase\"]\n"
        ),
    )
    .unwrap();
    plan
}

const SUBTITLES: &str = "1\n00:00:00,100 --> 00:00:00,900\nRust and Supabase.\n\n\
                         2\n00:00:01,100 --> 00:00:01,900\nShip it!\n\n";

#[test]
fn render_writes_subtitles_next_to_the_video() {
    let _lock = FFMPEG_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let plan = captioned_plan("cli-captions", true, "talk.words.json");
    let output = sabicut(&["render", plan.to_str().unwrap(), "--preset", "ultrafast"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    let srt = plan.with_extension("srt");
    assert_eq!(
        lines[2],
        format!("ok: wrote {} (subtitles: 2 cues)", srt.display())
    );
    assert_eq!(fs::read_to_string(&srt).unwrap(), SUBTITLES);
    let log = stderr(&output);
    assert!(
        log.contains("\ncaptions: 2 pages in a 756x240 band at x 162, y 1178, in Anton Regular\n"),
        "{log}"
    );
    assert!(log.contains("\nhook: frames 0..30 in a 756x"), "{log}");
}

#[test]
fn srt_writes_the_subtitles_without_rendering() {
    let plan = captioned_plan("cli-srt", false, "talk.words.json");
    let output = sabicut(&["srt", plan.to_str().unwrap(), "-o", "-"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), SUBTITLES);

    let output = sabicut(&["srt", plan.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let srt = plan.with_extension("srt");
    assert_eq!(
        stdout(&output),
        format!("ok: wrote {} (2 cues, 5 words)\n", srt.display())
    );
    assert_eq!(fs::read_to_string(&srt).unwrap(), SUBTITLES);
}

#[test]
fn a_missing_words_file_is_named() {
    let plan = captioned_plan("cli-srt-missing", false, "missing.words.json");
    for command in ["srt", "render"] {
        let output = sabicut(&[command, plan.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        let message = stderr(&output);
        let missing = plan.with_file_name("missing.words.json");
        assert!(
            message.starts_with(&format!("error: {}: ", missing.display())),
            "{command}: {message}"
        );
    }
}
