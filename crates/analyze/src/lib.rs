//! Offline transcription for Sabicut: the speech in a video or audio file becomes word
//! timings (`words.json`, [`sabicut_core::Transcript`]).
//!
//! The pipeline makes no network calls:
//!
//! 1. ffmpeg decodes the audio to 16 kHz mono (it may only open local files).
//! 2. Silero (through whisper.cpp, on 1 thread) finds the stretches of speech.
//! 3. The speech is packed into chunks of at most 30 s, and whisper.cpp transcribes one
//!    chunk at a time, with one word per segment and token-level timestamps.
//! 4. Every time is mapped back to the original audio, each word ends inside its stretch
//!    of speech, and punctuation joins its neighbouring word.
//!
//! Models are read from disk and never downloaded (see [`ModelDirs`]). Before the
//! model loads, free memory is checked, and only one transcription runs at a time on a
//! machine.
//!
//! ```no_run
//! use std::path::Path;
//! use sabicut_analyze::{ModelDirs, Settings, analyze};
//!
//! let settings = Settings::new(ModelDirs::from_env());
//! let analysis = analyze(Path::new("clip.mp4"), &settings, &mut |_| {})?;
//! std::fs::write("clip.words.json", analysis.transcript().to_json()?)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]
#![warn(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod assemble;
mod audio;
mod chunks;
mod lock;
mod memory;
mod models;
mod transcribe;
mod vad;

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sabicut_core::{SpeechSegment, Transcript, ValidationErrors};

pub use audio::{AudioError, SAMPLE_RATE, decode};
pub use models::{MODELS_ENV, ModelChoice, ModelDirs, ModelError, ModelSize, VAD_MODEL_FILE};
pub use vad::VadSettings;

use crate::assemble::{HeardWord, seconds};
use crate::chunks::Chunk;
use crate::transcribe::Engine;

/// Keywords may add up to this many characters.
const MAX_KEYWORD_CHARS: usize = 600;
/// Most CPU threads Whisper may use.
pub const MAX_THREADS: u16 = 64;
/// Longest padding around speech, in milliseconds.
pub const MAX_VAD_PAD_MS: u32 = 2000;

/// A language Sabicut transcribes. Pidgin is transcribed as English.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// English (and Cameroonian Pidgin English).
    En,
    /// French.
    Fr,
}

impl Language {
    /// The code written in words.json, like "en".
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
        }
    }
}

/// How to transcribe.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The Whisper model. Defaults to `small`.
    pub model: ModelChoice,
    /// Where to look for models.
    pub dirs: ModelDirs,
    /// The spoken language, or `None` to tell English from French automatically.
    pub language: Option<Language>,
    /// CPU threads for Whisper, from 1 to [`MAX_THREADS`]. Silero always uses 1.
    pub threads: u16,
    /// Names and terms to spell right, given to Whisper as a hint.
    pub keywords: Vec<String>,
    /// Overrides the tuned speech threshold of the language.
    pub vad_threshold: Option<f32>,
    /// Overrides the tuned padding around speech of the language, in milliseconds.
    pub vad_pad_ms: Option<u32>,
    /// Only find the speech: no Whisper model is loaded and no words are written.
    pub vad_only: bool,
    /// Word starts from whisper.cpp's dynamic time warping instead of its timestamp
    /// heuristics. Needs a named model (tiny, base or small).
    pub dtw: bool,
    /// whisper.cpp's flash attention (not combined with `dtw`).
    pub flash_attn: bool,
    /// Let whisper.cpp print its own log lines to standard error.
    pub whisper_logs: bool,
}

impl Settings {
    /// The default settings, looking for models in `dirs`.
    pub fn new(dirs: ModelDirs) -> Self {
        Self {
            model: ModelChoice::default(),
            dirs,
            language: None,
            threads: 4,
            keywords: Vec::new(),
            vad_threshold: None,
            vad_pad_ms: None,
            vad_only: false,
            dtw: false,
            flash_attn: false,
            whisper_logs: false,
        }
    }

