//! Where Sabicut looks for its speech models. It never downloads them.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// File name of the Silero voice-activity model (v6.2.0, whisper.cpp's ggml format).
pub const VAD_MODEL_FILE: &str = "ggml-silero-v6.2.0.bin";

/// Environment variable naming a model folder that is searched first.
pub const MODELS_ENV: &str = "SABICUT_MODELS";

/// The Whisper models Sabicut knows by name, all quantized to q5_1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelSize {
    /// `ggml-tiny-q5_1.bin`, 32 MB: fast, but French is unusable.
    Tiny,
    /// `ggml-base-q5_1.bin`, 60 MB: good English, weak French.
    Base,
    /// `ggml-small-q5_1.bin`, 190 MB: the default, and the only one with usable French.
    Small,
}

impl ModelSize {
    /// Every named model, smallest first.
    pub const ALL: [Self; 3] = [Self::Tiny, Self::Base, Self::Small];

    /// The name used on the command line, like "small".
    pub fn name(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Base => "base",
            Self::Small => "small",
        }
    }

    /// The model's file name, like "ggml-small-q5_1.bin".
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Tiny => "ggml-tiny-q5_1.bin",
            Self::Base => "ggml-base-q5_1.bin",
            Self::Small => "ggml-small-q5_1.bin",
        }
    }

    /// The next smaller model, if there is one.
    pub fn smaller(self) -> Option<Self> {
        match self {
            Self::Tiny => None,
            Self::Base => Some(Self::Tiny),
            Self::Small => Some(Self::Base),
        }
    }

    /// Guesses the size of a model from its file name, like "ggml-small.en-q8_0.bin".
    pub fn from_file_name(name: &str) -> Option<Self> {
        name.split(|c: char| !c.is_ascii_alphanumeric())
            .find_map(|part| Self::ALL.into_iter().find(|size| size.name() == part))
    }
}

/// Which Whisper model to use: one of the named models, or a model file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelChoice {
    /// A named model, looked up in the model folders.
    Named(ModelSize),
    /// A ggml model file anywhere on disk.
    File(PathBuf),
}

impl Default for ModelChoice {
    fn default() -> Self {
        Self::Named(ModelSize::Small)
    }
}

impl FromStr for ModelChoice {
    type Err = String;

    /// "tiny", "base" and "small" name a model; anything else is a file path.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.trim().is_empty() {
            return Err("must be tiny, base, small, or the path of a ggml model file".to_owned());
        }
        Ok(ModelSize::ALL
            .into_iter()
            .find(|size| size.name() == text)
            .map_or_else(|| Self::File(PathBuf::from(text)), Self::Named))
    }
}

/// The folders searched for models, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDirs(Vec<PathBuf>);

impl ModelDirs {
    /// The folders named by this process's environment: `$SABICUT_MODELS`, then
    /// `$XDG_DATA_HOME/sabicut/models`, then `~/.local/share/sabicut/models`.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// The same folders, with environment variables read through `lookup`.
    ///
    /// Empty variables are ignored, as is an `XDG_DATA_HOME` that is not an absolute
    /// path (as the XDG specification requires). A folder is listed once.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let var = |key: &str| lookup(key).filter(|value| !value.is_empty());
        let mut dirs = Vec::new();
        if let Some(dir) = var(MODELS_ENV) {
            dirs.push(PathBuf::from(dir));
        }
        if let Some(data) = var("XDG_DATA_HOME").map(PathBuf::from)
            && data.is_absolute()
        {
            dirs.push(data.join("sabicut").join("models"));
        }
        if let Some(home) = var("HOME") {
            dirs.push(
                PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("sabicut")
                    .join("models"),
            );
        }
        let mut unique: Vec<PathBuf> = Vec::with_capacity(dirs.len());
        for dir in dirs {
            if !unique.contains(&dir) {
                unique.push(dir);
            }
        }
        Self(unique)
    }

    /// The folders, in search order.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.0
    }

    /// The first existing file called `file_name` in the folders.
    pub fn find(&self, file_name: &str) -> Result<PathBuf, ModelError> {
        let candidates: Vec<PathBuf> = self.0.iter().map(|dir| dir.join(file_name)).collect();
        candidates
            .iter()
            .find(|path| path.is_file())
            .cloned()
            .ok_or_else(|| ModelError::NotFound {
                file: file_name.to_owned(),
                candidates,
            })
    }
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// A model file that is not where it should be.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// A named model (or the voice-activity model) is in none of the model folders.
    #[error("{}", not_found(.file, .candidates))]
    NotFound {
        /// The file that was looked for.
        file: String,
        /// Every path that was tried.
        candidates: Vec<PathBuf>,
    },
    /// The model file given on the command line does not exist.
    #[error("model file {} not found", .0.display())]
    MissingFile(PathBuf),
}

