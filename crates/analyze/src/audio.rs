//! Reading the audio of a media file with ffmpeg, as 16 kHz mono samples.
//!
//! ffmpeg runs as a subprocess, may only open local files (so a playlist that points
//! at the network cannot make it go online), and writes raw 32-bit float samples to a
//! pipe.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;

/// Samples per second of the audio that the models hear.
pub const SAMPLE_RATE: usize = 16_000;

/// Longer recordings are refused, so a wrong file cannot fill the memory.
pub(crate) const MAX_SECONDS: usize = 3 * 60 * 60;

const READ_BUFFER: usize = 64 * 1024;
const MAX_MESSAGE: u64 = 16 * 1024;

/// Why the audio could not be read.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The media file cannot be opened.
    #[error("{}: {source}", .path.display())]
    Open {
        /// The media file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The path is a folder or something else that is not a file.
    #[error("{}: not a file", .0.display())]
    NotAFile(PathBuf),
    /// ffmpeg is not installed: not on the `PATH`, or not where `SABICUT_FFMPEG` says.
    #[error(
        "ffmpeg not found: Sabicut uses it to read audio, so install it (for example with \
         your package manager), or set SABICUT_FFMPEG to its path, and try again"
    )]
    FfmpegMissing,
    /// ffmpeg could not be started.
    #[error("cannot run ffmpeg: {0}")]
    Spawn(io::Error),
    /// The file has no audio track.
    #[error("{}: no audio track", .0.display())]
    NoAudioTrack(PathBuf),
    /// ffmpeg failed, for example because the file is not a media file.
    #[error("{}: ffmpeg cannot read the audio ({status}): {message}", .path.display())]
    Ffmpeg {
        /// The media file.
        path: PathBuf,
        /// How ffmpeg exited.
        status: ExitStatus,
        /// What ffmpeg said, possibly empty.
        message: String,
    },
    /// Reading ffmpeg's output failed.
    #[error("{}: reading the audio from ffmpeg failed: {source}", .path.display())]
    Read {
        /// The media file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The audio track is empty.
    #[error("{}: the audio track is empty", .0.display())]
    Empty(PathBuf),
    /// The recording is longer than [`MAX_SECONDS`].
    #[error("{}: longer than {} hours; split it into shorter files first", .0.display(), MAX_SECONDS / 3600)]
    TooLong(PathBuf),
}

/// Checks that `path` is an existing file, before any slow work starts.
pub(crate) fn check_file(path: &Path) -> Result<(), AudioError> {
    let metadata = std::fs::metadata(path).map_err(|source| AudioError::Open {
        path: path.to_owned(),
        source,
    })?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(AudioError::NotAFile(path.to_owned()))
    }
}

/// Decodes the first audio track of `path` to 16 kHz mono samples.
pub fn decode(path: &Path) -> Result<Vec<f32>, AudioError> {
    check_file(path)?;
    let mut input = OsString::from("file:");
    input.push(path.as_os_str());
    let mut child = Command::new(sabicut_core::host::ffmpeg_program())
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(["-protocol_whitelist", "file", "-i"])
        .arg(&input)
        .args([
            "-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-ar", "16000",
        ])
        .args(["-c:a", "pcm_f32le", "-f", "f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => AudioError::FfmpegMissing,
            _ => AudioError::Spawn(error),
        })?;

    let stderr = child.stderr.take();
    let messages = thread::spawn(move || stderr.map(read_message).unwrap_or_default());
    let samples = match child.stdout.take() {
        Some(stdout) => read_samples(stdout, SAMPLE_RATE * MAX_SECONDS),
        None => Err(io::Error::other("ffmpeg's output is not connected")),
    };
    let samples = match samples {
        Ok(Some(samples)) => samples,
        Ok(None) => {
            stop(&mut child);
            return Err(AudioError::TooLong(path.to_owned()));
        }
        Err(source) => {
            stop(&mut child);
            return Err(AudioError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let status = child.wait().map_err(|source| AudioError::Read {
        path: path.to_owned(),
        source,
    })?;
    let message = messages.join().unwrap_or_default();
    if !status.success() {
        if message.contains("matches no streams") {
            return Err(AudioError::NoAudioTrack(path.to_owned()));
        }
        // ffmpeg's last line sums up the problem; earlier ones carry internal detail.
        let summary = message.lines().rev().find(|line| !line.trim().is_empty());
        return Err(AudioError::Ffmpeg {
            path: path.to_owned(),
            status,
            message: summary.unwrap_or_default().trim().to_owned(),
        });
    }
    if samples.is_empty() {
        return Err(AudioError::Empty(path.to_owned()));
    }
    Ok(samples)
}

fn stop(child: &mut Child) {
    // ffmpeg may already have exited; there is nothing more to do either way.
    let _ = child.kill();
    let _ = child.wait();
}

/// Keeps the start of ffmpeg's messages, and reads the rest so ffmpeg never blocks.
fn read_message(mut stderr: impl Read) -> String {
    let mut kept = Vec::new();
    let _ = (&mut stderr).take(MAX_MESSAGE).read_to_end(&mut kept);
    let _ = io::copy(&mut stderr, &mut io::sink());
    String::from_utf8_lossy(&kept).into_owned()
}

/// Reads little-endian 32-bit float samples. Returns `None` once there are more than
/// `max` samples.
fn read_samples(mut reader: impl Read, max: usize) -> io::Result<Option<Vec<f32>>> {
    let mut samples = Vec::new();
    let mut buffer = vec![0_u8; READ_BUFFER];
    let mut pending: Vec<u8> = Vec::with_capacity(READ_BUFFER + 4);
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        pending.extend_from_slice(buffer.get(..read).unwrap_or_default());
        let (whole, rest) = pending.as_chunks::<4>();
        samples.extend(whole.iter().copied().map(f32::from_le_bytes));
        pending = rest.to_vec();
        if samples.len() > max {
            return Ok(None);
        }
    }
    Ok(Some(samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_read_across_buffer_boundaries() {
        let values = [0.5_f32, -0.25, 1.0, 0.0, -1.0];
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        // A reader that returns 3 bytes at a time splits every sample.
        struct Trickle(Vec<u8>, usize);
        impl Read for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let end = (self.1 + 3).min(self.0.len());
                let chunk = &self.0[self.1..end];
                buf[..chunk.len()].copy_from_slice(chunk);
                self.1 = end;
                Ok(chunk.len())
            }
        }
        let read = read_samples(Trickle(bytes, 0), 100).unwrap().unwrap();
        assert_eq!(read, values);
    }

    #[test]
    fn too_many_samples_stop_the_read() {
        let bytes = [0_u8; 4 * 10];
        assert_eq!(read_samples(&bytes[..], 9).unwrap(), None);
        assert_eq!(read_samples(&bytes[..], 10).unwrap().unwrap().len(), 10);
    }

    #[test]
    fn missing_files_are_reported_before_ffmpeg_runs() {
        let error = decode(Path::new("/nonexistent/clip.mp4")).unwrap_err();
        assert!(matches!(error, AudioError::Open { .. }), "{error}");
        let error = decode(Path::new("/")).unwrap_err();
        assert!(matches!(error, AudioError::NotAFile(_)), "{error}");
    }
}
