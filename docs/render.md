# Rendering and previews

`sabicut render` turns a plan into the finished vertical MP4 and a cover JPG (PRD F5).
`sabicut preview` writes PNG stills and a contact sheet so you can check crops and zooms
first (PRD F4). Both drive the `ffmpeg` binary as a subprocess (PRD §8) and share the
code that builds the filter graph, so a still shows exactly what the render shows at the
same time.

Captions, the hook text, the end card and the SRT are not drawn yet: they come with the
overlay crate (F6). A plan's `[hook]` and `[captions]` are accepted and ignored, with a
note.

```sh
sabicut preview plan.toml --at 1.5,8,22 --safe-zones   # stills + contact sheet
sabicut render plan.toml                               # plan.mp4 + plan.jpg
sabicut render plan.toml -o short.mp4 --cover-at 3.2 --preset veryfast
```

| Option | Meaning |
|---|---|
| `render -o, --output` | The MP4 to write. Default: the plan's path ending in `.mp4` |
| `render --cover` | The cover JPG to write. Default: the video's path ending in `.jpg` |
| `render --cover-at` | Output time of the cover frame in seconds. Default: 1 s, or the middle of a video shorter than 2 s |
| `render --no-cover` | Write no cover |
| `render --preset` | x264 speed preset: `medium` (default, personal use) or `veryfast` (hub); any x264 preset name works |
| `preview --at` | Output times of the stills, comma-separated |
| `preview -o, --out-dir` | Directory for the stills and sheet. Default: `<plan>-preview` next to the plan |
| `preview --safe-zones` | Shade the areas the apps cover with their interface (see below) |

## What you need

ffmpeg and ffprobe 6.1 or newer, on `PATH` or named by `SABICUT_FFMPEG` and
`SABICUT_FFPROBE` (without `SABICUT_FFPROBE`, the `ffprobe` next to `SABICUT_FFMPEG` is
used). `sabicut analyze` uses the same setting. Tested with Ubuntu 24.04's 6.1.1 (CI), a 6.1.3 static build and Arch's 9.0.2.
The build needs:

- encoders `libx264`, `aac`, `png` (stills) and `mjpeg` (cover);
- the usual built-in filters, plus `zoompan` for zooms;
- `zscale` (libzimg) and `tonemap`, only for HDR sources.

Sabicut lists ffmpeg's filters and encoders before it starts and stops with a message
naming what is missing, for example: `ffmpeg 6.1.1 lacks the zscale filter needed to
tone-map HDR (PQ or HLG) video to SDR; install an ffmpeg build that has it`.

Source paths are opened as local files only (`file:` URLs with
`-protocol_whitelist file`), so a plan can never make ffmpeg open a network address.

## How a render runs

1. **Audio.** One ffmpeg run reads each cut's audio, trims it on the cut's frame
   boundaries, joins the cuts and cleans them up into a 32-bit float WAV.
2. **Loudness.** `loudnorm` measures the WAV (first pass).
3. **Video.** One encoding ffmpeg runs for the whole render. Each cut is decoded by a
   short-lived ffmpeg of its own, one cut after another, which trims, crops, tone-maps,
   scales and zooms it and pipes the raw frames to the encoder. The encoder adds the
   overlays, normalizes the WAV (second loudness pass) and writes the MP4.

A render first waits while a transcription (`sabicut analyze`) or another render runs on
the same machine: they share one lock, so only one heavy job runs at a time (PRD H4).
Previews do not wait, because they are light and meant to be quick.

Intermediate files go in a hidden scratch directory next to the output (not `/tmp`, which
is often RAM) and are removed afterwards. The MP4 is written to the scratch directory
first, and its frame count is checked against the timeline before it replaces the
output, so a failed render never leaves a broken video behind. Each cut must also
deliver exactly its number of frames to the encoder, or the render stops and names the
cut.

### Memory

Peak memory has to stay under 800 MB (PRD §8), on a laptop that may also be running
Whisper.

- **Not `split`.** The obvious graph opens the source once and uses `split`, `trim` and
  `concat`. `concat` reads its inputs in order, so while the first cut is encoded,
  `split` holds every decoded frame meant for the later cuts.
- **Not one input per cut either.** Opening the source once per cut in a single ffmpeg
  run, each input seeked just before its cut, avoids that. But ffmpeg opens every input's
  decoder at the start, and each keeps its frame buffers until the end. Measured with 20
  cuts from a 1080p source, that peaked at 1.23 GB.
- **One decoder at a time.** So the render runs one decoding ffmpeg per cut, in turn, and
  pipes raw frames (1080×1920 4:2:0, 3.1 MB a frame) into one encoder. Memory is one
  decoder plus the encoder, whatever the number of cuts.

The audio pass still opens one input per cut in a single run, because audio decoders are
tiny. Filter graphs are passed in files (`-/filter_complex`, or `-filter_complex_script`
before ffmpeg 7), so plans with many cuts never hit command-line limits. Measured times
and memory are in [bench/render.md](bench/render.md).

