//! Golden tests for the derived timeline: exact frame counts, and source-to-output
//! mapping at the edges (before the first cut, in gaps, after the last cut, one-frame
//! cuts, zooms). Every expected number was worked out by hand; the comments show how.

use sabicut_core::{Landing, Plan, Rational, Timeline};

/// Builds and validates a plan. Each cut is `(src, in, out, zoom list contents)`.
fn timeline(fps: u32, cuts: &[(&str, &str, &str, &str)]) -> Timeline {
    let mut text = format!("schema = 1\n[meta]\nfps = {fps}\nout = {{ w = 1080, h = 1920 }}\n");
    let mut ids: Vec<&str> = Vec::new();
    for &(src, ..) in cuts {
        if !ids.contains(&src) {
            ids.push(src);
            text += &format!("[[source]]\nid = \"{src}\"\npath = \"{src}.mov\"\ncrop_x = 0.5\n");
        }
    }
    for (src, start, end, zoom) in cuts {
        text += &format!("[[cut]]\nsrc = \"{src}\"\nin = {start}\nout = {end}\nzoom = [{zoom}]\n");
    }
    let plan = Plan::from_toml_str(&text).unwrap();
    Timeline::from_plan(&plan).unwrap_or_else(|errors| panic!("{errors}\n\n{text}"))
}

fn rational(numer: i64, denom: i64) -> Rational {
    Rational::new(numer, denom).unwrap()
}

#[test]
fn single_cut_frame_counts() {
    // (fps, in, out) -> (source_in, source_out, frames). Boundaries are round(t * fps)
    // with exact halves rounded up, on decimal times exact to the microsecond.
    let cases = [
        (30, "3.120", "9.880", (94, 296, 202)), // 93.6 -> 94, 296.4 -> 296 (PRD example)
        (30, "0", "1", (0, 30, 30)),            // whole seconds
        (30, "0.05", "0.15", (2, 5, 3)),        // exactly 1.5 -> 2 and 4.5 -> 5
        (25, "1.02", "1.06", (26, 27, 1)),      // exactly 25.5 -> 26 and 26.5 -> 27
        (30, "10", "10.0333", (300, 301, 1)),   // 300.999 -> 301
        (30, "10.0166", "10.05", (300, 302, 2)), // 300.498 -> 300, 301.5 -> 302
        (60, "1.5", "2.25", (90, 135, 45)),
        (1, "0.4", "0.6", (0, 1, 1)),       // 0.4 -> 0, 0.6 -> 1
        (240, "0.002", "0.006", (0, 1, 1)), // 0.48 -> 0, 1.44 -> 1
        (24, "0", "86400", (0, 2_073_600, 2_073_600)), // the longest cut allowed
    ];
    for (fps, start, end, (source_in, source_out, frames)) in cases {
        let timeline = timeline(fps, &[("a", start, end, "")]);
        let segment = &timeline.segments()[0];
        let label = format!("{fps} fps, {start}..{end}");
        assert_eq!(segment.source_in(), source_in, "{label}");
        assert_eq!(segment.source_out(), source_out, "{label}");
        assert_eq!(segment.frames(), frames, "{label}");
        assert_eq!(timeline.total_frames(), frames, "{label}");
        assert_eq!(
            timeline.duration(),
            Rational::new(frames as i64, i64::from(fps)).unwrap()
        );
    }
}

/// Cuts from two sources, with gaps, two one-frame cuts, and zooms at the edges.
///
/// | cut | source | in..out (s)  | source frames | output frames |
/// |-----|--------|--------------|---------------|---------------|
/// | 0   | a      | 2.0..3.0     | 60..90        | 0..30         |
/// | 1   | a      | 5.0..6.0     | 150..180      | 30..60        |
/// | 2   | b      | 1.0..1.0333  | 30..31        | 60..61        |
/// | 3   | a      | 8.0..8.02    | 240..241      | 61..62        |
/// | 4   | b      | 0.0..0.5     | 0..15         | 62..77        |
fn edge_case_timeline() -> Timeline {
    timeline(
        30,
        &[
            ("a", "2.0", "3.0", "{ at = 2.0, scale = 1.1 }"),
            (
                "a",
                "5.0",
                "6.0",
                "{ at = 5.5, scale = 1.2 }, { at = 5.95, scale = 1.0 }",
            ),
            ("b", "1.0", "1.0333", ""),
            ("a", "8.0", "8.02", "{ at = 8.0, scale = 1.3 }"),
            ("b", "0.0", "0.5", ""),
        ],
    )
}

