//! Finding and running the ffmpeg and ffprobe binaries (PRD §8: video I/O goes through
//! the ffmpeg binary as a subprocess; nothing links libav).

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

/// Environment variable naming the ffmpeg binary to use instead of `ffmpeg` on `PATH`.
pub const FFMPEG_ENV: &str = "SABICUT_FFMPEG";
/// Environment variable naming the ffprobe binary to use. Without it, the `ffprobe`
/// next to the chosen ffmpeg is used, or `ffprobe` on `PATH`.
pub const FFPROBE_ENV: &str = "SABICUT_FFPROBE";

/// How much of ffmpeg's stderr is kept for error messages and log parsing.
const STDERR_KEEP_BYTES: usize = 256 * 1024;
/// How many trailing stderr lines an error message shows.
const STDERR_TAIL_LINES: usize = 15;

/// Why ffmpeg or ffprobe could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The binary could not be started.
    #[error("cannot run {}: {source}{}", .program.display(), install_hint(.source))]
    Spawn {
        /// The binary that was run.
        program: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The binary ran and failed.
    #[error("{} failed ({status}){}", .program.display(), tail(.stderr))]
    Failed {
        /// The binary that was run.
        program: PathBuf,
        /// Its exit status.
        status: ExitStatus,
        /// The end of what it printed on stderr.
        stderr: String,
    },
    /// The binary printed something that is not what ffmpeg prints.
    #[error("{} does not look like ffmpeg or ffprobe: {message}", .program.display())]
    Unexpected {
        /// The binary that was run.
        program: PathBuf,
        /// What was unexpected.
        message: String,
    },
    /// This ffmpeg build lacks filters that the plan needs.
    #[error(
        "ffmpeg {version} lacks the {} filter{} needed to {purpose}; install an ffmpeg build that has {}",
        .missing.join(", "),
        if .missing.len() == 1 { "" } else { "s" },
        if .missing.len() == 1 { "it" } else { "them" }
    )]
    MissingFilters {
        /// The ffmpeg version string.
        version: String,
        /// The filters that are missing.
        missing: Vec<String>,
        /// What they are needed for.
        purpose: String,
    },
    /// This ffmpeg build lacks an encoder that rendering needs.
    #[error(
        "ffmpeg {version} lacks the {name} encoder needed to {purpose}; install an ffmpeg build that has it"
    )]
    MissingEncoder {
        /// The ffmpeg version string.
        version: String,
        /// The encoder that is missing.
        name: String,
        /// What it is needed for.
        purpose: String,
    },
}

fn install_hint(error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        format!(" (install ffmpeg, or set {FFMPEG_ENV} and {FFPROBE_ENV} to the binaries)")
    } else {
        String::new()
    }
}

/// The last lines of `stderr`, indented, or nothing if it is empty.
fn tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    let shown = lines.get(start..).unwrap_or_default();
    if shown.is_empty() {
        String::new()
    } else {
        format!(":\n  {}", shown.join("\n  "))
    }
}

/// The ffmpeg and ffprobe binaries, and what this ffmpeg build can do.
#[derive(Clone, Debug)]
pub struct Ffmpeg {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    version: String,
    major: Option<u32>,
    filters: BTreeSet<String>,
    encoders: BTreeSet<String>,
}

