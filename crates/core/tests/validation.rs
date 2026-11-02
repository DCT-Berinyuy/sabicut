//! Validation reports every problem, each naming its field and cut index.

use sabicut_core::{Plan, Timeline};

/// A valid plan, with `{source}`, `{cuts}` and `{extra}` spliced in.
fn plan_with(meta: &str, sources: &str, cuts: &str, extra: &str) -> String {
    format!("schema = 1\n[meta]\n{meta}\n{sources}\n{cuts}\n{extra}\n")
}

const META: &str = "fps = 30\nout = { w = 1080, h = 1920 }";
const SOURCE_A: &str = "[[source]]\nid = \"a\"\npath = \"a.mov\"\ncrop_x = 0.5";
const CUT_A: &str = "[[cut]]\nsrc = \"a\"\nin = 1.0\nout = 3.0";

/// Every problem in `text`, as displayed.
fn problems(text: &str) -> Vec<String> {
    let plan = Plan::from_toml_str(text).unwrap();
    let errors = plan.validate().unwrap_err();
    assert_eq!(Timeline::from_plan(&plan).unwrap_err(), errors);
    errors.iter().map(ToString::to_string).collect()
}

fn cuts_problems(cuts: &str) -> Vec<String> {
    problems(&plan_with(META, SOURCE_A, cuts, ""))
}

fn cut(src: &str, start: &str, end: &str, zoom: &str) -> String {
    format!("[[cut]]\nsrc = \"{src}\"\nin = {start}\nout = {end}\nzoom = [{zoom}]\n")
}

#[test]
fn out_not_after_in_names_the_cut_index() {
    let cuts = [
        cut("a", "1", "2", ""),
        cut("a", "2", "3", ""),
        cut("a", "3", "4", ""),
        cut("a", "9.5", "9.5", ""),
        cut("a", "8", "7", ""),
    ]
    .concat();
    assert_eq!(
        cuts_problems(&cuts),
        ["cut[3].out must be > in", "cut[4].out must be > in"]
    );
}

#[test]
fn errors_are_split_into_field_and_message() {
    let plan =
        Plan::from_toml_str(&plan_with(META, SOURCE_A, &cut("a", "5", "4", ""), "")).unwrap();
    let errors = plan.validate().unwrap_err();
    let error = &errors.as_slice()[0];
    assert_eq!(
        (error.field(), error.message()),
        ("cut[0].out", "must be > in")
    );
}

#[test]
fn cut_rules() {
    let cuts = [
        cut("z", "1", "2", ""),
        cut("a", "-1", "2", ""),
        cut("a", "nan", "2", ""),
        cut("a", "1", "inf", ""),
        cut("a", "1", "86400.5", ""),
    ]
    .concat();
    assert_eq!(
        cuts_problems(&cuts),
        [
            r#"cut[0].src must be the id of a [[source]], got "z""#,
            "cut[1].in must be between 0 and 86400 seconds, got -1",
            "cut[2].in must be between 0 and 86400 seconds, got NaN",
            "cut[3].out must be between 0 and 86400 seconds, got inf",
            "cut[4].out must be between 0 and 86400 seconds, got 86400.5",
        ]
    );
}

#[test]
fn a_cut_must_be_at_least_one_frame_long() {
    // At 30 fps, 1.000 s and 1.010 s both snap to frame 30. The zoom inside it is not
    // reported as well.
    assert_eq!(
        cuts_problems(&cut("a", "1.0", "1.01", "{ at = 1.005, scale = 1.2 }")),
        [
            "cut[0] must be at least 1 frame long at 30 fps, but in (1) and out (1.01) both snap to frame 30"
        ]
    );
}

#[test]
fn zooms_must_last_at_least_one_frame() {
    // At 30 fps, 2.0 s and 2.01 s both snap to frame 60, and 2.99 s snaps to frame 90,
    // where the cut ends.
    let zooms = "{ at = 2.0, scale = 1.2 }, { at = 2.01, scale = 1.1 }, { at = 2.99, scale = 1.3 }";
    assert_eq!(
        cuts_problems(&cut("a", "1", "3", zooms)),
        [
            "cut[0].zoom[1].at must be at least 1 frame after zoom[0].at at 30 fps, got 2.01",
            "cut[0].zoom[2].at must be at least 1 frame before out at 30 fps, got 2.99",
        ]
    );
}

