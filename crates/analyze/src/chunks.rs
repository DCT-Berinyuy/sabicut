//! Packing speech segments into chunks that Whisper hears in one go, and mapping times
//! in a chunk back to the original audio.
//!
//! A chunk joins consecutive speech segments with 100 ms of silence, as whisper.cpp's
//! own voice-activity detection does, and is at most 30 s long (Whisper's window), so
//! each chunk costs one pass of the encoder. Because every chunk records where each
//! segment came from, every time maps back exactly, and a word can be kept inside the
//! segment it was heard in.

use std::ops::Range;

use crate::audio::SAMPLE_RATE;

/// Silence between two speech segments in a chunk.
pub(crate) const GAP: usize = SAMPLE_RATE / 10;
/// Whisper hears 30 s at a time. A chunk is longer only if one segment is.
pub(crate) const MAX_CHUNK: usize = 30 * SAMPLE_RATE;

/// One speech segment's place in a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    /// Index of the speech segment.
    segment: usize,
    /// Where the segment starts in the chunk, in samples.
    packed: usize,
    /// Where the segment starts in the original audio, in samples.
    original: usize,
    /// Length in samples.
    len: usize,
}

impl Span {
    fn packed_end(&self) -> usize {
        self.packed + self.len
    }

    /// How many samples of `start..end` (chunk samples) fall in this span.
    fn overlap(&self, start: usize, end: usize) -> usize {
        end.min(self.packed_end())
            .saturating_sub(start.max(self.packed))
    }
}

/// Consecutive speech segments that Whisper hears together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chunk {
    spans: Vec<Span>,
    len: usize,
}

/// Where a word lands in the original audio, in samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Index of the speech segment the word belongs to.
    pub segment: usize,
    /// Start, inside the segment.
    pub start: usize,
    /// End, inside the segment and not before the start.
    pub end: usize,
}

