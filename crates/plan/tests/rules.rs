//! End-to-end tests of the rules engine on synthetic transcripts: every plan it writes
//! passes validation, and no cut lands inside a word, checked independently of the
//! engine with sabicut-core's exact timeline.

use sabicut_core::{Plan, Rational, SpeechSegment, Timeline, Transcript, Word};
use sabicut_plan::{
    CaptionStyle, FalseStarts, HookText, Options, Reason, SourceInput, Zooms, plan, source_id,
};

/// A clip whose speech is given as phrases: `(start, words)`, each word 0.3 s long and
/// said back to back, as Whisper times words within a stretch of speech. Each phrase is
/// one speech segment, padded by 30 ms like `sabicut analyze`'s.
fn clip(duration: f64, phrases: &[(f64, &str)]) -> Transcript {
    let mut speech = Vec::new();
    let mut words = Vec::new();
    for &(start, text) in phrases {
        let mut t = start;
        for word in text.split(' ') {
            words.push(Word::new(word, t, t + 0.3, 0.9));
            t += 0.3;
        }
        speech.push(SpeechSegment::new(start - 0.03, t));
    }
    Transcript::new("clip.mp4", "en", "test", duration, speech, words).unwrap()
}

fn source(index: usize, transcript: Transcript) -> SourceInput {
    // The words are those of this clip, so there is no warning about a mismatch.
    let media = format!("clip{index}.mp4");
    let transcript = Transcript::new(
        media.clone(),
        transcript.language(),
        transcript.model(),
        transcript.duration(),
        transcript.speech().to_vec(),
        transcript.words().to_vec(),
    )
    .unwrap();
    SourceInput {
        id: source_id(index),
        path: media,
        words_path: format!("clip{index}.words.json"),
        transcript,
        crop_x: 0.5,
        video_end: None,
    }
}

/// Where a source's media ends: with its audio, or its video if that ends sooner.
fn end_of_media(source: &SourceInput) -> Rational {
    let audio = source.transcript.duration();
    let end = source.video_end.map_or(audio, |video| video.min(audio));
    Rational::from_secs_f64(end).unwrap()
}

/// Cut points that lie inside a word (start and end included), found with exact
/// rational time. The first frame of a clip and the end of its media are allowed.
fn mid_word_cuts(plan: &Plan, sources: &[SourceInput]) -> Vec<String> {
    let timeline = Timeline::from_plan(plan).unwrap();
    let mut found = Vec::new();
    for segment in timeline.segments() {
        let source = sources.iter().find(|s| s.id == segment.source()).unwrap();
        let fps = i64::from(timeline.fps());
        let end = end_of_media(source);
        let end_of_media = end.numer() * fps / end.denom();
        for frame in [segment.source_in(), segment.source_out()] {
            let at = timeline.frame_time(frame);
            if frame == 0 || i64::try_from(frame).unwrap() >= end_of_media {
                continue;
            }
            for word in source.transcript.words() {
                let start = Rational::from_secs_f64(word.start()).unwrap();
                let end = Rational::from_secs_f64(word.end()).unwrap();
                if start <= at && at <= end {
                    found.push(format!(
                        "{} {} in {:?}",
                        source.id,
                        at.to_f64(),
                        word.text()
                    ));
                }
            }
        }
    }
    found
}

fn cut_times(plan: &Plan) -> Vec<(String, f64, f64)> {
    plan.cuts()
        .iter()
        .map(|cut| (cut.src().to_owned(), cut.in_point(), cut.out_point()))
        .collect()
}

/// Cuts that end after the end of their source's video, found with exact rational time.
fn cuts_past_the_video(plan: &Plan, sources: &[SourceInput]) -> Vec<String> {
    let timeline = Timeline::from_plan(plan).unwrap();
    timeline
        .segments()
        .iter()
        .filter_map(|segment| {
            let source = sources.iter().find(|s| s.id == segment.source()).unwrap();
            let video_end = Rational::from_secs_f64(source.video_end?).unwrap();
            let out = timeline.frame_time(segment.source_out());
            (out > video_end).then(|| {
                format!(
                    "{} ends at {}, after its video ({})",
                    source.id,
                    out.to_f64(),
                    video_end.to_f64()
                )
            })
        })
        .collect()
}

