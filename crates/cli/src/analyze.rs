//! `sabicut analyze`: transcribe the speech in a video or audio file to word timings
//! (words.json).

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, ValueEnum};
use sabicut_analyze::{
    Analysis, AnalyzeError, Language, MAX_THREADS, ModelChoice, ModelDirs, Progress, Settings,
};

use crate::CliError;
use crate::files::{self, FileError};

/// Set to any value to let whisper.cpp print its own log lines to standard error.
const WHISPER_LOG_ENV: &str = "SABICUT_WHISPER_LOG";

#[derive(Debug, Args)]
pub(crate) struct AnalyzeArgs {
    /// Video or audio file to transcribe (any format ffmpeg reads)
    media: PathBuf,
    /// Where to write the word timings, or "-" for standard output
    /// [default: next to the media file, as <name>.words.json]
    #[arg(short, long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Speech model: tiny, base, small, or the path of a ggml model file
    #[arg(long, default_value = "small", value_name = "MODEL")]
    model: ModelChoice,
    /// Spoken language. Pidgin is transcribed as English
    #[arg(long, value_enum, default_value_t = Lang::Auto)]
    lang: Lang,
    /// CPU threads for the speech model
    #[arg(long, default_value_t = 4, value_name = "N",
          value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_THREADS)))]
    threads: u16,
    /// Names and terms to spell right, separated by commas (a hint for the speech model)
    #[arg(long, value_delimiter = ',', value_name = "WORDS")]
    keywords: Vec<String>,
    /// Speech probability threshold, between 0 and 1 [default: tuned per language]
    #[arg(long, value_name = "P")]
    vad_threshold: Option<f32>,
    /// Audio kept around each stretch of speech, in milliseconds [default: tuned per
    /// language]
    #[arg(long, value_name = "MS")]
    vad_pad: Option<u32>,
    /// Only find the stretches of speech, without transcribing them
    #[arg(long, hide = true)]
    vad_only: bool,
    /// Word starts from whisper.cpp's dynamic time warping (experimental)
    #[arg(long, hide = true)]
    dtw: bool,
    /// whisper.cpp's flash attention (experimental)
    #[arg(long, hide = true)]
    flash_attn: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Lang {
    /// Tell English from French automatically
    Auto,
    /// English
    En,
    /// French
    Fr,
    /// Cameroonian Pidgin English, transcribed as English
    Pidgin,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
    #[error(transparent)]
    File(#[from] FileError),
    #[error("cannot write the word timings: {0}")]
    Json(String),
}

pub(crate) fn run(args: &AnalyzeArgs, out: &mut impl Write) -> Result<(), CliError> {
    let destination = match &args.out {
        Some(path) if path.as_os_str() == "-" => None,
        Some(path) => Some(path.clone()),
        None => Some(args.media.with_extension("words.json")),
    };
    if let Some(path) = &destination {
        files::check_folder(path).map_err(Error::from)?;
    }

    let mut progress = ProgressLine::new();
    let result = sabicut_analyze::analyze(&args.media, &settings(args), &mut |event| {
        progress.show(&event);
    });
    progress.clear();
    let analysis = result.map_err(Error::from)?;
    let json = analysis
        .transcript()
        .to_json()
        .map_err(|error| Error::Json(error.to_string()))?;

    match &destination {
        None => {
            out.write_all(json.as_bytes())?;
            out.flush()?;
            eprint!("{}", summary("standard output", &analysis));
        }
        Some(path) => {
            files::write_atomic(path, &json).map_err(Error::from)?;
            write!(out, "{}", summary(&path.display().to_string(), &analysis))?;
        }
    }
    Ok(())
}

fn settings(args: &AnalyzeArgs) -> Settings {
    let mut settings = Settings::new(ModelDirs::from_env());
    settings.model = args.model.clone();
    settings.language = match args.lang {
        Lang::Auto => None,
        Lang::En | Lang::Pidgin => Some(Language::En),
        Lang::Fr => Some(Language::Fr),
    };
    settings.threads = args.threads;
    settings.keywords = args
        .keywords
        .iter()
        .map(|keyword| keyword.trim())
        .filter(|keyword| !keyword.is_empty())
        .map(str::to_owned)
        .collect();
    settings.vad_threshold = args.vad_threshold;
    settings.vad_pad_ms = args.vad_pad;
    settings.vad_only = args.vad_only;
    settings.dtw = args.dtw;
    settings.flash_attn = args.flash_attn;
    settings.whisper_logs = std::env::var_os(WHISPER_LOG_ENV).is_some_and(|v| !v.is_empty());
    settings
}

/// What was written, what was found, and how long it took.
fn summary(destination: &str, analysis: &Analysis) -> String {
    let transcript = analysis.transcript();
    let report = analysis.report();
    let language = match (report.language, report.detected) {
        (Some(language), Some(p)) => {
            format!("{} (detected, {:.0}% sure)", language.code(), p * 100.0)
        }
        (Some(language), None) => language.code().to_owned(),
        (None, _) => "language unknown".to_owned(),
    };
    let contents =
        if report.chunks == 0 && transcript.words().is_empty() && report.speech_segments > 0 {
            "speech only, no words".to_owned()
        } else {
            crate::count(transcript.words().len(), "word")
        };
    let memory = report
        .peak_memory
        .map(|bytes| format!("; peak memory {} MiB", bytes.div_ceil(1024 * 1024)))
        .unwrap_or_default();
    let mut text = format!(
        "wrote {destination}: {contents}, {language}, {}\n",
        report.model
    );
    text.push_str(&format!(
        "  audio {:.1} s, speech {:.1} s in {} (padding {} ms, threshold {:.2})\n",
        report.audio_seconds,
        report.speech_seconds,
        crate::count(report.speech_segments, "stretch"),
        report.vad.pad_ms,
        report.vad.threshold,
    ));
    text.push_str(&format!(
        "  took {}: audio {}, speech detection {}, model {}, transcription {} ({}){memory}\n",
        secs(report.total),
        secs(report.reading_audio),
        secs(report.detecting_speech),
        secs(report.loading_model),
        secs(report.transcribing),
        crate::count(report.chunks, "chunk"),
    ));
    if transcript.words().is_empty() && report.speech_segments == 0 {
        text.push_str("  no speech found\n");
    }
    text
}

fn secs(duration: Duration) -> String {
    format!("{:.1} s", duration.as_secs_f64())
}

/// Progress on standard error: one updating line on a terminal, plain lines otherwise.
struct ProgressLine {
    terminal: bool,
    shown: bool,
}

impl ProgressLine {
    fn new() -> Self {
        Self {
            terminal: io::stderr().is_terminal(),
            shown: false,
        }
    }

    fn show(&mut self, event: &Progress) {
        let text = match event {
            Progress::Waiting => "waiting for another transcription to finish...".to_owned(),
            Progress::LockUnavailable(error) => {
                self.clear();
                eprintln!(
                    "warning: cannot make sure only one transcription runs at a time ({error})"
                );
                return;
            }
            Progress::ReadingAudio => "reading the audio...".to_owned(),
            Progress::DetectingSpeech => "finding speech...".to_owned(),
            Progress::LoadingModel => "loading the speech model...".to_owned(),
            Progress::DetectedLanguage {
                language,
                probability,
            } => format!(
                "language: {} ({:.0}% sure)",
                language.code(),
                probability * 100.0
            ),
            Progress::Transcribing { done, total } => {
                if !self.terminal && *done > 0.0 {
                    return;
                }
                let percent = if *total > 0.0 {
                    done / total * 100.0
                } else {
                    100.0
                };
                format!("transcribing {total:.0} s of speech: {percent:.0}%")
            }
        };
        if self.terminal {
            // Carriage return and "erase line", so the line updates in place.
            eprint!("\r\x1b[2K{text}");
            let _ = io::stderr().flush();
            self.shown = true;
        } else {
            eprintln!("{text}");
        }
    }

    fn clear(&mut self) {
        if self.shown {
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
            self.shown = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn the_default_output_sits_next_to_the_media() {
        assert_eq!(
            Path::new("raw/IMG_0412.mov").with_extension("words.json"),
            Path::new("raw/IMG_0412.words.json")
        );
        assert_eq!(
            Path::new("clip").with_extension("words.json"),
            Path::new("clip.words.json")
        );
    }
}
