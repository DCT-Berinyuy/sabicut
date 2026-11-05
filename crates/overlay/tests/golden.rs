//! Golden tests: caption and hook frames compared pixel for pixel with committed PNGs,
//! and subtitles compared with committed SRT files, for English and French.
//!
//! To accept new output (after an intended change), run with `SABICUT_BLESS=1` and look
//! at the PNGs before committing them. On a mismatch, the frame that was drawn is
//! written next to the test binary's scratch files for comparison.

use std::path::{Path, PathBuf};

use sabicut_core::{Plan, SpeechSegment, Timeline, Transcript, Word};
use sabicut_overlay::{Font, Graphics, OutputWord, Placement, WordTimings, safe_zone};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn blessing() -> bool {
    std::env::var_os("SABICUT_BLESS").is_some_and(|value| !value.is_empty())
}

fn plan(size: (u32, u32), hook: &str, keywords: &str) -> Plan {
    Plan::from_toml_str(&format!(
        r##"
        schema = 1
        [meta]
        fps = 30
        out = {{ w = {}, h = {} }}
        [[source]]
        id = "a"
        path = "talk.mp4"
        crop_x = 0.5
        [[cut]]
        src = "a"
        in = 0.5
        out = 6.0
        [[cut]]
        src = "a"
        in = 7.0
        out = 12.0
        [hook]
        text = "{hook}"
        dur = 2.0
        [captions]
        words = "talk.words.json"
        keywords = [{keywords}]
        font = "Anton-Regular.ttf"
        active = "#FFC400"
        stroke = 10
        "##,
        size.0, size.1
    ))
    .unwrap()
}

fn transcript(language: &str, words: &[(&str, f64, f64)]) -> WordTimings {
    let words = words
        .iter()
        .map(|&(text, start, end)| Word::new(text, start, end, 0.9))
        .collect();
    let transcript = Transcript::new(
        "talk.mp4",
        language,
        "ggml-small-q5_1.bin",
        20.0,
        vec![SpeechSegment::new(0.5, 12.0)],
        words,
    )
    .unwrap();
    let mut timings = WordTimings::new();
    timings.insert("a", transcript);
    timings
}

const ENGLISH: &[(&str, f64, f64)] = &[
    ("Stop", 0.6, 0.9),
    ("leaking", 0.9, 1.3),
    ("your", 1.3, 1.5),
    ("API", 1.5, 1.9),
    ("keys.", 1.9, 2.4),
    ("Rust", 2.8, 3.1),
    ("and", 3.1, 3.3),
    ("Supabase", 3.3, 3.9),
    ("make", 3.9, 4.2),
    ("it", 4.2, 4.3),
    ("easy,", 4.3, 4.8),
    ("I", 5.0, 5.1),
    ("promise.", 5.1, 5.7),
    // Between the cuts: dropped.
    ("Removed", 6.2, 6.6),
    ("words", 6.6, 6.9),
    // Starts before the second cut: clamped to it.
    ("straddle", 6.9, 7.3),
    ("Then", 7.5, 7.8),
    ("we", 7.8, 7.9),
    ("ship", 7.9, 8.3),
    ("it", 8.3, 8.4),
    ("tonight!", 8.4, 9.1),
    // Too wide for a line at full size: shrunk to fit.
    ("Supercalifragilisticexpialidocious", 9.5, 10.6),
    ("works", 10.6, 11.0),
    ("too.", 11.0, 11.5),
];

const FRENCH: &[(&str, f64, f64)] = &[
    ("Été", 0.6, 0.9),
    ("comme", 0.9, 1.2),
    ("hiver,", 1.2, 1.6),
    ("le", 1.7, 1.8),
    ("moulin", 1.8, 2.2),
    ("tourne\u{a0}!", 2.2, 2.8),
    ("Ça,", 3.0, 3.3),
    ("c’est", 3.3, 3.5),
    ("l’œuvre", 3.5, 3.9),
    ("de", 3.9, 4.0),
    ("maître", 4.0, 4.4),
    ("Cornille.", 4.4, 5.0),
    ("«\u{a0}Bravo\u{a0}»", 5.2, 5.8),
    ("Où", 7.2, 7.5),
    ("êtes-vous", 7.5, 8.1),
    ("allés\u{a0}?", 8.1, 8.8),
];

fn english() -> Graphics {
    let plan = plan(
        (1080, 1920),
        "Stop leaking your API keys",
        r#""Rust", "Supabase", "API""#,
    );
    let timeline = Timeline::from_plan(&plan).unwrap();
    Graphics::new(
        &plan,
        &timeline,
        &transcript("en", ENGLISH),
        &Font::bundled(),
    )
    .unwrap()
}

