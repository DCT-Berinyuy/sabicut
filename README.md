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
(`plan.toml`) and three commands for it.

| Command | What it does |
|---|---|
| `sabicut validate <plan.toml>` | Checks a plan and lists every problem, naming the field and cut index |
| `sabicut schema` | Prints the JSON Schema of the plan format |
| `sabicut timeline <plan.toml>` | Prints the output timeline derived from the plan's cuts, to the frame |

Transcription, automatic planning, previews, rendering, and the Wi-Fi hub come next.
The full plan is in the [product requirements document](docs/PRD.md).

## Build and run

You need a recent stable Rust toolchain from [rustup](https://rustup.rs) (the code uses
the 2024 edition).

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

## Repository layout

```text
crates/core/     sabicut-core: plan types, schema v1, validation, rational time, timeline
crates/cli/      sabicut-cli: the `sabicut` command
schema/v1.json   JSON Schema of plan.toml, generated from crates/core
examples/        an example plan
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
the plan types, regenerate the committed schema (a test fails until you do):

```sh
cargo run -p sabicut-cli -- schema > schema/v1.json
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