#[test]
fn no_cut_goes_past_the_end_of_the_video() {
    // The audio lasts 10 s, but the video ends at 9.7 s, while "now." is said.
    let sources = [SourceInput {
        video_end: Some(9.7),
        ..source(
            0,
            clip(10.0, &[(1.0, "Stop leaking your keys."), (9.2, "Bye now.")]),
        )
    }];
    let planned = plan(&sources, &Options::default()).unwrap();
    assert_eq!(
        cut_times(planned.plan()),
        [("a".to_owned(), 0.833, 2.3), ("a".to_owned(), 9.033, 9.7)]
    );
    let report = planned.report();
    assert_eq!(report.mid_word, 0);
    assert_eq!(
        mid_word_cuts(planned.plan(), &sources),
        Vec::<String>::new()
    );
    let reasons: Vec<Reason> = report.removals.iter().map(|r| r.reason).collect();
    assert_eq!(reasons, [Reason::Pause, Reason::Pause]);
    assert_eq!(
        report.warnings,
        [
            "source a: the video ends at 9.700 s, before the speech does (9.800 s), so the speech after it is cut"
        ]
    );

    // Ending at 9.4 s, the video leaves too little of "Bye now." to keep.
    let sources = [SourceInput {
        video_end: Some(9.4),
        ..sources[0].clone()
    }];
    let planned = plan(&sources, &Options::default()).unwrap();
    assert_eq!(cut_times(planned.plan()), [("a".to_owned(), 0.833, 2.3)]);
    let last = planned.report().removals.last().unwrap();
    assert_eq!(
        (last.reason, last.text.as_str()),
        (Reason::Short, "Bye now.")
    );
    assert!((last.start - 2.3).abs() < 1e-9 && (last.end - 9.4).abs() < 1e-9);
    assert!(
        planned
            .toml()
            .contains("# removed a 2.300 - 9.400 (7.100 s): too short \"Bye now.\"")
    );
    assert_eq!(planned.report().warnings.len(), 1);

    // A video that ends after the audio changes nothing, and gets no warning.
    let longer = [SourceInput {
        video_end: Some(10.5),
        ..sources[0].clone()
    }];
    let without = [SourceInput {
        video_end: None,
        ..sources[0].clone()
    }];
    let planned = plan(&longer, &Options::default()).unwrap();
    assert_eq!(
        planned.toml(),
        plan(&without, &Options::default()).unwrap().toml()
    );
    assert!(planned.report().warnings.is_empty());
}

#[test]
fn a_video_end_must_be_a_time() {
    for bad in [f64::NAN, -1.0, f64::INFINITY] {
        let sources = [SourceInput {
            video_end: Some(bad),
            ..source(0, clip(3.0, &[(1.0, "Hello.")]))
        }];
        let error = plan(&sources, &Options::default()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("video_end must be a time of 0 s or more"),
            "{error}"
        );
    }
}

