//! A scratch directory for intermediate files, removed when done.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::RenderError;

/// A hidden scratch directory, created next to the outputs rather than in the system
/// temp directory: `/tmp` is often in RAM, and the intermediate audio of a long video
/// would count against the memory budget. Removed (with its contents) when dropped,
/// whether the work succeeded or not.
#[derive(Debug)]
pub(crate) struct WorkDir {
    path: PathBuf,
}

impl WorkDir {
    /// Creates a new scratch directory inside `parent`.
    pub(crate) fn new(parent: &Path) -> Result<Self, RenderError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let path = parent.join(format!(".sabicut-work-{}-{nanos:09}", std::process::id()));
        fs::create_dir(&path).map_err(RenderError::io(&path))?;
        Ok(Self { path })
    }

    /// The path of `name` inside the directory.
    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Writes `text` to `name` inside the directory and returns its path.
    pub(crate) fn write(&self, name: &str, text: &str) -> Result<PathBuf, RenderError> {
        let path = self.file(name);
        fs::write(&path, text).map_err(RenderError::io(&path))?;
        Ok(path)
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        // Best effort: a leftover scratch directory is harmless, and there is no one to
        // report the error to while unwinding or returning another error.
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_directory_is_removed_with_its_files() {
        let parent = std::env::temp_dir();
        let work = WorkDir::new(&parent).unwrap();
        let file = work.write("graph.txt", "null").unwrap();
        assert!(file.exists());
        let dir = file.parent().unwrap().to_owned();
        drop(work);
        assert!(!dir.exists());
    }
}
