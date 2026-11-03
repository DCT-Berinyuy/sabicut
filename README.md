# Sabicut

**An offline video studio for communities without laptops or cheap data.**

A Rust engine turns raw phone clips into captioned vertical shorts. One laptop "hub"
serves a whole club over its own Wi-Fi: students upload from their phone browsers and
get finished videos back. No internet, no data cost, no app to install.

*Sabi* is Pidgin for "to know / know-how".

## Status

**Pre-alpha and time-boxed.** Sabicut cannot make a video yet.

| Phase | Window | Exit criterion |
|---|---|---|
| Engine v0.1 | 2026-11-02 → 2026-11-16 | One DCT Lab short made with Sabicut and published |
| Hub v0.2 | 2026-11-16 → 2026-11-30 | With internet off, 5 phones upload clips over the hub's Wi-Fi and each gets back a finished short |
| Pilot | by 2026-12-07 | One real club session in which at least 10 students each leave with a short |

If a phase misses its exit criterion by more than a week, work stops for a review.

What works today is the foundation that everything else builds on: the edit-plan format
(`plan.toml`) and its commands, offline transcription to word timings (`words.json`), and
transcript fixes (`fixes.toml`).

| Command | What it does |
|---|---|
| `sabicut validate <plan.toml>` | Checks a plan and lists every problem, naming the field and cut index |
| `sabicut schema [plan\|words\|fixes]` | Prints the JSON Schema of a file format (plan.toml by default) |
| `sabicut timeline <plan.toml>` | Prints the output timeline derived from the plan's cuts, to the frame |
| `sabicut analyze <media>` | Transcribes the speech in a video or audio file to word timings, offline |
| `sabicut fix <words.json> <fixes.toml>` | Applies transcript fixes to word timings and lists what changes |

Automatic planning, previews, rendering, and the Wi-Fi hub come next.
The full plan is in the [product requirements document](docs/PRD.md).

## Build and run

You need a recent stable Rust toolchain from [rustup](https://rustup.rs) (the code uses
the 2024 edition). Building also compiles whisper.cpp, so you need CMake, a C and C++
compiler, and libclang (on Debian and Ubuntu: `cmake`, `build-essential` and
`libclang-dev`); the first build takes a few minutes. `sabicut analyze` runs `ffmpeg`, so
install that too.

```sh
git clone https://github.com/DCT-Berinyuy/sabicut
cd sabicut
cargo run -- validate examples/plan.toml
cargo run -- timeline examples/plan.toml
cargo run -- schema
```

To put `sabicut` on your `PATH`, run `cargo install --path crates/cli`.

`sabicut timeline examples/plan.toml` prints:

```text
examples/plan.toml: 3 cuts, 412 frames (13.733 s) at 30 fps

cut  source        source time (s)        output time (s)  frames  output frames
  0  a           3.133 -     9.867      0.000 -     6.733     202  0..202
     zoom x1.25 at source 5.400 s -> output 2.267 s (frame 68)
  1  a          12.000 -    15.500      6.733 -    10.233     105  202..307
     zoom x1.15 at source 13.000 s -> output 7.733 s (frame 232)
     zoom x1.00 at source 14.200 s -> output 8.933 s (frame 268)
  2  b           0.500 -     4.000     10.233 -    13.733     105  307..412

hook: output 0.000 - 2.000 s (frames 0..60)
```

When a plan has problems, `validate` and `timeline` list all of them and exit with
status 1:

```text
error: broken.toml: 2 problems
  - cut[0].out must be > in
  - cut[1].src must be the id of a [[source]], got "b"
```

Indexes count from 0, in file order. Usage errors exit with status 2.

## The edit plan

A short is described by a hand-editable `plan.toml`. Schema v1 is frozen: within v1
only optional fields may be added, and a breaking change means `schema = 2`. See
[`examples/plan.toml`](examples/plan.toml) and section 5 of the [PRD](docs/PRD.md).

