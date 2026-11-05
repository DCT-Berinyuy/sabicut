//! The rules on real transcripts: 10-minute and 2-minute LibriVox readings in English and
//! French, transcribed by `sabicut analyze` with the `small` model, plus sample fixes.
//! The files are not in the repository, so these tests are ignored by default. With the
//! files in `$SABICUT_BENCH/words` (default `~/.cache/sabicut/bench/words`), run:
//!
//! ```sh
//! cargo test -p sabicut-plan --test real -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use sabicut_core::{Fixes, Timeline, Transcript, apply_fixes};
use sabicut_plan::{Options, Reason, SourceInput, plan};

fn bench_words() -> PathBuf {
    std::env::var_os("SABICUT_BENCH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/sabicut/bench"))
        })
        .unwrap()
        .join("words")
}

fn load(name: &str, fixes: &str) -> Transcript {
    let dir = bench_words();
    let words = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
    let transcript = Transcript::from_json_str(&words).unwrap();
    let fixes = std::fs::read_to_string(dir.join(format!("{fixes}.toml"))).unwrap();
    let fixes = Fixes::from_toml_str(&fixes).unwrap();
    apply_fixes(&transcript, &fixes).unwrap().into_transcript()
}

fn check(name: &str, fixes: &str) {
    let transcript = load(name, fixes);
    let words = transcript.words().len();
    let source = SourceInput {
        id: "a".into(),
        path: transcript.media().to_owned(),
        words_path: format!("{name}.json"),
        transcript,
        crop_x: 0.5,
    };
    let planned = plan(&[source], &Options::default()).unwrap();
    let report = planned.report();
    assert!(Timeline::from_plan(planned.plan()).is_ok());
    assert_eq!(report.mid_word, 0, "{name}");
    println!(
        "{name}: {words} words, {} cuts, {:.1} s -> {:.1} s; cut {:.1} s of pauses, \
         {:.1} s of false starts, {:.1} s too short; {} false starts kept; mid-word {} of {}",
        planned.plan().cuts().len(),
        report.input_seconds(),
        report.output_seconds(),
        report.removed(Reason::Pause),
        report.removed(Reason::FalseStart),
        report.removed(Reason::Short),
        report.kept_false_starts.len(),
        report.mid_word,
        report.cut_points,
    );
    for removal in report.removals.iter().filter(|r| r.reason != Reason::Pause) {
        println!(
            "  {:?} {:.3}-{:.3}: {:?}",
            removal.reason, removal.start, removal.end, removal.text
        );
    }
    for span in &report.kept_false_starts {
        println!("  kept {:.3}-{:.3}: {:?}", span.start, span.end, span.text);
    }
}

#[test]
#[ignore = "needs the benchmark transcripts in ~/.cache/sabicut/bench"]
fn english_ten_minutes() {
    check("full-small-en", "fixes-en");
}

#[test]
#[ignore = "needs the benchmark transcripts in ~/.cache/sabicut/bench"]
fn french_ten_minutes() {
    check("full-small-fr", "fixes-fr");
}

#[test]
#[ignore = "needs the benchmark transcripts in ~/.cache/sabicut/bench"]
fn english_two_minutes() {
    check("dod2-small-en", "fixes-en");
}

#[test]
#[ignore = "needs the benchmark transcripts in ~/.cache/sabicut/bench"]
fn french_two_minutes() {
    check("dod2-small-fr", "fixes-fr");
}
