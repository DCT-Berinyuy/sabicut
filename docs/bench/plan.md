# Rules for `sabicut plan`

This note records the measurements behind the defaults of `sabicut plan` (PRD feature F3):
how pauses are cut, how much silence is kept around speech, and how false starts are
found. It uses the word timings that `sabicut analyze` wrote for the benchmark's LibriVox
readings (see [analyze.md](analyze.md)), checked against the same audio. It ran on
5 November 2026.

## Decisions

- **Pauses longer than 0.4 s are cut** (`--max-pause`), keeping **0.15 s of silence ahead
  of speech and 0.1 s after it** (`--pad-before`, `--pad-after`). A pause is only cut if
  that removes at least **0.2 s** (`--min-cut`), and kept sections shorter than **0.5 s**
  are dropped (`--min-keep`). On the 10-minute readings this removes 34 s of English
  (5.7%) and 59 s of French (9.9%) and cuts into no phrase start. 125 of 134 English and
  140 of 170 French cut points are in silence, most of the rest on breath or room noise,
  and the cut sits at least 0.15 s ahead of the voice for 90% of the phrases.
- **Speech runs to the edges of its speech segment**, up to 1 s past its words. The
  voice-activity segments find where a voice starts and stops better than Whisper's word
  times do. With the same padding around the words alone, 14 English cut points fell on
  the voice and 1.4 s of voice was cut.
- **No cut lands inside a word.** Every cut point is a frame boundary strictly between two
  words, so the rate of cuts inside a word is 0 by construction, not by tuning: 0 of 392
  cut points on the four transcripts, and none in a test of 300 random transcripts at 1 to
  60 fps. PRD §9 allows up to 5%.
- **False starts are cut conservatively.** The readings have none, but they repeat words on
  purpose in 7 places, such as "Down, down, down", "Oh dear, oh dear" and "nous nous", and
  the rule kept all of them. The synthetic tests cover the restarts it does cut.
- **Zooms** follow [docs/render.md](../render.md): each one eases in over 0.2 s and holds
  until the next zoom or the end of the cut. The keyword zoom is 1.25×, inside the earlier
  Python pipeline's 1.1–1.35. Zooming back out after a keyword (`--zoom-hold`) and zooming
  every second cut (`--cut-zoom`) are options, off by default.

## How it was measured

- **Transcripts.** `sabicut analyze` with the `small` model and default settings, on the
  10-minute readings (`full-small-en.json`, 2063 words; `full-small-fr.json`, 1899 words)
  and on the 2-minute windows of the WER check (`dod2-small-*.json`), with the sample
  fixes of [analyze.md](analyze.md) applied.
- **Audio checks** ([`plan_check.py`](../../spikes/whisper-bench/plan_check.py)) use the
  benchmark's 10 ms frame energies, relative to loud speech:
  - *Silence* is below the noise floor (10th-percentile frame) plus 6 dB.
  - *Voice* is at least −35 dB, or the noise floor plus 12 dB if that is higher. English
    needs the second part: its noise floor is −31 dB.
  - A *cut point* is a cut's `in` or `out`, not counting the start and the end of the
    output. It is *silent* when every frame within 20 ms of it is silence, *voiced* when
    one is voice, and *noise* otherwise: breaths, room tone, clicks.
  - The *onset margin* runs from a cut's `in` to its first voice frame, and the *tail
    margin* from its last voice frame to its `out`. A margin under 20 ms is *tight*: either
    the cut clips speech, or a breath or click above the voice level sits right at the
    cut. Every tight margin in the tables below was checked by hand, and the notes under
    the sweep say which they are.
  - *Mid-word* means a cut point inside a word's start and end, both included.
- **Sweep** ([`plan_sweep.py`](../../spikes/whisper-bench/plan_sweep.py)): one setting
  changed at a time from the defaults, on the 10-minute readings.

## Results

### Defaults

