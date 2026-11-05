//! Everything drawn or written from a plan's words and hook: [`Graphics`].

use std::ops::Range;
use std::path::PathBuf;

use sabicut_core::{Captions, Plan, Timeline};

use crate::captions::{CaptionLayout, TextStyle, Timing, cap_height};
use crate::error::OverlayError;
use crate::font::Font;
use crate::hook::{HookLayout, HookText};
use crate::layout::{Frame, Placement};
use crate::paint::Painter;
use crate::shape::Shaper;
use crate::srt;
use crate::timing::{OutputWord, WordTimings, place_words};

/// Text fill and outline colours, RGBA.
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// The captions, hook and subtitles of one plan, laid out and ready to draw.
///
/// Build it with [`Graphics::load`] (reads the files the plan names) or
/// [`Graphics::new`]; draw frames with a [`Painter`].
#[derive(Clone, Debug, PartialEq)]
pub struct Graphics {
    fps: u32,
    total_frames: u64,
    words: Option<Vec<OutputWord>>,
    pub(crate) captions: Option<CaptionLayout>,
    pub(crate) hook: Option<HookLayout>,
    font: String,
    missing: Vec<char>,
}

/// Where and when the hook is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
    /// Where its pictures go on the output frame.
    pub placement: Placement,
    /// The output frames it is drawn on.
    pub frames: Range<u64>,
    /// How many of its first frames are animated; it stays the same after them.
    pub animated: u64,
}

impl Graphics {
    /// Lays out the captions and hook of `plan`, whose output timeline is `timeline`,
    /// with `words` (fixes already applied) and `font`.
    ///
    /// Captions need word timings for every source a cut uses. Without `[captions]`,
    /// word timings are still placed on the output for subtitles when every source has
    /// them.
    pub fn new(
        plan: &Plan,
        timeline: &Timeline,
        words: &WordTimings,
        font: &Font,
    ) -> Result<Self, OverlayError> {
        let out = plan.meta().out();
        let frame = Frame::new(out.w(), out.h());
        let timing = Timing {
            fps: timeline.fps(),
            total_frames: timeline.total_frames(),
        };
        let settings = plan.captions();
        let keywords = settings.map_or(&[][..], Captions::keywords);
        let placed = place_words(timeline, words, keywords);
        let face = font.face().map_err(|source| OverlayError::Font {
            path: PathBuf::from(font.name()),
            source,
        })?;
        let mut shaper = Shaper::new(face);
        let base = base_style(settings, &frame, &shaper)?;
        let captions = match settings {
            Some(_) => {
                let placed = placed
                    .as_deref()
                    .ok_or_else(|| missing_words(plan, timeline, words))?;
                Some(CaptionLayout::new(
                    &frame,
                    &mut shaper,
                    placed,
                    base.clone(),
                    timing,
                )?)
            }
            None => None,
        };
        let hook = match (plan.hook(), timeline.hook_frames()) {
            (Some(hook), Some(frames)) => Some(HookLayout::new(
                &frame,
                &mut shaper,
                HookText {
                    text: hook.text(),
                    keywords,
                    frames,
                    fps: timing.fps,
                },
                &base,
                captions.as_ref().map(|captions| captions.band),
            )?),
            _ => None,
        };
        Ok(Self {
            fps: timing.fps,
            total_frames: timing.total_frames,
            words: placed,
            captions,
            hook,
            font: font.name().to_owned(),
            missing: shaper.missing().to_vec(),
        })
    }

    /// Where the caption band goes on the output frame, if the plan has captions.
    pub fn band(&self) -> Option<Placement> {
        self.captions.as_ref().map(|captions| captions.band)
    }

    /// Where and when the hook is drawn, if the plan has one.
    pub fn hook(&self) -> Option<Hook> {
        self.hook.as_ref().map(|hook| Hook {
            placement: hook.placement,
            frames: 0..hook.frames,
            animated: hook.entrance,
        })
    }

    /// The words on the output timeline, if every source a cut uses has word timings.
    pub fn words(&self) -> Option<&[OutputWord]> {
        self.words.as_deref()
    }

    /// How many caption pages there are.
    pub fn page_count(&self) -> usize {
        self.captions
            .as_ref()
            .map_or(0, |captions| captions.pages.len())
    }

    /// The subtitle cues, if there are word timings for every source.
    pub fn cues(&self) -> Option<Vec<srt::Cue>> {
        let words = self.words.as_deref()?;
        Some(srt::cues(words, self.fps, self.total_frames))
    }

    /// The subtitles as an SRT file, if there are word timings for every source.
    pub fn subtitles(&self) -> Option<String> {
        self.cues().map(|cues| srt::to_srt(&cues, self.fps))
    }

    /// Characters in the captions or hook that the font has no glyph for; they are
    /// drawn as the font's empty box.
    pub fn missing_glyphs(&self) -> &[char] {
        &self.missing
    }

    /// The name of the font the text is set in.
    pub fn font_name(&self) -> &str {
        &self.font
    }

    /// Output frame rate.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Whether there is anything to draw: captions or a hook.
    pub fn draws(&self) -> bool {
        self.captions.is_some() || self.hook.is_some()
    }

    /// A painter that draws this plan's frames.
    pub fn painter(&self) -> Painter<'_> {
        Painter::new(self)
    }
}

/// The caption text style: the plan's colour and outline, at the caption size for
/// `frame`. A plan without `[captions]` (a hook only) gets sabicut-core's defaults, as
/// a `[captions]` table without `active` and `stroke` does.
fn base_style(
    settings: Option<&Captions>,
    frame: &Frame,
    shaper: &Shaper<'_>,
) -> Result<TextStyle, OverlayError> {
    let active = settings.map_or(Captions::DEFAULT_ACTIVE, Captions::active);
    let accent = parse_colour(active).ok_or_else(|| OverlayError::Colour(active.to_owned()))?;
    let stroke = settings.map_or(Captions::DEFAULT_STROKE, Captions::stroke);
    let cap = cap_height(frame);
    Ok(TextStyle {
        scale: cap / shaper.metrics().cap,
        cap,
        stroke: stroke as f32,
        fill: WHITE,
        accent,
        outline: BLACK,
    })
}

/// `#RRGGBB` as opaque RGBA.
fn parse_colour(text: &str) -> Option<[u8; 4]> {
    let hex = text.strip_prefix('#').filter(|hex| hex.len() == 6)?;
    let channel = |range: Range<usize>| {
        hex.get(range)
            .and_then(|digits| u8::from_str_radix(digits, 16).ok())
    };
    Some([channel(0..2)?, channel(2..4)?, channel(4..6)?, 255])
}

/// The error for captions whose words are incomplete: the first source a cut uses
/// that has no word timings.
fn missing_words(plan: &Plan, timeline: &Timeline, words: &WordTimings) -> OverlayError {
    let source_id = timeline
        .segments()
        .iter()
        .map(|segment| segment.source())
        .find(|id| words.get(id).is_none())
        .unwrap_or_default()
        .to_owned();
    OverlayError::NoWords {
        source_id,
        single_source: plan.sources().len() == 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse() {
        assert_eq!(parse_colour("#FFC400"), Some([255, 196, 0, 255]));
        assert_eq!(parse_colour("#ffc400"), Some([255, 196, 0, 255]));
        assert_eq!(parse_colour("FFC400"), None);
        assert_eq!(parse_colour("#FFC40"), None);
        assert_eq!(parse_colour("#GGC400"), None);
        assert_eq!(
            parse_colour(Captions::DEFAULT_ACTIVE),
            Some([255, 196, 0, 255])
        );
    }
}
