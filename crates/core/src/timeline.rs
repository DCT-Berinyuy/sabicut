//! The output timeline derived from a plan's cuts, and source-to-output time mapping.

use std::num::NonZeroU32;

use crate::plan::Plan;
use crate::rational::Rational;
use crate::validate::{self, ValidationErrors};

/// Frame boundaries at `fps` frames per second, starting at time zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameGrid {
    fps: NonZeroU32,
}

impl FrameGrid {
    pub(crate) fn new(fps: NonZeroU32) -> Self {
        Self { fps }
    }

    pub(crate) fn fps(self) -> u32 {
        self.fps.get()
    }

    /// The frame boundary nearest to `t`; exact halves round up. Exact for every input,
    /// and saturates at the `i64` limits instead of overflowing.
    pub(crate) fn boundary(self, t: Rational) -> i64 {
        // round(t · fps) = floor((2·n·fps + d) / 2d) where t = n/d. |2·n·fps| < 2^96,
        // so none of this can overflow i128.
        let (n, d) = (i128::from(t.numer()), i128::from(t.denom()));
        let fps = i128::from(self.fps.get());
        let frame = (2 * n * fps + d).div_euclid(2 * d);
        i64::try_from(frame).unwrap_or(if frame < 0 { i64::MIN } else { i64::MAX })
    }

    /// The time of frame boundary `frame`, in seconds.
    pub(crate) fn time(self, frame: u64) -> Rational {
        Rational::from_ratio(i64::try_from(frame).unwrap_or(i64::MAX), self.fps)
    }
}

/// The output timeline derived from a valid plan.
///
/// Each cut becomes one [`Segment`], in plan order. A cut's `in` and `out` are snapped
/// to the nearest boundary of a frame grid at `meta.fps` that starts at the beginning
/// of its source clip (boundary `k` is at `k / fps` seconds; exact halves round up).
/// So every segment is a whole number of frames, and segments follow each other on
/// the output with no gaps or overlaps.
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    grid: FrameGrid,
    segments: Vec<Segment>,
    total_frames: u64,
    hook_frames: Option<u64>,
}

/// One cut, placed on the output timeline.
///
/// Positions are frame boundaries: the segment covers output frames
/// `output_start()..output_end()`, taken from source frames `source_in()..source_out()`.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    source: String,
    source_in: u64,
    source_out: u64,
    output_start: u64,
    zooms: Vec<ZoomEvent>,
}

/// A punch-in zoom, placed on the output timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomEvent {
    source_frame: u64,
    output_frame: u64,
    scale: f64,
}

/// Where a source time lands on the output timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMapping {
    cut: usize,
    output_frame: u64,
    landing: Landing,
}

/// How a mapped source time relates to the cut it was mapped through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// The time is inside the cut, its `in` and `out` included.
    Inside,
    /// The time is before the cut, so it was clamped to the cut's first frame.
    ClampedToStart,
    /// The time is after the cut, so it was clamped to the cut's end.
    ClampedToEnd,
}

impl Timeline {
    /// Validates `plan` and derives its output timeline.
    pub fn from_plan(plan: &Plan) -> Result<Self, ValidationErrors> {
        validate::check(plan)
    }

    /// Output frame rate in frames per second.
    pub fn fps(&self) -> u32 {
        self.grid.fps()
    }

    /// One segment per cut, in output order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Length of the output in frames.
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Length of the output in seconds, exactly.
    pub fn duration(&self) -> Rational {
        self.grid.time(self.total_frames)
    }

    /// The time of frame boundary `frame` (`frame / fps` seconds), exactly. Works for
    /// output frames and for source frames alike.
    pub fn frame_time(&self, frame: u64) -> Rational {
        self.grid.time(frame)
    }

    /// How many output frames the hook covers from the start (frames `0..n`), if the
    /// plan has a hook.
    pub fn hook_frames(&self) -> Option<u64> {
        self.hook_frames
    }

    /// Maps a time on source clip `source` to a frame boundary on the output.
    ///
    /// `t` is snapped to the nearest frame boundary of the source, like cut points are.
    /// If that is inside a cut taken from `source` (its `in` and `out` included), the
    /// result is the matching point on the output. Otherwise the time is clamped to the
    /// nearest cut taken from `source`: a time before it maps to the cut's first frame
    /// and a time after it maps to the cut's end. When several cuts contain the time,
    /// or two are equally near, the one that comes first in the output wins.
    ///
    /// Returns `None` only when no cut uses `source`. Never panics, whatever `t` is.
    pub fn map_source_time(&self, source: &str, t: Rational) -> Option<SourceMapping> {
        let at = i128::from(self.grid.boundary(t));
        let mut nearest: Option<(i128, SourceMapping)> = None;
        for (cut, segment) in self.segments.iter().enumerate() {
            if segment.source != source {
                continue;
            }
            let (start, end) = (
                i128::from(segment.source_in),
                i128::from(segment.source_out),
            );
            let (distance, offset, landing) = if at < start {
                (start - at, 0, Landing::ClampedToStart)
            } else if at > end {
                (at - end, segment.frames(), Landing::ClampedToEnd)
            } else {
                let offset = u64::try_from(at - start).unwrap_or_default();
                (0, offset, Landing::Inside)
            };
            if nearest.is_none_or(|(best, _)| distance < best) {
                let output_frame = segment.output_start.saturating_add(offset);
                let mapping = SourceMapping {
                    cut,
                    output_frame,
                    landing,
                };
                nearest = Some((distance, mapping));
            }
            if distance == 0 {
                break;
            }
        }
        nearest.map(|(_, mapping)| mapping)
    }

