//! Checks the written plan: every cut point must lie outside every word.

use sabicut_core::Timeline;

use crate::SourceInput;
use crate::grid::{Grid, micros};

/// How many cut points (each cut's `in` and `out`) the timeline has, and how many of
/// them lie inside a word, its start and end included. The start of a clip and the end
/// of its media are clean cuts even when a word touches them.
pub(crate) fn mid_word(timeline: &Timeline, sources: &[SourceInput], grid: Grid) -> (usize, usize) {
    let mut points = 0;
    let mut inside = 0;
    for segment in timeline.segments() {
        let Some(source) = sources.iter().find(|source| source.id == segment.source()) else {
            continue;
        };
        let end_of_media = grid.at_or_before(micros(source.transcript.duration()));
        let frames = [segment.source_in(), segment.source_out()]
            .map(|f| i64::try_from(f).unwrap_or(i64::MAX));
        for (k, frame) in frames.into_iter().enumerate() {
            points += 1;
            let edge = if k == 0 {
                frame == 0
            } else {
                frame >= end_of_media
            };
            if !edge && in_a_word(source, frame, grid) {
                inside += 1;
            }
        }
    }
    (points, inside)
}

fn in_a_word(source: &SourceInput, frame: i64, grid: Grid) -> bool {
    let words = source.transcript.words();
    // The first word that ends at or after the boundary is the only one that can hold it.
    let first = words.partition_point(|word| grid.is_after(frame, micros(word.end())));
    words
        .get(first)
        .is_some_and(|word| grid.within(frame, micros(word.start()), micros(word.end())))
}
