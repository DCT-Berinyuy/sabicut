//! Exact frame arithmetic on a source's frame grid, matching how sabicut-core snaps
//! plan times: times are read to the microsecond, and boundary `k` is at `k / fps` s.

/// Microseconds in a second.
const MICROS: i64 = 1_000_000;

/// Seconds to whole microseconds, the precision sabicut-core reads plan times with.
/// Times here come from validated files, so they are finite and at most a day long.
pub(crate) fn micros(secs: f64) -> i64 {
    (secs * 1e6).round() as i64
}

/// Frame boundaries at `fps` frames per second, from time zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Grid {
    fps: i64,
}

impl Grid {
    pub(crate) fn new(fps: u32) -> Self {
        Self {
            fps: i64::from(fps.max(1)),
        }
    }

    /// The boundary nearest to `us`, exact halves rounding up: what sabicut-core
    /// snaps a time of `us` microseconds to.
    pub(crate) fn nearest(self, us: i64) -> i64 {
        (2 * us * self.fps + MICROS).div_euclid(2 * MICROS)
    }

    /// The first boundary strictly after `us`.
    pub(crate) fn after(self, us: i64) -> i64 {
        (us * self.fps).div_euclid(MICROS) + 1
    }

    /// The last boundary strictly before `us`.
    pub(crate) fn before(self, us: i64) -> i64 {
        (us * self.fps + MICROS - 1).div_euclid(MICROS) - 1
    }

    /// The last boundary at or before `us`.
    pub(crate) fn at_or_before(self, us: i64) -> i64 {
        (us * self.fps).div_euclid(MICROS)
    }

    /// Whether boundary `frame` lies within `start_us..=end_us`.
    pub(crate) fn within(self, frame: i64, start_us: i64, end_us: i64) -> bool {
        let at = frame * MICROS;
        start_us * self.fps <= at && at <= end_us * self.fps
    }

    /// Whether boundary `frame` lies strictly after `us`.
    pub(crate) fn is_after(self, frame: i64, us: i64) -> bool {
        frame * MICROS > us * self.fps
    }

    /// Whether boundary `frame` lies strictly before `us`.
    pub(crate) fn is_before(self, frame: i64, us: i64) -> bool {
        frame * MICROS < us * self.fps
    }

    /// Whether some boundary lies strictly between `start_us` and `end_us`.
    pub(crate) fn fits_between(self, start_us: i64, end_us: i64) -> bool {
        self.after(start_us) <= self.before(end_us)
    }

    /// How many frames last at least `secs` seconds, rounded up.
    pub(crate) fn frames_for(self, secs: f64) -> i64 {
        (micros(secs) * self.fps + MICROS - 1).div_euclid(MICROS)
    }

    /// Boundary `frame` in seconds, for reports.
    pub(crate) fn seconds(self, frame: i64) -> f64 {
        frame as f64 / self.fps as f64
    }

    /// Boundary `frame` in whole milliseconds, rounded to the nearest. Written to a plan,
    /// this snaps back to `frame` at any rate up to 240 fps, because it is off by at
    /// most half a millisecond and frames are more than 4 ms apart.
    pub(crate) fn millis(self, frame: i64) -> i64 {
        (2 * frame * 1000 + self.fps).div_euclid(2 * self.fps)
    }
}

/// Milliseconds as seconds with three decimals, like `3.133`.
pub(crate) fn millis_text(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.unsigned_abs();
    format!("{sign}{}.{:03}", ms / 1000, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_matches_core_rounding() {
        let grid = Grid::new(30);
        assert_eq!(grid.nearest(micros(3.12)), 94); // 93.6
        assert_eq!(grid.nearest(micros(9.88)), 296); // 296.4
        assert_eq!(grid.nearest(micros(0.05)), 2); // exactly 1.5 rounds up
    }

    #[test]
    fn strict_neighbours() {
        let grid = Grid::new(30);
        // 1.0 s is boundary 30 exactly.
        assert_eq!(grid.after(micros(1.0)), 31);
        assert_eq!(grid.before(micros(1.0)), 29);
        assert_eq!(grid.at_or_before(micros(1.0)), 30);
        // 1.01 s is between 30 and 31.
        assert_eq!(grid.after(micros(1.01)), 31);
        assert_eq!(grid.before(micros(1.01)), 30);
        assert!(grid.fits_between(micros(1.0), micros(1.04)));
        assert!(!grid.fits_between(micros(1.0), micros(1.03)));
        assert_eq!(grid.before(0), -1);
    }

    #[test]
    fn within_is_closed() {
        let grid = Grid::new(30);
        assert!(grid.within(30, micros(0.9), micros(1.0)));
        assert!(grid.within(30, micros(1.0), micros(1.2)));
        assert!(!grid.within(30, micros(1.001), micros(1.2)));
    }

    #[test]
    fn before_and_after_are_strict() {
        let grid = Grid::new(30);
        assert!(grid.is_before(30, micros(1.001)));
        assert!(!grid.is_before(30, micros(1.0)));
        assert!(grid.is_after(30, micros(0.999)));
        assert!(!grid.is_after(30, micros(1.0)));
    }

    #[test]
    fn written_millis_snap_back_to_the_same_frame_at_every_rate() {
        for fps in 1..=240 {
            let grid = Grid::new(fps);
            let last = i64::from(fps) * 86_400;
            for frame in (0..2_000).chain(last - 2_000..=last) {
                let ms = grid.millis(frame);
                assert_eq!(grid.nearest(ms * 1000), frame, "{fps} fps, frame {frame}");
            }
        }
    }

    #[test]
    fn frames_for_rounds_up() {
        let grid = Grid::new(30);
        assert_eq!(grid.frames_for(0.2), 6);
        assert_eq!(grid.frames_for(0.21), 7);
        assert_eq!(grid.frames_for(0.0), 0);
    }

    #[test]
    fn millis_text_has_three_decimals() {
        assert_eq!(millis_text(3133), "3.133");
        assert_eq!(millis_text(0), "0.000");
        assert_eq!(millis_text(12_000), "12.000");
        assert_eq!(millis_text(-5), "-0.005");
    }
}