/// Packs speech segments (sample ranges of the original audio, in order) into chunks
/// of about equal length, each at most [`MAX_CHUNK`] unless a single segment is longer.
/// Empty segments are skipped.
pub(crate) fn pack(segments: &[Range<usize>]) -> Vec<Chunk> {
    let lens: Vec<usize> = segments.iter().map(ExactSizeIterator::len).collect();
    let count = lens.iter().filter(|len| **len > 0).count();
    let total = lens.iter().sum::<usize>() + GAP * count.saturating_sub(1);
    let target = total.div_ceil(total.div_ceil(MAX_CHUNK).max(1));

    let mut chunks = Vec::new();
    let mut current = Chunk::default();
    for (index, (segment, len)) in segments.iter().zip(lens).enumerate() {
        if len == 0 {
            continue;
        }
        if !current.spans.is_empty()
            && (current.len + GAP + len > MAX_CHUNK || current.len >= target)
        {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.spans.is_empty() {
            current.len += GAP;
        }
        current.spans.push(Span {
            segment: index,
            packed: current.len,
            original: segment.start,
            len,
        });
        current.len += len;
    }
    if !current.spans.is_empty() {
        chunks.push(current);
    }
    chunks
}

impl Chunk {
    /// Samples of speech, silences excluded.
    pub(crate) fn speech_len(&self) -> usize {
        self.spans.iter().map(|span| span.len).sum()
    }

    /// The chunk's audio: its segments copied from `samples`, joined by silence.
    pub(crate) fn audio(&self, samples: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; self.len];
        for span in &self.spans {
            let target = out.get_mut(span.packed..span.packed_end());
            let source = samples.get(span.original..span.original + span.len);
            if let (Some(target), Some(source)) = (target, source) {
                target.copy_from_slice(source);
            }
        }
        out
    }

    /// Places a word heard from `start` to `end` (samples of this chunk) in the
    /// original audio.
    ///
    /// The word belongs to the segment it overlaps most. A word that lies entirely in a
    /// silence belongs to the next segment (Whisper tends to start words early), or to
    /// the last one at the end of the chunk. Times are clamped to the segment, and a
    /// word moved out of a silence keeps its length where the segment allows.
    pub(crate) fn place(&self, start: usize, end: usize) -> Option<Placement> {
        let end = end.max(start);
        let mut best: Option<(&Span, usize)> = None;
        for span in &self.spans {
            let overlap = span.overlap(start, end);
            if overlap > 0 && best.is_none_or(|(_, most)| overlap > most) {
                best = Some((span, overlap));
            }
        }
        let span = match best {
            Some((span, _)) => span,
            None => self
                .spans
                .iter()
                .find(|span| span.packed_end() > start)
                .or_else(|| self.spans.last())?,
        };
        let first = start.clamp(span.packed, span.packed_end());
        let mut last = end.clamp(span.packed, span.packed_end());
        if last == first {
            last = (first + (end - start)).min(span.packed_end());
        }
        Some(Placement {
            segment: span.segment,
            start: span.original + (first - span.packed),
            end: span.original + (last - span.packed),
        })
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Lists of one speech segment are intended.
mod tests {
    use super::*;

    const S: usize = SAMPLE_RATE;

    #[test]
    fn segments_are_joined_by_short_silences() {
        let chunks = pack(&[S..2 * S, 5 * S..7 * S]);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert_eq!(chunk.len, S + GAP + 2 * S);
        assert_eq!(chunk.speech_len(), 3 * S);

        let samples: Vec<f32> = (0..8 * S).map(|i| i as f32).collect();
        let audio = chunk.audio(&samples);
        assert_eq!(audio[0], S as f32);
        assert_eq!(audio[S - 1], (2 * S - 1) as f32);
        assert!(audio[S..S + GAP].iter().all(|v| *v == 0.0));
        assert_eq!(audio[S + GAP], (5 * S) as f32);
    }

    #[test]
    fn chunks_are_balanced_and_at_most_30_seconds() {
        // 62 one-second segments: 3 chunks of about 21 s, not 30 + 30 + 2.
        let segments: Vec<_> = (0..62).map(|i| 2 * i * S..(2 * i + 1) * S).collect();
        let chunks = pack(&segments);
        assert_eq!(chunks.len(), 3);
        for chunk in &chunks {
            assert!(chunk.len <= MAX_CHUNK);
            assert!(chunk.speech_len() >= 19 * S, "{}", chunk.speech_len());
        }
        assert_eq!(chunks.iter().map(Chunk::speech_len).sum::<usize>(), 62 * S);
    }

    #[test]
    fn a_long_segment_gets_a_chunk_of_its_own() {
        let chunks = pack(&[0..S, 2 * S..42 * S, 43 * S..44 * S]);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[1].len, 40 * S);
        assert!(pack(&[]).is_empty());
        assert!(pack(&[5..5]).is_empty());
    }

    #[test]
    fn times_map_back_to_the_original_audio() {
        let chunk = &pack(&[10 * S..12 * S, 20 * S..21 * S])[0];
        // Inside the first segment.
        assert_eq!(
            chunk.place(S / 2, S),
            Some(Placement {
                segment: 0,
                start: 10 * S + S / 2,
                end: 11 * S
            })
        );
        // Inside the second segment, which starts at 2 s + 100 ms in the chunk.
        let second = 2 * S + GAP;
        assert_eq!(
            chunk.place(second + S / 4, second + S / 2),
            Some(Placement {
                segment: 1,
                start: 20 * S + S / 4,
                end: 20 * S + S / 2
            })
        );
    }

    #[test]
    fn words_stay_inside_their_segment() {
        let chunk = &pack(&[10 * S..12 * S, 20 * S..21 * S])[0];
        let second = 2 * S + GAP;
        // Runs past the end of the first segment into the silence: clamped.
        assert_eq!(
            chunk.place(2 * S - S / 10, 2 * S + GAP / 2),
            Some(Placement {
                segment: 0,
                start: 12 * S - S / 10,
                end: 12 * S
            })
        );
        // Starts in the silence, mostly in the second segment: moved to its start.
        assert_eq!(
            chunk.place(2 * S + GAP / 2, second + S / 5),
            Some(Placement {
                segment: 1,
                start: 20 * S,
                end: 20 * S + S / 5
            })
        );
        // Entirely in the silence: goes to the next segment and keeps its length.
        let placed = chunk.place(2 * S + 100, 2 * S + 900).unwrap();
        assert_eq!(
            (placed.segment, placed.start, placed.end),
            (1, 20 * S, 20 * S + 800)
        );
        // Past the end of the chunk: the end of the last segment.
        let placed = chunk.place(10 * S, 11 * S).unwrap();
        assert_eq!(
            (placed.segment, placed.start, placed.end),
            (1, 21 * S, 21 * S)
        );
        // A word whose end is before its start ends where it starts.
        let placed = chunk.place(S, S / 2).unwrap();
        assert_eq!((placed.start, placed.end), (11 * S, 11 * S));
    }
}