#[test]
fn pauses_are_cut_and_speech_is_kept() {
    let sources = [source(
        0,
        clip(
            12.0,
            &[
                (1.0, "Stop leaking your keys."),
                (4.0, "Here is how."),
                (9.0, "Bye."),
            ],
        ),
    )];
    let planned = plan(&sources, &Options::default()).unwrap();
    // Each phrase keeps 0.15 s before its segment (which starts 30 ms early) and 0.1 s
    // after it, on 30 fps boundaries.
    assert_eq!(
        cut_times(planned.plan()),
        [
            ("a".to_owned(), 0.833, 2.3),
            ("a".to_owned(), 3.833, 5.0),
            ("a".to_owned(), 8.833, 9.4),
        ]
    );
    let report = planned.report();
    assert_eq!(report.mid_word, 0);
    assert_eq!(report.cut_points, 6);
    let reasons: Vec<Reason> = report.removals.iter().map(|r| r.reason).collect();
    assert_eq!(reasons, [Reason::Pause; 4]);
    assert!((report.input_seconds() - 12.0).abs() < 1e-9);
    // 44 + 35 + 17 frames.
    assert_eq!(report.output_frames, 96);
    assert!((report.output_seconds() - 3.2).abs() < 1e-9);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn a_false_start_is_cut_and_reported() {
    let transcript = Transcript::new(
        "clip.mp4",
        "en",
        "test",
        8.0,
        vec![SpeechSegment::new(0.97, 1.9), SpeechSegment::new(2.27, 4.9)],
        ["So", "today", "we..."]
            .iter()
            .enumerate()
            .map(|(i, text)| Word::new(*text, 1.0 + 0.3 * i as f64, 1.3 + 0.3 * i as f64, 0.9))
            .chain(
                [
                    "So", "today", "we're", "going", "to", "talk", "about", "Rust.",
                ]
                .iter()
                .enumerate()
                .map(|(i, text)| Word::new(*text, 2.3 + 0.3 * i as f64, 2.6 + 0.3 * i as f64, 0.9)),
            )
            .collect(),
    )
    .unwrap();
    let sources = [source(0, transcript)];
    let planned = plan(&sources, &Options::default()).unwrap();
    assert_eq!(cut_times(planned.plan()), [("a".to_owned(), 2.133, 5.0)]);
    let false_starts: Vec<(f64, f64, &str)> = planned
        .report()
        .removals
        .iter()
        .filter(|r| r.reason == Reason::FalseStart)
        .map(|r| (r.start, r.end, r.text.as_str()))
        .collect();
    assert_eq!(false_starts.len(), 1);
    let (start, end, text) = false_starts[0];
    assert_eq!(text, "So today we...");
    assert!(start < 1.0 && (end - 2.133).abs() < 0.001, "{start} {end}");
    assert!(
        planned
            .toml()
            .contains("# removed a 0.000 - 2.133 (2.133 s): false start \"So today we...\"")
    );
    assert_eq!(
        mid_word_cuts(planned.plan(), &sources),
        Vec::<String>::new()
    );

    // Turned off, the take stays.
    let options = Options {
        false_starts: None,
        ..Options::default()
    };
    let kept = plan(&sources, &options).unwrap();
    assert_eq!(cut_times(kept.plan()), [("a".to_owned(), 0.833, 5.0)]);
}

#[test]
fn a_false_start_with_no_pause_is_kept_and_listed() {
    // "today-" and the restart are timed back to back, with no silence to cut in.
    let words = ["So", "today-", "so", "today", "we", "start."];
    let transcript = Transcript::new(
        "clip.mp4",
        "en",
        "test",
        4.0,
        vec![SpeechSegment::new(0.97, 2.8)],
        words
            .iter()
            .enumerate()
            .map(|(i, text)| Word::new(*text, 1.0 + 0.3 * i as f64, 1.3 + 0.3 * i as f64, 0.9))
            .collect(),
    )
    .unwrap();
    let sources = [source(0, transcript)];
    let planned = plan(&sources, &Options::default()).unwrap();
    assert_eq!(planned.plan().cuts().len(), 1);
    let kept = &planned.report().kept_false_starts;
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].text, "So today-");
}

#[test]
fn emphasis_is_never_cut() {
    let sources = [source(
        0,
        clip(6.0, &[(1.0, "This is very,"), (2.2, "very good.")]),
    )];
    let planned = plan(&sources, &Options::default()).unwrap();
    assert!(
        planned
            .report()
            .removals
            .iter()
            .all(|r| r.reason != Reason::FalseStart)
    );
    assert_eq!(planned.plan().cuts().len(), 1);
}

#[test]
fn several_clips_keep_their_order_and_name_their_own_words() {
    let sources = [
        source(0, clip(5.0, &[(1.0, "First clip here."), (3.5, "More.")])),
        source(1, clip(4.0, &[(0.5, "Second clip.")])),
    ];
    let planned = plan(&sources, &Options::default()).unwrap();
    let srcs: Vec<String> = planned
        .plan()
        .cuts()
        .iter()
        .map(|c| c.src().to_owned())
        .collect();
    assert_eq!(srcs, ["a", "a", "b"]);
    let plan = planned.plan();
    assert_eq!(plan.sources()[1].words(), Some("clip1.words.json"));
    assert_eq!(plan.captions().unwrap().words(), None);
    assert_eq!(plan.caption_words("b"), Some("clip1.words.json"));
    assert_eq!(planned.report().sources.len(), 2);
    assert_eq!(mid_word_cuts(plan, &sources), Vec::<String>::new());
}

