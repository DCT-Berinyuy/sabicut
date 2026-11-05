//! Where TikTok, Instagram Reels and YouTube Shorts draw their own interface over a
//! vertical video. Captions and the hook are laid out clear of these zones, and
//! `sabicut preview --safe-zones` shades them.
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
//! Fractions of the frame are used so the zones fit any `meta.out` size.

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

/// The status bar, the Following/For You tabs and search: the top 12.5%, 240 px on
/// 1920, which clears the tallest top bar (Shorts, 234 px).
pub const TOP_BAR: Zone = Zone {
    name: "top bar",
    left: 0.0,
    top: 0.0,
    right: 1.0,
    bottom: 0.125,
};

/// The caption, sound ticker, profile and buttons: from y 1440 on 1920, which clears
/// the Shorts subscribe button (1480), the bottom of TikTok's action column (1536) and
/// every caption and sound bar.
pub const BOTTOM: Zone = Zone {
    name: "caption and buttons",
    left: 0.0,
    top: 0.75,
    right: 1.0,
    bottom: 1.0,
};

/// Like, comment, share and remix buttons: x 918–1080 and y 480–1440 on 1080×1920,
/// which clears all three action columns above the bottom zone (they start at x 918 or
/// further right, and at y 499 or lower).
pub const ACTION_COLUMN: Zone = Zone {
    name: "action column",
    left: 0.85,
    top: 0.25,
    right: 1.0,
    bottom: 0.75,
};

/// Every zone.
pub const ZONES: [Zone; 3] = [TOP_BAR, BOTTOM, ACTION_COLUMN];

/// The horizontal band, as fractions of the frame width, that text centred on the frame
/// can use anywhere between the top bar and the bottom zone: it stays left of the action
/// column, and as far from the left edge as from the right.
pub const TEXT_COLUMN: (f64, f64) = (1.0 - ACTION_COLUMN.left, ACTION_COLUMN.left);

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_text_column_misses_the_action_column() {
        assert!((TEXT_COLUMN.0 - 0.15).abs() < 1e-12);
        assert!(TEXT_COLUMN.1 <= ACTION_COLUMN.left);
        assert!((TEXT_COLUMN.0 - (1.0 - TEXT_COLUMN.1)).abs() < 1e-12);
    }
}
