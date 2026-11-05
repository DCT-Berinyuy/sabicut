# Render benchmark

This benchmark checks `sabicut render` against PRD §8 and §9: peak memory under 800 MB,
and a 10-minute recording turned into a 60 s short in at most 8 minutes on 4 CPUs
(transcription excluded). The harness is [`spikes/render-bench/`](../../spikes/render-bench/)
(`bench.py`). It ran on 3 November 2026.

## Result

- **Memory: under budget for 1080p sources.** Rendering 20 cuts from a 10-minute 1080p30
  source peaked at **656 MB** with the `medium` preset and **588 MB** with `veryfast`.
  These are the summed resident memory of `sabicut` and all its ffmpeg processes. Each
  process counts the shared ffmpeg libraries again; split proportionally (PSS), the peaks
  are 614 and 546 MB.
- **Time: well inside the 8 minutes.** 141 s with `medium` and 90 s with `veryfast` for a
  60 s short. These are upper bounds: another session's Whisper runs kept the load average
  at 8 to 9 on this 4-thread laptop, which can only slow a render down. 87% of the time is
  the video encode.
- **Every frame was right.** All 1,800 output frames showed the source frame the
  timeline asks for, read back from a frame-number patch in the source. That includes a
  pair of cuts used out of order and six cuts with zooms. Loudness came out at −13.8 LUFS
  (target −14 ± 1).
- **4K HDR is over budget: 848 MB** for a 4K 10-bit HEVC HLG source, like iPhone HDR
  video (see below). The PRD already asks creators to record in 1080p (§13). Bringing 4K
  under 800 MB needs a design choice, listed at the end.

These runs are not clean timings. The laptop was shared with other work all evening
(load 8 to 9, and swap well used by other programs), and a quiet re-run was not possible,
so the clean measurement moves to the real hub laptop, as for the Whisper benchmark. Swap
cannot have inflated the times beyond that, and it is unlikely to have hidden memory:
`VmSwap` was not recorded, but free memory (`MemAvailable`) never fell below 1.45 GB
during these runs, so the kernel had no reason to swap out ffmpeg's working set.

## Setup

| | |
|---|---|
| Machine | Intel Core i7-6600U (2 cores, 4 threads, 2.6–3.4 GHz), 7.6 GiB RAM, Arch Linux |
| ffmpeg | 9.0.2 (Arch), and the 6.1.3 static build from BtbN for comparison |
| Source | 600 s, 1920×1080, 30 fps, H.264 (x264 `veryfast`, CRF 23, 1 s GOP like a phone), AAC stereo 128 kb/s with a 220 Hz tone whose level drifts. A grey patch in the middle encodes the frame number |
| Plan | 20 cuts of 2.4 to 3.6 s spread over the 10 minutes (60.0 s in all). Cuts 7 and 8 are swapped, a retake used out of order. A punch-in on every third cut, and an in-and-out zoom on cut 10 |
| Output | 1080×1920, 30 fps, x264 CRF 20, AAC 160 kb/s 48 kHz, cover JPG |
| Memory | VmRSS of `sabicut` and every process under it, sampled every 0.1 s and summed. The PSS sum and the largest single process (`wait4` `ru_maxrss`) are recorded too |
| Guard | Each run in its own systemd scope with `MemoryMax=1200M` and no swap |

## Results

| ffmpeg | Preset | Wall time | Audio | Loudness pass | Video | Peak memory (RSS / PSS) | Largest process | Wrong frames | Loudness | Load before |
|---|---|---|---|---|---|---|---|---|---|---|
| 9.0.2 | medium | 140.6 s | 5.2 s | 6.6 s | 121.8 s | 656 / 614 MB | 487 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 8.0 |
| 9.0.2 | veryfast | 89.9 s | 2.5 s | 5.0 s | 76.2 s | 588 / 546 MB | 427 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 8.9 |

The rest of each run is the final checks (frame count and loudness of the file, about
5 s) and the cover (under 1 s). Peak memory is always reached in the video step, where
one cut's decoder runs next to the encoder. The audio step peaked at 124 MB.

## How memory came down

Every row renders the same plan with `medium` on ffmpeg 9.0.2.

| Design | Peak memory (RSS) |
|---|---|
| One ffmpeg run, one input per cut (each seeked before its cut), `concat` | 1,229 MB |
| One decoding ffmpeg per cut, in turn, piping raw frames to one encoder | 817 MB |
| … with 2 decoder threads instead of ffmpeg's default of 5 | 783 MB |
| … with x264's lookahead capped at 20 frames (`medium` uses 40) | **656 MB** |

- **One input per cut is not enough.** It avoids `split` holding frames for later cuts.
  But ffmpeg opens every input's decoder at the start and keeps its buffers to the end,
  so memory grows with the number of cuts: about 30 MB per cut here.
