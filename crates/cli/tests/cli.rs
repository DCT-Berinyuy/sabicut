//! End-to-end tests of the `sabicut` binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `sabicut` from the workspace root, so relative paths in the output are stable.
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

/// Writes `text` to a scratch file for this test and returns its path.
fn scratch_file(name: &str, text: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn validate_accepts_the_example_plan() {
    let output = sabicut(&["validate", "examples/plan.toml"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "ok: examples/plan.toml is a valid plan (3 cuts, 412 frames = 13.733 s at 30 fps)\n"
    );
}

#[test]
fn validate_lists_every_problem_and_fails() {
    let plan = scratch_file(
        "invalid-plan.toml",
        r#"
        schema = 1
        [meta]
        fps = 30
        out = { w = 1080, h = 1920 }
        [[source]]
        id = "a"
        path = "a.mov"
        crop_x = 0.5
        [[cut]]
        src = "a"
        in = 4.0
        out = 3.0
        [[cut]]
        src = "b"
        in = 1.0
        out = 2.0
        "#,
    );
    let output = sabicut(&["validate", plan.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!(
            "error: {}: 2 problems\n  - cut[0].out must be > in\n  - cut[1].src must be the id of a [[source]], got \"b\"\n",
            plan.display()
        )
    );
}

#[test]
fn validate_reports_toml_errors_with_their_location() {
    let plan = scratch_file("typo-plan.toml", "schema = 1\n[meta]\nfsp = 30\n");
    let output = sabicut(&["validate", plan.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.starts_with(&format!("error: {}: ", plan.display())),
        "{message}"
    );
    assert!(message.contains("line 3"), "{message}");
    assert!(message.contains("unknown field `fsp`"), "{message}");
}

#[test]
fn validate_reports_a_missing_file() {
    let output = sabicut(&["validate", "examples/no-such-plan.toml"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("error: examples/no-such-plan.toml: "),
        "{}",
        stderr(&output)
    );
}

#[test]
fn timeline_prints_the_derived_timeline() {
    let output = sabicut(&["timeline", "examples/plan.toml"]);
    assert!(output.status.success(), "{}", stderr(&output));
    // Worked out by hand: at 30 fps, 3.120..9.880 snaps to source frames 94..296
    // (202 frames), the zoom at 5.4 s is source frame 162, so output frame 68; and so on.
    let expected = "\
examples/plan.toml: 3 cuts, 412 frames (13.733 s) at 30 fps

cut  source        source time (s)        output time (s)  frames  output frames
  0  a           3.133 -     9.867      0.000 -     6.733     202  0..202
     zoom x1.25 at source 5.400 s -> output 2.267 s (frame 68)
  1  a          12.000 -    15.500      6.733 -    10.233     105  202..307
     zoom x1.15 at source 13.000 s -> output 7.733 s (frame 232)
     zoom x1.00 at source 14.200 s -> output 8.933 s (frame 268)
  2  b           0.500 -     4.000     10.233 -    13.733     105  307..412

hook: output 0.000 - 2.000 s (frames 0..60)
";
    assert_eq!(stdout(&output), expected);
}

#[test]
fn schema_prints_the_committed_json_schema() {
    let output = sabicut(&["schema"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let committed = fs::read_to_string(workspace_root().join("schema/v1.json")).unwrap();
    assert_eq!(stdout(&output), committed.replace("\r\n", "\n"));
}

#[test]
fn unknown_subcommands_are_usage_errors() {
    let output = sabicut(&["no-such-command", "examples/plan.toml"]);
    assert_eq!(output.status.code(), Some(2));
}

/// ffmpeg jobs are heavy, so the tests that render run one at a time.
static FFMPEG_JOBS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ffmpeg_binary() -> String {
    std::env::var("SABICUT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned())
}

/// A fresh directory holding a 4 s 640×360 clip with a tone, and a plan that cuts two
/// seconds from it with a relative path. Returns the plan's path.
fn clip_and_plan(name: &str, extra: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let status = Command::new(ffmpeg_binary())
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=4"])
        .args(["-f", "lavfi", "-i", "sine=f=440:r=48000:d=4,volume=0.2"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac"])
        .arg(dir.join("clip.mp4"))
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg could not make the test clip");
    let plan = dir.join("plan.toml");
    fs::write(
        &plan,
        format!(
            "schema = 1\n[meta]\nfps = 30\nout = {{ w = 1080, h = 1920 }}\n[[source]]\nid = \"a\"\npath = \"clip.mp4\"\ncrop_x = 0.5\n[[cut]]\nsrc = \"a\"\nin = 0.5\nout = 1.5\n[[cut]]\nsrc = \"a\"\nin = 2.5\nout = 3.5\nzoom = [{{ at = 3.0, scale = 1.3 }}]\n{extra}"
        ),
    )
    .unwrap();
    plan
}

#[test]
fn render_writes_the_video_and_cover() {
    let _lock = FFMPEG_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let plan = clip_and_plan("cli-render", "[hook]\ntext = \"Hi\"\ndur = 1.0\n");
    let output = sabicut(&["render", plan.to_str().unwrap(), "--preset", "ultrafast"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let video = plan.with_extension("mp4");
    let cover = plan.with_extension("jpg");
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with(&format!(
            "ok: wrote {} (60 frames = 2.000 s at 30 fps, 1080x1920, -1",
            video.display()
        )),
        "{out}"
    );
    assert!(lines[0].ends_with(" LUFS)"), "{out}");
    assert_eq!(
        lines[1],
        format!("ok: wrote {} (cover: frame 30, 1.000 s)", cover.display())
    );
    assert!(video.exists() && cover.exists());
    // A plan without word timings gets no subtitles.
    assert!(!plan.with_extension("srt").exists());
    let log = stderr(&output);
    assert!(log.starts_with("source a: "), "{log}");
    assert!(log.contains("\nhook: frames 0..30 in a 756x"), "{log}");
}

#[test]
fn preview_writes_stills_and_a_contact_sheet() {
    let _lock = FFMPEG_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let plan = clip_and_plan("cli-preview", "");
    let output = sabicut(&[
        "preview",
        plan.to_str().unwrap(),
        "--at",
        "0.5,1.25",
        "--safe-zones",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let dir = plan.parent().unwrap().join("plan-preview");
    let expected = format!(
        "ok: wrote {} (frame 15 at 0.500 s)\nok: wrote {} (frame 37 at 1.233 s)\nok: wrote {} (2 stills)\n",
        dir.join("still-01-0.500s.png").display(),
        dir.join("still-02-1.250s.png").display(),
        dir.join("contact-sheet.png").display(),
    );
    assert_eq!(stdout(&output), expected);
    for name in [
        "still-01-0.500s.png",
        "still-02-1.250s.png",
        "contact-sheet.png",
    ] {
        assert!(dir.join(name).exists(), "{name}");
    }
}

#[test]
fn render_reports_a_missing_source() {
    let plan = scratch_file(
        "missing-source.toml",
        "schema = 1\n[meta]\nfps = 30\nout = { w = 1080, h = 1920 }\n[[source]]\nid = \"a\"\npath = \"no-such-clip.mp4\"\ncrop_x = 0.5\n[[cut]]\nsrc = \"a\"\nin = 0.0\nout = 1.0\n",
    );
    let output = sabicut(&["render", plan.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(message.starts_with("error: source `a` ("), "{message}");
    assert!(message.contains("no-such-clip.mp4"), "{message}");
}

#[test]
fn preview_rejects_a_time_past_the_end() {
    let _lock = FFMPEG_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let plan = clip_and_plan("cli-preview-range", "");
    let output = sabicut(&["preview", plan.to_str().unwrap(), "--at", "1,9"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: --at 9 s is not within the output (0 to 2.000 s)\n"
    );
}

#[test]
fn render_refuses_to_overwrite_a_source() {
    let _lock = FFMPEG_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let plan = clip_and_plan("cli-overwrite", "");
    let clip = plan.with_file_name("clip.mp4");
    let before = fs::metadata(&clip).unwrap().len();
    let output = sabicut(&[
        "render",
        plan.to_str().unwrap(),
        "-o",
        clip.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.ends_with(&format!(
            "error: {} is source `a`; writing there would destroy it, so choose another output path\n",
            clip.display()
        )),
        "{message}"
    );
    assert_eq!(fs::metadata(&clip).unwrap().len(), before);
}
