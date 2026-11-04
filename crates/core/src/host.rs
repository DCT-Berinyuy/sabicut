//! The machine Sabicut runs on: one heavy job at a time, and which ffmpeg to run.
//!
//! Transcription (`sabicut analyze`) and rendering (`sabicut render`) each take most of
//! the memory and CPU a small laptop has, so they take the same [`JobLock`] and run one
//! at a time (PRD H4). Both run the ffmpeg chosen by [`ffmpeg_program`].

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// Environment variable naming the ffmpeg binary to use instead of `ffmpeg` on `PATH`.
pub const FFMPEG_ENV: &str = "SABICUT_FFMPEG";
/// Environment variable naming the ffprobe binary. Without it, the `ffprobe` next to
/// the chosen ffmpeg is used, or `ffprobe` on `PATH`.
pub const FFPROBE_ENV: &str = "SABICUT_FFPROBE";

/// The ffmpeg binary to run: [`FFMPEG_ENV`] if it is set, else `ffmpeg` on `PATH`.
pub fn ffmpeg_program() -> PathBuf {
    std::env::var_os(FFMPEG_ENV)
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from)
}

/// The ffprobe binary to run: [`FFPROBE_ENV`] if it is set, else the `ffprobe` next to
/// [`ffmpeg_program`] when that is a path, else `ffprobe` on `PATH`.
pub fn ffprobe_program() -> PathBuf {
    std::env::var_os(FFPROBE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_ffprobe(&ffmpeg_program()))
}

/// `ffprobe` in the same directory as `ffmpeg` when `ffmpeg` is a path, else `ffprobe`.
fn sibling_ffprobe(ffmpeg: &Path) -> PathBuf {
    let has_dir = ffmpeg
        .parent()
        .is_some_and(|dir| !dir.as_os_str().is_empty());
    if !has_dir {
        return PathBuf::from("ffprobe");
    }
    let name = match ffmpeg.extension() {
        Some(ext) => format!("ffprobe.{}", ext.to_string_lossy()),
        None => "ffprobe".to_owned(),
    };
    ffmpeg.with_file_name(name)
}

/// Held while a heavy job (a transcription or a render) runs; dropping it lets the
/// next one start.
#[derive(Debug)]
pub struct JobLock {
    _file: File,
}

/// Waits until no other heavy Sabicut job runs on this machine, calling `on_wait` once
/// if it has to wait. The lock is a file lock, so it is released when the process ends,
/// however it ends.
pub fn acquire_job_lock(on_wait: impl FnOnce()) -> io::Result<JobLock> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path())?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            on_wait();
            file.lock()?;
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    Ok(JobLock { _file: file })
}

fn lock_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    match runtime {
        Some(dir) => dir.join("sabicut-jobs.lock"),
        None => {
            let user: String = std::env::var("USER")
                .unwrap_or_default()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            std::env::temp_dir().join(format!("sabicut-jobs-{user}.lock"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_waits_for_the_first() {
        let first = acquire_job_lock(|| {}).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let mut waited = false;
            let _second = acquire_job_lock(|| waited = true).unwrap();
            sender.send(waited).unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            receiver.try_recv().is_err(),
            "the second holder did not wait"
        );
        drop(first);
        assert!(receiver.recv().unwrap());
        waiter.join().unwrap();
    }

    #[test]
    fn ffprobe_is_found_next_to_an_explicit_ffmpeg() {
        assert_eq!(
            sibling_ffprobe(Path::new("/opt/ff/bin/ffmpeg")),
            Path::new("/opt/ff/bin/ffprobe")
        );
        assert_eq!(
            sibling_ffprobe(Path::new("C:/ff/ffmpeg.exe")),
            Path::new("C:/ff/ffprobe.exe")
        );
        assert_eq!(sibling_ffprobe(Path::new("ffmpeg")), Path::new("ffprobe"));
    }
}
