# Transcription settings for `sabicut analyze`

This note records the measurements behind the defaults of `sabicut analyze` (PRD feature
F1): the voice-activity detection (VAD) settings, ending words at their speech segment,
whisper.cpp's dynamic time warping (DTW) timestamps, flash attention, and the keyword
hint. It follows the [day-one benchmark](whisper.md), on the same machine and audio, with
the `small` q5_1 model and 4 threads. It ran on 3 November 2026.

## Decisions

- **VAD: Silero with whisper.cpp's default settings, for English and French alike:**
  threshold 0.5 and 30 ms of padding. More padding made word timing worse in both
  languages and added 4–9% more audio to transcribe (100–200 ms of padding). It recovered a few French phrase
  starts that Silero finds late, but the French word error rate (WER) changed by 3 words
  out of 395, which is within decoding noise. A lower threshold changed almost nothing.
- **Every word ends inside its speech segment.** Together with transcribing exactly the VAD
  segments, this cut the words that sit in a pause from 7.6% to 0.6% (English) and from
  3.6% to 0.5% (French), and word ends now run at most 170 ms into a pause instead of up to
  1.1 s.
- **DTW timestamps are not used.** Read correctly, they move word starts by 5–10 ms at the
  median and do not fix the tail, and they cost 45–65 MiB of memory and 16% more time.
  `--dtw` stays as a hidden, experimental option.
- **Flash attention is not used either.** It saves 135 MiB (23%) of memory and gives the
  same words, but it was about 30% slower on the 2-minute windows, and `small` has almost
  no time to spare. `--flash-attn` stays hidden; it is worth re-measuring on the hub laptop,
  where memory may be tighter than time.
- **Feeding audio:** speech is packed into chunks of at most 30 s (Whisper's window), each
  transcribed on its own with the keyword hint and the end of the previous text as its
  prompt. A chunk costs one encoder pass, as with a single long call, and every time maps
  back exactly.

## How it was measured

- **Audio.** The benchmark's LibriVox recordings, cut to the parts that hold the
  hand-checked WER windows: English 190–330 s (140 s) and French 224–354 s (130 s). The VAD
  sweep used the whole 10-minute files.
- **Runs.** One transcription at a time, each in its own systemd scope with
  `MemoryMax=1536M` and no swap, started only when at least 1.5 GiB was free and no
  compiler or ffmpeg was running. Peak memory is the process's `VmHWM`.
- **Timing checks** ([`analyze_check.py`](../../spikes/whisper-bench/analyze_check.py)):
  - *Onset*: as in the benchmark, the end of each pause of at least 300 ms, where the energy
    rises 6 dB above the noise floor. The first word after the pause should start there.
  - *Voice onset*: after each such pause, the first 10 ms frame within 35 dB of loud speech.
    The French recording is so quiet (noise floor 52 dB below loud speech, against 25 dB in
    English) that the benchmark's onset fires on breaths before a phrase; the voice onset
    does not. In English the two are the same.
  - *Short pauses*: voice onsets after pauses of at least 120 ms, which adds boundaries
    inside phrases.
  - *Words in pauses*: words whose midpoint lies in a pause of at least 300 ms, as in the
    benchmark. *Overrun*: how far a word's end runs into the pause after it.
  - *WER* on the hand-checked window, with the benchmark's `wer.py`.
- **Baseline.** The benchmark's own `small` run (one whisper.cpp call on the whole file,
  VAD with 30 ms of padding, no end clamping), cut to the same windows.
- **Shared machine.** As in the benchmark, the Copilot app used about 1.3 CPU cores and
  3 GB of RAM. Another session rendered video with ffmpeg and ran cargo builds through most
  of the evening; runs that overlapped them are marked, and their times are not comparable.
  Word timings and WER do not depend on this: greedy decoding is deterministic.

## Results

### VAD padding and threshold

Silero alone on the whole 10-minute files. "Clipped" counts the phrase starts where the
speech segment begins more than 50 ms after the start of the phrase.

| Language | Threshold | Padding | Segments | Speech kept | Clipped (benchmark onset) | Clipped (voice onset) |
|---|---|---|---|---|---|---|
| English | 0.5 | 30 ms | 164 | 522.6 s | 10 of 157 | 10 of 157 |
| English | 0.5 | 100 ms | 164 | 545.5 s | 7 | 7 |
| English | 0.5 | 200 ms | 164 | 570.8 s | 4 | 4 |
| English | 0.5 | 300 ms | 164 | 584.4 s | 2 | 2 |
| English | 0.35 | 30 ms | 151 | 528.9 s | 9 | 9 |
| French | 0.5 | 30 ms | 127 | 507.4 s | 40 of 105 | 12 of 101 |
| French | 0.5 | 100 ms | 127 | 525.2 s | 30 | 9 |
| French | 0.5 | 200 ms | 127 | 547.6 s | 27 | 7 |
| French | 0.5 | 300 ms | 127 | 565.1 s | 22 | 6 |
| French | 0.35 | 30 ms | 121 | 510.8 s | 40 | 10 |