impl Ffmpeg {
    /// Finds ffmpeg and ffprobe: from [`FFMPEG_ENV`] and [`FFPROBE_ENV`] if set, else on
    /// `PATH`. Runs both once to check that they work and to list ffmpeg's filters
    /// and encoders.
    pub fn from_env() -> Result<Self, ToolError> {
        let ffmpeg = std::env::var_os(FFMPEG_ENV)
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from);
        let ffprobe = std::env::var_os(FFPROBE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| sibling_ffprobe(&ffmpeg));
        Self::new(ffmpeg, ffprobe)
    }

    /// Uses the given binaries. Runs both once to check that they work and to list
    /// ffmpeg's filters and encoders.
    pub fn new(ffmpeg: PathBuf, ffprobe: PathBuf) -> Result<Self, ToolError> {
        let banner = capture(&ffmpeg, ["-hide_banner", "-version"])?;
        let version = parse_version(&banner).ok_or_else(|| ToolError::Unexpected {
            program: ffmpeg.clone(),
            message: "`-version` did not print `ffmpeg version ...`".into(),
        })?;
        let major = major_version(&version);
        let filters = parse_filters(&capture(&ffmpeg, ["-hide_banner", "-filters"])?);
        let encoders = parse_encoders(&capture(&ffmpeg, ["-hide_banner", "-encoders"])?);
        if filters.is_empty() || encoders.is_empty() {
            return Err(ToolError::Unexpected {
                program: ffmpeg,
                message: "it listed no filters or no encoders".into(),
            });
        }
        capture(&ffprobe, ["-hide_banner", "-version"])?;
        Ok(Self {
            ffmpeg,
            ffprobe,
            version,
            major,
            filters,
            encoders,
        })
    }

    /// The ffmpeg binary.
    pub fn ffmpeg_path(&self) -> &Path {
        &self.ffmpeg
    }

    /// The ffprobe binary.
    pub fn ffprobe_path(&self) -> &Path {
        &self.ffprobe
    }

    /// The version ffmpeg reports, like `6.1.1-3ubuntu5` or `n9.0.2`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Whether this ffmpeg build has the filter `name`.
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.contains(name)
    }

    /// Fails with a clear message unless every filter in `names` is available.
    pub fn require_filters(&self, names: &[&str], purpose: &str) -> Result<(), ToolError> {
        let missing: Vec<String> = names
            .iter()
            .filter(|name| !self.has_filter(name))
            .map(|name| (*name).to_owned())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(ToolError::MissingFilters {
                version: self.version.clone(),
                missing,
                purpose: purpose.to_owned(),
            })
        }
    }

    /// Fails with a clear message unless the encoder `name` is available.
    pub fn require_encoder(&self, name: &str, purpose: &str) -> Result<(), ToolError> {
        if self.encoders.contains(name) {
            Ok(())
        } else {
            Err(ToolError::MissingEncoder {
                version: self.version.clone(),
                name: name.to_owned(),
                purpose: purpose.to_owned(),
            })
        }
    }

    /// The arguments that load a filter graph from `script`. ffmpeg 7 replaced
    /// `-filter_complex_script` with `-/filter_complex`, and ffmpeg 8 dropped the old
    /// form, so this depends on the version. Builds from git report no release number
    /// and get the new form.
    pub(crate) fn graph_script_args(&self, script: &Path) -> [OsString; 2] {
        let option = match self.major {
            Some(major) if major < 7 => "-filter_complex_script",
            _ => "-/filter_complex",
        };
        [option.into(), script.as_os_str().to_owned()]
    }

    /// Runs ffmpeg with `args` to completion and returns what it printed on stderr.
    pub(crate) fn run_ffmpeg(&self, args: &[OsString]) -> Result<String, ToolError> {
        self.spawn_ffmpeg(args, Stdio::null(), Stdio::null())?
            .finish()
    }

    /// Starts ffmpeg with `args`, its stdin and stdout connected as given.
    pub(crate) fn spawn_ffmpeg(
        &self,
        args: &[OsString],
        stdin: Stdio,
        stdout: Stdio,
    ) -> Result<Running, ToolError> {
        Running::spawn(&self.ffmpeg, args, stdin, stdout)
    }

    /// Runs ffprobe with `args` and returns what it printed on stdout.
    pub(crate) fn run_ffprobe(&self, args: &[OsString]) -> Result<Vec<u8>, ToolError> {
        let output = Command::new(&self.ffprobe)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|source| ToolError::Spawn {
                program: self.ffprobe.clone(),
                source,
            })?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(ToolError::Failed {
                program: self.ffprobe.clone(),
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }
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

/// Runs a program to completion and returns its stdout as text.
fn capture<const N: usize>(program: &Path, args: [&str; N]) -> Result<String, ToolError> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| ToolError::Spawn {
            program: program.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(ToolError::Failed {
            program: program.to_owned(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A running ffmpeg. Its stderr is collected on a thread so that it can never fill up
/// and stall ffmpeg. Dropping it before [`Running::finish`] kills the process, so an
/// error elsewhere in a pipeline never leaves ffmpeg running.
#[derive(Debug)]
pub(crate) struct Running {
    program: PathBuf,
    child: Option<Child>,
    stderr: Option<JoinHandle<String>>,
}

impl Running {
    fn spawn(
        program: &Path,
        args: &[OsString],
        stdin: Stdio,
        stdout: Stdio,
    ) -> Result<Self, ToolError> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| ToolError::Spawn {
                program: program.to_owned(),
                source,
            })?;
        let stderr = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let mut kept = VecDeque::new();
                let mut buffer = [0_u8; 8192];
                while let Ok(n) = stderr.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    kept.extend(buffer.iter().take(n));
                    let excess = kept.len().saturating_sub(STDERR_KEEP_BYTES);
                    kept.drain(..excess);
                }
                String::from_utf8_lossy(kept.make_contiguous()).into_owned()
            })
        });
        Ok(Self {
            program: program.to_owned(),
            child: Some(child),
            stderr,
        })
    }

    /// Its stdin, if it was piped and not taken yet.
    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.as_mut()?.stdin.take()
    }

    /// Its stdout, if it was piped and not taken yet.
    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }

    /// Waits for it to exit. Returns its stderr if it succeeded.
    pub(crate) fn finish(mut self) -> Result<String, ToolError> {
        let Some(mut child) = self.child.take() else {
            return Ok(String::new());
        };
        let status = child.wait().map_err(|source| ToolError::Spawn {
            program: self.program.clone(),
            source,
        })?;
        let stderr = self
            .stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        if status.success() {
            Ok(stderr)
        } else {
            Err(ToolError::Failed {
                program: self.program.clone(),
                status,
                stderr,
            })
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Best effort: it may have exited already.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The version from `ffmpeg -version`'s first line, `ffmpeg version <version> ...`.
fn parse_version(banner: &str) -> Option<String> {
    let line = banner.lines().next()?;
    let rest = line.strip_prefix("ffmpeg version ")?;
    rest.split_whitespace().next().map(str::to_owned)
}

/// The major release number in a version like `6.1.1-3ubuntu5`, `n9.0.2` or
/// `n6.1.3-1-gabc`. Git builds (`N-112345-gabc`, `2024-01-01-git-abc`) have none, or a
/// year, which both count as recent.
fn major_version(version: &str) -> Option<u32> {
    let digits: String = version
        .strip_prefix('n')
        .unwrap_or(version)
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Filter names from `ffmpeg -filters`, whose lines look like
/// ` TS afftdn            A->A       Denoise audio samples using FFT.`
fn parse_filters(listing: &str) -> BTreeSet<String> {
    listing
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let (_flags, name, kind) = (tokens.next()?, tokens.next()?, tokens.next()?);
            kind.contains("->").then(|| name.to_owned())
        })
        .collect()
}

/// Encoder names from `ffmpeg -encoders`, whose lines look like
/// ` V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10`.
fn parse_encoders(listing: &str) -> BTreeSet<String> {
    listing
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let (flags, name) = (tokens.next()?, tokens.next()?);
            let is_entry = flags.len() == 6
                && flags.starts_with(['V', 'A', 'S'])
                && name != "="
                && !name.starts_with('=');
            is_entry.then(|| name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_from_distro_release_and_git_builds() {
        let banner = "ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023 the FFmpeg developers\nbuilt with gcc 13";
        let version = parse_version(banner).unwrap();
        assert_eq!(version, "6.1.1-3ubuntu5");
        assert_eq!(major_version(&version), Some(6));
        assert_eq!(major_version("n9.0.2"), Some(9));
        assert_eq!(major_version("n6.1.3-1-g0a1b2c"), Some(6));
        assert_eq!(major_version("N-112345-g0a1b2c3d4e"), None);
        assert_eq!(major_version("2024-03-28-git-5d71f97e0e"), Some(2024));
        assert_eq!(parse_version("ffprobe version 6.1"), None);
    }

    #[test]
    fn filter_listing_is_parsed_without_its_legend() {
        let listing = "Filters:\n  T.. = Timeline support\n  .S. = Slice threading\n  A = Audio input/output\n  ... = no flags\n TS afftdn            A->A       Denoise audio samples using FFT.\n ... concat            N->N       Concatenate audio and video streams.\n";
        let filters = parse_filters(listing);
        assert_eq!(
            filters.into_iter().collect::<Vec<_>>(),
            ["afftdn", "concat"]
        );
    }

    #[test]
    fn encoder_listing_is_parsed_without_its_legend() {
        let listing = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n V....D libx264              libx264 H.264\n A....D aac                  AAC (Advanced Audio Coding)\n";
        let encoders = parse_encoders(listing);
        assert_eq!(encoders.into_iter().collect::<Vec<_>>(), ["aac", "libx264"]);
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

    #[test]
    fn error_tail_keeps_the_last_lines() {
        let stderr: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let shown = tail(&stderr);
        assert!(shown.starts_with(":\n  line 16\n"), "{shown}");
        assert!(shown.ends_with("line 30"), "{shown}");
        assert_eq!(tail("  \n"), "");
    }
}
