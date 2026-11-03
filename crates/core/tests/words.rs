//! words.json: the committed JSON Schema stays in sync with the Rust types, files
//! round-trip, unknown keys are rejected, and every rule is checked.

use std::path::Path;

use sabicut_core::{SpeechSegment, Transcript, TranscriptParseError, Word};

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
        .replace("\r\n", "\n")
}

fn sample() -> Transcript {
    Transcript::new(
        "IMG_0412.mov",
        "fr",
        "ggml-small-q5_1.bin",
        10.0,
        vec![SpeechSegment::new(0.5, 4.0), SpeechSegment::new(5.0, 9.5)],
        vec![
            Word::new("«\u{a0}Le", 0.6, 0.8, 0.95),
            Word::new("moulin\u{a0}!\u{a0}»", 0.8, 1.4, 0.875),
            Word::new("Supabase,", 5.1, 5.9, 0.5),
            Word::new("going to", 5.9, 6.3, 1.0),
        ],
    )
    .unwrap()
}

/// A minimal valid words.json with `words` as its word list.
fn with_words(words: &str) -> String {
    format!(
        r#"{{"schema": 1, "media": "a.mp4", "language": "en", "model": "m.bin",
            "duration": 10, "speech": [{{"start": 0, "end": 10}}], "words": [{words}]}}"#
    )
}

fn problems(text: &str) -> Vec<String> {
    match Transcript::from_json_str(text).unwrap().validate() {
        Ok(()) => Vec::new(),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn committed_schema_matches_the_types() {
    assert!(
        repo_file("schema/words-v1.json") == Transcript::json_schema(),
        "schema/words-v1.json is stale. Regenerate it with:\n\n    \
         cargo run -p sabicut-cli -- schema words > schema/words-v1.json\n"
    );
}

#[test]
fn repository_example_is_valid() {
    let transcript = Transcript::from_json_str(&repo_file("examples/words.json")).unwrap();
    assert_eq!(transcript.validate(), Ok(()));
    assert_eq!(transcript.media(), "IMG_0412.mov");
    assert_eq!(transcript.words().len(), 24);
    // The example is written in the canonical layout.
    assert_eq!(
        transcript.to_json().unwrap(),
        repo_file("examples/words.json")
    );
}

#[test]
fn transcripts_round_trip_through_json() {
    let transcript = sample();
    let json = transcript.to_json().unwrap();
    assert_eq!(Transcript::from_json_str(&json).unwrap(), transcript);
    assert_eq!(transcript.words()[1].probability(), 0.875);
    assert_eq!(transcript.speech()[1].end(), 9.5);
}

#[test]
fn unknown_keys_are_rejected_with_their_location() {
    let text = sample()
        .to_json()
        .unwrap()
        .replace("\"probability\":0.5", "\"probability\":0.5,\"speaker\":2");
    let message = Transcript::from_json_str(&text).unwrap_err().to_string();
    assert!(message.contains("unknown field `speaker`"), "{message}");
    assert!(message.contains("line 14"), "{message}");

    let text = with_words("").replace("\"schema\": 1,", "\"schema\": 1, \"title\": \"x\",");
    let message = Transcript::from_json_str(&text).unwrap_err().to_string();
    assert!(message.contains("unknown field `title`"), "{message}");
}

#[test]
fn schema_version_is_checked_before_the_rest_of_the_file() {
    assert!(matches!(
        Transcript::from_json_str(r#"{"schema": 2, "anything": true}"#),
        Err(TranscriptParseError::UnsupportedSchema(2))
    ));
    assert!(matches!(
        Transcript::from_json_str(r#"{"words": []}"#),
        Err(TranscriptParseError::MissingSchema)
    ));
    assert!(matches!(
        Transcript::from_json_str(r#"{"schema": "1"}"#),
        Err(TranscriptParseError::SchemaNotInteger)
    ));
    assert!(matches!(
        Transcript::from_json_str(r#"{"schema": 1.5}"#),
        Err(TranscriptParseError::SchemaNotInteger)
    ));
    assert!(matches!(
        Transcript::from_json_str("{\"schema\": 1,"),
        Err(TranscriptParseError::Json(_))
    ));
    assert_eq!(
        Transcript::from_json_str(r#"{"schema": 2}"#)
            .unwrap_err()
            .to_string(),
        "words.json schema 2 is not supported: this version of sabicut reads schema 1"
    );
}

#[test]
fn word_rules() {
    let word = |text: &str, start: f64, end: f64, p: f64| {
        format!(r#"{{"text": {text:?}, "start": {start}, "end": {end}, "probability": {p}}}"#)
    };
    assert_eq!(
        problems(&with_words(
            &[
                word("Hi", 1.0, 1.5, 0.9),
                word("there", 1.4, 1.3, 1.2),
                word(" padded", 2.0, 2.0, 0.5),
                word("", 3.0, 3.1, 0.5),
                word("line\nbreak", 4.0, 11.0, -0.1),
            ]
            .join(",")
        )),
        [
            "words[1].end must be >= start",
            "words[1].start must be >= words[0].end (1.5), got 1.4",
            "words[1].probability must be between 0 and 1, got 1.2",
            "words[2].text must not start or end with spaces",
            "words[3].text must not be empty",
            "words[4].text must not contain control characters such as line breaks",
            "words[4].end must be between 0 and 10 seconds (the duration), got 11",
            "words[4].probability must be between 0 and 1, got -0.1",
        ]
    );
    // Touching words and zero-length words are fine.
    assert!(
        problems(&with_words(
            &[word("a", 1.0, 1.5, 1.0), word("b", 1.5, 1.5, 0.0)].join(",")
        ))
        .is_empty()
    );
}

#[test]
fn speech_and_metadata_rules() {
    let text = r#"{"schema": 1, "media": "raw/a.mp4", "language": "EN", "model": " ",
        "duration": -1, "speech": [{"start": 2, "end": 2}, {"start": 1, "end": 3},
        {"start": 4, "end": 90000}], "words": []}"#;
    assert_eq!(
        problems(text),
        [
            "media must be a file name without folders, got \"raw/a.mp4\"",
            "language must be a lowercase language code like \"en\" or \"fr\", got \"EN\"",
            "model must not be empty",
            "duration must be between 0 and 86400 seconds, got -1",
            "speech[0].end must be > start",
            "speech[1].start must be >= speech[0].end (2), got 1",
            "speech[2].end must be between 0 and 86400 seconds, got 90000",
        ]
    );
    let unknown = with_words("").replace("\"en\"", "\"und\"");
    assert!(problems(&unknown).is_empty());
}

#[test]
fn transcripts_are_checked_when_built() {
    let errors = Transcript::new(
        "a.mp4",
        "en",
        "m.bin",
        1.0,
        vec![],
        vec![Word::new("late", 0.5, 1.5, 1.0)],
    )
    .unwrap_err();
    assert_eq!(
        errors.to_string(),
        "words[0].end must be between 0 and 1 seconds (the duration), got 1.5"
    );
}