#[test]
fn zooms_may_be_one_frame_apart_and_on_the_last_frame() {
    // 2.034 s snaps to frame 61 (61.02) and 2.98 s to frame 89 (89.4); the cut ends at 90.
    let zooms =
        "{ at = 2.0, scale = 1.2 }, { at = 2.034, scale = 1.1 }, { at = 2.98, scale = 1.0 }";
    let text = plan_with(META, SOURCE_A, &cut("a", "1", "3", zooms), "");
    let timeline = Timeline::from_plan(&Plan::from_toml_str(&text).unwrap()).unwrap();
    let frames: Vec<_> = timeline.segments()[0]
        .zooms()
        .iter()
        .map(|zoom| (zoom.source_frame(), zoom.output_frame()))
        .collect();
    assert_eq!(frames, [(60, 30), (61, 31), (89, 59)]);
}

#[test]
fn zoom_rules() {
    let cuts = [
        cut(
            "a",
            "1",
            "3",
            "{ at = 0.5, scale = 1.2 }, { at = 3, scale = 1.2 }",
        ),
        cut(
            "a",
            "1",
            "3",
            "{ at = 2, scale = 1.2 }, { at = 1.5, scale = 1.1 }, { at = 1.5, scale = 1.1 }",
        ),
        cut(
            "a",
            "1",
            "3",
            "{ at = 2, scale = 0.5 }, { at = 2.5, scale = 4.5 }, { at = 2.6, scale = nan }",
        ),
    ]
    .concat();
    assert_eq!(
        cuts_problems(&cuts),
        [
            "cut[0].zoom[0].at must be inside the cut (in <= at < out), got 0.5",
            "cut[0].zoom[1].at must be inside the cut (in <= at < out), got 3",
            "cut[1].zoom[1].at must be greater than zoom[0].at",
            "cut[1].zoom[2].at must be greater than zoom[1].at",
            "cut[2].zoom[0].scale must be between 1 and 4, got 0.5",
            "cut[2].zoom[1].scale must be between 1 and 4, got 4.5",
            "cut[2].zoom[2].scale must be between 1 and 4, got NaN",
        ]
    );
}

#[test]
fn meta_rules() {
    let cases = [
        (
            "fps = 0\nout = { w = 1080, h = 1920 }",
            "meta.fps must be between 1 and 240, got 0",
        ),
        (
            "fps = 241\nout = { w = 1080, h = 1920 }",
            "meta.fps must be between 1 and 240, got 241",
        ),
        (
            "fps = 30\nout = { w = 1081, h = 1920 }",
            "meta.out.w must be even, got 1081",
        ),
        (
            "fps = 30\nout = { w = 1080, h = 0 }",
            "meta.out.h must be between 2 and 8192, got 0",
        ),
        (
            "fps = 30\nout = { w = 1080, h = 8194 }",
            "meta.out.h must be between 2 and 8192, got 8194",
        ),
    ];
    for (meta, expected) in cases {
        assert_eq!(
            problems(&plan_with(meta, SOURCE_A, CUT_A, "")),
            [expected],
            "{meta}"
        );
    }
}

#[test]
fn source_rules() {
    let sources = r#"
        [[source]]
        id = "a"
        path = "a.mov"
        crop_x = 0.5
        [[source]]
        id = "a"
        path = ""
        crop_x = 1.5
        [[source]]
        id = "b c"
        path = "c.mov"
        crop_x = -0.1
    "#;
    assert_eq!(
        problems(&plan_with(META, sources, CUT_A, "")),
        [
            r#"source[1].id must be unique, but "a" is already source[0].id"#,
            "source[1].path must not be empty",
            "source[1].crop_x must be between 0 and 1, got 1.5",
            r#"source[2].id must be 1 to 64 letters, digits, '_' or '-', got "b c""#,
            "source[2].crop_x must be between 0 and 1, got -0.1",
        ]
    );
}

