//! Application-level voice-activity detection (VAD).
//!
//! whisper.cpp 1.8.3 applies `params.vad` only inside `whisper_full()`, which uses the
//! context's default state. `whisper_full_with_state()`, which `WhisperState::full` calls,
//! ignores it silently, so `FullParams::enable_vad` has no effect through whisper-rs 0.16.
//! This module reproduces whisper.cpp's approach outside the library: Silero finds speech,
//! the speech spans are joined with 100 ms of silence, and a time map restores original times.

use std::time::Instant;

use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

const SAMPLE_RATE: usize = 16_000;
const SAMPLES_PER_CS: usize = SAMPLE_RATE / 100;
/// Silence inserted between speech spans, as in whisper.cpp's `whisper_vad`.
const GAP_SAMPLES: usize = SAMPLE_RATE / 10;

/// Processed audio `[proc_start, proc_start + len)` is a copy of original `[orig_start, ..)`.
#[derive(Clone, Copy)]
struct Span {
    proc_start: usize,
    orig_start: usize,
    len: usize,
}

pub struct VadAudio {
    pub samples: Vec<f32>,
    pub n_segments: usize,
    pub vad_ms: f64,
    spans: Vec<Span>,
    orig_len: usize,
}

impl VadAudio {
    pub fn speech_s(&self) -> f64 {
        self.spans.iter().map(|s| s.len).sum::<usize>() as f64 / SAMPLE_RATE as f64
    }

    /// Maps a whisper timestamp (centiseconds of processed audio) to original audio time.
    pub fn to_original_cs(&self, processed_cs: i64) -> i64 {
        let p = usize::try_from(processed_cs.max(0)).unwrap_or(0) * SAMPLES_PER_CS;
        (self.to_original_samples(p) / SAMPLES_PER_CS) as i64
    }

    fn to_original_samples(&self, p: usize) -> usize {
        let idx = self.spans.partition_point(|s| s.proc_start <= p);
        let Some(span) = idx.checked_sub(1).map(|i| self.spans[i]) else {
            return self.spans.first().map_or(p, |s| s.orig_start);
        };
        let offset = p - span.proc_start;
        let orig_end = span.orig_start + span.len;
        if offset <= span.len {
            return span.orig_start + offset;
        }
        let Some(next) = self.spans.get(idx) else {
            return (orig_end + offset - span.len).min(self.orig_len);
        };
        // Inside the inserted silence: spread it over the original gap.
        let next_orig = next.orig_start.max(orig_end);
        let into_gap = (offset - span.len).min(GAP_SAMPLES);
        orig_end + (next_orig - orig_end) * into_gap / GAP_SAMPLES
    }
}

fn cs_to_samples(cs: f32) -> usize {
    (f64::from(cs.max(0.0)) / 100.0 * SAMPLE_RATE as f64 + 0.5) as usize
}

/// Detects speech and returns only the speech audio plus the map back to original time.
pub fn filter_speech(model: &str, threads: i32, samples: &[f32]) -> Result<VadAudio, String> {
    let started = Instant::now();
    let mut ctx_params = WhisperVadContextParams::new();
    ctx_params.set_n_threads(threads);
    ctx_params.set_use_gpu(false);
    let mut vctx = WhisperVadContext::new(model, ctx_params)
        .map_err(|e| format!("load VAD model {model}: {e:?}"))?;
    let params = WhisperVadParams::new();
    let overlap = (0.1 * SAMPLE_RATE as f64) as usize; // WhisperVadParams default samples_overlap
    let segments: Vec<_> = vctx
        .segments_from_samples(params, samples)
        .map_err(|e| format!("VAD detection failed: {e:?}"))?
        .collect();

    let mut out = Vec::with_capacity(samples.len());
    let mut spans = Vec::with_capacity(segments.len());
    for (i, seg) in segments.iter().enumerate() {
        let is_last = i + 1 == segments.len();
        let start = cs_to_samples(seg.start).min(samples.len());
        let end = (cs_to_samples(seg.end) + if is_last { 0 } else { overlap }).min(samples.len());
        if end <= start {
            continue;
        }
        if !spans.is_empty() {
            out.resize(out.len() + GAP_SAMPLES, 0.0);
        }
        spans.push(Span {
            proc_start: out.len(),
            orig_start: start,
            len: end - start,
        });
        out.extend_from_slice(&samples[start..end]);
    }

    Ok(VadAudio {
        samples: out,
        n_segments: segments.len(),
        vad_ms: started.elapsed().as_secs_f64() * 1000.0,
        spans,
        orig_len: samples.len(),
    })
}