    /// The voice-activity settings for a language, with any overrides applied.
    pub fn vad_settings(&self, language: Option<Language>) -> VadSettings {
        let tuned = VadSettings::for_language(language);
        VadSettings {
            threshold: self.vad_threshold.unwrap_or(tuned.threshold),
            pad_ms: self.vad_pad_ms.unwrap_or(tuned.pad_ms),
        }
    }

    fn check(&self) -> Result<(), AnalyzeError> {
        let invalid = |message: String| Err(AnalyzeError::Settings(message));
        if !(1..=MAX_THREADS).contains(&self.threads) {
            return invalid(format!(
                "threads must be between 1 and {MAX_THREADS}, got {}",
                self.threads
            ));
        }
        if let Some(threshold) = self.vad_threshold
            && !(threshold > 0.0 && threshold < 1.0)
        {
            return invalid(format!(
                "the speech threshold must be between 0 and 1, got {threshold}"
            ));
        }
        if let Some(pad) = self.vad_pad_ms
            && pad > MAX_VAD_PAD_MS
        {
            return invalid(format!(
                "the padding around speech must be at most {MAX_VAD_PAD_MS} ms, got {pad}"
            ));
        }
        for keyword in &self.keywords {
            if keyword.trim().is_empty() || keyword.trim() != keyword {
                return invalid(format!(
                    "keywords must not be empty or start or end with spaces, got {keyword:?}"
                ));
            }
            if keyword.chars().any(char::is_control) {
                return invalid(format!(
                    "keywords must not contain control characters, got {keyword:?}"
                ));
            }
        }
        let chars: usize = self.keywords.iter().map(|k| k.chars().count()).sum();
        if chars > MAX_KEYWORD_CHARS {
            return invalid(format!(
                "the keywords are {chars} characters long, more than {MAX_KEYWORD_CHARS}"
            ));
        }
        if self.dtw && self.flash_attn {
            return invalid("dtw and flash attention cannot be combined".to_owned());
        }
        Ok(())
    }
}

/// What a transcription is doing, for progress messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Another transcription is running on this machine; this one waits for it.
    Waiting,
    /// The one-transcription-at-a-time lock could not be taken; this one runs anyway.
    LockUnavailable(String),
    /// ffmpeg is decoding the audio.
    ReadingAudio,
    /// Silero is finding the speech.
    DetectingSpeech,
    /// The Whisper model is loading.
    LoadingModel,
    /// The language was detected.
    DetectedLanguage {
        /// The language.
        language: Language,
        /// Its probability against the other language, from 0.5 to 1.
        probability: f32,
    },
    /// Whisper is transcribing.
    Transcribing {
        /// Seconds of speech transcribed so far.
        done: f64,
        /// Seconds of speech in total.
        total: f64,
    },
}

/// A finished transcription.
#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    transcript: Transcript,
    report: Report,
}

impl Analysis {
    /// The word timings.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Takes the word timings.
    pub fn into_transcript(self) -> Transcript {
        self.transcript
    }

    /// How the transcription went.
    pub fn report(&self) -> &Report {
        &self.report
    }
}

/// How a transcription went: what was found, and how long each step took.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// The model that wrote the words (the Silero model with `vad_only`).
    pub model: String,
    /// The language, if known.
    pub language: Option<Language>,
    /// The probability of the detected language, if it was detected.
    pub detected: Option<f32>,
    /// The voice-activity settings used.
    pub vad: VadSettings,
    /// Seconds of audio.
    pub audio_seconds: f64,
    /// Seconds of speech found.
    pub speech_seconds: f64,
    /// Stretches of speech found.
    pub speech_segments: usize,
    /// Chunks transcribed.
    pub chunks: usize,
    /// Memory the transcription was expected to need, in bytes.
    pub memory_needed: u64,
    /// Memory available when it started, in bytes, if known.
    pub memory_available: Option<u64>,
    /// Peak resident memory of this process so far, in bytes, if known.
    pub peak_memory: Option<u64>,
    /// Time spent decoding the audio.
    pub reading_audio: Duration,
    /// Time spent finding speech.
    pub detecting_speech: Duration,
    /// Time spent loading the model.
    pub loading_model: Duration,
    /// Time spent detecting the language and transcribing.
    pub transcribing: Duration,
    /// Total time.
    pub total: Duration,
}