| Transcript | Words | Cuts | Kept | Cut | Mid-word | Cut points: silent / noise / voiced | Voice cut | Tight onsets / tails |
|---|---|---|---|---|---|---|---|---|
| English, 10 min | 2063 | 68 | 565.8 s | 34.2 s | 0 of 136 | 125 / 8 / 1 | 0.17 s | 0 of 68 / 0 of 67 |
| French, 10 min | 1899 | 86 | 540.7 s | 59.3 s | 0 of 172 | 140 / 29 / 1 | 0.18 s | 0 of 86 / 0 of 85 |
| English, 2 min | 494 | 23 | 130.1 s | 9.9 s | 0 of 46 | | | |
| French, 2 min | 415 | 19 | 117.5 s | 12.5 s | 0 of 38 | | | |

The audio was only checked on the whole readings. The 0.17 s of English voice that is cut
is mostly a loud 0.14 s sound at 286.8 s, right after "suddenly, thump!" in the text,
that neither the speech detection nor Whisper counted as speech, so no rule based on the
words can keep it. The one voiced cut point in each language is a burst of breath or lip
noise 20–30 ms after the cut, not a word: the next word starts 0.46 s (English) and 0.2 s
(French) later. Onset and tail margins are 0.17–0.23 s at the median.

### Sweep

| Language | Setting | Cuts | Cut | Cut points: silent / noise / voiced | Voice cut | Onset margin p10 / median | Tight onsets | Tail margin p10 / median | Tight tails |
|---|---|---|---|---|---|---|---|---|---|
| en | max-pause 0.3 | 68 | 34.2 s | 125 / 8 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| en | **max-pause 0.4** | 68 | 34.2 s | 125 / 8 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| en | max-pause 0.5 | 65 | 33.6 s | 119 / 8 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| en | max-pause 0.75 | 40 | 26.0 s | 73 / 4 / 1 | 0.14 s | 0.16 / 0.18 | 0 | 0.21 / 0.23 | 0 |
| en | max-pause 1.0 | 15 | 13.1 s | 27 / 1 / 0 | 0.00 s | 0.16 / 0.18 | 0 | 0.22 / 0.24 | 0 |
| en | pads 0.08 / 0.08 | 83 | 43.8 s | 155 / 9 / 0 | 0.18 s | 0.09 / 0.12 | 0 | 0.19 / 0.22 | 0 |
| en | pads 0.12 / 0.08 | 75 | 39.2 s | 142 / 5 / 1 | 0.18 s | 0.13 / 0.15 | 0 | 0.19 / 0.21 | 0 |
| en | **pads 0.15 / 0.1** | 68 | 34.2 s | 125 / 8 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| en | pads 0.2 / 0.1 | 60 | 29.7 s | 112 / 6 / 0 | 0.17 s | 0.21 / 0.23 | 0 | 0.21 / 0.23 | 0 |
| en | pads 0.2 / 0.2 | 46 | 21.9 s | 83 / 7 / 0 | 0.17 s | 0.22 / 0.23 | 0 | 0.31 / 0.33 | 0 |
| en | min-cut 0.1 | 83 | 36.3 s | 154 / 9 / 1 | 0.17 s | 0.16 / 0.18 | 0 | 0.21 / 0.23 | 0 |
| en | **min-cut 0.2** | 68 | 34.2 s | 125 / 8 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| en | min-cut 0.3 | 52 | 30.5 s | 97 / 4 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| en | vad-reach 0 (words only) | 88 | 47.2 s | 148 / 12 / 14 | 1.43 s | 0.01 / 0.14 | 9 | 0.08 / 0.22 | 4 |
| en | vad-reach 0.3 | 68 | 34.8 s | 124 / 8 / 2 | 0.20 s | 0.16 / 0.19 | 0 | 0.20 / 0.23 | 1 |
| en | **vad-reach 1.0** | 68 | 34.2 s | 125 / 8 / 1 | 0.17 s | 0.16 / 0.19 | 0 | 0.21 / 0.23 | 0 |
| fr | max-pause 0.3 | 86 | 59.3 s | 140 / 29 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | **max-pause 0.4** | 86 | 59.3 s | 140 / 29 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | max-pause 0.5 | 83 | 58.7 s | 137 / 26 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.18 | 0 |
| fr | max-pause 0.75 | 57 | 49.4 s | 95 / 17 / 0 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.18 | 0 |
| fr | max-pause 1.0 | 31 | 33.6 s | 56 / 4 / 0 | 0.11 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | pads 0.08 / 0.08 | 94 | 69.2 s | 146 / 38 / 2 | 0.21 s | 0.08 / 0.10 | 2 | 0.14 / 0.17 | 0 |
| fr | pads 0.12 / 0.08 | 92 | 64.9 s | 146 / 34 / 2 | 0.19 s | 0.12 / 0.14 | 1 | 0.14 / 0.16 | 0 |
| fr | **pads 0.15 / 0.1** | 86 | 59.3 s | 140 / 29 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | pads 0.2 / 0.1 | 82 | 54.2 s | 137 / 24 / 1 | 0.17 s | 0.20 / 0.23 | 1 | 0.16 / 0.18 | 0 |
| fr | pads 0.2 / 0.2 | 73 | 44.8 s | 130 / 12 / 2 | 0.15 s | 0.20 / 0.22 | 1 | 0.26 / 0.28 | 1 |
| fr | min-cut 0.1 | 95 | 60.7 s | 152 / 35 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | **min-cut 0.2** | 86 | 59.3 s | 140 / 29 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | min-cut 0.3 | 80 | 57.9 s | 132 / 25 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.18 | 0 |
| fr | vad-reach 0 (words only) | 94 | 64.3 s | 141 / 37 / 8 | 0.67 s | 0.11 / 0.17 | 2 | 0.09 / 0.18 | 6 |
| fr | vad-reach 0.3 | 86 | 59.3 s | 140 / 29 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |
| fr | **vad-reach 1.0** | 86 | 59.3 s | 140 / 29 / 1 | 0.18 s | 0.15 / 0.17 | 0 | 0.16 / 0.19 | 0 |

