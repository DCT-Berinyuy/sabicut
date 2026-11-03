//! Punch-in zooms: the zoom level over a segment, frame by frame.
//!
//! Plan schema v1 only says when a zoom starts and how far it goes, so the motion is
//! defined here (and in `docs/render.md`):
//!
//! - Every cut starts unzoomed (scale 1). A zoom does not carry over a cut.
//! - From its first frame, a zoom eases from the current level to its scale over
//!   [`EASE_SECONDS`] (an ease-out cubic: fast at first, settling gently), cut short if
//!   the next zoom or the end of the cut comes sooner. So each zoom reaches its scale
//!   before the next one starts.
//! - It then holds that scale until the next zoom or the end of the cut. A zoom with
//!   scale 1 eases back out.
//! - The zoom is centred on the crop window.

use sabicut_core::Segment;

/// How long a zoom takes to reach its scale, in seconds.
pub const EASE_SECONDS: f64 = 0.2;

/// The zoom level over one segment.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomCurve {
    steps: Vec<Step>,
}

/// One zoom: from local frame `start`, ease from `from` to `to` over `ease` frames.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Step {
    start: u64,
    ease: u64,
    from: f64,
    to: f64,
}

impl ZoomCurve {
    /// The zoom curve of `segment` at `fps` frames per second. Frame numbers are local
    /// to the segment: 0 is its first frame.
    pub fn new(segment: &Segment, fps: u32) -> Self {
        let ease_frames = ease_frames(fps);
        let frames = segment.frames();
        let starts: Vec<(u64, f64)> = segment
            .zooms()
            .iter()
            .map(|zoom| {
                let start = zoom.output_frame().saturating_sub(segment.output_start());
                (start.min(frames.saturating_sub(1)), zoom.scale())
            })
            .collect();
        let mut steps = Vec::with_capacity(starts.len());
        let mut level = 1.0;
        for (k, &(start, to)) in starts.iter().enumerate() {
            let next = starts.get(k + 1).map_or(frames, |&(next, _)| next);
            let ease = ease_frames.min(next.saturating_sub(start)).max(1);
            steps.push(Step {
                start,
                ease,
                from: level,
                to,
            });
            level = to;
        }
        Self { steps }
    }

    /// Whether the level is 1 on every frame, so the segment needs no zoom filter.
    pub fn is_flat(&self) -> bool {
        self.steps
            .iter()
            .all(|step| step.from == 1.0 && step.to == 1.0)
    }

    /// The zoom level on local frame `n`.
    pub fn level(&self, n: u64) -> f64 {
        let mut level = 1.0;
        for step in &self.steps {
            if n < step.start {
                break;
            }
            let done = n - step.start + 1;
            level = if done >= step.ease {
                step.to
            } else {
                step.from + (step.to - step.from) * ease_out(done as f64 / step.ease as f64)
            };
        }
        level
    }

    /// An ffmpeg expression for the level, given an expression `n` for the local frame
    /// number. It computes exactly what [`ZoomCurve::level`] does.
    pub fn expression(&self, n: &str) -> String {
        let mut expr = self
            .steps
            .last()
            .map_or_else(|| "1".to_owned(), |step| format!("{}", step.to));
        for step in self.steps.iter().rev() {
            let (start, ease, from, to) = (step.start, step.ease, step.from, step.to);
            let end = start + ease;
            let easing = if ease == 1 {
                format!("{to}")
            } else {
                format!("{from}+({to}-{from})*(1-pow(1-({n}-{start}+1)/{ease},3))")
            };
            expr = format!("if(lt({n},{start}),{from},if(lt({n},{end}),{easing},{expr}))");
        }
        expr
    }
}

/// How many frames a zoom takes to reach its scale: [`EASE_SECONDS`] rounded to whole
/// frames, at least 1.
pub fn ease_frames(fps: u32) -> u64 {
    let frames = (EASE_SECONDS * f64::from(fps)).round();
    // fps is at most a few hundred, so this is small, whole and positive.
    (frames as u64).max(1)
}