fn french() -> Graphics {
    let plan = plan(
        (1080, 1920),
        "Le secret de maître Cornille",
        r#""Cornille""#,
    );
    let timeline = Timeline::from_plan(&plan).unwrap();
    Graphics::new(
        &plan,
        &timeline,
        &transcript("fr", FRENCH),
        &Font::bundled(),
    )
    .unwrap()
}

/// The output word with this text.
fn word<'g>(graphics: &'g Graphics, text: &str) -> &'g OutputWord {
    graphics
        .words()
        .unwrap()
        .iter()
        .find(|word| word.text == text)
        .unwrap_or_else(|| panic!("no word {text:?}"))
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).unwrap();
    buffer.truncate(info.buffer_size());
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Eight),
        "{}",
        path.display()
    );
    (info.width, info.height, buffer)
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Best);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
}

/// Compares a straight-RGBA picture with `tests/golden/<name>.png`.
fn assert_golden_png(name: &str, placement: Placement, rgba: &[u8]) {
    let (width, height) = (placement.width, placement.height);
    assert_eq!(rgba.len(), (width * height * 4) as usize, "{name}");
    assert!(rgba.chunks(4).any(|p| p[3] > 0), "{name} is empty");
    let path = golden_dir().join(format!("{name}.png"));
    if blessing() {
        write_png(&path, width, height, rgba);
        return;
    }
    let (golden_width, golden_height, golden) = read_png(&path);
    let actual = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
    if (golden_width, golden_height) != (width, height) {
        write_png(&actual, width, height, rgba);
        panic!(
            "{name}: {width}x{height}, golden is {golden_width}x{golden_height}; drawn: {}",
            actual.display()
        );
    }
    let differing = rgba
        .chunks(4)
        .zip(golden.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    if differing > 0 {
        write_png(&actual, width, height, rgba);
        panic!(
            "{name}: {differing} pixels differ from {}; drawn: {}",
            path.display(),
            actual.display()
        );
    }
}

fn assert_golden_text(name: &str, text: &str) {
    let path = golden_dir().join(name);
    if blessing() {
        std::fs::write(&path, text).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, golden, "{name} differs from {}", path.display());
}

#[test]
fn english_caption_frames_match_the_goldens() {
    let graphics = english();
    let band = graphics.band().unwrap();
    let mut painter = graphics.painter();
    let supabase = word(&graphics, "Supabase").start;
    // The second frame of the pop, while it overshoots, and once it has settled.
    assert_golden_png("en-pop", band, painter.band_rgba(supabase + 1));
    assert_golden_png("en-active", band, painter.band_rgba(supabase + 10));
    let long = word(&graphics, "Supercalifragilisticexpialidocious").start;
    assert_golden_png("en-long-word", band, painter.band_rgba(long + 8));
}

#[test]
fn french_caption_frames_match_the_goldens() {
    let graphics = french();
    let band = graphics.band().unwrap();
    let mut painter = graphics.painter();
    let oeuvre = word(&graphics, "l’œuvre").start;
    assert_golden_png("fr-accents", band, painter.band_rgba(oeuvre + 6));
    let bravo = word(&graphics, "«\u{a0}Bravo\u{a0}»").start;
    assert_golden_png("fr-quotes", band, painter.band_rgba(bravo + 6));
    assert!(
        graphics.missing_glyphs().is_empty(),
        "{:?}",
        graphics.missing_glyphs()
    );
}

#[test]
fn hook_frames_match_the_goldens() {
    let graphics = english();
    let hook = graphics.hook().unwrap();
    assert_eq!(hook.frames, 0..60);
    assert_eq!(hook.animated, 9);
    let mut painter = graphics.painter();
    // The first frame of the entrance: 83% of full size, half faded in.
    assert_golden_png("en-hook-entrance", hook.placement, painter.hook_rgba(0));
    assert_golden_png("en-hook", hook.placement, painter.hook_rgba(30));
    // Still, from the end of the entrance to the end of the hook.
    let still = painter.hook_rgba(8).to_vec();
    assert_eq!(painter.hook_rgba(59), still.as_slice());
    assert!(painter.hook(60).is_none());
    assert!(painter.hook_rgba(60).iter().all(|&byte| byte == 0));
    let french = french();
    let hook = french.hook().unwrap();
    assert_golden_png("fr-hook", hook.placement, french.painter().hook_rgba(30));
}

#[test]
fn subtitles_match_the_goldens() {
    assert_golden_text("en.srt", &english().subtitles().unwrap());
    assert_golden_text("fr.srt", &french().subtitles().unwrap());
}

#[test]
fn drawing_is_deterministic() {
    let graphics = english();
    let frames: Vec<u64> = (0..315).step_by(7).collect();
    let mut first = graphics.painter();
    let mut second = graphics.painter();
    // The second painter draws the frames in reverse, so its caches differ.
    let drawn: Vec<Vec<u8>> = frames
        .iter()
        .map(|&f| first.band_rgba(f).to_vec())
        .collect();
    for (&frame, expected) in frames.iter().zip(&drawn).rev() {
        assert_eq!(
            second.band_rgba(frame),
            expected.as_slice(),
            "frame {frame}"
        );
    }
    let again = english();
    let mut third = again.painter();
    for (&frame, expected) in frames.iter().zip(&drawn) {
        assert_eq!(third.band_rgba(frame), expected.as_slice(), "frame {frame}");
    }
}

#[test]
fn words_are_placed_on_the_output_timeline() {
    let graphics = english();
    let words = graphics.words().unwrap();
    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    assert!(!texts.contains(&"Removed") && !texts.contains(&"words"));
    // "straddle" (6.9 to 7.3 s) is clamped to the second cut, which starts at 7.0 s:
    // output frame 165.
    let straddle = word(&graphics, "straddle");
    assert_eq!((straddle.start, straddle.end), (165, 174));
    let keywords: Vec<&str> = words
        .iter()
        .filter(|w| w.keyword)
        .map(|w| w.text.as_str())
        .collect();
    assert_eq!(keywords, ["API", "Rust", "Supabase"]);
    // No caption shows before the first word, nor after the last page: "too." ends at
    // output frame 300 and its page stays 0.4 s (12 frames) more.
    let mut painter = graphics.painter();
    assert!(painter.band(2).is_none());
    assert!(painter.band(311).is_some());
    assert!(painter.band(312).is_none());
    assert!(painter.band_rgba(312).iter().all(|&byte| byte == 0));
}

#[test]
fn captions_and_hook_stay_clear_of_the_apps_and_of_each_other() {
    // About as long as a hook can be: ten words.
    let long_hook = "Stop leaking your API keys: bots scan GitHub every minute";
    for size in [
        (1080, 1920),
        (720, 1280),
        (1080, 1350),
        (1920, 1080),
        (2160, 3840),
    ] {
        for hook in ["Stop", "Stop leaking your API keys", long_hook] {
            let plan = plan(size, hook, r#""Rust""#);
            let timeline = Timeline::from_plan(&plan).unwrap();
            let graphics = Graphics::new(
                &plan,
                &timeline,
                &transcript("en", ENGLISH),
                &Font::bundled(),
            )
            .unwrap();
            let band = graphics.band().unwrap();
            let hook = graphics.hook().unwrap().placement;
            assert!(!band.overlaps(&hook), "{size:?} {hook:?} {band:?}");
            let (w, h) = (f64::from(size.0), f64::from(size.1));
            for placed in [band, hook] {
                for value in [placed.x, placed.y, placed.width, placed.height] {
                    assert_eq!(value % 2, 0, "{size:?} {placed:?}");
                }
                assert!(
                    f64::from(placed.y) >= safe_zone::TOP_BAR.bottom * h,
                    "{size:?} {placed:?}"
                );
                assert!(
                    f64::from(placed.bottom()) <= safe_zone::BOTTOM.top * h,
                    "{size:?} {placed:?}"
                );
                assert!(f64::from(placed.x) >= (1.0 - safe_zone::ACTION_COLUMN.left) * w - 1e-6);
                assert!(
                    f64::from(placed.right()) <= safe_zone::ACTION_COLUMN.left * w + 1e-6,
                    "{size:?} {placed:?}"
                );
            }
        }
    }
}

#[test]
fn a_hook_too_long_for_the_space_is_an_error() {
    let words = vec!["word"; 400].join(" ");
    let plan = plan((1080, 1920), &words, "");
    let timeline = Timeline::from_plan(&plan).unwrap();
    let error = Graphics::new(
        &plan,
        &timeline,
        &transcript("en", ENGLISH),
        &Font::bundled(),
    )
    .unwrap_err();
    assert!(
        error.to_string().starts_with("hook.text is too long"),
        "{error}"
    );
}

#[test]
fn characters_the_font_lacks_are_listed() {
    let plan = plan((1080, 1920), "Ship it \u{1F680}", "");
    let timeline = Timeline::from_plan(&plan).unwrap();
    let graphics = Graphics::new(
        &plan,
        &timeline,
        &transcript("en", ENGLISH),
        &Font::bundled(),
    )
    .unwrap();
    assert_eq!(graphics.missing_glyphs(), ['\u{1F680}']);
}