Bold rows are the defaults. Each row changes one setting; the others keep their defaults.
Mid-word cut points are 0 in every row.

- **Padding** sets how much air stays around the voice. At the 10th percentile the cut
  sits about `--pad-before` ahead of the voice. The tail margin is 0.06–0.13 s more than
  `--pad-after`, because a speech segment ends a little after the voice fades below the
  voice level. No padding setting cut into speech: each tight margin in these rows is a
  short breath or click (10–60 ms) above the voice level at the cut, with the voice
  100–200 ms away. The defaults are the smallest padding that keeps the cut at least
  0.15 s ahead of the voice for 90% of phrases in both languages, which leaves room for a
  phrase that the speech detection finds late. More padding keeps hardly any more voice
  (French 0.15–0.17 s cut instead of 0.18 s, English no change) and leaves more silence:
  0.2 / 0.2 s cuts 36% less dead air in English and 24% less in French. How much air
  sounds right is a judgment to confirm by ear on rendered shorts.
- **Smallest cut.** At 0.1 s there are 15 (English) and 9 (French) more cuts, for 1–2 s
  more removed. That is many tiny jumps for little gain. At 0.3 s, 3.7 s (English) and
  1.4 s (French) more of the pauses stay in.
- **Threshold.** Above 0.5 s the threshold leaves real dead air: at 1 s English keeps 21 s
  more of it. The 2-minute windows cut 7–10% at the default.
- **Speech-segment edges.** Anchored on the word times alone (`vad-reach 0`), the cuts run
  into the voice. In English, 7 phrase starts and 4 phrase ends lose some voice, and 2
  more starts begin right at the cut. In French, 1 start and 5 ends lose some voice; the
  other two tight margins are a click and a phrase end that stops right at the cut. In
  all, 1.4 s of English and 0.7 s of French voice is cut. Letting speech run up to 0.3 s
  past its words leaves one English phrase end that loses about 30 ms, and 1 s none.

