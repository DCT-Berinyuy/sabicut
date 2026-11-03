//! One transcription at a time on this machine. A transcription takes most of the
//! memory a small laptop has, so a second `sabicut analyze` waits for the first.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::PathBuf;

/// Held while a transcription runs; dropping it lets the next one start.
#[derive(Debug)]
pub(crate) struct TranscriptionLock {
    _file: File,
}

/// Waits until no other transcription is running, calling `on_wait` once if it has to.
pub(crate) fn acquire(on_wait: impl FnOnce()) -> io::Result<TranscriptionLock> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path())?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            on_wait();
            file.lock()?;
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    Ok(TranscriptionLock { _file: file })
}

fn path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    match runtime {
        Some(dir) => dir.join("sabicut-analyze.lock"),
        None => {
            let user: String = std::env::var("USER")
                .unwrap_or_default()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            std::env::temp_dir().join(format!("sabicut-analyze-{user}.lock"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_waits_for_the_first() {
        let first = acquire(|| {}).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let mut waited = false;
            let _second = acquire(|| waited = true).unwrap();
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
}