## Frame-exact cuts

`sabicut-core` snaps every cut to a frame grid at `meta.fps` that starts at **source
time 0**. Sabicut defines source time 0 as the start of the file, that is ffprobe's
`format.start_time` (the earliest stream). Players count from there too, and ffmpeg's
`-copyts -start_at_zero` produces it. Phone clips often start at a non-zero timestamp,
and their audio and video may start at slightly different times.

Each cut's video goes through:

```text
fps=fps=<meta.fps>:start_time=0, trim=start_pts=<in frame>:end_pts=<out frame>, setpts=…
```

The `fps` filter converts any source, constant or variable frame rate, to the output
rate on that same grid. Output frame `k` shows the last source frame whose timestamp,
times the fps, rounds to `k` or less (halves round away from zero). Because the grid is
anchored at source time 0 with `start_time=0`, the seek point does not move it, and a
still and the render pick the same source frame. `trim` then keeps exactly the cut's
frames. Two edge cases are padded:

- A cut that starts before the first video frame (the file starts with audio) repeats
  the first frame for those slots.
- A cut that ends less than one frame after the end of the source holds the last frame.
  A cut that ends more than a frame after it is an error naming the cut.

The tests check this on a 60 fps clip, a rotated portrait clip, and a variable-frame-rate
clip with jittered timestamps, dropped frames and a 1.5 s start offset. The clips carry a
patch whose brightness encodes the source frame number, so every output frame is checked,
not just the cut boundaries.

> **Note for transcription (F1):** word times in `words.json` must use the same origin.
> Either decode with `-copyts -start_at_zero` and `aresample=async=1:first_pts=0`, or add
> the audio stream's start minus `format.start_time` to times counted from the first
> audio sample.

## Crop

The crop is worked out on the frame **as displayed**. ffmpeg applies the rotation from
the clip's display matrix as it decodes, so a portrait phone clip stored as 1920×1080
with a 90° rotation is cropped as 1080×1920. Non-square pixels are taken into account.

- A source **wider** than the output's shape (landscape, or 3:4 portrait) keeps its full
  height. The 9:16 window's centre is at `crop_x` of the width (0 = left edge, 1 = right
  edge), moved inwards so the window stays on the frame.
- A source **taller** than the output's shape (a 9:19.5 screen recording) keeps its full
  width and is cropped equally top and bottom. `crop_x` does not apply.
- A source with exactly the output's shape is not cropped.

All crop values are even, to line up with 4:2:0 chroma. The window is then scaled to
`meta.out` (bicubic).

## Zooms

Plan schema v1 says when a zoom starts (`at`, source time) and how far (`scale`, 1 to
4), not how it moves. Sabicut defines the motion:

- **Every cut starts unzoomed** (scale 1). Zooms do not carry over a cut.
- From its first frame, a zoom **eases** from the current scale to its own scale over
  **0.2 s** (6 frames at 30 fps), with an ease-out cubic curve: it moves quickly at first
  and settles gently, like a punch-in.
- The ease is **cut short** if the next zoom or the end of the cut comes sooner, so each
  zoom reaches its scale before the next one starts.
- It then **holds** that scale until the next zoom or the end of the cut. A zoom with
  `scale = 1` eases back out.
- The zoom is **centred** on the crop window.

For example, `zoom = [{ at = 13.0, scale = 1.15 }, { at = 14.2, scale = 1.0 }]` punches
in to 1.15× at 13.0 s, holds it, and eases back out at 14.2 s. Cuts with zooms use
ffmpeg's `zoompan`, which crops the zoomed window from the source pixels and scales it in
one step. Cuts without zooms use a plain `scale`.

## HDR

A source whose transfer is tagged PQ (`smpte2084`) or HLG (`arib-std-b67`), as iPhone
HDR video and many Android HDR clips are, is tone-mapped to SDR BT.709 after the crop.
`zscale` converts it to linear light, then `tonemap` (Hable curve, no desaturation) maps
it to BT.709. HEVC and 10-bit sources work. Dolby Vision profile 5 (no HDR10 or HLG base
layer) is not supported and comes out with wrong colours.

## Colour

Output is BT.709, limited range, `yuv420p`, and tagged that way. Sources tagged with
another matrix (BT.601, BT.2020) are converted. Untagged sources are taken to be BT.709
when 1280 pixels or more on the long side and BT.601 otherwise, as players do.
Full-range sources (`yuvj` formats, or tagged `pc`) are converted to limited range.

## Audio

Each cut's audio is cut on the **same frame boundaries as its video**. Its length is
counted between the cut's output frame boundaries, so audio never drifts from video over
many cuts, even at frame rates that do not divide 48 kHz. Sources are resampled to
48 kHz stereo. A source without audio gives silence, and a source whose audio ends early
is padded with silence. Every cut fades in and out over **10 ms** (480 samples) against
clicks.