- **One decoder at a time.** Memory no longer depends on the number of cuts. What is
  left is the encoder (x264 alone at 1080×1920 `medium`: 564 MB) and one decoder.
- **Lookahead.** Each x264 lookahead frame costs about 5 MB at 1080×1920. Capping it at
  20 frames saved 106 MB at the same speed. On real footage (a 10 s Big Buck Bunny clip
  cropped to 9:16, CRF 20) the cap gave a 6% smaller file with SSIM 0.9787 instead of
  0.9791, a difference you cannot see.

## 4K HDR HEVC

A 20 s 3840×2160 HEVC 10-bit HLG source (BT.2020, like iPhone HDR video), 6 cuts, `medium`:

| Change | Encoder | One cut's decoder | Peak (sum) |
|---|---|---|---|
| As for 1080p (2 decoder threads, tone mapping at the 1216×2160 crop size) | 519 MB | 369 MB | 886 MB |
| 1 decoder thread for sources over 1080p, tone mapping after scaling to 1080×1920 | 535 MB | 322 MB | **848 MB** |

The decoder holds 4K 10-bit reference frames (25 MB each) and the tone mapping works on
32-bit float RGB frames (25 MB each at 1080×1920). Decoding plus tone mapping ran at 6 to
8 fps, slower than the encoder, so a 60 s short from 4K HDR footage would take about 4 to
5 minutes. That is still inside the 8-minute target.

Options to bring 4K under 800 MB, not done yet because each costs something:

1. **Decode first, then encode.** Write the cuts to an intermediate file, then encode it.
   The peak becomes the larger of decoder and encoder (about 540 MB) instead of their sum.
   It costs disk space and time (decode and encode no longer overlap).
2. **A smaller encoder for 4K sources.** For example `veryfast` (427 MB), or a lookahead
   of 10 frames.
3. **Keep recommending 1080p recording** (PRD §13) and let 4K run slightly over budget.

## With captions (F6)

The same plan with word-by-word captions and a 2 s hook (`--captions`): the English
benchmark transcript (`full-small-en.json`, 10 minutes, 2,063 words) is read as the
source's speech, so every cut of the 60 s short has captions: 208 words on the output,
29 subtitle cues. Run on
5 November 2026 on the same laptop, ffmpeg 9.0.2. Each pair ran back to back, with the
load average at 2.3 to 2.5 (the Copilot app and a browser in the background).

| Preset | Captions | Wall time | Video | Peak memory (RSS / PSS) | Largest process | Wrong frames | Loudness | Load before |
|---|---|---|---|---|---|---|---|---|
| veryfast | no | 65.7 s | 55.6 s | 570 / 523 MB | 400 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 2.5 |
| veryfast | yes | 68.5 s | 58.7 s | 630 / 582 MB | 450 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 2.3 |
| medium | no | 98.1 s | 87.8 s | 670 / 623 MB | 496 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 2.5 |
| medium | yes | 103.6 s | 93.9 s | 691 / 645 MB | 515 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 2.5 |
| ultrafast | no | 53.7 s | 40.5 s | 354 / 311 MB | 207 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 2.4 |
| ultrafast | yes | 58.8 s | 45.5 s | 393 / 348 MB | 232 MB (encoder) | 0 of 1,800 | −13.8 LUFS | 2.5 |

- **Time: 3 to 6 s more per 60 s short** (4 to 9%): drawing the band, piping it, and
  the encoder's `overlay`. Single runs on this shared laptop vary more than that, though:
  `medium` without captions took 111.0 s in an earlier run at the same load, and with
  captions 135.4 s, so take the pairs, not single numbers.
- **Memory: 20 to 60 MB more, peak 691 MB with `medium`**, still under the 800 MB
  budget. The encoder holds a few band pictures in its input queue and the overlay's
  frames; `sabicut` itself keeps the word pictures of one caption page.
- **Before a fix, captions cost 70 MB with `medium`** (727 MB): the painter kept every
  word picture it had drawn, up to 512, and ffmpeg queued 8 band pictures. It now starts
  its cache again at every page and ffmpeg queues 4, like the video.

## Reproduce

```sh
cargo build --release -p sabicut-cli
python3 spikes/render-bench/bench.py                       # medium and veryfast
python3 spikes/render-bench/bench.py --preset medium --ffmpeg /path/to/other/ffmpeg
python3 spikes/render-bench/bench.py --preset medium --captions ~/.cache/sabicut/bench/words/full-small-en.json
```

The script generates the source (about 430 MB, in `~/.cache/sabicut-render-bench`) on the
first run, waits until no compiler is running and 1.5 GB of memory is free, and appends
one JSON line per run to `spikes/render-bench/out/results.jsonl`.