/// Why a transcription failed.
#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    /// The settings are not valid.
    #[error("{0}")]
    Settings(String),
    /// A model file is missing.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// The audio cannot be read.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// There is not enough free memory to transcribe safely.
    #[error("{}", not_enough_memory(.model, *.needed, *.available, *.smaller))]
    NotEnoughMemory {
        /// The model's file name.
        model: String,
        /// Bytes needed.
        needed: u64,
        /// Bytes available.
        available: u64,
        /// A smaller model to suggest, if there is one.
        smaller: Option<ModelSize>,
    },
    /// Silero failed.
    #[error("speech detection failed: {0}")]
    Vad(String),
    /// Whisper failed.
    #[error("{model}: {message}")]
    Whisper {
        /// The model's file name.
        model: String,
        /// What went wrong.
        message: String,
    },
    /// A bug: the words break the words.json rules.
    #[error("internal error, please report it: the words break the words.json rules:\n{0}")]
    Internal(ValidationErrors),
}

fn not_enough_memory(
    model: &str,
    needed: u64,
    available: u64,
    smaller: Option<ModelSize>,
) -> String {
    let alternative = smaller.map_or_else(
        || "transcribe a shorter file".to_owned(),
        |size| format!("use a smaller model with --model {}", size.name()),
    );
    format!(
        "not enough free memory for {model}: it needs about {} MiB, and {} MiB is available. \
         Close other programs (a build or a video render uses a lot), or {alternative}",
        memory::mib(needed),
        memory::mib(available),
    )
}

/// Transcribes the speech in `media`, a video or audio file that ffmpeg can read.
///
/// `progress` is called as the work goes on. On success, the transcript passes every
/// rule of words.json schema v1.
pub fn analyze(
    media: &Path,
    settings: &Settings,
    progress: &mut dyn FnMut(Progress),
) -> Result<Analysis, AnalyzeError> {
    let started = Instant::now();
    settings.check()?;
    audio::check_file(media)?;
    if !settings.whisper_logs {
        whisper_rs::install_logging_hooks();
    }
    let whisper = if settings.vad_only {
        None
    } else {
        Some(WhisperModel::find(settings)?)
    };
    let vad_model = settings.dirs.find(VAD_MODEL_FILE)?;
    if let Some(model) = &whisper {
        model.check_memory(0)?;
    }

    let _lock = match lock::acquire(|| progress(Progress::Waiting)) {
        Ok(lock) => Some(lock),
        Err(error) => {
            progress(Progress::LockUnavailable(error.to_string()));
            None
        }
    };

    progress(Progress::ReadingAudio);
    let step = Instant::now();
    let samples = audio::decode(media)?;
    let reading_audio = step.elapsed();
    let (memory_needed, memory_available) = match &whisper {
        Some(model) => model.check_memory(samples.len())?,
        None => (0, memory::available()),
    };

    progress(Progress::DetectingSpeech);
    let step = Instant::now();
    let mut vad = vad::Vad::detect(&vad_model, &samples).map_err(AnalyzeError::Vad)?;
    let detecting_speech = step.elapsed();

    let mut report = Report {
        model: models::file_name(&vad_model),
        language: settings.language,
        detected: None,
        vad: settings.vad_settings(settings.language),
        audio_seconds: seconds(samples.len()),
        speech_seconds: 0.0,
        speech_segments: 0,
        chunks: 0,
        memory_needed,
        memory_available,
        peak_memory: None,
        reading_audio,
        detecting_speech,
        loading_model: Duration::ZERO,
        transcribing: Duration::ZERO,
        total: Duration::ZERO,
    };

    let (segments, heard) = match whisper {
        None => (
            vad.segments(report.vad).map_err(AnalyzeError::Vad)?,
            Vec::new(),
        ),
        Some(model) => {
            progress(Progress::LoadingModel);
            let step = Instant::now();
            let mut engine = model.load(settings)?;
            report.loading_model = step.elapsed();
            report.model = model.name.clone();

            let step = Instant::now();
            let segments = speech_segments(
                settings,
                &mut vad,
                &mut engine,
                &samples,
                &mut report,
                progress,
            )
            .map_err(|message| model.error(message))?;
            drop(vad);
            let heard = transcribe(&mut engine, &segments, &samples, report.language, progress)
                .map_err(|message| model.error(message))?;
            report.transcribing = step.elapsed();
            (segments, heard)
        }
    };

    let chunks = chunks::pack(&segments);
    report.chunks = if report.loading_model.is_zero() {
        0
    } else {
        chunks.len()
    };
    report.speech_segments = segments.len();
    report.speech_seconds = seconds(segments.iter().map(ExactSizeIterator::len).sum());
    let speech = segments
        .iter()
        .map(|segment| SpeechSegment::new(seconds(segment.start), seconds(segment.end)))
        .collect();
    let words = assemble::finish(heard, &segments);
    let transcript = Transcript::new(
        media_name(media),
        report.language.map_or("und", Language::code),
        report.model.clone(),
        seconds(samples.len()),
        speech,
        words,
    )
    .map_err(AnalyzeError::Internal)?;
    report.peak_memory = memory::peak_rss();
    report.total = started.elapsed();
    Ok(Analysis { transcript, report })
}

