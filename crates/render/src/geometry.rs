//! Where the vertical crop window sits on a source frame.

/// A crop window on the upright source frame, in pixels. All four values are even, so
/// the window lines up with 4:2:0 chroma.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    /// Width of the window.
    pub w: u32,
    /// Height of the window.
    pub h: u32,
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
}

/// The largest window with the output's shape (9:16 for 1080×1920) that fits on an
/// upright `width`×`height` frame whose pixels have aspect ratio `sar` (num, den).
///
/// A frame wider than the output keeps its full height, and the window's centre sits
/// at `crop_x` (0 = left edge, 1 = right edge) of the frame's width, moved inwards as
/// needed to stay on the frame. A frame taller than the output keeps its full width and
/// is cropped equally at the top and bottom; `crop_x` does not apply.
pub fn crop_window(
    width: u32,
    height: u32,
    sar: (u32, u32),
    crop_x: f64,
    out: (u32, u32),
) -> CropRect {
    let (w, h) = (f64::from(width), f64::from(height));
    let (sar_num, sar_den) = (f64::from(sar.0.max(1)), f64::from(sar.1.max(1)));
    let (out_w, out_h) = (f64::from(out.0.max(1)), f64::from(out.1.max(1)));
    // Compare display aspect ratios: (w·sar) / h against out_w / out_h.
    if w * sar_num * out_h > out_w * h * sar_den {
        let crop_w = even_at_most(h * out_w * sar_den / (out_h * sar_num), width);
        let centre = crop_x.clamp(0.0, 1.0) * w;
        let max_x = width.saturating_sub(crop_w);
        let x = even_floor((centre - f64::from(crop_w) / 2.0).clamp(0.0, f64::from(max_x)));
        CropRect {
            w: crop_w,
            h: even_floor_u32(height),
            x,
            y: 0,
        }
    } else {
        let crop_h = even_at_most(w * sar_num * out_h / (out_w * sar_den), height);
        let y = even_floor(f64::from(height.saturating_sub(crop_h)) / 2.0);
        CropRect {
            w: even_floor_u32(width),
            h: crop_h,
            x: 0,
            y,
        }
    }
}

/// `value` rounded to the nearest even number, at least 2 and at most `limit` (rounded
/// down to even).
fn even_at_most(value: f64, limit: u32) -> u32 {
    let rounded = (value / 2.0).round() * 2.0;
    let limit = even_floor_u32(limit).max(2);
    if rounded.is_nan() || rounded < 2.0 {
        2
    } else if rounded >= f64::from(limit) {
        limit
    } else {
        // Between 2 and `limit`, and whole, so the cast is exact.
        rounded as u32
    }
}

/// The even number at or below a non-negative `value`.
fn even_floor(value: f64) -> u32 {
    if value.is_nan() || value <= 0.0 {
        0
    } else if value >= f64::from(u32::MAX) {
        u32::MAX - 1
    } else {
        // In range and truncated, so the cast is exact.
        even_floor_u32(value as u32)
    }
}

fn even_floor_u32(value: u32) -> u32 {
    value & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: (u32, u32) = (1080, 1920);

    #[test]
    fn landscape_keeps_full_height_and_follows_crop_x() {
        // 1080 × 9/16 = 607.5, rounded to the nearest even width.
        let centre = crop_window(1920, 1080, (1, 1), 0.5, OUT);
        assert_eq!(
            centre,
            CropRect {
                w: 608,
                h: 1080,
                x: 656,
                y: 0
            }
        );
        let left = crop_window(1920, 1080, (1, 1), 0.0, OUT);
        assert_eq!(left.x, 0);
        let right = crop_window(1920, 1080, (1, 1), 1.0, OUT);
        assert_eq!(right.x + right.w, 1920);
        assert_eq!(right.x, 1312);
        let off_centre = crop_window(1920, 1080, (1, 1), 0.42, OUT);
        // centre 806.4 − 304 = 502.4, rounded down to even.
        assert_eq!(off_centre.x, 502);
    }

    #[test]
    fn portrait_at_the_output_shape_is_untouched() {
        assert_eq!(
            crop_window(1080, 1920, (1, 1), 0.3, OUT),
            CropRect {
                w: 1080,
                h: 1920,
                x: 0,
                y: 0
            }
        );
        assert_eq!(
            crop_window(720, 1280, (1, 1), 0.5, OUT),
            CropRect {
                w: 720,
                h: 1280,
                x: 0,
                y: 0
            }
        );
    }

    #[test]
    fn taller_than_the_output_is_cropped_top_and_bottom() {
        // A 9:19.5 phone screen recording.
        let crop = crop_window(1170, 2532, (1, 1), 0.9, OUT);
        assert_eq!(
            crop,
            CropRect {
                w: 1170,
                h: 2080,
                x: 0,
                y: 226
            }
        );
    }

    #[test]
    fn four_by_three_portrait_is_cropped_at_the_sides() {
        let crop = crop_window(1080, 1440, (1, 1), 0.5, OUT);
        assert_eq!(
            crop,
            CropRect {
                w: 810,
                h: 1440,
                x: 134,
                y: 0
            }
        );
    }

    #[test]
    fn non_square_pixels_are_taken_into_account() {
        // 1440×1080 stored with 4:3 pixels is 1920×1080 on screen, so the window is
        // 607.5 display pixels = 455.6 stored pixels wide.
        let crop = crop_window(1440, 1080, (4, 3), 0.5, OUT);
        assert_eq!(crop.w, 456);
        assert_eq!(crop.x, 492);
    }

    #[test]
    fn odd_and_tiny_frames_stay_even_and_on_the_frame() {
        let crop = crop_window(1921, 1081, (1, 1), 1.0, OUT);
        assert_eq!(crop.h, 1080);
        assert!(crop.x + crop.w <= 1921);
        assert_eq!((crop.w % 2, crop.x % 2), (0, 0));
        let tiny = crop_window(3, 2, (1, 1), 0.5, OUT);
        assert_eq!((tiny.w, tiny.h), (2, 2));
    }
}