    /// Like [`Timeline::map_source_time`], for a time in seconds.
    ///
    /// Returns `None` when `secs` is NaN or no cut uses `source`. Infinities and other
    /// huge values clamp like any other time outside the cuts.
    pub fn map_source_secs(&self, source: &str, secs: f64) -> Option<SourceMapping> {
        if secs.is_nan() {
            return None;
        }
        let t = Rational::from_secs_f64(secs).unwrap_or(if secs < 0.0 {
            Rational::from_integer(i64::MIN)
        } else {
            Rational::from_integer(i64::MAX)
        });
        self.map_source_time(source, t)
    }
}

impl Segment {
    /// Id of the source clip.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Source frame boundary where the segment starts (the snapped `in`).
    pub fn source_in(&self) -> u64 {
        self.source_in
    }

    /// Source frame boundary where the segment ends (the snapped `out`).
    pub fn source_out(&self) -> u64 {
        self.source_out
    }

    /// Exact length in frames; always at least 1.
    pub fn frames(&self) -> u64 {
        self.source_out.saturating_sub(self.source_in)
    }

    /// Output frame boundary where the segment starts.
    pub fn output_start(&self) -> u64 {
        self.output_start
    }

    /// Output frame boundary where the segment ends and the next one starts.
    pub fn output_end(&self) -> u64 {
        self.output_start.saturating_add(self.frames())
    }

    /// Punch-in zooms in this segment, in time order. Each starts on a frame of its own
    /// before the segment ends, so each lasts at least 1 frame.
    pub fn zooms(&self) -> &[ZoomEvent] {
        &self.zooms
    }
}

impl ZoomEvent {
    /// Source frame boundary where the zoom starts (the snapped `at`).
    pub fn source_frame(&self) -> u64 {
        self.source_frame
    }

    /// Output frame boundary where the zoom starts.
    pub fn output_frame(&self) -> u64 {
        self.output_frame
    }

    /// Zoom factor; 1 means no zoom.
    pub fn scale(&self) -> f64 {
        self.scale
    }
}

impl SourceMapping {
    /// Index of the cut the time was mapped through.
    pub fn cut(&self) -> usize {
        self.cut
    }

    /// The output frame boundary the time maps to, from 0 to the total frame count.
    pub fn output_frame(&self) -> u64 {
        self.output_frame
    }

    /// Whether the time was inside the cut or clamped to one of its edges.
    pub fn landing(&self) -> Landing {
        self.landing
    }
}

/// Places validated cuts one after another on the output timeline.
pub(crate) struct TimelineBuilder {
    grid: FrameGrid,
    segments: Vec<Segment>,
    cursor: u64,
}

impl TimelineBuilder {
    pub(crate) fn new(grid: FrameGrid) -> Self {
        Self {
            grid,
            segments: Vec::new(),
            cursor: 0,
        }
    }

    /// Appends a cut covering source boundaries `source_in..source_out`, with zooms
    /// given as `(source boundary, scale)` pairs.
    pub(crate) fn push(
        &mut self,
        source: &str,
        source_in: u64,
        source_out: u64,
        zooms: &[(u64, f64)],
    ) {
        let output_start = self.cursor;
        let zooms = zooms
            .iter()
            .map(|&(at, scale)| {
                // Validation keeps `at` inside the cut; clamping keeps this total anyway.
                let offset = at.max(source_in).min(source_out).saturating_sub(source_in);
                ZoomEvent {
                    source_frame: at,
                    output_frame: output_start.saturating_add(offset),
                    scale,
                }
            })
            .collect();
        self.segments.push(Segment {
            source: source.to_owned(),
            source_in,
            source_out,
            output_start,
            zooms,
        });
        self.cursor = output_start.saturating_add(source_out.saturating_sub(source_in));
    }

    pub(crate) fn output_frames(&self) -> u64 {
        self.cursor
    }

    pub(crate) fn finish(self, hook_frames: Option<u64>) -> Timeline {
        Timeline {
            grid: self.grid,
            segments: self.segments,
            total_frames: self.cursor,
            hook_frames,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(fps: u32) -> FrameGrid {
        FrameGrid::new(NonZeroU32::new(fps).unwrap())
    }

    fn secs(s: f64) -> Rational {
        Rational::from_secs_f64(s).unwrap()
    }

    #[test]
    fn boundary_rounds_to_the_nearest_frame_with_halves_up() {
        let g = grid(30);
        assert_eq!(g.boundary(secs(3.12)), 94); // 93.6
        assert_eq!(g.boundary(secs(9.88)), 296); // 296.4
        assert_eq!(g.boundary(secs(0.05)), 2); // exactly 1.5
        assert_eq!(g.boundary(secs(-0.05)), -1); // exactly -1.5
    }

    #[test]
    fn boundary_is_exact_for_fractions_that_do_not_fit_after_multiplying() {
        // Close to 1 second, but numerator × 30 does not fit in i64.
        let almost_one = Rational::new(i64::MAX - 1, i64::MAX).unwrap();
        assert_eq!(grid(30).boundary(almost_one), 30);
    }

    #[test]
    fn boundary_saturates() {
        assert_eq!(
            grid(240).boundary(Rational::from_integer(i64::MAX)),
            i64::MAX
        );
        assert_eq!(
            grid(240).boundary(Rational::from_integer(i64::MIN)),
            i64::MIN
        );
    }

    #[test]
    fn time_is_frames_over_fps() {
        assert_eq!(grid(30).time(68), Rational::new(34, 15).unwrap());
        assert_eq!(
            grid(30).time(u64::MAX),
            Rational::new(i64::MAX, 30).unwrap()
        );
    }
}