/// The stretches of speech, with the language's tuned settings. When the language is
/// not given, it is detected on the first chunk found with the default settings.
fn speech_segments(
    settings: &Settings,
    vad: &mut vad::Vad,
    engine: &mut Engine,
    samples: &[f32],
    report: &mut Report,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<Range<usize>>, String> {
    let default = settings.vad_settings(None);
    let language = match settings.language {
        Some(language) => language,
        None => {
            let first = vad.segments(default)?;
            let Some(chunk) = chunks::pack(&first).into_iter().next() else {
                report.vad = default;
                return Ok(first);
            };
            let (language, probability) = engine.detect_language(&chunk.audio(samples))?;
            progress(Progress::DetectedLanguage {
                language,
                probability,
            });
            report.detected = Some(probability);
            if settings.vad_settings(Some(language)) == default {
                report.language = Some(language);
                report.vad = default;
                return Ok(first);
            }
            language
        }
    };
    report.language = Some(language);
    report.vad = settings.vad_settings(Some(language));
    vad.segments(report.vad)
}

fn transcribe(
    engine: &mut Engine,
    segments: &[Range<usize>],
    samples: &[f32],
    language: Option<Language>,
    progress: &mut dyn FnMut(Progress),
) -> Result<Vec<HeardWord>, String> {
    let chunks = chunks::pack(segments);
    let total = seconds(chunks.iter().map(Chunk::speech_len).sum());
    let mut done = 0;
    let mut heard = Vec::new();
    for chunk in &chunks {
        progress(Progress::Transcribing {
            done: seconds(done),
            total,
        });
        let audio = chunk.audio(samples);
        for word in engine.transcribe(&audio, language.unwrap_or(Language::En))? {
            if let Some(placed) = chunk.place(word.start, word.end) {
                heard.push(HeardWord {
                    text: word.text,
                    segment: placed.segment,
                    start: placed.start,
                    end: placed.end,
                    probability: word.probability,
                });
            }
        }
        done += chunk.speech_len();
    }
    progress(Progress::Transcribing { done: total, total });
    Ok(heard)
}

/// The Whisper model of a transcription.
struct WhisperModel {
    path: PathBuf,
    name: String,
    size: Option<ModelSize>,
    bytes: u64,
}

impl WhisperModel {
    fn find(settings: &Settings) -> Result<Self, AnalyzeError> {
        let (path, size) = match &settings.model {
            ModelChoice::Named(size) => (settings.dirs.find(size.file_name())?, Some(*size)),
            ModelChoice::File(path) => {
                if !path.is_file() {
                    return Err(ModelError::MissingFile(path.clone()).into());
                }
                let name = models::file_name(path);
                (path.clone(), ModelSize::from_file_name(&name))
            }
        };
        if settings.dtw && size.is_none() {
            return Err(AnalyzeError::Settings(format!(
                "dtw needs a model whose name says its size (tiny, base or small), got {}",
                path.display()
            )));
        }
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            name: models::file_name(&path),
            path,
            size,
            bytes,
        })
    }

    /// Fails if the machine lacks the memory for this model and `samples` of audio.
    /// Returns the bytes needed and available.
    fn check_memory(&self, samples: usize) -> Result<(u64, Option<u64>), AnalyzeError> {
        let needed = memory::needed(self.bytes, samples);
        let available = memory::available();
        if let Some(available) = available
            && available < needed
        {
            return Err(AnalyzeError::NotEnoughMemory {
                model: self.name.clone(),
                needed,
                available,
                smaller: match self.size {
                    Some(size) => size.smaller(),
                    None => Some(ModelSize::Base),
                },
            });
        }
        Ok((needed, available))
    }

    fn load(&self, settings: &Settings) -> Result<Engine, AnalyzeError> {
        let options = transcribe::Options {
            threads: settings.threads,
            flash_attn: settings.flash_attn,
            dtw: self.size.filter(|_| settings.dtw),
        };
        let mut engine =
            Engine::load(&self.path, options).map_err(|message| self.error(message))?;
        engine
            .set_keywords(&settings.keywords)
            .map_err(AnalyzeError::Settings)?;
        Ok(engine)
    }

    fn error(&self, message: String) -> AnalyzeError {
        AnalyzeError::Whisper {
            model: self.name.clone(),
            message,
        }
    }
}

