//! Voice-activity detection with Silero, run by whisper.cpp.
//!
//! whisper-rs 0.16 silently ignores `FullParams::enable_vad` (whisper.cpp only runs VAD
//! inside `whisper_full`, and `WhisperState::full` calls `whisper_full_with_state`), so
//! Sabicut runs Silero itself through `WhisperVadContext`. Silero gets 1 thread: with 4
//! it competed with Whisper and took 90 s instead of 5 s for 10 minutes of audio.

use std::ops::Range;
use std::path::Path;

use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::Language;
use crate::audio::SAMPLE_RATE;

/// Silero's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VadSettings {
    /// Probability above which a 32 ms frame counts as speech, from 0 to 1. Speech
    /// ends when it falls 0.15 below this.
    pub threshold: f32,
    /// Audio kept before and after each stretch of speech, in milliseconds. Where two
    /// stretches are closer than twice this, the silence is shared between them.
    pub pad_ms: u32,
}

impl VadSettings {
    /// The tuned settings for a language, or for audio whose language is not known
    /// yet. See `docs/bench/analyze.md` for the measurements behind them.
    ///
    /// English and French get the same settings: 30 ms of padding and a threshold of
    /// 0.5 (whisper.cpp's defaults). The French onsets that seemed clipped were mostly
    /// breaths before phrases, and more padding made word timing worse in both
    /// languages (Whisper places words in the extra silence) while costing 11% more
    /// audio to transcribe.
    pub fn for_language(language: Option<Language>) -> Self {
        match language {
            Some(Language::En | Language::Fr) | None => Self {
                threshold: 0.5,
                pad_ms: 30,
            },
        }
    }
}

/// Speech probabilities of a whole recording, from which segments are derived.
pub(crate) struct Vad {
    context: WhisperVadContext,
    samples: usize,
}

impl Vad {
    /// Loads the Silero model and computes the speech probabilities of `samples`.
    pub(crate) fn detect(model: &Path, samples: &[f32]) -> Result<Self, String> {
        let path = model
            .to_str()
            .ok_or_else(|| format!("{}: the path is not valid UTF-8", model.display()))?;
        let mut params = WhisperVadContextParams::new();
        params.set_n_threads(1);
        params.set_use_gpu(false);
        let mut context = WhisperVadContext::new(path, params)
            .map_err(|error| format!("cannot load {}: {error}", model.display()))?;
        context
            .detect_speech(samples)
            .map_err(|error| format!("Silero failed: {error}"))?;
        Ok(Self {
            context,
            samples: samples.len(),
        })
    }

    /// The stretches of speech, as sample ranges in time order.
    pub(crate) fn segments(&mut self, settings: VadSettings) -> Result<Vec<Range<usize>>, String> {
        let mut params = WhisperVadParams::new();
        params.set_threshold(settings.threshold);
        params.set_speech_pad(i32::try_from(settings.pad_ms).unwrap_or(i32::MAX));
        let segments = self
            .context
            .segments_from_probabilities(params)
            .map_err(|error| format!("Silero failed: {error}"))?;
        Ok(to_samples(
            segments.map(|segment| (segment.start, segment.end)),
            self.samples,
        ))
    }
}

/// Converts Silero's centisecond times to sample ranges within `0..len`, dropping empty
/// and out-of-order ones.
fn to_samples(centiseconds: impl Iterator<Item = (f32, f32)>, len: usize) -> Vec<Range<usize>> {
    let to_sample = |cs: f32| {
        let samples = (f64::from(cs.max(0.0)) * (SAMPLE_RATE / 100) as f64).round();
        (samples as usize).min(len)
    };
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (start, end) in centiseconds {
        let start = to_sample(start).max(ranges.last().map_or(0, |last| last.end));
        let end = to_sample(end);
        if end > start {
            ranges.push(start..end);
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centiseconds_become_sample_ranges() {
        let ranges = to_samples(
            [
                (0.0, 51.2),
                (60.8, 120.0),
                (119.0, 130.0),
                (140.0, 140.0),
                (150.0, 900.0),
            ]
            .into_iter(),
            200_000,
        );
        assert_eq!(
            ranges,
            [0..8192, 9728..19_200, 19_200..20_800, 24_000..144_000]
        );
        assert_eq!(to_samples([(1000.0, 2000.0)].into_iter(), 100), []);
    }
}
