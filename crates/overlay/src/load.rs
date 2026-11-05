//! Reading what a plan names for captions: word timings (fixes applied) and the font.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sabicut_core::{Captions, Fixes, Plan, Timeline, Transcript, apply_fixes};

use crate::error::OverlayError;
use crate::font::Font;
use crate::graphics::Graphics;
use crate::timing::WordTimings;

/// words.json files are rarely more than a few megabytes, even for hours of speech.
const MAX_WORDS_BYTES: usize = 64 * 1024 * 1024;
/// fixes.toml files are small text files.
const MAX_FIXES_BYTES: usize = 16 * 1024 * 1024;

/// Reads the word timings of every source that a cut uses and that has a words file,
/// and applies `captions.fixes` to them. Paths are relative to `plan_dir`.
///
/// A source's own `words` comes first; `captions.words` is used only when the plan has
/// a single source ([`Plan::caption_words`]). Sources without word timings are left
/// out.
pub fn load_word_timings(
    plan: &Plan,
    timeline: &Timeline,
    plan_dir: &Path,
) -> Result<WordTimings, OverlayError> {
    let fixes = match plan.captions().and_then(Captions::fixes) {
        Some(relative) => {
            let path = plan_dir.join(relative);
            let fixes = read_fixes(&path)?;
            Some((fixes, path))
        }
        None => None,
    };
    // Sources may share a words file; read and fix each file once.
    let mut files: BTreeMap<PathBuf, Transcript> = BTreeMap::new();
    let mut timings = WordTimings::new();
    for segment in timeline.segments() {
        let id = segment.source();
        if timings.get(id).is_some() {
            continue;
        }
        let Some(relative) = plan.caption_words(id) else {
            continue;
        };
        let path = plan_dir.join(relative);
        let transcript = match files.get(&path) {
            Some(transcript) => transcript.clone(),
            None => {
                let mut transcript = read_words(&path)?;
                if let Some((fixes, fixes_path)) = &fixes {
                    transcript = apply_fixes(&transcript, fixes)
                        .map_err(|source| OverlayError::ApplyFixes {
                            path: fixes_path.clone(),
                            source,
                        })?
                        .into_transcript();
                }
                files.insert(path, transcript.clone());
                transcript
            }
        };
        timings.insert(id, transcript);
    }
    Ok(timings)
}

impl Graphics {
    /// Reads the word timings, fixes and font that `plan` names (relative to
    /// `plan_dir`, the plan file's folder) and lays out its captions and hook. Without
    /// `captions.font`, the bundled Anton is used.
    ///
    /// Returns `None` when the plan has no captions, no hook and no word timings.
    pub fn load(
        plan: &Plan,
        timeline: &Timeline,
        plan_dir: &Path,
    ) -> Result<Option<Self>, OverlayError> {
        let words = load_word_timings(plan, timeline, plan_dir)?;
        if plan.captions().is_none() && plan.hook().is_none() && words.is_empty() {
            return Ok(None);
        }
        let font = match plan.captions().and_then(Captions::font) {
            Some(relative) => Font::load(&plan_dir.join(relative))?,
            None => Font::bundled(),
        };
        let graphics = Self::new(plan, timeline, &words, &font)?;
        Ok((graphics.draws() || graphics.words().is_some()).then_some(graphics))
    }
}

fn read_text(path: &Path, limit: usize, kind: &'static str) -> Result<String, OverlayError> {
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(limit as u64 + 1).read_to_string(&mut text))
        .map_err(OverlayError::read(path))?;
    if text.len() > limit {
        return Err(OverlayError::TooLarge {
            path: path.to_owned(),
            limit,
            kind,
        });
    }
    Ok(text)
}

fn read_words(path: &Path) -> Result<Transcript, OverlayError> {
    let text = read_text(path, MAX_WORDS_BYTES, "a words.json")?;
    let transcript = Transcript::from_json_str(&text).map_err(|source| OverlayError::Words {
        path: path.to_owned(),
        source,
    })?;
    transcript
        .validate()
        .map_err(|errors| OverlayError::InvalidWords {
            path: path.to_owned(),
            errors,
        })?;
    Ok(transcript)
}

