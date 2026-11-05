//! The plan as TOML text, with comments that show what each cut keeps and what was cut.

use std::fmt::Write as _;

use crate::grid::{Grid, millis_text};
use crate::options::Options;
use crate::sections::{Gap, Reason, SourceCuts};
use crate::text::{snippet, toml_float, toml_string};
use crate::{CutDraft, SourceInput};

/// Longest text shown in a comment, in characters.
const COMMENT_TEXT: usize = 72;

/// Everything the plan file says.
pub(crate) struct Document<'a> {
    pub options: &'a Options,
    pub sources: &'a [SourceInput],
    pub plans: &'a [SourceCuts],
    pub cuts: &'a [CutDraft],
    /// Hook text and its duration as written.
    pub hook: Option<(&'a str, String)>,
    pub grid: Grid,
}

pub(crate) fn render(doc: &Document<'_>) -> String {
    let mut out = String::new();
    header(&mut out, doc);
    let _ = writeln!(out, "schema = 1\n");
    let _ = writeln!(out, "[meta]");
    let _ = writeln!(out, "fps = {}", doc.options.fps);
    let _ = writeln!(
        out,
        "out = {{ w = {}, h = {} }}",
        doc.options.width, doc.options.height
    );
    let several = doc.sources.len() > 1;
    for source in doc.sources {
        let _ = writeln!(out, "\n[[source]]");
        let _ = writeln!(out, "id = {}", toml_string(&source.id));
        let _ = writeln!(out, "path = {}", toml_string(&source.path));
        let _ = writeln!(out, "crop_x = {}", toml_float(source.crop_x));
        if several {
            let _ = writeln!(out, "words = {}", toml_string(&source.words_path));
        }
    }
    for (index, source) in doc.sources.iter().enumerate() {
        cuts_of(&mut out, doc, index, source);
    }
    if let Some((text, dur)) = &doc.hook {
        let _ = writeln!(out, "\n[hook]");
        let _ = writeln!(out, "text = {}", toml_string(text));
        let _ = writeln!(out, "dur = {dur}");
    }
    if let Some(style) = &doc.options.captions {
        let _ = writeln!(out, "\n[captions]");
        if let ([only], false) = (doc.sources, several) {
            let _ = writeln!(out, "words = {}", toml_string(&only.words_path));
        }
        if let Some(fixes) = &style.fixes {
            let _ = writeln!(out, "fixes = {}", toml_string(fixes));
        }
        if !doc.options.keywords.is_empty() {
            let list: Vec<String> = doc
                .options
                .keywords
                .iter()
                .map(|keyword| toml_string(keyword.trim()))
                .collect();
            let _ = writeln!(out, "keywords = [{}]", list.join(", "));
        }
        if let Some(font) = &style.font {
            let _ = writeln!(out, "font = {}", toml_string(font));
        }
        if let Some(active) = &style.active {
            let _ = writeln!(out, "active = {}", toml_string(active));
        }
        if let Some(stroke) = style.stroke {
            let _ = writeln!(out, "stroke = {stroke}");
        }
    }
    out
}

fn header(out: &mut String, doc: &Document<'_>) {
    let kept: i64 = doc
        .cuts
        .iter()
        .map(|cut| cut.section.end - cut.section.start)
        .sum();
    let input: f64 = doc
        .sources
        .iter()
        .map(|source| source.transcript.duration())
        .sum();
    let cuts = match doc.cuts.len() {
        1 => "1 cut keeps".to_owned(),
        n => format!("{n} cuts keep"),
    };
    let _ = writeln!(
        out,
        "# Edit plan written by `sabicut plan`: {cuts} {:.3} s of {input:.3} s.",
        doc.grid.seconds(kept)
    );
    let _ = writeln!(
        out,
        "# Times are in seconds of source time. A \"# removed\" line shows a range that was\n\
         # cut and why: to bring it back, move the `out` of the cut before it (or the `in`\n\
         # of the cut after it) over the range. Check edits with `sabicut validate` and\n\
         # `sabicut timeline`.\n"
    );
}

/// The cuts of source `index`, in time order, with the ranges cut around them.
fn cuts_of(out: &mut String, doc: &Document<'_>, index: usize, source: &SourceInput) {
    let Some(plan) = doc.plans.get(index) else {
        return;
    };
    if source.transcript.words().is_empty() {
        let _ = writeln!(
            out,
            "\n# {}: {} has no words, so this clip has no cuts. Add some by hand to use it.",
            source.id, source.words_path
        );
        return;
    }
    let mut gaps = plan.gaps.iter().peekable();
    for cut in doc.cuts.iter().filter(|cut| cut.source == index) {
        let _ = writeln!(out);
        while let Some(gap) = gaps.next_if(|gap| gap.end <= cut.section.start) {
            removed(out, doc, source, gap);
        }
        cut_block(out, doc, source, cut);
    }
    let rest: Vec<&Gap> = gaps.collect();
    if !rest.is_empty() {
        let _ = writeln!(out);
    }
    for gap in rest {
        removed(out, doc, source, gap);
    }
}

fn removed(out: &mut String, doc: &Document<'_>, source: &SourceInput, gap: &Gap) {
    let grid = doc.grid;
    let what = match gap.reason {
        Reason::Pause => "pause".to_owned(),
        Reason::FalseStart => format!("false start {}", quoted(source, &gap.words)),
        Reason::Short => format!("too short {}", quoted(source, &gap.words)),
    };
    let _ = writeln!(
        out,
        "# removed {} {} - {} ({:.3} s): {what}",
        source.id,
        millis_text(grid.millis(gap.start)),
        millis_text(grid.millis(gap.end)),
        grid.seconds(gap.end - gap.start),
    );
}

fn cut_block(out: &mut String, doc: &Document<'_>, source: &SourceInput, cut: &CutDraft) {
    let grid = doc.grid;
    let _ = writeln!(out, "[[cut]]  # {}", quoted(source, &cut.section.words));
    let _ = writeln!(out, "src = {}", toml_string(&source.id));
    let _ = writeln!(out, "in = {}", millis_text(grid.millis(cut.section.start)));
    let _ = writeln!(out, "out = {}", millis_text(grid.millis(cut.section.end)));
    if cut.zooms.is_empty() {
        return;
    }
    let list: Vec<String> = cut
        .zooms
        .iter()
        .map(|zoom| {
            format!(
                "{{ at = {}, scale = {} }}",
                millis_text(grid.millis(zoom.frame)),
                toml_float(zoom.scale)
            )
        })
        .collect();
    let keywords: Vec<String> = cut
        .zooms
        .iter()
        .filter_map(|zoom| zoom.keyword.as_deref())
        .map(|keyword| format!("\"{}\"", snippet(keyword, COMMENT_TEXT)))
        .collect();
    let note = if keywords.is_empty() {
        String::new()
    } else {
        format!("  # {}", keywords.join(", "))
    };
    let _ = writeln!(out, "zoom = [{}]{note}", list.join(", "));
}

/// The words in `range` of a source, quoted and shortened for a comment.
fn quoted(source: &SourceInput, range: &std::ops::Range<usize>) -> String {
    let words = source.transcript.words();
    let text: Vec<&str> = words
        .get(range.clone())
        .unwrap_or_default()
        .iter()
        .map(|word| word.text())
        .collect();
    format!("\"{}\"", snippet(&text.join(" "), COMMENT_TEXT))
}
