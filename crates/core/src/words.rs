//! Word timings, `words.json` (schema v1): what `sabicut analyze` writes, and what
//! fixes, captions and plan rules read. Nothing here depends on the speech model.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::plan::MAX_SECONDS;
use crate::validate::{Report, ValidationErrors};

/// The words.json format version this build reads and writes.
pub const WORDS_SCHEMA_VERSION: u32 = 1;

/// # Sabicut word timings (words.json, schema v1)
///
/// The transcript of one media file: the stretches of speech that voice-activity
/// detection found, and every word with its timing. Times are in seconds from the
/// start of the media file, to the millisecond.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    /// Format version. Always 1 for this schema.
    #[schemars(extend("const" = 1))]
    schema: u32,
    /// File name of the transcribed media, without its folder, like "IMG_0412.mov".
    #[schemars(pattern(r"^[^/\\]*\S[^/\\]*$"))]
    media: String,
    /// Spoken language as a lowercase code: "en", "fr", or "und" when it is not
    /// known. Pidgin is transcribed as "en".
    #[schemars(pattern(r"^[a-z]{2,3}$"))]
    language: String,
    /// File name of the speech model that wrote the words, like "ggml-small-q5_1.bin".
    #[schemars(pattern(r"\S"))]
    model: String,
    /// Length of the audio, in seconds.
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    duration: f64,
    /// Stretches of speech found by voice-activity detection, in time order and not
    /// overlapping. Only these were transcribed; everything else is silence or noise.
    speech: Vec<SpeechSegment>,
    /// Every word, in spoken order. A word starts no earlier than the previous one
    /// ends.
    words: Vec<Word>,
}

/// A stretch of speech found by voice-activity detection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpeechSegment {
    /// Where the speech starts, in seconds.
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    start: f64,
    /// Where the speech ends, in seconds. Greater than start.
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    end: f64,
}

/// One word and when it is spoken.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Word {
    /// The word as written, with its punctuation, like "Supabase,". Not empty, no
    /// spaces at either end, no control characters. It may contain inner spaces:
    /// punctuation that stands apart, as in French "moulin !", is joined to its word
    /// with a no-break space, and a fix may join several words into one.
    #[schemars(pattern(r"^\S(.*\S)?$"))]
    text: String,
    /// When the word starts, in seconds.
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    start: f64,
    /// When the word ends, in seconds. At least start. `sabicut analyze` ends every
    /// word inside the stretch of speech it starts in.
    #[schemars(range(min = 0.0, max = MAX_SECONDS))]
    end: f64,
    /// How sure the speech model is of the word, from 0 to 1 (the mean probability of
    /// its tokens).
    #[schemars(range(min = 0.0, max = 1.0))]
    probability: f64,
}

/// Why a `words.json` could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TranscriptParseError {
    /// The file has no top-level `schema` key.
    #[error("missing `\"schema\": 1` (the words.json format version)")]
    MissingSchema,
    /// `schema` is present but is not a whole number.
    #[error("`schema` must be a whole number, like `\"schema\": 1`")]
    SchemaNotInteger,
    /// The file uses a format version that this build cannot read.
    #[error(
        "words.json schema {0} is not supported: this version of sabicut reads schema {WORDS_SCHEMA_VERSION}"
    )]
    UnsupportedSchema(i64),
    /// The text is not valid JSON, or does not have the shape of a schema v1 transcript.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Reads only the version, so a file from a newer format gets a clear error.
#[derive(Deserialize)]
struct VersionProbe {
    schema: Option<serde_json::Value>,
}