/// Ease-out cubic: 0 at 0, 1 at 1, fast at first and settling gently.
fn ease_out(progress: f64) -> f64 {
    1.0 - (1.0 - progress).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sabicut_core::{Plan, Timeline};

    fn segment(zooms: &str, fps: u32) -> Segment {
        let plan = Plan::from_toml_str(&format!(
            r#"
            schema = 1
            [meta]
            fps = {fps}
            out = {{ w = 1080, h = 1920 }}
            [[source]]
            id = "a"
            path = "a.mp4"
            crop_x = 0.5
            [[cut]]
            src = "a"
            in = 10.0
            out = 12.0
            zoom = [{zooms}]
            "#
        ))
        .unwrap();
        Timeline::from_plan(&plan).unwrap().segments()[0].clone()
    }

    /// Evaluates the ffmpeg expression for a frame number, with the handful of
    /// functions it uses.
    fn eval(expr: &str, n: u64) -> f64 {
        struct P<'a> {
            s: &'a [u8],
            i: usize,
            n: f64,
        }
        impl P<'_> {
            fn peek(&self) -> u8 {
                self.s.get(self.i).copied().unwrap_or(0)
            }
            fn eat(&mut self, c: u8) {
                assert_eq!(self.peek(), c, "at {}", self.i);
                self.i += 1;
            }
            fn sum(&mut self) -> f64 {
                let mut v = self.product();
                loop {
                    match self.peek() {
                        b'+' => {
                            self.i += 1;
                            v += self.product();
                        }
                        b'-' => {
                            self.i += 1;
                            v -= self.product();
                        }
                        _ => return v,
                    }
                }
            }
            fn product(&mut self) -> f64 {
                let mut v = self.atom();
                loop {
                    match self.peek() {
                        b'*' => {
                            self.i += 1;
                            v *= self.atom();
                        }
                        b'/' => {
                            self.i += 1;
                            v /= self.atom();
                        }
                        _ => return v,
                    }
                }
            }
            fn args(&mut self) -> Vec<f64> {
                self.eat(b'(');
                let mut args = vec![self.sum()];
                while self.peek() == b',' {
                    self.i += 1;
                    args.push(self.sum());
                }
                self.eat(b')');
                args
            }
            fn atom(&mut self) -> f64 {
                let rest = &self.s[self.i..];
                if rest.starts_with(b"if(") {
                    self.i += 2;
                    let a = self.args();
                    return if a[0] != 0.0 { a[1] } else { a[2] };
                }
                if rest.starts_with(b"lt(") {
                    self.i += 2;
                    let a = self.args();
                    return f64::from(u8::from(a[0] < a[1]));
                }
                if rest.starts_with(b"pow(") {
                    self.i += 3;
                    let a = self.args();
                    return a[0].powf(a[1]);
                }
                if rest.starts_with(b"N") {
                    self.i += 1;
                    return self.n;
                }
                if self.peek() == b'(' {
                    self.i += 1;
                    let v = self.sum();
                    self.eat(b')');
                    return v;
                }
                if self.peek() == b'-' {
                    self.i += 1;
                    return -self.atom();
                }
                let start = self.i;
                while matches!(self.peek(), b'0'..=b'9' | b'.' | b'e') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .unwrap()
                    .parse()
                    .unwrap()
            }
        }
        let mut p = P {
            s: expr.as_bytes(),
            i: 0,
            n: n as f64,
        };
        let v = p.sum();
        assert_eq!(p.i, expr.len(), "trailing input in {expr}");
        v
    }

    #[test]
    fn no_zoom_is_flat() {
        let curve = ZoomCurve::new(&segment("", 30), 30);
        assert!(curve.is_flat());
        assert_eq!(curve.level(0), 1.0);
        assert_eq!(curve.expression("N"), "1");
    }

    #[test]
    fn a_zoom_eases_in_then_holds() {
        // At 30 fps the cut starts at source frame 300; the zoom at 10.5 s is local
        // frame 15 and eases over 6 frames.
        let curve = ZoomCurve::new(&segment("{ at = 10.5, scale = 1.5 }", 30), 30);
        assert!(!curve.is_flat());
        assert_eq!(curve.level(14), 1.0);
        assert!(curve.level(15) > 1.0 && curve.level(15) < 1.5);
        assert!(curve.level(19) < 1.5);
        assert_eq!(curve.level(20), 1.5);
        assert_eq!(curve.level(59), 1.5);
        // Each eased frame moves less than the one before.
        let steps: Vec<f64> = (15..=20)
            .map(|n| curve.level(n) - curve.level(n - 1))
            .collect();
        assert!(steps.windows(2).all(|w| w[1] < w[0]), "{steps:?}");
    }

    #[test]
    fn close_zooms_finish_before_the_next_starts() {
        // Zooms 2 frames apart: the first eases over 2 frames, the second over the
        // usual 6, and the last (1 frame before the end of the cut) jumps.
        let curve = ZoomCurve::new(
            &segment(
                "{ at = 10.5, scale = 2.0 }, { at = 10.5667, scale = 1.25 }, { at = 11.9667, scale = 1.0 }",
                30,
            ),
            30,
        );
        assert_eq!(curve.level(16), 2.0);
        assert!(curve.level(17) < 2.0 && curve.level(17) > 1.25);
        assert!(curve.level(21) > 1.25);
        assert_eq!(curve.level(22), 1.25);
        assert_eq!(curve.level(58), 1.25);
        assert_eq!(curve.level(59), 1.0);
    }

    #[test]
    fn the_expression_matches_level_on_every_frame() {
        for fps in [24, 30, 60] {
            let curve = ZoomCurve::new(
                &segment(
                    "{ at = 10.0, scale = 1.3 }, { at = 10.25, scale = 4.0 }, { at = 11.0, scale = 1.0 }, { at = 11.9, scale = 1.1 }",
                    fps,
                ),
                fps,
            );
            let expr = curve.expression("N");
            for n in 0..u64::from(2 * fps) {
                let (want, got) = (curve.level(n), eval(&expr, n));
                assert!(
                    (want - got).abs() < 1e-9,
                    "fps {fps} frame {n}: {want} vs {got}"
                );
            }
        }
    }

    #[test]
    fn ease_length_rounds_to_frames() {
        assert_eq!(ease_frames(30), 6);
        assert_eq!(ease_frames(24), 5);
        assert_eq!(ease_frames(1), 1);
        assert_eq!(ease_frames(2), 1);
    }
}