The benchmark's "40 of 101 French segments start late" is reproduced, but 28 of those 40
are breaths: measured from where the voice starts, French is clipped about as often as
English. The rest is real: on a few quiet phrase starts ("On…", "Toujours", "En…"), Silero
fires 300–800 ms after the voice begins, and 30 ms of padding can drop the first word.

### Word timing and accuracy

`small`, one row per run on the 2-minute windows. Times are median / 90th percentile /
median signed error, in milliseconds; a positive error means the word starts late.

| Run | Voice onset | Short pauses | Words in pauses | Overrun (median / p90 / max) | WER |
|---|---|---|---|---|---|
| **English** | | | | | |
| Benchmark run (baseline) | 110 / 280 / +95 | 110 / 300 / +60 | 7.6% | 270 / 740 / 850 | 3.5% |
| analyze, padding 30 ms (default) | **30 / 130 / 0** | **40 / 300 / −10** | **0.6%** | **110 / 130 / 170** | 3.3% |
| analyze, padding 200 ms | 90 / 180 / −10 | 90 / 210 / −10 | 6.0% | 225 / 300 / 350 | 3.3% |
| analyze, 30 ms + DTW | 25 / 80 / −20 | 30 / 390 / −20 | 0.0% | 110 / 150 / 170 | 3.3% |
| **French** | | | | | |
| Benchmark run (baseline) | 95 / 370 / +35 | 95 / 390 / +20 | 3.6% | 250 / 690 / 1140 | 10.6% |
| analyze, padding 30 ms (default) | **30 / 510 / +10** | **30 / 510 / 0** | **0.5%** | **30 / 50 / 50** | 12.4% |
| analyze, padding 100 ms | 80 / 440 / −5 | 70 / 250 / 0 | 0.7% | 100 / 140 / 150 | 11.9% |
| analyze, padding 200 ms | 80 / 340 / +20 | 80 / 250 / +10 | 2.0% | 195 / 250 / 280 | 11.6% |
| analyze, 30 ms + DTW | 20 / 510 / +10 | 25 / 510 / 0 | 0.0% | 30 / 50 / 50 | 12.4% |

There are 42 voice onsets and 65 short-pause onsets in the English window, and 24 and 38 in
the French one. The French WER rows differ by 49, 47 and 46 errors out of 395 words; the
differences are scattered substitutions of similar-sounding words ("près" / "après",
"nus" / "nu"), plus one phrase start that 30 ms of padding loses.

### DTW timestamps

whisper.cpp's `t_dtw` is not where a token starts. Its placement loop starts from token 0
and records the time at which the alignment *leaves* each token, so a token's `t_dtw` is
where the next token begins. Used as word starts, those times are late:

| Run | Voice onset (EN) | Voice onset (FR) | Peak memory (EN / FR) |
|---|---|---|---|
| Padding 200 ms, no DTW | 90 / 180 / −10 | 80 / 340 / +20 | 594 / 590 MiB |
| Padding 200 ms, `t_dtw` of the word's first token | 90 / 180 / **+75** | 120 / 600 / **+120** | 649 / 628 MiB |
| Padding 30 ms, no DTW | 30 / 130 / 0 | 30 / 510 / +10 | 592 / 591 MiB |
| Padding 30 ms, `t_dtw` of the token before the word | 25 / 80 / −20 | 20 / 510 / +10 | 636 / 656 MiB |

Read correctly (the second pair), DTW improves the median by 5–10 ms and the English
90th percentile at phrase starts, but not the tail at short pauses (English 300 → 390 ms)
or the French tail, which comes from VAD clipping. It costs 45–65 MiB of memory and runs
slower; see below. That is not a clear improvement, so DTW is off.

### Speed and memory

These runs had the machine to themselves: no compiler or ffmpeg ran, and the load average
was 5–6.5 (the Copilot app and the desktop). Runs were interleaved so that slow periods hit
every setting.

| Clip | Setting | Wall clock | Peak memory | WER (window) |
|---|---|---|---|---|
| English, 60 s | default | 67.7 s, 66.5 s | 541 MiB | – |
| English, 60 s | flash attention | 67.0 s, 60.3 s | 404 MiB | – |
| English, 60 s | DTW | 81.7 s, 73.8 s | 592 MiB | – |
| English, 140 s | default | 143.1 s | 592 MiB | 3.3% |
| English, 140 s | flash attention | 183.2 s | 457 MiB | 3.5% |
| French, 130 s | default | 117.1 s, 143.8 s | 590 MiB | 12.4% |
| French, 130 s | flash attention | 184.9 s, 163.8 s | 453 MiB | 11.4% |