/// Rounds to the millisecond, the precision of times in words.json.
fn round_seconds(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

impl Transcript {
    /// Builds a transcript and checks it against every rule of schema v1.
    ///
    /// `media` and `model` are file names. The duration is rounded to the millisecond;
    /// segment and word times already are (see [`SpeechSegment::new`] and
    /// [`Word::new`]).
    pub fn new(
        media: impl Into<String>,
        language: impl Into<String>,
        model: impl Into<String>,
        duration: f64,
        speech: Vec<SpeechSegment>,
        words: Vec<Word>,
    ) -> Result<Self, ValidationErrors> {
        let transcript = Self {
            schema: WORDS_SCHEMA_VERSION,
            media: media.into(),
            language: language.into(),
            model: model.into(),
            duration: round_seconds(duration),
            speech,
            words,
        };
        transcript.validate()?;
        Ok(transcript)
    }

    /// Parses a `words.json` document.
    ///
    /// This checks the format version and the shape of the document (unknown keys are
    /// rejected). Call [`Transcript::validate`] to check the values.
    pub fn from_json_str(text: &str) -> Result<Self, TranscriptParseError> {
        let probe: VersionProbe = serde_json::from_str(text)?;
        match probe.schema {
            None => return Err(TranscriptParseError::MissingSchema),
            Some(serde_json::Value::Number(number)) => match number.as_i64() {
                Some(version) if version == i64::from(WORDS_SCHEMA_VERSION) => {}
                Some(version) => return Err(TranscriptParseError::UnsupportedSchema(version)),
                None => return Err(TranscriptParseError::SchemaNotInteger),
            },
            Some(_) => return Err(TranscriptParseError::SchemaNotInteger),
        }
        Ok(serde_json::from_str(text)?)
    }

    /// Writes the transcript as `words.json`, with one speech segment or word per line
    /// so the file reads and diffs well, ending with a newline.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut out = String::new();
        let scalars = [
            ("schema", serde_json::to_string(&self.schema)?),
            ("media", serde_json::to_string(&self.media)?),
            ("language", serde_json::to_string(&self.language)?),
            ("model", serde_json::to_string(&self.model)?),
            ("duration", serde_json::to_string(&self.duration)?),
        ];
        out.push_str("{\n");
        for (key, value) in scalars {
            let _ = writeln!(out, "  \"{key}\": {value},");
        }
        write_list(&mut out, "speech", &self.speech, ",")?;
        write_list(&mut out, "words", &self.words, "")?;
        out.push_str("}\n");
        Ok(out)
    }

    /// Checks every rule of schema v1 and reports all problems at once.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut report = Report::default();
        if self.schema != WORDS_SCHEMA_VERSION {
            report.push(
                "schema",
                format!("must be {WORDS_SCHEMA_VERSION}, got {}", self.schema),
            );
        }
        if let Some(problem) = text_problem(&self.media) {
            report.push("media", problem);
        } else if self.media.contains(['/', '\\']) {
            report.push(
                "media",
                format!("must be a file name without folders, got {:?}", self.media),
            );
        }
        if !is_language_code(&self.language) {
            report.push(
                "language",
                format!(
                    "must be a lowercase language code like \"en\" or \"fr\", got {:?}",
                    self.language
                ),
            );
        }
        if self.model.trim().is_empty() {
            report.push("model", "must not be empty");
        }
        let duration_ok = (0.0..=MAX_SECONDS).contains(&self.duration);
        if !duration_ok {
            report.push(
                "duration",
                format!(
                    "must be between 0 and {MAX_SECONDS} seconds, got {}",
                    self.duration
                ),
            );
        }
        let limit = duration_ok.then_some(self.duration);
        self.check_speech(&mut report, limit);
        self.check_words(&mut report, limit);
        report.finish()
    }

    fn check_speech(&self, report: &mut Report, limit: Option<f64>) {
        let mut previous_end: Option<f64> = None;
        for (i, segment) in self.speech.iter().enumerate() {
            let field = format!("speech[{i}]");
            let start_ok = check_time(report, &field, "start", segment.start, limit);
            let end_ok = check_time(report, &field, "end", segment.end, limit);
            if start_ok && end_ok && segment.end <= segment.start {
                report.push(format!("{field}.end"), "must be > start");
            }
            if let Some(previous) = previous_end
                && start_ok
                && segment.start < previous
            {
                report.push(
                    format!("{field}.start"),
                    format!(
                        "must be >= speech[{}].end ({previous}), got {}",
                        i - 1,
                        segment.start
                    ),
                );
            }
            previous_end = Some(segment.end);
        }
    }

    fn check_words(&self, report: &mut Report, limit: Option<f64>) {
        let mut previous_end: Option<f64> = None;
        for (i, word) in self.words.iter().enumerate() {
            let field = format!("words[{i}]");
            if let Some(problem) = text_problem(&word.text) {
                report.push(format!("{field}.text"), problem);
            }
            let start_ok = check_time(report, &field, "start", word.start, limit);
            let end_ok = check_time(report, &field, "end", word.end, limit);
            if start_ok && end_ok && word.end < word.start {
                report.push(format!("{field}.end"), "must be >= start");
            }
            if let Some(previous) = previous_end
                && start_ok
                && word.start < previous
            {
                report.push(
                    format!("{field}.start"),
                    format!(
                        "must be >= words[{}].end ({previous}), got {}",
                        i - 1,
                        word.start
                    ),
                );
            }
            if !(0.0..=1.0).contains(&word.probability) {
                report.push(
                    format!("{field}.probability"),
                    format!("must be between 0 and 1, got {}", word.probability),
                );
            }
            previous_end = Some(word.end);
        }
    }

    /// The same transcript with other words. Fixes use it; they keep the order and
    /// timing rules intact.
    pub(crate) fn with_words(&self, words: Vec<Word>) -> Self {
        Self {
            schema: self.schema,
            media: self.media.clone(),
            language: self.language.clone(),
            model: self.model.clone(),
            duration: self.duration,
            speech: self.speech.clone(),
            words,
        }
    }

    /// Format version.
    pub fn schema(&self) -> u32 {
        self.schema
    }

    /// File name of the transcribed media.
    pub fn media(&self) -> &str {
        &self.media
    }

    /// Spoken language code, like "en" or "fr" ("und" when not known).
    pub fn language(&self) -> &str {
        &self.language
    }

    /// File name of the speech model that wrote the words.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Length of the audio, in seconds.
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Stretches of speech, in time order.
    pub fn speech(&self) -> &[SpeechSegment] {
        &self.speech
    }

    /// Every word, in spoken order.
    pub fn words(&self) -> &[Word] {
        &self.words
    }

    /// The JSON Schema of words.json (schema v1), pretty-printed with a trailing
    /// newline. It is exactly the content of the repository's `schema/words-v1.json`.
    pub fn json_schema() -> String {
        crate::schema_gen::pretty::<Self>()
    }
}

