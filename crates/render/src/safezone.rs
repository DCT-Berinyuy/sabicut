//! The safe-zone guide on preview stills: where TikTok, Instagram Reels and YouTube
//! Shorts draw their own interface over a vertical video, so captions, faces and text
//! can be kept clear of it.
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

/// A region of the frame that platform UI covers, as fractions of the output frame
/// (0 = left or top edge, 1 = right or bottom edge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    /// What covers it.
    pub name: &'static str,
    /// Left edge.
    pub left: f64,
    /// Top edge.
    pub top: f64,
    /// Right edge.
    pub right: f64,
    /// Bottom edge.
    pub bottom: f64,
}

/// The zones the guide shades.
pub const ZONES: [Zone; 3] = [
    // 240 px on 1920: clears the tallest top bar (Shorts, 234 px).
    Zone {
        name: "top bar",
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 0.125,
    },
    // From y 1440 on 1920: clears the Shorts subscribe button (1480), the bottom of
    // TikTok's action column (1536) and every caption and sound bar.
    Zone {
        name: "caption and buttons",
        left: 0.0,
        top: 0.75,
        right: 1.0,
        bottom: 1.0,
    },
    // x 918–1080 and y 480–1440 on 1080×1920: clears all three action columns above
    // the bottom zone (they start at x 918 or further right, and at y 499 or lower).
    Zone {
        name: "action column",
        left: 0.85,
        top: 0.25,
        right: 1.0,
        bottom: 0.75,
    },
];

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

    #[test]
    fn zones_cover_every_measured_platform_element() {
        // (x0, y0, x1, y1) on 1080×1920, from the table in the module docs.
        let elements = [
            (0, 0, 1080, 130),
            (0, 0, 1080, 210),
            (0, 0, 1080, 234),
            (0, 1570, 1080, 1920),
            (0, 1590, 1080, 1920),
            (0, 1575, 1080, 1920),
            (48, 1480, 228, 1560),
            (918, 730, 1080, 1536),
            (996, 614, 1080, 1460),
            (936, 499, 1080, 1460),
        ];
        let covered = |x: u32, y: u32| {
            ZONES.iter().any(|z| {
                (f64::from(x) + 0.5) / 1080.0 >= z.left
                    && (f64::from(x) + 0.5) / 1080.0 <= z.right
                    && (f64::from(y) + 0.5) / 1920.0 >= z.top
                    && (f64::from(y) + 0.5) / 1920.0 <= z.bottom
            })
        };
        for (x0, y0, x1, y1) in elements {
            for y in (y0..y1).step_by(4) {
                for x in (x0..x1).step_by(4) {
                    assert!(
                        covered(x, y),
                        "pixel {x},{y} of element {x0},{y0}-{x1},{y1}"
                    );
                }
            }
        }
    }
}
