//! `sabicut fix`: apply transcript fixes (fixes.toml) to word timings (words.json) and
//! show what changes. The input words.json is never changed: fixes are reapplied
//! whenever the words are used, so the raw transcript stays available.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use sabicut_core::{
    FixError, Fixed, Fixes, FixesParseError, Transcript, TranscriptParseError, ValidationErrors,
    apply_fixes,
};

use crate::files::{self, FileError, MAX_FIXES_BYTES, MAX_WORDS_BYTES};
use crate::{CliError, bullets, count};

#[derive(Debug, Args)]
pub(crate) struct FixArgs {
    /// Word timings (words.json)
    words: PathBuf,
    /// Replacement rules (fixes.toml)
    fixes: PathBuf,
    /// Also write the fixed word timings to FILE, or "-" for standard output. The input
    /// words.json is never changed
    #[arg(short, long, value_name = "FILE")]
    out: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    File(#[from] FileError),
    #[error("{}: {source}", .path.display())]
    Words {
        path: PathBuf,
        source: TranscriptParseError,
    },
    #[error("{}: {}\n{}", .path.display(), count(.errors.as_slice().len(), "problem"), bullets(.errors))]
    InvalidWords {
        path: PathBuf,
        errors: ValidationErrors,
    },
    #[error("{}: {source}", .path.display())]
    Fixes {
        path: PathBuf,
        source: FixesParseError,
    },
    #[error("{}: {}\n{}", .path.display(), count(.errors.as_slice().len(), "problem"), bullets(.errors))]
    InvalidFixes {
        path: PathBuf,
        errors: ValidationErrors,
    },
    #[error("{}: {source}", .path.display())]
    Apply { path: PathBuf, source: FixError },
    #[error(
        "{} is the input words.json: write the fixed words to another file, so the raw \
         transcript stays available when the fixes change",
        .0.display()
    )]
    WouldOverwriteInput(PathBuf),
    #[error("cannot write the fixed words: {0}")]
    Json(String),
}

pub(crate) fn run(args: &FixArgs, out: &mut impl Write) -> Result<(), CliError> {
    let to_stdout = args.out.as_deref() == Some(Path::new("-"));
    if let Some(path) = args.out.as_deref().filter(|_| !to_stdout) {
        if files::same_file(path, &args.words) {
            return Err(Error::WouldOverwriteInput(path.to_owned()).into());
        }
        files::check_folder(path).map_err(Error::from)?;
    }
    let transcript = load_words(&args.words)?;
    let fixes = load_fixes(&args.fixes)?;
    let fixed = apply(&transcript, &fixes, &args.fixes)?;

    let report = report(args, &fixes, &fixed);
    if to_stdout {
        out.write_all(json(&fixed)?.as_bytes())?;
        eprint!("{report}");
    } else {
        out.write_all(report.as_bytes())?;
        if let Some(path) = &args.out {
            files::write_atomic(path, &json(&fixed)?).map_err(Error::from)?;
            writeln!(
                out,
                "wrote {} ({})",
                path.display(),
                count(fixed.transcript().words().len(), "word")
            )?;
        }
    }
    Ok(())
}

/// Applies `fixes`, read from `path`, to the words.
pub(crate) fn apply(transcript: &Transcript, fixes: &Fixes, path: &Path) -> Result<Fixed, Error> {
    apply_fixes(transcript, fixes).map_err(|source| match source {
        FixError::Invalid(errors) => Error::InvalidFixes {
            path: path.to_owned(),
            errors,
        },
        source => Error::Apply {
            path: path.to_owned(),
            source,
        },
    })
}

/// Reads and validates a words.json file.
pub(crate) fn load_words(path: &Path) -> Result<Transcript, Error> {
    let text = files::read_text(path, MAX_WORDS_BYTES, "words.json")?;
    let transcript = Transcript::from_json_str(&text).map_err(|source| Error::Words {
        path: path.to_owned(),
        source,
    })?;
    transcript
        .validate()
        .map_err(|errors| Error::InvalidWords {
            path: path.to_owned(),
            errors,
        })?;
    Ok(transcript)
}

/// Reads a fixes.toml file.
pub(crate) fn load_fixes(path: &Path) -> Result<Fixes, Error> {
    let text = files::read_text(path, MAX_FIXES_BYTES, "fixes.toml")?;
    Fixes::from_toml_str(&text).map_err(|source| Error::Fixes {
        path: path.to_owned(),
        source,
    })
}

fn json(fixed: &Fixed) -> Result<String, Error> {
    fixed
        .transcript()
        .to_json()
        .map_err(|error| Error::Json(error.to_string()))
}

/// Every change with its time and rules, then the rules that changed nothing.
fn report(args: &FixArgs, fixes: &Fixes, fixed: &Fixed) -> String {
    let changes = fixed.changes();
    let mut text = format!("{}: ", args.words.display());
    if changes.is_empty() {
        text.push_str("no changes");
    } else {
        text.push_str(&count(changes.len(), "change"));
    }
    let _ = writeln!(text, " from {}", args.fixes.display());
    for change in changes {
        let rules: Vec<String> = change
            .rules()
            .iter()
            .map(|rule| format!("fix[{rule}]"))
            .collect();
        let _ = writeln!(
            text,
            "  {:>9.3} - {:>9.3}  \"{}\" -> \"{}\"  ({})",
            change.start(),
            change.end(),
            change.before(),
            change.after(),
            rules.join(", ")
        );
    }
    let used: BTreeSet<usize> = changes
        .iter()
        .flat_map(|change| change.rules().iter().copied())
        .collect();
    for (index, rule) in fixes.rules().iter().enumerate() {
        if !used.contains(&index) {
            let _ = writeln!(
                text,
                "note: fix[{index}] changed nothing (\"{}\" -> \"{}\")",
                rule.from(),
                rule.to()
            );
        }
    }
    text
}
