//! Checking that the machine has room for a transcription before it starts.

use std::fs;

/// Bytes in a mebibyte.
pub(crate) const MIB: u64 = 1024 * 1024;

/// Memory a transcription uses besides the weights and the audio: whisper.cpp's
/// caches and compute buffers, and the program itself. Fitted to measured peaks of the
/// q5_1 models (see `docs/bench/analyze.md`).
const FIXED: u64 = 245 * MIB;
/// The caches and compute buffers also grow with the model: in total, the weights
/// take about this many times their file size.
const MODEL_FACTOR: f64 = 1.95;
/// Memory left free for the rest of the system.
const HEADROOM: u64 = 100 * MIB;
/// Dynamic time warping keeps attention weights for its alignment (measured: 55 MiB
/// more with the small model).
pub(crate) const DTW_EXTRA: u64 = 64 * MIB;

/// Expected peak memory of a transcription, in bytes.
pub(crate) fn estimated_peak(model_bytes: u64, audio_samples: usize) -> u64 {
    let audio = (audio_samples as u64).saturating_mul(4);
    let model = (model_bytes as f64 * MODEL_FACTOR) as u64;
    FIXED.saturating_add(model).saturating_add(audio)
}

/// Memory that must be available before a transcription starts, in bytes.
pub(crate) fn needed(model_bytes: u64, audio_samples: usize) -> u64 {
    estimated_peak(model_bytes, audio_samples).saturating_add(HEADROOM)
}

/// `MemAvailable` from `/proc/meminfo`, in bytes, or `None` where it cannot be read.
pub(crate) fn available() -> Option<u64> {
    parse_kb(&fs::read_to_string("/proc/meminfo").ok()?, "MemAvailable")
}

/// This process's peak resident memory (`VmHWM`), in bytes, where it can be read.
pub(crate) fn peak_rss() -> Option<u64> {
    parse_kb(&fs::read_to_string("/proc/self/status").ok()?, "VmHWM")
}

/// Reads a `Field:   123 kB` line, as found in `/proc/meminfo` and `/proc/self/status`.
fn parse_kb(text: &str, field: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(field)?.strip_prefix(':')?;
        let kb: u64 = value.trim().strip_suffix("kB")?.trim().parse().ok()?;
        kb.checked_mul(1024)
    })
}

/// Bytes as whole mebibytes, rounded up.
pub(crate) fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(MIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_available_is_read_in_bytes() {
        let meminfo = "MemTotal:        7974812 kB\nMemFree:          226032 kB\n\
                       MemAvailable:    1020252 kB\nBuffers:            1236 kB\n";
        assert_eq!(parse_kb(meminfo, "MemAvailable"), Some(1_020_252 * 1024));
        assert_eq!(parse_kb(meminfo, "MemTotal"), Some(7_974_812 * 1024));
        assert_eq!(parse_kb(meminfo, "SwapFree"), None);
        assert_eq!(parse_kb("MemAvailable: lots\n", "MemAvailable"), None);
        assert_eq!(
            parse_kb("VmHWM:\t  712340 kB\n", "VmHWM"),
            Some(712_340 * 1024)
        );
    }

    #[test]
    fn estimates_grow_with_the_model_and_the_audio() {
        let small = estimated_peak(190_085_487, 0);
        let tiny = estimated_peak(32_152_673, 0);
        assert!(tiny < small);
        // The small model peaked at about 0.7 GB on 10 minutes of audio.
        let ten_minutes = 600 * 16_000;
        let small_10_min = mib(estimated_peak(190_085_487, ten_minutes));
        assert!((600..800).contains(&small_10_min), "{small_10_min} MiB");
        assert_eq!(needed(0, 0), FIXED + HEADROOM);
        assert_eq!(mib(1), 1);
    }
}
