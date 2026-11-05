//! What the rules do, with defaults measured on real transcripts (see the README).

/// How `sabicut plan` builds a plan. [`Options::default`] gives the documented defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Output frame rate (`meta.fps`): cuts land on frame boundaries at this rate.
    pub fps: u32,
    /// Output width in pixels (`meta.out.w`).
    pub width: u32,
    /// Output height in pixels (`meta.out.h`).
    pub height: u32,
    /// How pauses are cut.
    pub dead_air: DeadAir,
    /// How false starts are found, or `None` to keep them all.
    pub false_starts: Option<FalseStarts>,
    /// Which zooms are added.
    pub zooms: Zooms,
    /// Names and terms that get a punch-in zoom and are always drawn in the active
    /// caption colour (`captions.keywords`). A keyword may have several words.
    pub keywords: Vec<String>,
    /// The hook (`[hook]`), if any.
    pub hook: Option<HookText>,
    /// Caption settings (`[captions]`), or `None` for a plan without captions.
    pub captions: Option<CaptionStyle>,
    /// Output length, in seconds, above which the report warns.
    pub target: Option<f64>,
}

/// How pauses ("dead air") are cut.
///
/// A pause between two words that is longer than `max_pause` is cut. The cut keeps
/// `pad_after` after the speech before it and `pad_before` ahead of the speech after it,
/// where speech runs to the edges of its voice-activity segment, at most `vad_reach`
/// beyond the words. Cuts land on frame boundaries outside every word.
#[derive(Clone, Debug, PartialEq)]
pub struct DeadAir {
    /// Pauses longer than this are cut, in seconds.
    pub max_pause: f64,
    /// Silence kept ahead of speech, in seconds.
    pub pad_before: f64,
    /// Silence kept after speech, in seconds.
    pub pad_after: f64,
    /// How far a voice-activity segment may extend the speech beyond its words, in seconds.
    pub vad_reach: f64,
    /// A pause is only cut if that removes at least this much, in seconds; otherwise the
    /// sections on both sides stay together.
    pub min_cut: f64,
    /// Kept sections shorter than this are dropped, in seconds.
    pub min_keep: f64,
}

/// How false starts are found: a phrase that the speaker breaks off and then says again
/// from its start, as in "So today we… So today we're going to talk about Rust". The
/// last take is kept and the fragments before it are cut.
///
/// A fragment must start a phrase, break off at a dash, an ellipsis or a pause of at
/// least `min_pause`, not end a sentence, and match the start of the next phrase word
/// for word (its last word may be cut short, as "tod-" for "today"). The next phrase
/// must go on past the repeated words. So emphasis ("very very good"), one-word repeats,
/// a clause said twice with the same punctuation ("Oh dear, oh dear, I shall be late")
/// and complete sentences said twice are kept.
#[derive(Clone, Debug, PartialEq)]
pub struct FalseStarts {
    /// The fewest repeated words that make a false start.
    pub min_words: usize,
    /// The most words in a false start.
    pub max_words: usize,
    /// The shortest pause, in seconds, that breaks a phrase when there is no punctuation.
    pub min_pause: f64,
}

/// Which zooms the plan gets. Each zoom holds until the next zoom in its cut or the end
/// of the cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Zooms {
    /// Scale of the punch-in at the start of each keyword, or `None` for no keyword zooms.
    pub keyword_scale: Option<f64>,
    /// Seconds after a keyword ends at which the zoom returns to the cut's scale, or
    /// `None` to hold it to the end of the cut.
    pub hold: Option<f64>,
    /// Scale of every second cut, from its first frame, which hides jump cuts; `None`
    /// leaves every cut at scale 1.
    pub alternate: Option<f64>,
}

/// Hook text for the opening seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct HookText {
    /// The hook line.
    pub text: String,
    /// How long it shows, in seconds; shortened to the output if that is shorter.
    pub dur: f64,
}

/// Caption settings written to `[captions]`. The word timings come from the sources.
/// Style fields left as `None` are left out of the plan, so the defaults apply: the
/// bundled Anton font, gold, and a 10 px outline (see [`sabicut_core::Captions`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptionStyle {
    /// Fixes file (`captions.fixes`), as written in the plan, if any.
    pub fixes: Option<String>,
    /// Font file (`captions.font`), as written in the plan.
    pub font: Option<String>,
    /// Active-word colour, `#RRGGBB`.
    pub active: Option<String>,
    /// Outline width in pixels, 0 to 64.
    pub stroke: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fps: 30,
            width: 1080,
            height: 1920,
            dead_air: DeadAir::default(),
            false_starts: Some(FalseStarts::default()),
            zooms: Zooms::default(),
            keywords: Vec::new(),
            hook: None,
            captions: Some(CaptionStyle::default()),
            target: Some(60.0),
        }
    }
}