After the cuts are joined:

| Step | Filter | Setting |
|---|---|---|
| Denoise | `afftdn` | 12 dB reduction, −40 dB noise floor, noise tracking on |
| High-pass | `highpass` | 80 Hz, against rumble and handling noise |
| Compressor | `acompressor` | −20 dB threshold, 3:1, 5 ms attack, 120 ms release |
| Loudness | `loudnorm`, two passes | −14 LUFS integrated, −1.5 dBTP true peak, 11 LU range |
| Encode | `aac` | 160 kb/s, 48 kHz, stereo |

The second `loudnorm` pass applies one constant gain when it can reach −14 LUFS without
exceeding the true-peak limit. Otherwise (or when the loudness range is 0, as with a
test tone) it normalizes dynamically. Dynamic mode's timestamps jump partway through,
which would put the audio out of sync, so the audio is re-stamped from its sample count
after `loudnorm`. The render measures its own output afterwards and reports the
loudness. The tests require −14 ± 1 LU, measured with ffmpeg's EBU R128 meter.

## Video encoding

libx264, CRF 20, High profile, `yuv420p`, constant `meta.fps`, `+faststart` (the index at
the front, so phones can start playing before the download ends). `--preset` trades speed
for size at the same quality.

One change from x264's presets: lookahead is capped at 20 frames (`-rc-lookahead 20`)
for `fast` and slower presets. At 1080×1920 each lookahead frame costs about 5 MB, so
`medium`'s default of 40 frames takes about 200 MB of the 800 MB budget. The cap saves
about 100 MB at the same speed. On real footage at CRF 20 it gave a file 6% smaller, with
SSIM 0.9787 instead of 0.9791 (PSNR 0.06 dB lower), which looks the same. `veryfast`
and faster presets already look ahead 20 frames or fewer.

## Cover

The cover is the frame of the **finished video** at `--cover-at`, as a full-size
1080×1920 JPG (quality 2 of 31). It therefore includes captions once they are drawn. The
default is 1 s in: late enough to miss a blink on the first frame, and inside the hook
(the first 2 s) once hooks are drawn.

## Previews

`sabicut preview plan.toml --at 1.5,8,22` writes `still-01-1.500s.png`,
`still-02-8.000s.png`, … and `contact-sheet.png` (stills 360 pixels wide, five per row,
in the order given) into the preview directory. A time maps to the frame being shown at
that moment. Each still seeks its source to just before the frame it needs, decodes up to
that one frame and runs the same filters as the render. ffmpeg makes up to four stills
per run (each still needs a decoder of its own, so this bounds memory), then lays out
the sheet from the PNGs. Five stills take about 2 to 3 seconds.

### Safe-zone guide

`--safe-zones` shades, in translucent red with a white edge, the parts of the frame that
TikTok, Instagram Reels and YouTube Shorts cover with their own interface. Keep faces,
captions and on-screen text out of them. The zones are fractions of the frame, so they
fit any output size:

| Zone | Covers | Fraction of the frame | On 1080×1920 |
|---|---|---|---|
| Top bar | status bar, Following/For You tabs, search | top 12.5% | y 0–240 |
| Bottom | caption, sound ticker, profile, Shorts' subscribe button | bottom 25% | y 1440–1920 |
| Right column | like, comment, share, remix buttons | right 15%, from 25% to 75% down | x 918–1080, y 480–1440 |

These are the union of the three apps' organic (non-ad) layouts, rounded outwards, from
CapQA's measured masks ([capqa.app/guides/social-media-safe-zones][capqa], version of
11 September 2026):

| App | Top bar | Bottom area starts at | Action column |
|---|---|---|---|
| TikTok | 130 px | y 1570 | x 918–1080, y 730–1536 |
| Instagram Reels | 210 px | y 1590 | x 996–1080, y 614–1460 |
| YouTube Shorts | 234 px | y 1575 (subscribe button from y 1480) | x 936–1080, y 499–1460 |

Paid ads are stricter. Meta asks for 14% at the top, 35% at the bottom and 6% at each
side of a Reels ad, so treat the guide as the minimum.

[capqa]: https://capqa.app/guides/social-media-safe-zones

## Overlays (for F6)

`RenderOptions::overlays` is the extension point for captions, the hook and the end
card. Each `Overlay` names a source (for now a local file: a PNG with transparency is
held, a video is drawn frame by frame), a top-left position, and optionally the output
frames it is drawn on. It is placed with `overlay=x:y` after the cuts are joined, with
`enable='between(n,…)'` for its frame window. This matches PRD §8: render only the
caption band (about 1080×400 RGBA), place it with `overlay`, and draw other graphics only
inside their time windows. The caption band will arrive as a new `OverlaySource` (raw
RGBA frames piped to ffmpeg).