#[test]
fn a_single_clip_names_its_words_in_the_captions() {
    let sources = [source(0, clip(3.0, &[(1.0, "Use Supabase now.")]))];
    let options = Options {
        keywords: vec!["Supabase".into()],
        captions: Some(CaptionStyle {
            fixes: Some("fixes.toml".into()),
            ..CaptionStyle::default()
        }),
        ..Options::default()
    };
    let planned = plan(&sources, &options).unwrap();
    let captions = planned.plan().captions().unwrap();
    assert_eq!(captions.words(), Some("clip0.words.json"));
    assert_eq!(captions.fixes(), Some("fixes.toml"));
    assert_eq!(captions.keywords(), ["Supabase"]);
    // Style fields are left out, so the defaults apply.
    assert_eq!(captions.font(), None);
    assert_eq!(captions.active(), "#FFC400");
    assert!(!planned.toml().contains("stroke"));
    assert_eq!(planned.plan().sources()[0].words(), None);
    // "Supabase" starts at 1.3 s.
    let zooms = planned.plan().cuts()[0].zooms();
    assert_eq!(zooms.len(), 1);
    assert_eq!((zooms[0].at(), zooms[0].scale()), (1.3, 1.25));

    let none = Options {
        captions: None,
        ..Options::default()
    };
    assert!(plan(&sources, &none).unwrap().plan().captions().is_none());
}

#[test]
fn the_hook_is_written_and_fits_the_output() {
    let sources = [source(0, clip(3.0, &[(1.0, "Short one.")]))];
    let options = Options {
        hook: Some(HookText {
            text: "Stop leaking keys".into(),
            dur: HookText::DEFAULT_DUR,
        }),
        ..Options::default()
    };
    let planned = plan(&sources, &options).unwrap();
    let hook = planned.plan().hook().unwrap();
    assert_eq!(hook.text(), "Stop leaking keys");
    // The output is 26 frames (0.867 s), shorter than 2 s.
    assert_eq!(hook.dur(), 0.867);
    assert_eq!(planned.report().warnings.len(), 1);
    assert!(Timeline::from_plan(planned.plan()).is_ok());
}

#[test]
fn a_clip_without_words_gets_no_cuts() {
    let empty = Transcript::new("b.mp4", "und", "test", 5.0, vec![], vec![]).unwrap();
    let sources = [
        source(0, clip(3.0, &[(1.0, "Hello there.")])),
        source(1, empty),
    ];
    let planned = plan(&sources, &Options::default()).unwrap();
    assert_eq!(planned.plan().cuts().len(), 1);
    assert!(
        planned
            .toml()
            .contains("# b: clip1.words.json has no words")
    );
    assert!(
        planned
            .report()
            .warnings
            .iter()
            .any(|w| w.contains("no words"))
    );

    let only_empty = [source(
        0,
        Transcript::new("b.mp4", "und", "t", 5.0, vec![], vec![]).unwrap(),
    )];
    assert!(plan(&only_empty, &Options::default()).is_err());
}

#[test]
fn bad_options_are_rejected() {
    let sources = [source(0, clip(3.0, &[(1.0, "Hello.")]))];
    let cases = [
        Options {
            fps: 0,
            ..Options::default()
        },
        Options {
            zooms: Zooms {
                keyword_scale: Some(5.0),
                ..Zooms::default()
            },
            ..Options::default()
        },
        Options {
            false_starts: Some(FalseStarts {
                min_words: 3,
                max_words: 2,
                ..FalseStarts::default()
            }),
            ..Options::default()
        },
        Options {
            keywords: vec![" ".into()],
            ..Options::default()
        },
    ];
    for options in cases {
        assert!(plan(&sources, &options).is_err(), "{options:?}");
    }
}

/// A small deterministic random number generator (xorshift), so the tests need no crate.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `low..high`, to the millisecond.
    fn seconds(&mut self, low: f64, high: f64) -> f64 {
        let steps = ((high - low) * 1000.0) as u64;
        low + (self.next() % steps.max(1)) as f64 / 1000.0
    }
}

