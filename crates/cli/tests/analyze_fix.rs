//! End-to-end tests of `sabicut analyze` (the parts that need no speech model),
//! `sabicut fix`, and `sabicut schema words|fixes`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sabicut_core::Transcript;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs `sabicut` from the workspace root, so relative paths in the output are stable.
fn sabicut(args: &[&str]) -> Output {
    sabicut_with_env(args, &[])
}

fn sabicut_with_env(args: &[&str], env: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sabicut"));
    command.args(args).current_dir(workspace_root());
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
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

const EXAMPLE_CHANGES: &str = "\
examples/words.json: 3 changes from examples/fixes.toml
      4.020 -     4.620  \"Supa base\" -> \"Supabase\"  (fix[0])
      4.760 -     5.100  \"rust\" -> \"Rust\"  (fix[1])
      6.600 -     7.050  \"get hub.\" -> \"GitHub.\"  (fix[2])
";

#[test]
fn fix_lists_the_changes() {
    let output = sabicut(&["fix", "examples/words.json", "examples/fixes.toml"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), EXAMPLE_CHANGES);
}

#[test]
fn fix_writes_the_fixed_words_and_keeps_the_input() {
    let dir = scratch_dir("fix-out");
    let out = dir.join("fixed.json");
    let before = fs::read_to_string(workspace_root().join("examples/words.json")).unwrap();
    let output = sabicut(&[
        "fix",
        "examples/words.json",
        "examples/fixes.toml",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("{EXAMPLE_CHANGES}wrote {} (22 words)\n", out.display())
    );
    let fixed = Transcript::from_json_str(&fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(fixed.validate(), Ok(()));
    assert_eq!(fixed.words()[10].text(), "Supabase");
    let after = fs::read_to_string(workspace_root().join("examples/words.json")).unwrap();
    assert_eq!(after, before);

    // Fixing the fixed words again changes nothing.
    let again = sabicut(&["fix", out.to_str().unwrap(), "examples/fixes.toml"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert!(stdout(&again).contains(": no changes from examples/fixes.toml\n"));
    assert!(stdout(&again).contains("note: fix[0] changed nothing"));
}

#[test]
fn fix_can_write_json_to_standard_output() {
    let output = sabicut(&[
        "fix",
        "examples/words.json",
        "examples/fixes.toml",
        "-o",
        "-",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let fixed = Transcript::from_json_str(&stdout(&output)).unwrap();
    assert_eq!(fixed.words().len(), 22);
    assert_eq!(stderr(&output), EXAMPLE_CHANGES);
}

#[test]
fn fix_refuses_to_overwrite_the_raw_words() {
    let output = sabicut(&[
        "fix",
        "examples/words.json",
        "examples/fixes.toml",
        "--out",
        "examples/words.json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("error: examples/words.json is the input words.json"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn fix_reports_every_problem_in_the_rules() {
    let dir = scratch_dir("fix-bad-rules");
    let fixes = dir.join("fixes.toml");
    fs::write(
        &fixes,
        "schema = 1\n[[fix]]\nfrom = \"!\"\nto = \"x\"\n[[fix]]\nfrom = \"a\"\nto = \"\"\n",
    )
    .unwrap();
    let output = sabicut(&["fix", "examples/words.json", fixes.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "error: {}: 2 problems\n  - fix[0].from must contain a word (letters or digits), \
             got \"!\"\n  - fix[1].to must not be empty\n",
            fixes.display()
        )
    );

    fs::write(&fixes, "schema = 1\n[[fix]]\nform = \"a\"\nto = \"b\"\n").unwrap();
    let output = sabicut(&["fix", "examples/words.json", fixes.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("unknown field `form`"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn fix_reports_invalid_words() {
    let dir = scratch_dir("fix-bad-words");
    let words = dir.join("words.json");
    let text = fs::read_to_string(workspace_root().join("examples/words.json"))
        .unwrap()
        .replace("\"start\":0.8,", "\"start\":0.7,");
    fs::write(&words, text).unwrap();
    let output = sabicut(&["fix", words.to_str().unwrap(), "examples/fixes.toml"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "error: {}: 1 problem\n  - words[1].start must be >= words[0].end (0.8), got 0.7\n",
            words.display()
        )
    );
}

#[test]
fn schema_prints_the_words_and_fixes_schemas() {
    for (format, file) in [
        ("words", "schema/words-v1.json"),
        ("fixes", "schema/fixes-v1.json"),
    ] {
        let output = sabicut(&["schema", format]);
        assert!(output.status.success(), "{}", stderr(&output));
        let committed = fs::read_to_string(workspace_root().join(file)).unwrap();
        assert_eq!(stdout(&output), committed.replace("\r\n", "\n"), "{file}");
    }
    let output = sabicut(&["schema", "plan"]);
    let committed = fs::read_to_string(workspace_root().join("schema/v1.json")).unwrap();
    assert_eq!(stdout(&output), committed.replace("\r\n", "\n"));
}

#[test]
fn analyze_names_every_path_where_the_model_was_looked_for() {
    let dir = scratch_dir("analyze-no-models");
    let models = dir.join("models");
    let home = dir.join("home");
    let output = sabicut_with_env(
        &["analyze", "examples/plan.toml", "--model", "base"],
        &[
            ("SABICUT_MODELS", &models),
            ("HOME", &home),
            ("XDG_DATA_HOME", Path::new("")),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.starts_with("error: model ggml-base-q5_1.bin not found. Sabicut looked for:\n"),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "  {}\n",
            models.join("ggml-base-q5_1.bin").display()
        )),
        "{message}"
    );
    assert!(
        message.contains(
            &home
                .join(".local/share/sabicut/models/ggml-base-q5_1.bin")
                .display()
                .to_string()
        ),
        "{message}"
    );
    assert!(message.contains("never downloads models"), "{message}");
    assert!(!workspace_root().join("examples/plan.words.json").exists());
}

#[test]
fn analyze_reports_missing_files() {
    let output = sabicut(&["analyze", "examples/no-such-clip.mp4"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).starts_with("error: examples/no-such-clip.mp4: "),
        "{}",
        stderr(&output)
    );

    let output = sabicut(&[
        "analyze",
        "examples/plan.toml",
        "--model",
        "no/such/model.bin",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: model file no/such/model.bin not found\n"
    );

    let output = sabicut(&[
        "analyze",
        "examples/plan.toml",
        "-o",
        "no/such/folder/words.json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("the folder does not exist"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn analyze_rejects_bad_options() {
    for args in [
        ["analyze", "examples/plan.toml", "--threads", "0"],
        ["analyze", "examples/plan.toml", "--lang", "de"],
        ["analyze", "examples/plan.toml", "--model", ""],
    ] {
        let output = sabicut(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    let output = sabicut(&["analyze", "examples/plan.toml", "--vad-threshold", "1.5"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("the speech threshold must be between 0 and 1, got 1.5"),
        "{}",
        stderr(&output)
    );
}