impl Default for DeadAir {
    fn default() -> Self {
        Self {
            max_pause: 0.4,
            pad_before: 0.15,
            pad_after: 0.1,
            vad_reach: 1.0,
            min_cut: 0.2,
            min_keep: 0.5,
        }
    }
}

impl Default for FalseStarts {
    fn default() -> Self {
        Self {
            min_words: 2,
            max_words: 15,
            min_pause: 0.2,
        }
    }
}

impl Default for Zooms {
    fn default() -> Self {
        Self {
            keyword_scale: Some(1.25),
            hold: None,
            alternate: None,
        }
    }
}

impl HookText {
    /// The default length of the hook, in seconds.
    pub const DEFAULT_DUR: f64 = 2.0;
}

/// The longest pause or padding, in seconds.
const MAX_SECONDS: f64 = 3600.0;

impl Options {
    /// Checks every option, returning the first one that is out of range as
    /// `(option name, problem)`.
    pub(crate) fn check(&self) -> Result<(), (&'static str, String)> {
        let in_range = |name: &'static str, value: f64, min: f64, max: f64| {
            if value.is_finite() && (min..=max).contains(&value) {
                Ok(())
            } else {
                Err((
                    name,
                    format!("must be between {min} and {max}, got {value}"),
                ))
            }
        };
        if !(1..=240).contains(&self.fps) {
            return Err((
                "fps",
                format!("must be between 1 and 240, got {}", self.fps),
            ));
        }
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if !(2..=8192).contains(&value) || value % 2 != 0 {
                return Err((name, format!("must be even, from 2 to 8192, got {value}")));
            }
        }
        let air = &self.dead_air;
        in_range("max_pause", air.max_pause, 0.0, MAX_SECONDS)?;
        in_range("pad_before", air.pad_before, 0.0, MAX_SECONDS)?;
        in_range("pad_after", air.pad_after, 0.0, MAX_SECONDS)?;
        in_range("vad_reach", air.vad_reach, 0.0, MAX_SECONDS)?;
        in_range("min_cut", air.min_cut, 0.0, MAX_SECONDS)?;
        in_range("min_keep", air.min_keep, 0.0, MAX_SECONDS)?;
        if let Some(rule) = &self.false_starts {
            if rule.min_words == 0 || rule.max_words < rule.min_words {
                return Err((
                    "false_starts",
                    format!(
                        "need 1 <= min_words <= max_words, got {} and {}",
                        rule.min_words, rule.max_words
                    ),
                ));
            }
            in_range("false_starts.min_pause", rule.min_pause, 0.0, MAX_SECONDS)?;
        }
        if let Some(scale) = self.zooms.keyword_scale {
            in_range("keyword zoom", scale, 1.0, 4.0)?;
        }
        if let Some(scale) = self.zooms.alternate {
            in_range("alternate zoom", scale, 1.0, 4.0)?;
        }
        if let Some(hold) = self.zooms.hold {
            in_range("zoom hold", hold, 0.0, MAX_SECONDS)?;
        }
        if let Some(keyword) = self.keywords.iter().find(|k| k.trim().is_empty()) {
            return Err(("keywords", format!("must not be blank, got {keyword:?}")));
        }
        if let Some(hook) = &self.hook {
            if hook.text.trim().is_empty() {
                return Err(("hook", "must not be blank".to_owned()));
            }
            if hook.text.chars().any(char::is_control) {
                return Err(("hook", "must be one line of text".to_owned()));
            }
            in_range("hook duration", hook.dur, 0.001, 86_400.0)?;
        }
        if let Some(style) = &self.captions {
            if let Some(active) = &style.active {
                let hex = active.strip_prefix('#').is_some_and(|hex| {
                    hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())
                });
                if !hex {
                    return Err((
                        "active colour",
                        format!("must look like #FFC400, got {active:?}"),
                    ));
                }
            }
            if let Some(stroke) = style.stroke
                && stroke > 64
            {
                return Err(("stroke", format!("must be 0 to 64, got {stroke}")));
            }
            for (name, path) in [("font", &style.font), ("fixes", &style.fixes)] {
                if path.as_deref().is_some_and(|path| {
                    path.trim().is_empty() || path.chars().any(char::is_control)
                }) {
                    return Err((name, format!("must be a file path, got {path:?}")));
                }
            }
        }
        if let Some(target) = self.target {
            in_range("target", target, 0.0, 86_400.0)?;
        }
        Ok(())
    }
}