fn not_found(file: &str, candidates: &[PathBuf]) -> String {
    if candidates.is_empty() {
        return format!(
            "model {file} not found: no model folder is known. Set {MODELS_ENV} to the folder \
             that has it"
        );
    }
    let mut message = format!("model {file} not found. Sabicut looked for:");
    for path in candidates {
        let _ = write!(message, "\n  {}", path.display());
    }
    let _ = write!(
        message,
        "\nSabicut never downloads models: copy the file to one of these paths, or set \
         {MODELS_ENV} to the folder that has it"
    );
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(vars: &[(&str, &str)]) -> ModelDirs {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        ModelDirs::from_lookup(|key| {
            vars.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| OsString::from(v))
        })
    }

    #[test]
    fn folders_are_searched_in_order_without_duplicates() {
        let found = dirs(&[
            ("SABICUT_MODELS", "/opt/models"),
            ("XDG_DATA_HOME", "/home/u/.local/share"),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(
            found.dirs(),
            [
                PathBuf::from("/opt/models"),
                PathBuf::from("/home/u/.local/share/sabicut/models"),
            ]
        );
    }

    #[test]
    fn empty_and_relative_variables_are_ignored() {
        let found = dirs(&[
            ("SABICUT_MODELS", ""),
            ("XDG_DATA_HOME", "relative/data"),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(
            found.dirs(),
            [PathBuf::from("/home/u/.local/share/sabicut/models")]
        );
        assert!(dirs(&[]).dirs().is_empty());
    }

    #[test]
    fn missing_models_name_every_path_tried() {
        let found = dirs(&[
            ("SABICUT_MODELS", "/nonexistent/a"),
            ("HOME", "/nonexistent/h"),
        ]);
        let message = found.find("ggml-small-q5_1.bin").unwrap_err().to_string();
        assert!(
            message.contains("/nonexistent/a/ggml-small-q5_1.bin\n"),
            "{message}"
        );
        assert!(
            message.contains("/nonexistent/h/.local/share/sabicut/models/ggml-small-q5_1.bin"),
            "{message}"
        );
        assert!(message.contains("never downloads"), "{message}");
        assert!(
            dirs(&[])
                .find("x.bin")
                .unwrap_err()
                .to_string()
                .contains("SABICUT_MODELS")
        );
    }

    #[test]
    fn model_names_and_paths_are_told_apart() {
        assert_eq!(
            "small".parse::<ModelChoice>(),
            Ok(ModelChoice::Named(ModelSize::Small))
        );
        assert_eq!(
            "./small".parse::<ModelChoice>(),
            Ok(ModelChoice::File(PathBuf::from("./small")))
        );
        assert!("".parse::<ModelChoice>().is_err());
        assert_eq!(
            ModelSize::from_file_name("ggml-small.en-q8_0.bin"),
            Some(ModelSize::Small)
        );
        assert_eq!(ModelSize::from_file_name("ggml-database.bin"), None);
        assert_eq!(ModelSize::Small.smaller(), Some(ModelSize::Base));
    }
}