/// A clip like `sabicut analyze` writes: stretches of speech, each holding words timed
/// back to back (sometimes with a short pause or a zero-length word), with pauses of
/// all sizes between them, and sometimes a phrase broken off and said again.
fn random_clip(random: &mut Random) -> Transcript {
    const VOCABULARY: [&str; 12] = [
        "so",
        "today",
        "we",
        "learn",
        "Rust,",
        "and",
        "Supabase.",
        "it",
        "is",
        "fast...",
        "very",
        "good-",
    ];
    let mut t = random.seconds(0.0, 1.0);
    let mut speech = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    for _ in 0..(3 + random.next() % 12) {
        let start = t;
        let phrase: Vec<&str> = (0..(1 + random.next() % 8))
            .map(|_| VOCABULARY[(random.next() % VOCABULARY.len() as u64) as usize])
            .collect();
        let repeat = random.next().is_multiple_of(4);
        let spoken: Vec<&str> = if repeat {
            let cut = 1 + phrase.len() / 2;
            phrase[..cut.min(phrase.len())]
                .iter()
                .chain(phrase.iter())
                .copied()
                .collect()
        } else {
            phrase
        };
        for text in spoken {
            let length = if random.next().is_multiple_of(15) {
                0.0
            } else {
                random.seconds(0.04, 0.6)
            };
            words.push(Word::new(text, t, t + length, 0.9));
            t += length;
            if random.next().is_multiple_of(6) {
                t += random.seconds(0.01, 0.35);
            }
        }
        let lead = random.seconds(0.0, 0.2);
        let tail = random.seconds(0.0, 0.3);
        speech.push(SpeechSegment::new((start - lead).max(0.0), t + tail));
        t += tail + random.seconds(0.05, 2.5);
    }
    // Sometimes the recording stops mid-word.
    let duration = if random.next().is_multiple_of(3) {
        words.last().map_or(t, Word::end)
    } else {
        t
    };
    let speech = merge_overlaps(speech, duration);
    Transcript::new("clip.mp4", "en", "test", duration, speech, words).unwrap()
}

/// Speech segments in order, joined where padding made them overlap, within the media.
fn merge_overlaps(segments: Vec<SpeechSegment>, duration: f64) -> Vec<SpeechSegment> {
    let mut out: Vec<SpeechSegment> = Vec::new();
    for segment in segments {
        let (start, end) = (segment.start(), segment.end().min(duration));
        match out.last_mut() {
            Some(last) if start <= last.end() => {
                *last = SpeechSegment::new(last.start(), end.max(last.end()));
            }
            _ if end > start => out.push(SpeechSegment::new(start, end)),
            _ => {}
        }
    }
    out
}

#[test]
fn no_cut_lands_inside_a_word_on_any_clip_or_frame_rate() {
    let mut random = Random(0x5AB1_C07F);
    // Video ends come from a generator of their own, so the clips stay the same.
    let mut ends = Random(0xE17D_0F71);
    let mut plans = 0;
    for round in 0..300 {
        let fps = [24, 25, 30, 60, 23, 7, 1][round % 7];
        let clips: Vec<SourceInput> = (0..(1 + round % 3))
            .map(|i| {
                let clip = source(i, random_clip(&mut random));
                // Half the videos end up to 1 s before their audio, some a little after.
                let audio = clip.transcript.duration();
                let video_end = match ends.next() % 4 {
                    0 | 1 => Some((audio - ends.seconds(0.0, 1.0)).max(0.0)),
                    2 => Some(audio + ends.seconds(0.0, 0.5)),
                    _ => None,
                };
                SourceInput { video_end, ..clip }
            })
            .collect();
        let options = Options {
            fps,
            keywords: vec!["Rust".into(), "Supabase".into()],
            zooms: Zooms {
                hold: round.is_multiple_of(2).then_some(0.3),
                alternate: round.is_multiple_of(3).then_some(1.1),
                ..Zooms::default()
            },
            hook: Some(HookText {
                text: "Hook".into(),
                dur: 2.0,
            }),
            ..Options::default()
        };
        let Ok(planned) = plan(&clips, &options) else {
            continue;
        };
        plans += 1;
        assert!(
            Timeline::from_plan(planned.plan()).is_ok(),
            "round {round}:\n{}",
            planned.toml()
        );
        assert_eq!(
            mid_word_cuts(planned.plan(), &clips),
            Vec::<String>::new(),
            "round {round} at {fps} fps:\n{}",
            planned.toml()
        );
        assert_eq!(planned.report().mid_word, 0, "round {round}");
        assert_eq!(
            cuts_past_the_video(planned.plan(), &clips),
            Vec::<String>::new(),
            "round {round} at {fps} fps:\n{}",
            planned.toml()
        );
        // The written text is the plan.
        assert_eq!(
            &Plan::from_toml_str(planned.toml()).unwrap(),
            planned.plan()
        );
    }
    assert!(plans > 250, "only {plans} plans");
}