| Key | Required | Meaning |
|---|---|---|
| `schema` | yes | Plan format version, always `1` |
| `meta.fps` | yes | Output frame rate, 1 to 240 |
| `meta.out` | yes | Output size `{ w, h }` in pixels: even, 2 to 8192 |
| `[[source]]` `id`, `path`, `crop_x` | yes | Source clips. `id` is 1 to 64 letters, digits, `_` or `-`; `crop_x` is the centre of the 9:16 crop, 0 to 1 |
| `[[cut]]` `src`, `in`, `out` | yes | Ranges kept from the sources, in output order, in seconds of source time (0 to 86400) |
| `[[cut]]` `zoom` | no | Punch-in zooms `[{ at, scale }]`: `at` is source time inside the cut, at least 1 frame apart and before `out`; `scale` is 1 to 4 |
| `[hook]` `text`, `dur` | no | Hook text shown for the first `dur` seconds of the output |
| `[captions]` `words`, `font`, `active`, `stroke` | no | Word timings file, font file, active-word colour (`#RRGGBB`), outline width (0 to 64 px) |
| `[captions]` `fixes`, `keywords` | no | Replacement rules file, and words always drawn in the active colour |

Relative paths are resolved from the plan file's directory. Unknown keys are rejected, so
typos are caught.

**Timing is derived, never stored.** Times are read to the microsecond. Each cut's `in`
and `out` snap to the nearest frame boundary at `meta.fps`, so every cut is a whole number
of frames and the output length is exact. Mapping a source time to the output (for
zooms and, later, captions) clamps to the nearest cut and never panics.

