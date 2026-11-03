//! fixes.toml: the committed JSON Schema stays in sync, rules match whole words without
//! regard to capitals, keep the timing and punctuation, and applying them is idempotent.

use std::path::Path;

use sabicut_core::{
    Fix, FixError, Fixes, FixesParseError, SpeechSegment, Transcript, Word, apply_fixes,
};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
        .replace("\r\n", "\n")
}

/// A transcript of `texts`, one word per second with probability 0.5 (the last 1.0).
fn transcript(texts: &[&str]) -> Transcript {
    let words = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let p = if i + 1 == texts.len() { 1.0 } else { 0.5 };
            Word::new(*text, i as f64, i as f64 + 0.8, p)
        })
        .collect();
    Transcript::new(
        "clip.mp4",
        "en",
        "ggml-small-q5_1.bin",
        100.0,
        vec![SpeechSegment::new(0.0, 99.0)],
        words,
    )
    .unwrap()
}

fn fixes(rules: &[(&str, &str)]) -> Fixes {
    Fixes::new(
        rules
            .iter()
            .map(|(from, to)| Fix::new(*from, *to, false))
            .collect(),
    )
}

fn texts(transcript: &Transcript) -> Vec<&str> {
    transcript.words().iter().map(Word::text).collect()
}

fn fixed_texts(words: &[&str], rules: &[(&str, &str)]) -> Vec<String> {
    let fixed = apply_fixes(&transcript(words), &fixes(rules)).unwrap();
    texts(fixed.transcript())
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn committed_schema_matches_the_types() {
    assert!(
        repo_file("schema/fixes-v1.json") == Fixes::json_schema(),
        "schema/fixes-v1.json is stale. Regenerate it with:\n\n    \
         cargo run -p sabicut-cli -- schema fixes > schema/fixes-v1.json\n"
    );
}

#[test]
fn repository_example_applies_to_the_example_words() {
    let fixes = Fixes::from_toml_str(&repo_file("examples/fixes.toml")).unwrap();
    assert_eq!(fixes.validate(), Ok(()));
    let words = Transcript::from_json_str(&repo_file("examples/words.json")).unwrap();
    let fixed = apply_fixes(&words, &fixes).unwrap();
    let fixed_words = fixed.transcript().words();
    assert_eq!(fixed_words.len(), words.words().len() - 2);
    let supabase = &fixed_words[10];
    assert_eq!(
        (supabase.text(), supabase.start(), supabase.end()),
        ("Supabase", 4.02, 4.62)
    );
    assert_eq!(fixed_words[12].text(), "Rust");
    assert_eq!(fixed_words.last().unwrap().text(), "GitHub.");
    assert_eq!(fixed.changes().len(), 3);
}

#[test]
fn many_words_become_one_from_the_start_of_the_first_to_the_end_of_the_last() {
    let fixed = apply_fixes(
        &transcript(&["I", "love", "Supa", "base", "!"]),
        &fixes(&[("supa base", "Supabase")]),
    )
    .unwrap();
    let words = fixed.transcript().words();
    assert_eq!(texts(fixed.transcript()), ["I", "love", "Supabase", "!"]);
    assert_eq!((words[2].start(), words[2].end()), (2.0, 3.8));
    assert_eq!(words[2].probability(), 0.5);
    // Speech, language and model are unchanged.
    assert_eq!(fixed.transcript().speech(), transcript(&["x"]).speech());
    assert_eq!(fixed.transcript().model(), "ggml-small-q5_1.bin");

    let change = &fixed.changes()[0];
    assert_eq!(change.words(), 2..4);
    assert_eq!((change.before(), change.after()), ("Supa base", "Supabase"));
    assert_eq!(
        (change.start(), change.end(), change.rules()),
        (2.0, 3.8, &[0][..])
    );
}

#[test]
fn one_word_for_one_keeps_its_timing_and_punctuation() {
    let fixed = apply_fixes(
        &transcript(&["«Dino,", "come", "here»."]),
        &fixes(&[("dino", "Dinah"), ("here", "HERE")]),
    )
    .unwrap();
    assert_eq!(texts(fixed.transcript()), ["«Dinah,", "come", "HERE»."]);
    assert_eq!(fixed.transcript().words()[0].start(), 0.0);
    assert_eq!(fixed.transcript().words()[2].end(), 2.8);
    assert_eq!(fixed.changes().len(), 2);
}

#[test]
fn equal_word_counts_are_replaced_word_by_word() {
    let fixed = apply_fixes(
        &transcript(&["visual", "studio", "code."]),
        &fixes(&[("visual studio code", "Visual Studio Code")]),
    )
    .unwrap();
    assert_eq!(texts(fixed.transcript()), ["Visual", "Studio", "Code."]);
    assert_eq!(fixed.transcript().words()[1].start(), 1.0);
    // One change covers the three words.
    assert_eq!(fixed.changes().len(), 1);
    assert_eq!(fixed.changes()[0].before(), "visual studio code.");
}

#[test]
fn punctuation_around_a_match_is_kept_and_inside_it_is_dropped() {
    assert_eq!(
        fixed_texts(&["«Supa,", "base»,", "ok"], &[("Supa base", "Supabase")]),
        ["«Supabase»,", "ok"]
    );
    // Punctuation joined with a no-break space, as `sabicut analyze` writes it.
    assert_eq!(
        fixed_texts(&["moulin\u{a0}!", "ok"], &[("moulin", "Moulin")]),
        ["Moulin\u{a0}!", "ok"]
    );
}

#[test]
fn matching_ignores_capitals_unless_asked() {
    assert_eq!(
        fixed_texts(&["RUST", "Rust", "rust"], &[("rust", "Rust")]),
        ["Rust", "Rust", "Rust"]
    );
    let strict = Fixes::new(vec![Fix::new("rust", "Rust", true)]);
    let fixed = apply_fixes(&transcript(&["RUST", "rust"]), &strict).unwrap();
    assert_eq!(texts(fixed.transcript()), ["RUST", "Rust"]);
    // Accented capitals too.
    assert_eq!(fixed_texts(&["ÉCOLE"], &[("école", "École")]), ["École"]);
}

#[test]
fn only_whole_words_match() {
    assert_eq!(
        fixed_texts(
            &["database", "Supa", "baseline", "base"],
            &[("base", "BASE"), ("supa base", "X")]
        ),
        ["database", "Supa", "baseline", "BASE"]
    );
    // Symbols are part of words: "C" does not match "C++".
    assert_eq!(fixed_texts(&["C++", "C"], &[("c", "C#")]), ["C++", "C#"]);
}

#[test]
fn the_longest_match_wins() {
    assert_eq!(
        fixed_texts(
            &["Supa", "base", "Supa"],
            &[("supa", "Super"), ("supa base", "Supabase")]
        ),
        ["Supabase", "Super"]
    );
}

#[test]
fn one_word_can_become_several() {
    let fixed = apply_fixes(
        &transcript(&["gonna", "go"]),
        &fixes(&[("gonna", "going to")]),
    )
    .unwrap();
    let word = &fixed.transcript().words()[0];
    assert_eq!(
        (word.text(), word.start(), word.end()),
        ("going to", 0.0, 0.8)
    );
    assert_eq!(fixed.transcript().validate(), Ok(()));
}

#[test]
fn applying_fixes_twice_changes_nothing() {
    let rules = fixes(&[
        ("supa base", "Supabase"),
        ("rust", "Rust"),
        ("gonna", "going to"),
        ("base", "base camp"),
        ("the the", "the"),
    ]);
    let input = transcript(&[
        "Supa", "base", "and", "rust", "gonna", "base", "the", "the", "the", "end.",
    ]);
    let once = apply_fixes(&input, &rules).unwrap();
    let twice = apply_fixes(once.transcript(), &rules).unwrap();
    assert_eq!(twice.transcript(), once.transcript());
    assert!(twice.changes().is_empty());
    assert_eq!(
        texts(once.transcript()),
        [
            "Supabase",
            "and",
            "Rust",
            "going to",
            "base camp",
            "the",
            "end."
        ]
    );
}

#[test]
fn chains_and_repeats_settle() {
    assert_eq!(
        fixed_texts(&["a", "b"], &[("a", "b"), ("b", "c")]),
        ["c", "c"]
    );
    assert_eq!(fixed_texts(&["the"; 16], &[("the the", "the")]), ["the"]);
}

#[test]
fn rules_that_undo_each_other_are_an_error() {
    let error = apply_fixes(
        &transcript(&["colour"]),
        &fixes(&[("colour", "color"), ("color", "colour")]),
    )
    .unwrap_err();
    assert!(matches!(&error, FixError::NotSettled { rules, .. } if rules == &[0, 1]));
    assert!(
        error.to_string().contains("check fix[0], fix[1]"),
        "{error}"
    );
}

#[test]
fn rule_problems_name_the_rule() {
    let text = r#"
        schema = 1
        [[fix]]
        from = "« ! »"
        to = "x"
        [[fix]]
        from = "Supa base"
        to = "Supabase"
        [[fix]]
        from = "supa  BASE"
        to = "  "
        [[fix]]
        from = "tab"
        to = "a	b"
    "#;
    let errors = Fixes::from_toml_str(text).unwrap().validate().unwrap_err();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "fix[0].from must contain a word (letters or digits), got \"« ! »\"",
            "fix[2].from must be unique, but \"supa  BASE\" is already fix[1].from",
            "fix[2].to must not be empty",
        ]
    );
    // A tab between words is whitespace, so it is accepted (and becomes one space).
    let long = vec!["w"; 33].join(" ");
    let errors = Fixes::new(vec![Fix::new(long, "x", false)])
        .validate()
        .unwrap_err();
    assert_eq!(
        errors.to_string(),
        "fix[0].from must have at most 32 words, got 33"
    );
    assert!(matches!(
        apply_fixes(&transcript(&["x"]), &fixes(&[("", "y")])),
        Err(FixError::Invalid(_))
    ));
}