### False starts

The readings are clean takes, so the rule finds no false starts in them: none cut and none
kept for lack of a pause, in 4871 words. They do repeat words on purpose, and the rule kept
every one:

| Transcript | Repeats in the words | Why they are kept |
|---|---|---|
| English | "Down, down, down." (twice), "thump, thump," | One word repeated is never a false start |
| English | "Oh dear, oh dear, I shall be late" | No pause after the first "dear,", so nothing breaks off. With a pause it would be the same clause said twice, which is kept too |
| English | "Do cats eat bats? Do cats eat bats?" | The first take ends a sentence |
| French | "nous nous" (as in "nous nous sommes"), "Non, non," | One word repeated |

The rule on synthetic transcripts (unit tests in `crates/plan`):

| Speech | Result |
|---|---|
| "So today we… ‖ So today we're going to talk about Rust." | Cuts "So today we…" |
| "So today… ‖ So today we- ‖ So today we're going to talk about Rust." | Cuts both fragments, keeps the last take |
| "I think it ‖ I think it works." (no punctuation, 0.3 s pause) | Cuts "I think it" |
| "Let me tod- ‖ let me today show you." | Cuts "Let me tod-" (a word cut short) |
| "So today we, ‖ so today we're going to talk." | Cuts "So today we," |
| "So today- so today we start." (no pause) | Keeps it and lists it: there is no silence to cut in |
| "This is very, ‖ very good." / "No no no, it is fine." / "I- I think so." | Keeps them: one word repeated |
| "Oh dear, ‖ oh dear, ‖ I shall be late." | Keeps it: the same clause, comma included |
| "Thank you. ‖ Thank you so much." | Keeps it: the first take is a complete sentence |
| "Go team, ‖ go team" | Keeps it: the second take goes no further |
| "I like Rust. ‖ I like Go too." / "So today we ‖ so tomorrow we will see." | Keeps them: the words differ |

‖ is a pause of 0.3 s.

## What surprised me

1. **Whisper's words inside a speech segment are back to back.** 1896 of the 2062 English
   word gaps are exactly 0, and every pause longer than 0.25 s falls between two speech
   segments. So the pauses that `plan` cuts are the gaps between speech segments. A gap
   shorter than about 0.45 s is never cut, whatever the threshold: once 0.25 s of padding
   is kept, less than the smallest cut (0.2 s) would go. That is why the thresholds 0.3
   and 0.4 s give the same plan. The threshold stays at 0.4 s for transcripts that do have
   pauses inside their speech segments.
2. **Word times alone cut into speech.** The first word of a phrase often starts later than
   the voice: its speech segment starts up to 440 ms (English) and 220 ms (French) before
   it, 30 ms at the median in English. Anchoring cuts on the speech segments instead of
   the words took the English cut points on the voice from 14 to 1.
3. **French cut points land on breaths** (29 of 170, against 8 of 134 in English). The
   French recording is so quiet that a breath before a phrase rises above the silence
   level. They are more than 35 dB below loud speech, and the render fades every cut over
   10 ms, so they do not click.

## Reproduce

The scripts are in [`spikes/whisper-bench/`](../../spikes/whisper-bench/). They read the
word timings and fixes from `$SABICUT_BENCH/words` and the audio from
`$SABICUT_BENCH/audio` (default `~/.cache/sabicut/bench`), and use the release build of
`sabicut` unless `SABICUT_BIN` names another:

```sh
cargo build --release
cargo test -p sabicut-plan --test real -- --ignored --nocapture   # all four transcripts
cd spikes/whisper-bench
python3 plan_sweep.py                                              # the sweep table
B=~/.cache/sabicut/bench
mkdir -p out
../../target/release/sabicut plan $B/audio/en.wav --words $B/words/full-small-en.json \
  --fixes $B/words/fixes-en.toml -o out/plan-en.toml
python3 plan_check.py out/plan-en.toml $B/words/full-small-en.json $B/audio/en.wav
```