/// The media file's name as written in words.json: no folders, and no control
/// characters or backslashes.
fn media_name(media: &Path) -> String {
    let name: String = models::file_name(media)
        .chars()
        .map(|c| {
            if c.is_control() || c == '\\' || c == '/' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "media".to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked() {
        let mut settings = Settings::new(ModelDirs::from_lookup(|_| None));
        assert!(settings.check().is_ok());
        settings.threads = 0;
        assert!(settings.check().is_err());
        settings.threads = 4;
        settings.keywords = vec!["Supabase".to_owned(), " Rust".to_owned()];
        assert!(settings.check().is_err());
        settings.keywords = vec!["Supa\0base".to_owned()];
        assert!(settings.check().is_err());
        settings.keywords = vec!["x".repeat(MAX_KEYWORD_CHARS + 1)];
        assert!(settings.check().is_err());
        settings.keywords.clear();
        settings.vad_threshold = Some(1.5);
        assert!(settings.check().is_err());
        settings.vad_threshold = Some(0.35);
        settings.vad_pad_ms = Some(250);
        assert!(settings.check().is_ok());
        let vad = settings.vad_settings(Some(Language::Fr));
        assert_eq!((vad.threshold, vad.pad_ms), (0.35, 250));
    }

    #[test]
    fn media_names_have_no_folders_or_control_characters() {
        assert_eq!(media_name(Path::new("raw/IMG_0412.mov")), "IMG_0412.mov");
        assert_eq!(media_name(Path::new("a\nb\\c.mp4")), "a_b_c.mp4");
    }

    #[test]
    fn missing_memory_suggests_a_smaller_model() {
        let message = not_enough_memory(
            "ggml-small-q5_1.bin",
            700 * memory::MIB,
            512 * memory::MIB,
            Some(ModelSize::Base),
        );
        assert!(
            message.contains("needs about 700 MiB, and 512 MiB is available"),
            "{message}"
        );
        assert!(message.contains("--model base"), "{message}");
    }
}