#[test]
fn cuts_are_placed_back_to_back() {
    let timeline = edge_case_timeline();
    let placed: Vec<_> = timeline
        .segments()
        .iter()
        .map(|s| {
            (
                s.source(),
                s.source_in(),
                s.source_out(),
                s.output_start(),
                s.output_end(),
            )
        })
        .collect();
    assert_eq!(
        placed,
        [
            ("a", 60, 90, 0, 30),
            ("a", 150, 180, 30, 60),
            ("b", 30, 31, 60, 61),
            ("a", 240, 241, 61, 62),
            ("b", 0, 15, 62, 77),
        ]
    );
    assert_eq!(timeline.total_frames(), 77);
    assert_eq!(timeline.duration(), rational(77, 30));
}

#[test]
fn zoom_times_map_from_source_time_to_output_time() {
    let timeline = edge_case_timeline();
    let zooms: Vec<_> = timeline
        .segments()
        .iter()
        .flat_map(|s| s.zooms())
        .map(|z| (z.source_frame(), z.output_frame(), z.scale()))
        .collect();
    assert_eq!(
        zooms,
        [
            (60, 0, 1.1),   // at == in: the cut's first output frame
            (165, 45, 1.2), // 5.5 s -> source frame 165 -> 30 + (165 - 150)
            (179, 59, 1.0), // 5.95 s is exactly frame 178.5, which rounds up: the last frame
            (240, 61, 1.3), // inside a one-frame cut
        ]
    );
    // A zoom starts where its `at` maps to on the output.
    for (cut, segment) in timeline.segments().iter().enumerate() {
        for zoom in segment.zooms() {
            let at = timeline.frame_time(zoom.source_frame());
            let mapped = timeline.map_source_time(segment.source(), at).unwrap();
            assert_eq!(
                (mapped.cut(), mapped.output_frame()),
                (cut, zoom.output_frame())
            );
        }
    }
    assert_eq!(timeline.frame_time(45), rational(3, 2));
}

#[test]
fn source_times_map_to_the_output_and_clamp_to_the_nearest_cut() {
    use Landing::{ClampedToEnd as End, ClampedToStart as Start, Inside};
    let timeline = edge_case_timeline();
    // (source, seconds) -> (cut, output frame, landing)
    let cases = [
        // Before the first cut of `a`: clamp to the start of cut 0.
        ("a", -3.0, (0, 0, Start)),
        ("a", 0.5, (0, 0, Start)),
        // Inside cut 0, including its `in` and `out` points.
        ("a", 2.0, (0, 0, Inside)),
        ("a", 2.5, (0, 15, Inside)),
        ("a", 2.51, (0, 15, Inside)),  // 75.3 -> 75
        ("a", 2.517, (0, 16, Inside)), // 75.51 -> 76
        ("a", 3.0, (0, 30, Inside)),
        // Gap between cuts 0 and 1, which are adjacent on the output.
        ("a", 3.4, (0, 30, End)),   // 12 frames after cut 0, 48 before cut 1
        ("a", 4.0, (0, 30, End)),   // tie: the earlier cut wins
        ("a", 4.6, (1, 30, Start)), // 48 frames after cut 0, 12 before cut 1
        // Gap between cuts 1 and 3, which are not adjacent on the output.
        ("a", 6.5, (1, 60, End)),   // 15 after cut 1, 45 before cut 3
        ("a", 7.0, (1, 60, End)),   // tie: the earlier cut wins
        ("a", 7.8, (3, 61, Start)), // 54 after cut 1, 6 before cut 3
        // The one-frame cut 3 (source frames 240..241).
        ("a", 8.0, (3, 61, Inside)),
        ("a", 8.01, (3, 61, Inside)), // 240.3 -> 240
        ("a", 8.02, (3, 62, Inside)), // 240.6 -> 241, its `out`
        ("a", 8.05, (3, 62, End)),    // 241.5 -> 242
        // After the last cut of `a`.
        ("a", 100.0, (3, 62, End)),
        // Source `b`: a cut that contains the time beats an earlier one that does not.
        ("b", 0.25, (4, 70, Inside)), // 7.5 -> 8, cut 4 starts at 62
        ("b", 0.7, (4, 77, End)),     // 21: 6 after cut 4, 9 before cut 2
        ("b", 0.75, (2, 60, Start)),  // 22.5 -> 23: 8 after cut 4, 7 before cut 2
        ("b", 1.02, (2, 61, Inside)), // 30.6 -> 31, its `out`
        ("b", 5.0, (2, 61, End)),     // 119 after cut 2, 135 after cut 4
    ];
    for (source, secs, (cut, frame, landing)) in cases {
        let mapped = timeline.map_source_secs(source, secs).unwrap();
        assert_eq!(
            (mapped.cut(), mapped.output_frame(), mapped.landing()),
            (cut, frame, landing),
            "{source} at {secs} s"
        );
    }
}

