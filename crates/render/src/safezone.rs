//! The safe-zone guide on preview stills: where TikTok, Instagram Reels and YouTube
//! Shorts draw their own interface over a vertical video, so captions, faces and text
//! can be kept clear of it. The zones themselves are defined in `sabicut-overlay`,
//! which lays captions and the hook out clear of them.
//!
//! The zones are the union of the three apps' organic (non-ad) layouts on a 1080×1920
//! frame, rounded outwards, as measured by CapQA (capqa.app/guides/social-media-safe-zones,
//! version of 2026-09-11):
//!
//! | App | Top bar | Bottom (caption, sound, buttons) | Right action column |
//! |---|---|---|---|
//! | TikTok | 130 px | from y 1570 | x 918–1080, y 730–1536 |
//! | Reels | 210 px | from y 1590 | x 996–1080, y 614–1460 |
//! | Shorts | 234 px | from y 1575, subscribe button from y 1480 | x 936–1080, y 499–1460 |
//!
//! Fractions of the frame are used so the guide fits any `meta.out` size.

use std::fmt::Write as _;

pub use sabicut_overlay::safe_zone::{ZONES, Zone};

/// Fill colour of the shaded zones.
const SHADE: &str = "0xFF3B30@0.40";
/// Colour and width of the line around each zone.
const EDGE: &str = "0xFFFFFF@0.85";
const EDGE_PX: u32 = 3;

/// `drawbox` filters that shade [`ZONES`] on a `width`×`height` frame, with a thin
/// edge so the boundary shows on any picture.
pub(crate) fn guide_filters(width: u32, height: u32) -> String {
    let mut filters = String::new();
    for zone in ZONES {
        let (x0, x1) = (scale(zone.left, width), scale(zone.right, width));
        let (y0, y1) = (scale(zone.top, height), scale(zone.bottom, height));
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if !filters.is_empty() {
            filters.push(',');
        }
        // Writing to a String cannot fail.
        let _ = write!(
            filters,
            "drawbox=x={x0}:y={y0}:w={w}:h={h}:color={SHADE}:t=fill,drawbox=x={x0}:y={y0}:w={w}:h={h}:color={EDGE}:t={EDGE_PX}"
        );
    }
    filters
}

fn scale(fraction: f64, size: u32) -> u32 {
    let pixels = (fraction.clamp(0.0, 1.0) * f64::from(size)).round();
    // Between 0 and `size`, and whole, so the cast is exact.
    pixels as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_on_a_1080_by_1920_frame() {
        let filters = guide_filters(1080, 1920);
        let shades: Vec<&str> = filters
            .split(',')
            .filter(|f| f.contains("t=fill"))
            .collect();
        assert_eq!(
            shades,
            [
                "drawbox=x=0:y=0:w=1080:h=240:color=0xFF3B30@0.40:t=fill",
                "drawbox=x=0:y=1440:w=1080:h=480:color=0xFF3B30@0.40:t=fill",
                "drawbox=x=918:y=480:w=162:h=960:color=0xFF3B30@0.40:t=fill",
            ]
        );
    }
}