fn read_fixes(path: &Path) -> Result<Fixes, OverlayError> {
    let text = read_text(path, MAX_FIXES_BYTES, "a fixes.toml")?;
    Fixes::from_toml_str(&text).map_err(|source| OverlayError::Fixes {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder for one test.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sabicut-overlay-load-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A words.json with one word, `text`, from 0.2 to 0.6 s.
    fn words_json(text: &str) -> String {
        format!(
            r#"{{"schema": 1, "media": "clip.mp4", "language": "en", "model": "test",
                "duration": 2.0, "speech": [{{"start": 0.1, "end": 0.9}}],
                "words": [{{"text": "{text}", "start": 0.2, "end": 0.6, "probability": 0.9}}]}}"#
        )
    }

    fn texts(graphics: &Graphics) -> Vec<&str> {
        graphics
            .words()
            .unwrap()
            .iter()
            .map(|word| word.text.as_str())
            .collect()
    }

    const CUTS: &str = "[[cut]]\nsrc = \"a\"\nin = 0.0\nout = 1.0\n";

    fn plan(sources: &str, rest: &str) -> Plan {
        Plan::from_toml_str(&format!(
            "schema = 1\n[meta]\nfps = 30\nout = {{ w = 1080, h = 1920 }}\n{sources}\n{rest}"
        ))
        .unwrap()
    }

    #[test]
    fn each_source_reads_its_own_words_and_fixes_apply_once() {
        let dir = folder("two");
        std::fs::write(dir.join("a.json"), words_json("rust")).unwrap();
        std::fs::write(dir.join("b.json"), words_json("Supabase")).unwrap();
        std::fs::write(
            dir.join("fixes.toml"),
            "schema = 1\n[[fix]]\nfrom = \"rust\"\nto = \"Rust\"\n",
        )
        .unwrap();
        let plan = plan(
            "[[source]]\nid = \"a\"\npath = \"a.mp4\"\ncrop_x = 0.5\nwords = \"a.json\"\n\
             [[source]]\nid = \"b\"\npath = \"b.mp4\"\ncrop_x = 0.5\nwords = \"b.json\"\n",
            &format!(
                "{CUTS}[[cut]]\nsrc = \"b\"\nin = 0.0\nout = 1.0\n\
                 [captions]\nfixes = \"fixes.toml\"\nkeywords = [\"Rust\"]\n"
            ),
        );
        let timeline = Timeline::from_plan(&plan).unwrap();
        let graphics = Graphics::load(&plan, &timeline, &dir).unwrap().unwrap();
        assert_eq!(texts(&graphics), ["Rust", "Supabase"]);
        // Without captions.font, the bundled Anton draws the captions.
        assert_eq!(graphics.font_name(), "Anton Regular");
        assert!(graphics.band().is_some());
    }

    #[test]
    fn a_single_source_falls_back_to_captions_words() {
        let dir = folder("one");
        std::fs::write(dir.join("talk.json"), words_json("hello")).unwrap();
        std::fs::write(dir.join("own.json"), words_json("own")).unwrap();
        let source = "[[source]]\nid = \"a\"\npath = \"a.mp4\"\ncrop_x = 0.5\n";
        let fallback = plan(
            source,
            &format!("{CUTS}[captions]\nwords = \"talk.json\"\n"),
        );
        let timeline = Timeline::from_plan(&fallback).unwrap();
        let graphics = Graphics::load(&fallback, &timeline, &dir).unwrap().unwrap();
        assert_eq!(texts(&graphics), ["hello"]);
        // The source's own words come first.
        let own = plan(
            &format!("{source}words = \"own.json\"\n"),
            &format!("{CUTS}[captions]\nwords = \"talk.json\"\n"),
        );
        let graphics = Graphics::load(&own, &timeline, &dir).unwrap().unwrap();
        assert_eq!(texts(&graphics), ["own"]);
    }

    #[test]
    fn a_missing_words_file_is_named() {
        let plan = plan(
            "[[source]]\nid = \"a\"\npath = \"a.mp4\"\ncrop_x = 0.5\n",
            &format!("{CUTS}[captions]\nwords = \"talk.words.json\"\n"),
        );
        let timeline = Timeline::from_plan(&plan).unwrap();
        let error =
            Graphics::load(&plan, &timeline, Path::new("/nonexistent/sabicut")).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("/nonexistent/sabicut/talk.words.json: "),
            "{error}"
        );
    }

    #[test]
    fn a_plan_with_nothing_to_draw_gives_none() {
        let plan = plan(
            "[[source]]\nid = \"a\"\npath = \"a.mp4\"\ncrop_x = 0.5\n",
            CUTS,
        );
        let timeline = Timeline::from_plan(&plan).unwrap();
        assert!(
            Graphics::load(&plan, &timeline, Path::new("/nonexistent"))
                .unwrap()
                .is_none()
        );
    }
}