#[test]
fn mapping_handles_extreme_inputs_without_panicking() {
    let timeline = edge_case_timeline();
    let frame = |source, secs| {
        timeline
            .map_source_secs(source, secs)
            .map(|m| (m.cut(), m.output_frame(), m.landing()))
    };
    assert_eq!(frame("a", f64::MAX), Some((3, 62, Landing::ClampedToEnd)));
    assert_eq!(
        frame("a", f64::INFINITY),
        Some((3, 62, Landing::ClampedToEnd))
    );
    assert_eq!(
        frame("a", f64::NEG_INFINITY),
        Some((0, 0, Landing::ClampedToStart))
    );
    assert_eq!(frame("a", -f64::MAX), Some((0, 0, Landing::ClampedToStart)));
    assert_eq!(frame("a", f64::NAN), None);
    assert_eq!(frame("no-such-source", 1.0), None);

    let total = timeline.total_frames();
    let extremes = [i64::MIN, i64::MIN + 1, -1, 0, 1, 7, i64::MAX - 1, i64::MAX];
    for numer in extremes {
        for denom in [1, 2, 3, 30, 1_000_000, i64::MAX - 1, i64::MAX] {
            let Some(t) = Rational::new(numer, denom) else {
                continue;
            };
            for source in ["a", "b"] {
                let mapped = timeline.map_source_time(source, t).unwrap();
                assert!(mapped.output_frame() <= total, "{source} at {t}");
            }
        }
    }
}

#[test]
fn mapping_is_consistent_across_a_dense_sweep() {
    let timeline = edge_case_timeline();
    for source in ["a", "b"] {
        let mut previous: Option<(usize, u64)> = None;
        for step in -1_000..12_000 {
            let secs = f64::from(step) * 0.0137;
            let mapped = timeline.map_source_secs(source, secs).unwrap();
            let segment = &timeline.segments()[mapped.cut()];
            assert_eq!(segment.source(), source);
            let frame = mapped.output_frame();
            match mapped.landing() {
                Landing::Inside => {
                    assert!((segment.output_start()..=segment.output_end()).contains(&frame));
                }
                Landing::ClampedToStart => assert_eq!(frame, segment.output_start()),
                Landing::ClampedToEnd => assert_eq!(frame, segment.output_end()),
            }
            // Within one cut, later source times never map earlier on the output.
            if let Some((cut, earlier)) = previous
                && cut == mapped.cut()
            {
                assert!(frame >= earlier, "{source} at {secs} s");
            }
            previous = Some((mapped.cut(), frame));
        }
    }
}

#[test]
fn overlapping_cuts_map_through_the_first_one_in_the_output() {
    // Cut 0 takes source frames 30..60 (output 0..30), cut 1 takes 45..75 (output 30..60).
    let timeline = timeline(30, &[("a", "1.0", "2.0", ""), ("a", "1.5", "2.5", "")]);
    let map = |secs| {
        let m = timeline.map_source_secs("a", secs).unwrap();
        (m.cut(), m.output_frame(), m.landing())
    };
    assert_eq!(map(1.75), (0, 23, Landing::Inside)); // 52.5 -> 53, inside both
    assert_eq!(map(2.25), (1, 53, Landing::Inside)); // 67.5 -> 68, only inside cut 1
}

#[test]
fn cuts_out_of_source_order_still_clamp_to_the_nearest() {
    // Cut 0 takes source frames 150..180 (output 0..30), cut 1 takes 60..90 (output 30..60).
    let timeline = timeline(30, &[("a", "5.0", "6.0", ""), ("a", "2.0", "3.0", "")]);
    let map = |secs| {
        let m = timeline.map_source_secs("a", secs).unwrap();
        (m.cut(), m.output_frame(), m.landing())
    };
    assert_eq!(map(4.0), (0, 0, Landing::ClampedToStart)); // tie: the earlier cut wins
    assert_eq!(map(0.0), (1, 30, Landing::ClampedToStart));
    assert_eq!(map(9.0), (0, 30, Landing::ClampedToEnd));
}