**Editor support.** The JSON Schema is committed at [`schema/v1.json`](schema/v1.json).
Editors that use [Taplo](https://taplo.tamasfe.dev), such as the Even Better TOML
extension for VS Code, pick it up from the `#:schema ../schema/v1.json` line at the top
of the example and offer completion and inline errors.

## Transcription

`sabicut analyze` turns the speech in a video or audio file into word timings, with no
network connection: ffmpeg reads the audio, Silero finds the stretches of speech, and
whisper.cpp transcribes them, one chunk of up to 30 s at a time.

```sh
sabicut analyze raw/IMG_0412.mov                 # writes raw/IMG_0412.words.json
sabicut analyze talk.mp4 --lang fr -o words.json --keywords "Supabase,Rust"
```

| Option | Default | Meaning |
|---|---|---|
| `--model` | `small` | `tiny`, `base`, `small`, or the path of a ggml model file. Only `small` has usable French |
| `--lang` | `auto` | `auto` (tells English from French), `en`, `fr`, or `pidgin` (transcribed as English) |
| `--threads` | `4` | CPU threads for Whisper. Voice-activity detection always uses 1 |
| `--keywords` | none | Names and terms to spell right, separated by commas. Whisper gets them as a hint for the whole file; it helps but does not force a spelling (fixes.toml does) |
| `-o`, `--out` | `<name>.words.json` next to the media | Output file, or `-` for standard output |
| `--vad-pad`, `--vad-threshold` | tuned per language | Audio kept around each stretch of speech (ms), and the speech probability threshold (0 to 1) |

Every word ends inside the stretch of speech it starts in, so word ends no longer run on
into pauses, and punctuation that stands apart (as in French, `moulin !`) is joined to its
word with a no-break space. The measurements behind the defaults are in
[`docs/bench/analyze.md`](docs/bench/analyze.md).

Before the model loads, `analyze` checks the free memory (`MemAvailable`) and stops with a
suggestion of a smaller model if there is not enough: `small` needs about 0.7 GB. Only one
transcription runs at a time on a machine; a second one waits for the first. Set
`SABICUT_WHISPER_LOG=1` to see whisper.cpp's own log lines.

### Speech models

Sabicut never downloads anything. It looks for model files in `$SABICUT_MODELS`, then
`$XDG_DATA_HOME/sabicut/models`, then `~/.local/share/sabicut/models`, and if one is
missing it names every path it tried.

| File | Size | Needed for | SHA-256 |
|---|---|---|---|
| `ggml-silero-v6.2.0.bin` | 0.9 MB | every run | `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987` |
| `ggml-small-q5_1.bin` | 190 MB | `--model small` (default) | `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb` |
| `ggml-base-q5_1.bin` | 60 MB | `--model base` | `422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898` |
| `ggml-tiny-q5_1.bin` | 32 MB | `--model tiny` | `818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7` |

The Whisper models are published at <https://huggingface.co/ggerganov/whisper.cpp> and the
Silero model at <https://huggingface.co/ggml-org/whisper-vad>. To save data, copy them from
a machine that already has them, and check them with `sha256sum`.

### Word timings: `words.json`

`words.json` (schema v1) lists the stretches of speech and every word, with times in
seconds to the millisecond. Like the plan format, it is versioned, frozen at v0.1, and
strict: unknown keys are rejected. The JSON Schema is committed at
[`schema/words-v1.json`](schema/words-v1.json); see
[`examples/words.json`](examples/words.json).

```json
{
  "schema": 1,
  "media": "IMG_0412.mov",
  "language": "en",
  "model": "ggml-small-q5_1.bin",
  "duration": 7.5,
  "speech": [
    {"start":0.4,"end":2.35}
  ],
  "words": [
    {"text":"Stop","start":0.52,"end":0.8,"probability":0.98}
  ]
}
```

| Key | Meaning |
|---|---|
| `schema` | Format version, always `1` |
| `media` | File name of the transcribed media, without its folder |
| `language` | `en`, `fr`, or `und` when no speech was found to tell |
| `model` | File name of the speech model that wrote the words |
| `duration` | Length of the audio, in seconds |
| `speech` | Stretches of speech (`start` < `end`), in order and not overlapping. Only these were transcribed |
| `words` | Words in spoken order: `text` (with its punctuation), `start` <= `end`, and `probability` (0 to 1). A word starts no earlier than the previous one ends |

### Transcript fixes: `fixes.toml`

The speech model gets some words wrong, especially names. `fixes.toml` (schema v1) holds
replacement rules that are applied to the words every time they are used, so a new
transcription keeps them; the raw `words.json` is never changed. The JSON Schema is
committed at [`schema/fixes-v1.json`](schema/fixes-v1.json).

```toml
schema = 1

[[fix]]
from = "Supa base"   # several words become one, from the start of the first to the end of the last
to = "Supabase"

[[fix]]
from = "rust"        # one word for one keeps its timing
to = "Rust"
```

Rules match whole words and ignore capitals (unless `case_sensitive = true`) and the
punctuation around the words, which is kept: `get hub.` becomes `GitHub.`. Where rules
overlap, the one that matches more words wins. Rules are applied again until nothing
changes, so applying them twice gives the same result as once, and rules that undo each
other are an error. `sabicut fix` shows what the rules change:

```text
$ sabicut fix examples/words.json examples/fixes.toml
examples/words.json: 3 changes from examples/fixes.toml
      4.020 -     4.620  "Supa base" -> "Supabase"  (fix[0])
      4.760 -     5.100  "rust" -> "Rust"  (fix[1])
      6.600 -     7.050  "get hub." -> "GitHub."  (fix[2])
```

With `--out fixed.json` (or `-o -` for standard output) it also writes the fixed words.

## Repository layout

```text
crates/core/     sabicut-core: plan types, schema v1, validation, rational time, timeline,
                 words.json and fixes.toml (no dependency on the speech model)
crates/analyze/  sabicut-analyze: ffmpeg audio, Silero speech detection, whisper.cpp
crates/cli/      sabicut-cli: the `sabicut` command
schema/          JSON Schemas of plan.toml (v1.json), words.json and fixes.toml,
                 generated from crates/core
examples/        an example plan, word timings, and fixes
docs/PRD.md      product requirements
spikes/          throwaway experiments, kept out of the Cargo workspace
```

New crates are added only when they contain real code.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on every push to `main` and on every pull request. If you change
the plan, words or fixes types, regenerate the committed schemas (a test fails until you
do):

```sh
cargo run -p sabicut-cli -- schema > schema/v1.json
cargo run -p sabicut-cli -- schema words > schema/words-v1.json
cargo run -p sabicut-cli -- schema fixes > schema/fixes-v1.json
```

The transcription tests that need real models are ignored by default, because CI has no
models. With `ggml-tiny-q5_1.bin` and `ggml-silero-v6.2.0.bin` in a model folder and
ffmpeg installed, run them with:

```sh
cargo test -p sabicut-analyze -- --ignored
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
