//! The committed JSON Schema stays in sync with the Rust types, and the documented
//! example plans parse, validate, and round-trip.

use std::path::Path;

use sabicut_core::{ParseError, Plan, Timeline, json_schema};

/// The example from PRD section 5, verbatim.
const PRD_EXAMPLE: &str = r##"
schema = 1

[meta]
fps = 30
out = { w = 1080, h = 1920 }

[[source]]
id = "a"
path = "raw/IMG_0412.mov"
crop_x = 0.42                       # centre of the 9:16 window, 0–1

[[cut]]
src = "a"
in = 3.120
out = 9.880
zoom = [{ at = 5.4, scale = 1.25 }] # `at` is in source time

[hook]
text = "Stop leaking your API keys"
dur = 2.0

[captions]
words = "words.json"
fixes = "fixes.toml"
keywords = ["Rust", "Supabase"]
font = "fonts/Anton-Regular.ttf"
active = "#FFC400"
stroke = 10
"##;

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
        .replace("\r\n", "\n")
}

#[test]
fn committed_schema_matches_the_types() {
    assert!(
        repo_file("schema/v1.json") == json_schema(),
        "schema/v1.json is stale. Regenerate it with:\n\n    \
         cargo run -p sabicut-cli -- schema > schema/v1.json\n"
    );
}

#[test]
fn prd_example_is_valid() {
    let plan = Plan::from_toml_str(PRD_EXAMPLE).unwrap();
    let timeline = Timeline::from_plan(&plan).unwrap();

    assert_eq!(plan.sources()[0].crop_x(), 0.42);
    assert_eq!(plan.cuts()[0].zooms()[0].scale(), 1.25);
    assert_eq!(plan.captions().unwrap().keywords(), ["Rust", "Supabase"]);
    assert_eq!(timeline.total_frames(), 202);
    assert_eq!(timeline.hook_frames(), Some(60));
}

#[test]
fn repository_example_is_valid() {
    let plan = Plan::from_toml_str(&repo_file("examples/plan.toml")).unwrap();
    assert_eq!(plan.validate(), Ok(()));
}

#[test]
fn plans_round_trip_through_toml() {
    let plan = Plan::from_toml_str(&repo_file("examples/plan.toml")).unwrap();
    let written = toml::to_string(&plan).unwrap();
    assert_eq!(Plan::from_toml_str(&written).unwrap(), plan);
}

#[test]
fn optional_sections_can_be_left_out() {
    let plan = Plan::from_toml_str(
        r#"
        schema = 1
        meta = { fps = 25, out = { w = 1080, h = 1080 } }
        source = [{ id = "clip-1", path = "a.mp4", crop_x = 0.5 }]
        cut = [{ src = "clip-1", in = 0, out = 2 }]
        "#,
    )
    .unwrap();

    assert!(plan.hook().is_none());
    assert!(plan.captions().is_none());
    assert!(plan.cuts()[0].zooms().is_empty());
    assert_eq!(Timeline::from_plan(&plan).unwrap().total_frames(), 50);
}

#[test]
fn unknown_keys_are_rejected_with_their_location() {
    let text = PRD_EXAMPLE.replace("out = 9.880", "ou = 9.880");
    let message = Plan::from_toml_str(&text).unwrap_err().to_string();

    assert!(message.contains("unknown field `ou`"), "{message}");
    assert!(message.contains("line 16"), "{message}");
}

#[test]
fn schema_version_is_checked_before_the_rest_of_the_file() {
    let newer = "schema = 2\n[something]\nnew = true\n";
    assert!(matches!(
        Plan::from_toml_str(newer),
        Err(ParseError::UnsupportedSchema(2))
    ));
    assert!(matches!(
        Plan::from_toml_str("[meta]\nfps = 30\n"),
        Err(ParseError::MissingSchema)
    ));
    assert!(matches!(
        Plan::from_toml_str("schema = \"1\"\n"),
        Err(ParseError::SchemaNotInteger)
    ));
    assert_eq!(
        Plan::from_toml_str(newer).unwrap_err().to_string(),
        "schema 2 is not supported: this version of sabicut reads schema 1"
    );
}

#[test]
fn invalid_toml_is_a_parse_error() {
    assert!(matches!(
        Plan::from_toml_str("schema = 1\n[meta\n"),
        Err(ParseError::Toml(_))
    ));
}

#[test]
fn per_source_words_round_trip_through_toml() {
    let plan = Plan::from_toml_str(
        r##"
        schema = 1
        meta = { fps = 30, out = { w = 1080, h = 1920 } }
        source = [
            { id = "a", path = "a.mp4", crop_x = 0.5, words = "a.words.json" },
            { id = "b", path = "b.mp4", crop_x = 0.5, words = "b.words.json" },
        ]
        cut = [{ src = "a", in = 0, out = 2 }, { src = "b", in = 1, out = 2 }]
        captions = { font = "f.ttf", active = "#FFC400", stroke = 10 }
        "##,
    )
    .unwrap();
    assert_eq!(plan.validate(), Ok(()));
    assert_eq!(plan.captions().unwrap().words(), None);

    let written = toml::to_string(&plan).unwrap();
    assert!(written.contains("words = \"b.words.json\""), "{written}");
    let read = Plan::from_toml_str(&written).unwrap();
    assert_eq!(read, plan);
    assert_eq!(read.caption_words("b"), Some("b.words.json"));
}

#[test]
fn an_empty_captions_table_round_trips_without_style_fields() {
    let plan = Plan::from_toml_str(
        r#"
        schema = 1
        meta = { fps = 30, out = { w = 1080, h = 1920 } }
        source = [{ id = "a", path = "a.mp4", crop_x = 0.5, words = "a.words.json" }]
        cut = [{ src = "a", in = 0, out = 2 }]

        [captions]
        "#,
    )
    .unwrap();
    assert_eq!(plan.validate(), Ok(()));
    assert_eq!(plan.caption_words("a"), Some("a.words.json"));

    let written = toml::to_string(&plan).unwrap();
    assert!(written.contains("[captions]"), "{written}");
    for key in ["font", "active", "stroke"] {
        assert!(!written.contains(key), "{written}");
    }
    assert_eq!(Plan::from_toml_str(&written).unwrap(), plan);
}
