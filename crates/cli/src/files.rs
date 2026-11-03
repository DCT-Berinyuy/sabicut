//! Reading and writing the files of `sabicut analyze` and `sabicut fix`.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// words.json files are rarely more than a few megabytes, even for hours of speech.
pub(crate) const MAX_WORDS_BYTES: usize = 64 * 1024 * 1024;
/// fixes.toml files are small text files, like plans.
pub(crate) const MAX_FIXES_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileError {
    #[error("{}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("{}: file is larger than {} MiB, so it is not a {kind}", .path.display(), .limit / 1024 / 1024)]
    TooLarge {
        path: PathBuf,
        limit: usize,
        kind: &'static str,
    },
    #[error("cannot write {}: {source}", .path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("cannot write {}: the folder does not exist", .0.display())]
    NoFolder(PathBuf),
}

/// Reads a UTF-8 text file of at most `limit` bytes. `kind` names what it should be,
/// like "words.json".
pub(crate) fn read_text(
    path: &Path,
    limit: usize,
    kind: &'static str,
) -> Result<String, FileError> {
    let read_error = |source| FileError::Read {
        path: path.to_owned(),
        source,
    };
    let mut text = String::new();
    File::open(path)
        .map_err(read_error)?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(read_error)?;
    if text.len() > limit {
        return Err(FileError::TooLarge {
            path: path.to_owned(),
            limit,
            kind,
        });
    }
    Ok(text)
}

/// Fails unless the folder that `path` would be written to exists.
pub(crate) fn check_folder(path: &Path) -> Result<(), FileError> {
    let folder = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if folder.is_dir() {
        Ok(())
    } else {
        Err(FileError::NoFolder(path.to_owned()))
    }
}

/// Writes `contents` to `path` through a temporary file in the same folder, so an
/// interrupted run never leaves half a file behind.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), FileError> {
    let write_error = |source| FileError::Write {
        path: path.to_owned(),
        source,
    };
    check_folder(path)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp-{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let written = File::create(&temporary).and_then(|mut file| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = renamed {
        // The temporary file may not exist; either way the write failed.
        let _ = fs::remove_file(&temporary);
        return Err(write_error(error));
    }
    Ok(())
}

/// Whether two paths name the same existing file.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
