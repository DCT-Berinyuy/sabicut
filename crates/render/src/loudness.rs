//! Two-pass loudness normalization to −14 LUFS (PRD F5) with ffmpeg's `loudnorm`:
//! the first pass measures, the second applies one constant gain when the true-peak
//! limit allows it, which lands much closer to the target than a single pass.

use serde::Deserialize;

/// Target integrated loudness, in LUFS.
pub const TARGET_LUFS: f64 = -14.0;
/// Highest allowed true peak, in dBTP.
pub const TARGET_TRUE_PEAK: f64 = -1.5;
/// Target loudness range, in LU.
pub const TARGET_RANGE: f64 = 11.0;

/// Audio quieter than this (integrated, LUFS) counts as silence and is left alone.
const SILENCE_LUFS: f64 = -70.0;

/// A loudness measurement (EBU R128) by `loudnorm`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, in LUFS.
    pub integrated: f64,
    /// True peak, in dBTP.
    pub true_peak: f64,
    /// Loudness range, in LU.
    pub range: f64,
    /// Gating threshold, in LUFS.
    pub threshold: f64,
    /// Gain offset `loudnorm` suggests for the second pass, in LU.
    pub offset: f64,
}

impl Loudness {
    /// Whether the audio is silent (or as good as), so there is nothing to normalize.
    pub fn is_silent(&self) -> bool {
        !(self.integrated.is_finite() && self.integrated > SILENCE_LUFS)
    }

    /// The second-pass filter: normalize to the targets using this measurement.
    pub(crate) fn normalize_filter(&self) -> String {
        if self.is_silent() {
            return "anull".to_owned();
        }
        format!(
            "loudnorm=I={TARGET_LUFS}:TP={TARGET_TRUE_PEAK}:LRA={TARGET_RANGE}:measured_I={:.2}:measured_TP={:.2}:measured_LRA={:.2}:measured_thresh={:.2}:offset={:.2}:linear=true:print_format=none",
            self.integrated.clamp(-99.0, 0.0),
            clamp_or(self.true_peak, -99.0, 99.0, 0.0),
            clamp_or(self.range, 0.0, 99.0, 0.0),
            clamp_or(self.threshold, -99.0, 0.0, -99.0),
            clamp_or(self.offset, -99.0, 99.0, 0.0),
        )
    }
}

/// `value` clamped to `min..=max`, or `fallback` if it is not a number.
fn clamp_or(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(min, max)
    }
}

/// The first-pass filter: measure, and print the result as JSON on stderr.
pub(crate) fn measure_filter() -> String {
    format!("loudnorm=I={TARGET_LUFS}:TP={TARGET_TRUE_PEAK}:LRA={TARGET_RANGE}:print_format=json")
}

#[derive(Deserialize)]
struct Report {
    input_i: String,
    input_tp: String,
    input_lra: String,
    input_thresh: String,
    target_offset: String,
}

/// Reads the JSON block `loudnorm` prints at the end of a run (at log level `info`).
pub(crate) fn parse(stderr: &str) -> Option<Loudness> {
    let start = stderr.rfind('{')?;
    let end = start + stderr.get(start..)?.find('}')?;
    let report: Report = serde_json::from_str(stderr.get(start..=end)?).ok()?;
    let number = |text: &str| text.trim().parse::<f64>().ok();
    Some(Loudness {
        integrated: number(&report.input_i)?,
        true_peak: number(&report.input_tp)?,
        range: number(&report.input_lra)?,
        threshold: number(&report.input_thresh)?,
        offset: number(&report.target_offset)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STDERR: &str = r#"Input #0, wav, from 'audio.wav':
  Duration: 00:00:13.73, bitrate: 3072 kb/s
[Parsed_loudnorm_0 @ 0x5581c0d2e2c0]
{
	"input_i" : "-27.61",
	"input_tp" : "-4.47",
	"input_lra" : "18.06",
	"input_thresh" : "-39.20",
	"output_i" : "-16.58",
	"output_tp" : "-1.50",
	"output_lra" : "14.78",
	"output_thresh" : "-27.71",
	"normalization_type" : "dynamic",
	"target_offset" : "0.58"
}
"#;

    #[test]
    fn measurement_is_read_from_stderr() {
        let loudness = parse(STDERR).unwrap();
        assert_eq!(
            loudness,
            Loudness {
                integrated: -27.61,
                true_peak: -4.47,
                range: 18.06,
                threshold: -39.2,
                offset: 0.58
            }
        );
        assert_eq!(
            loudness.normalize_filter(),
            "loudnorm=I=-14:TP=-1.5:LRA=11:measured_I=-27.61:measured_TP=-4.47:measured_LRA=18.06:measured_thresh=-39.20:offset=0.58:linear=true:print_format=none"
        );
    }

    #[test]
    fn silence_is_left_alone() {
        let silent = parse(&STDERR.replace("\"-27.61\"", "\"-inf\"")).unwrap();
        assert!(silent.is_silent());
        assert_eq!(silent.normalize_filter(), "anull");
    }

    #[test]
    fn missing_or_broken_reports_are_none() {
        assert_eq!(parse("no json here"), None);
        assert_eq!(parse("{\"input_i\": \"-20\"}"), None);
    }
}
