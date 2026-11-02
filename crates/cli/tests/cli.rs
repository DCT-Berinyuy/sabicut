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
