//! Easing curves for the caption pop and the hook's entrance. Each maps progress from 0
//! to 1 onto 0 to 1 (ease-out-back overshoots on the way). Only plain arithmetic is used,
//! so results are the same on every machine.

/// How far ease-out-back overshoots: the classic value (Penner's), about 10%.
const BACK: f64 = 1.70158;

/// Ease-out cubic: fast at first, settling gently.
pub fn ease_out_cubic(progress: f64) -> f64 {
    let t = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - t * t * t
}

/// Ease-out-back: overshoots its target by about 10% around 60% of the way, then
/// settles on it, like a pop.
pub fn ease_out_back(progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0) - 1.0;
    1.0 + (BACK + 1.0) * t * t * t + BACK * t * t
}

/// The progress of an animation that lasts `frames` frames, on its frame `frame` (0 is
/// the first): the first frame already shows movement and the last one is at 1.
pub fn frame_progress(frame: u64, frames: u64) -> f64 {
    if frames == 0 {
        return 1.0;
    }
    let done = frame.saturating_add(1).min(frames);
    done as f64 / frames as f64
}

/// The number of frames an animation of `seconds` lasts at `fps`, at least 1.
pub fn frames_for(seconds: f64, fps: u32) -> u64 {
    let frames = (seconds * f64::from(fps)).round();
    // NaN is in no range, so it falls through too.
    if (1.0..1e9).contains(&frames) {
        // Whole, positive and in range, so the cast is exact.
        frames as u64
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_start_at_0_and_end_at_1() {
        for curve in [ease_out_cubic, ease_out_back] {
            assert!(curve(0.0).abs() < 1e-12);
            assert!((curve(1.0) - 1.0).abs() < 1e-12);
            assert_eq!(curve(-3.0), curve(0.0));
            assert_eq!(curve(7.0), curve(1.0));
        }
    }

    #[test]
    fn ease_out_back_overshoots_by_about_ten_percent() {
        let peak = (0..=1000)
            .map(|i| ease_out_back(f64::from(i) / 1000.0))
            .fold(0.0, f64::max);
        assert!((1.09..1.11).contains(&peak), "{peak}");
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn progress_counts_the_current_frame() {
        assert_eq!(frame_progress(0, 4), 0.25);
        assert_eq!(frame_progress(3, 4), 1.0);
        assert_eq!(frame_progress(9, 4), 1.0);
        assert_eq!(frame_progress(0, 0), 1.0);
        assert_eq!(frames_for(0.18, 30), 5);
        assert_eq!(frames_for(0.001, 30), 1);
        assert_eq!(frames_for(f64::NAN, 30), 1);
    }
}