Flash attention changed 2 words in 194 and none of the timing measures. On the 60-second
clip it was as fast as the default; on the 2-minute windows, with more text in the prompt,
it was 28–34% slower. The same setting varies by up to 25% from run to run on this laptop,
which is why the runs were repeated.

The whole 10-minute files, with the default settings and `--lang auto`, against the
benchmark's `small` runs of the same files:

| File | Run | Wall clock | Real-time factor | Peak memory | WER, window / whole clip | Onset error (median / p90 / bias) | Words in pauses |
|---|---|---|---|---|---|---|---|
| English | benchmark | 587 s | 0.98 | 694 MiB | 3.5% / 2.8% | 110 / 280 / +80 ms | 7.3% |
| English | `sabicut analyze` | 601 s | 1.00 | 626 MiB | 3.3% / 2.7% | 40 / 220 / +10 ms | 1.0% |
| French | benchmark | 579 s | 0.96 | 693 MiB | 10.6% / 12.4% | 170 / 500 / +115 ms | 3.9% |
| French | `sabicut analyze` | 557 s | 0.93 | 625 MiB | 10.9% / 13.0% | 50 / 590 / +50 ms | 0.1% |

The onset columns use the benchmark's definition, so they compare directly; measured from
where the voice starts, the French run's error is 30 / 100 / −10 ms. The first and second
halves of each file have the same error (English 30 and 40 ms, French 60 and 35 ms), so the
time map does not drift. Speed is the benchmark's within the run-to-run variance, and
`small` still runs at about real time on this CPU. Memory is 68 MiB lower, because only one
30-second chunk of audio is copied and turned into a spectrogram at a time. The WER is the
same within a few words.

### Keyword hint

`--keywords Dinah` on the English window (157 s, 592 MiB, WER 3.1% against 3.3%). The
reader says "DYE-nah", and Whisper writes "Dina" with or without the hint (the benchmark
run wrote "Dino"). The hint changed one mention, "Diana will miss me" to "Dina'll miss me",
but did not force the spelling. The hint stays, because it is cheap and helps a little;
names that must be right belong in fixes.toml. These two rules fix all five mentions:

```toml
schema = 1

[[fix]]
from = "Dina"         # "Dina," becomes "Dinah,": the punctuation stays
to = "Dinah"

[[fix]]
from = "Diana will"   # two words become one, timed from "Diana" to "will"
to = "Dinah'll"
```

## What surprised me

1. **The French "late onsets" were mostly breaths.** The French recording's noise floor is
   so low that a breath before a phrase crosses the benchmark's threshold. Measured from
   where the voice starts, Silero's segments start on time in both languages, with a median
   lead of 20–30 ms.
2. **More padding makes timing worse.** With 200 ms of padding, Whisper places words in the
   extra silence: the English median onset error triples (30 → 90 ms) and words in pauses
   rise tenfold (0.6% → 6.0%). Tight VAD segments are a better guide to where speech starts
   than Whisper's own timestamps, so the exact time map and the end clamp do most of the
   work.
3. **whisper.cpp's DTW times are off by one token,** as described above. Anyone reading
   `t_dtw` as a token's start gets words about one token late.
4. **Silero sometimes misses quiet French phrase starts by 300–800 ms.** Padding only
   partly helps (200 ms still leaves them 340–500 ms late). A detector that extends a segment
   start backwards while the voice is audible might fix them, but it needs testing on noisy
   phone audio first. Until then, `--vad-pad 200` is the workaround for recordings with
   soft phrase starts.

## Reproduce

The scripts are in [`spikes/whisper-bench/`](../../spikes/whisper-bench/) and use the
benchmark's audio (`audio/en.wav` and `audio/fr.wav`, or set `BENCH_AUDIO` to a folder that
has them) and the release build of `sabicut`:

```sh
cargo build --release
cd spikes/whisper-bench
python3 analyze_runs.py small-en en --lang en                  # 140 s of English
python3 analyze_runs.py small-fr-pad200 fr --lang fr --vad-pad 200
python3 analyze_runs.py small-en-dtw en --lang en --dtw
python3 analyze_check.py vad audio/fr.wav out/analyze/vad-fr.json  # after --vad-only
```

Each run appends its timings, peak memory and checks to `out/analyze/runs.jsonl`.
`sabicut analyze` has hidden options for these experiments: `--vad-only` writes the speech
segments without transcribing, `--dtw` uses DTW word starts, and `--flash-attn` turns on
flash attention.