#[test]
fn sources_and_cuts_cannot_be_empty() {
    // With no cuts there is no output to compare the hook with, so it is not reported.
    let hook = "[hook]\ntext = \"Hi\"\ndur = 2.0";
    let text = format!("schema = 1\nsource = []\ncut = []\n[meta]\n{META}\n{hook}\n");
    assert_eq!(
        problems(&text),
        [
            "source must have at least one entry",
            "cut must have at least one entry"
        ]
    );
}

#[test]
fn hook_rules() {
    let cases = [
        ("text = \"  \"\ndur = 1.0", "hook.text must not be blank"),
        (
            "text = \"Hi\"\ndur = 0",
            "hook.dur must be greater than 0 and at most 86400 seconds, got 0",
        ),
        (
            "text = \"Hi\"\ndur = nan",
            "hook.dur must be greater than 0 and at most 86400 seconds, got NaN",
        ),
        (
            "text = \"Hi\"\ndur = 0.01",
            "hook.dur must be at least 1 frame long at 30 fps, got 0.01",
        ),
        // The cut lasts 2 s (60 frames).
        (
            "text = \"Hi\"\ndur = 2.5",
            "hook.dur must not be longer than the output (2.000 s), got 2.5",
        ),
    ];
    for (hook, expected) in cases {
        let text = plan_with(META, SOURCE_A, CUT_A, &format!("[hook]\n{hook}"));
        assert_eq!(problems(&text), [expected], "{hook}");
    }
}

#[test]
fn hook_may_last_the_whole_output() {
    let text = plan_with(META, SOURCE_A, CUT_A, "[hook]\ntext = \"Hi\"\ndur = 2.0");
    let timeline = Timeline::from_plan(&Plan::from_toml_str(&text).unwrap()).unwrap();
    assert_eq!(timeline.hook_frames(), Some(60));
    assert_eq!(timeline.total_frames(), 60);
}

#[test]
fn caption_rules() {
    let captions = r#"
        [captions]
        words = ""
        fixes = ""
        keywords = ["Rust", " "]
        font = ""
        active = "gold"
        stroke = 65
    "#;
    assert_eq!(
        problems(&plan_with(META, SOURCE_A, CUT_A, captions)),
        [
            "captions.words must not be empty",
            "captions.fixes must not be empty",
            "captions.keywords[1] must not be blank",
            "captions.font must not be empty",
            r##"captions.active must be a #RRGGBB colour like "#FFC400", got "gold""##,
            "captions.stroke must be between 0 and 64, got 65",
        ]
    );
}

#[test]
fn colours_accept_any_case_but_need_six_digits() {
    for (active, ok) in [
        ("#ffc400", true),
        ("#FfC400", true),
        ("#FFC40", false),
        ("FFC400", false),
    ] {
        let captions = format!(
            "[captions]\nwords = \"w.json\"\nfont = \"f.ttf\"\nactive = \"{active}\"\nstroke = 0"
        );
        let plan = Plan::from_toml_str(&plan_with(META, SOURCE_A, CUT_A, &captions)).unwrap();
        assert_eq!(plan.validate().is_ok(), ok, "{active}");
    }
}

#[test]
fn every_problem_is_reported_at_once_in_schema_order() {
    let text = plan_with(
        "fps = 30\nout = { w = 1081, h = 1920 }",
        "[[source]]\nid = \"a\"\npath = \"a.mov\"\ncrop_x = 2",
        &[cut("a", "1", "3", ""), cut("x", "4", "3", "")].concat(),
        "[hook]\ntext = \"\"\ndur = 1",
    );
    assert_eq!(
        problems(&text),
        [
            "meta.out.w must be even, got 1081",
            "source[0].crop_x must be between 0 and 1, got 2",
            r#"cut[1].src must be the id of a [[source]], got "x""#,
            "cut[1].out must be > in",
            "hook.text must not be blank",
        ]
    );
}