impl SpeechSegment {
    /// A stretch of speech from `start` to `end` seconds, rounded to the millisecond.
    pub fn new(start: f64, end: f64) -> Self {
        Self {
            start: round_seconds(start),
            end: round_seconds(end),
        }
    }

    /// Where the speech starts, in seconds.
    pub fn start(&self) -> f64 {
        self.start
    }

    /// Where the speech ends, in seconds.
    pub fn end(&self) -> f64 {
        self.end
    }
}

impl Word {
    /// A word spoken from `start` to `end` seconds. Times are rounded to the
    /// millisecond and the probability to 3 decimals, the precision of words.json.
    pub fn new(text: impl Into<String>, start: f64, end: f64, probability: f64) -> Self {
        Self {
            text: text.into(),
            start: round_seconds(start),
            end: round_seconds(end),
            probability: (probability * 1000.0).round() / 1000.0,
        }
    }

    /// The word as written, with any punctuation attached.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// When the word starts, in seconds.
    pub fn start(&self) -> f64 {
        self.start
    }

    /// When the word ends, in seconds.
    pub fn end(&self) -> f64 {
        self.end
    }

    /// How sure the speech model is of the word, from 0 to 1.
    pub fn probability(&self) -> f64 {
        self.probability
    }
}

/// Checks that a time is within the duration (or, if the duration is not valid, within
/// `0..=MAX_SECONDS`); returns whether it is.
fn check_time(
    report: &mut Report,
    field: &str,
    key: &str,
    seconds: f64,
    duration: Option<f64>,
) -> bool {
    let ok = (0.0..=duration.unwrap_or(MAX_SECONDS)).contains(&seconds);
    if !ok {
        let range = match duration {
            Some(duration) => format!("0 and {duration} seconds (the duration)"),
            None => format!("0 and {MAX_SECONDS} seconds"),
        };
        report.push(
            format!("{field}.{key}"),
            format!("must be between {range}, got {seconds}"),
        );
    }
    ok
}

/// What is wrong with a word's text, if anything.
fn text_problem(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        Some("must not be empty")
    } else if text.trim() != text {
        Some("must not start or end with spaces")
    } else if text.chars().any(char::is_control) {
        Some("must not contain control characters such as line breaks")
    } else {
        None
    }
}

fn is_language_code(code: &str) -> bool {
    (2..=3).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Writes `"key": [ ... ]` with one compact JSON item per line.
fn write_list<T: Serialize>(
    out: &mut String,
    key: &str,
    items: &[T],
    after: &str,
) -> Result<(), serde_json::Error> {
    if items.is_empty() {
        let _ = writeln!(out, "  \"{key}\": []{after}");
        return Ok(());
    }
    let _ = writeln!(out, "  \"{key}\": [");
    for (i, item) in items.iter().enumerate() {
        let separator = if i + 1 < items.len() { "," } else { "" };
        let _ = writeln!(out, "    {}{separator}", serde_json::to_string(item)?);
    }
    let _ = writeln!(out, "  ]{after}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_round_to_the_millisecond() {
        let word = Word::new("hi", 1.23449, 1.2345, 0.98765);
        assert_eq!(
            (word.start(), word.end(), word.probability()),
            (1.234, 1.235, 0.988)
        );
        assert_eq!(round_seconds(0.1 + 0.2), 0.3);
    }

    #[test]
    fn json_has_one_item_per_line() {
        let transcript = Transcript::new(
            "clip \"1\".mp4",
            "en",
            "ggml-small-q5_1.bin",
            2.0,
            vec![SpeechSegment::new(0.25, 1.5)],
            vec![
                Word::new("Hi", 0.3, 0.5, 0.9),
                Word::new("there.", 0.5, 1.0, 1.0),
            ],
        )
        .unwrap();
        assert_eq!(
            transcript.to_json().unwrap(),
            r#"{
  "schema": 1,
  "media": "clip \"1\".mp4",
  "language": "en",
  "model": "ggml-small-q5_1.bin",
  "duration": 2.0,
  "speech": [
    {"start":0.25,"end":1.5}
  ],
  "words": [
    {"text":"Hi","start":0.3,"end":0.5,"probability":0.9},
    {"text":"there.","start":0.5,"end":1.0,"probability":1.0}
  ]
}
"#
        );
    }
}