#[test]
fn unknown_keys_and_versions_are_rejected() {
    let message =
        Fixes::from_toml_str("schema = 1\n[[fix]]\nfrom = \"a\"\nto = \"b\"\nwhole = true\n")
            .unwrap_err()
            .to_string();
    assert!(message.contains("unknown field `whole`"), "{message}");
    assert!(message.contains("line 5"), "{message}");
    assert!(matches!(
        Fixes::from_toml_str("schema = 2\n[[rule]]\n"),
        Err(FixesParseError::UnsupportedSchema(2))
    ));
    assert!(matches!(
        Fixes::from_toml_str("[[fix]]\nfrom = \"a\"\nto = \"b\"\n"),
        Err(FixesParseError::MissingSchema)
    ));
    assert!(matches!(
        Fixes::from_toml_str("schema = \"1\"\n"),
        Err(FixesParseError::SchemaNotInteger)
    ));
    // No rules at all is valid, and changes nothing.
    let empty = Fixes::from_toml_str("schema = 1\n").unwrap();
    assert!(empty.rules().is_empty());
    let fixed = apply_fixes(&transcript(&["a"]), &empty).unwrap();
    assert!(fixed.changes().is_empty());
}

#[test]
fn fixes_round_trip_through_toml() {
    let fixes = Fixes::new(vec![
        Fix::new("Supa base", "Supabase", false),
        Fix::new("rust", "Rust", true),
    ]);
    let text = toml::to_string(&fixes).unwrap();
    assert!(!text.contains("case_sensitive = false"), "{text}");
    assert_eq!(Fixes::from_toml_str(&text).unwrap(), fixes);
}
