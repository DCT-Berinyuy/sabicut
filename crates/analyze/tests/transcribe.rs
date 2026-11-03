//! End-to-end transcription with the real models and ffmpeg.
//!
//! CI has no models, so these tests are ignored there. To run them, put
//! `ggml-tiny-q5_1.bin` and `ggml-silero-v6.2.0.bin` in a model folder (see the README's
//! "Speech models" section), install ffmpeg, and run:
//!
//! ```sh
//! cargo test -p sabicut-analyze -- --ignored
//! ```
//!
//! Each test takes a few seconds with the tiny model. They run one at a time: every
//! transcription waits for the previous one to finish.

use std::path::{Path, PathBuf};

use sabicut_analyze::{Language, ModelChoice, ModelDirs, ModelSize, Progress, Settings, analyze};
use sabicut_core::Transcript;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn tiny() -> Settings {
    let mut settings = Settings::new(ModelDirs::from_env());
    settings.model = ModelChoice::Named(ModelSize::Tiny);
    settings
}

fn lowercase_text(transcript: &Transcript) -> String {
    transcript
        .words()
        .iter()
        .map(|word| word.text().to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rules sabicut analyze promises beyond the format: every word ends inside the
/// stretch of speech it starts in.
fn assert_words_inside_speech(transcript: &Transcript) {
    assert_eq!(transcript.validate(), Ok(()));
    assert!(!transcript.speech().is_empty());
    for word in transcript.words() {
        let segment = transcript
            .speech()
            .iter()
            .find(|s| s.start() <= word.start() && word.start() <= s.end())
            .unwrap_or_else(|| panic!("{word:?} starts outside the speech"));
        assert!(
            word.end() <= segment.end(),
            "{word:?} ends after {segment:?}"
        );
    }
}

#[test]
#[ignore = "needs the tiny and Silero models and ffmpeg"]
fn english_is_detected_and_transcribed() {
    let mut events = Vec::new();
    let analysis = analyze(&fixture("en-librivox.opus"), &tiny(), &mut |event| {
        events.push(event)
    })
    .unwrap();
    let transcript = analysis.transcript();

    assert_eq!(transcript.media(), "en-librivox.opus");
    assert_eq!(transcript.language(), "en");
    assert_eq!(transcript.model(), "ggml-tiny-q5_1.bin");
    assert!((4.8..4.9).contains(&transcript.duration()));
    let text = lowercase_text(transcript);
    for word in ["recording", "public", "domain"] {
        assert!(text.contains(word), "{text}");
    }
    assert_words_inside_speech(transcript);
    assert!(events.iter().any(|event| matches!(
        event,
        Progress::DetectedLanguage {
            language: Language::En,
            ..
        }
    )));
    assert_eq!(analysis.report().language, Some(Language::En));
}

#[test]
#[ignore = "needs the tiny and Silero models and ffmpeg"]
fn french_is_detected_and_transcribed() {
    let analysis = analyze(&fixture("fr-librivox.opus"), &tiny(), &mut |_| {}).unwrap();
    let transcript = analysis.transcript();

    assert_eq!(transcript.language(), "fr");
    let text = lowercase_text(transcript);
    for word in ["domaine", "public"] {
        assert!(text.contains(word), "{text}");
    }
    assert_words_inside_speech(transcript);
}

#[test]
#[ignore = "needs the tiny and Silero models and ffmpeg"]
fn keywords_and_dtw_run_with_a_given_language() {
    let mut settings = tiny();
    settings.language = Some(Language::En);
    settings.keywords = vec!["LibriVox".to_owned()];
    settings.dtw = true;
    let analysis = analyze(&fixture("en-librivox.opus"), &settings, &mut |_| {}).unwrap();
    let transcript = analysis.transcript();

    assert_eq!(transcript.language(), "en");
    assert_eq!(analysis.report().detected, None);
    assert!(lowercase_text(transcript).contains("domain"));
    assert_words_inside_speech(transcript);
}

#[test]
#[ignore = "needs the Silero model and ffmpeg"]
fn speech_can_be_found_without_transcribing() {
    let mut settings = tiny();
    settings.vad_only = true;
    let analysis = analyze(&fixture("fr-librivox.opus"), &settings, &mut |_| {}).unwrap();
    let transcript = analysis.transcript();

    assert_eq!(transcript.language(), "und");
    assert_eq!(transcript.model(), "ggml-silero-v6.2.0.bin");
    assert!(transcript.words().is_empty());
    assert!(!transcript.speech().is_empty());
    assert_eq!(transcript.validate(), Ok(()));
}
