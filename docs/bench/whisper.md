# Whisper day-one benchmark

This benchmark decides which Whisper model Sabicut's `analyze` crate uses. It runs whisper.cpp
through `whisper-rs` on the CPU with 4 threads, using the quantized q5_1 tiny, base and small
models on 10 minutes of English and 10 minutes of French speech. The harness lives in
[`spikes/whisper-bench/`](../../spikes/whisper-bench/). It ran on 2 November 2026.

## Recommendation

- **The `analyze` crate should use `small` (q5_1).** All three models pass the PRD rule, and
  small is the largest of them. It is also the only one with usable French: 10.6% word error
  rate (WER) on the hand-checked window, against 27% for base and 42% for tiny. Its margin is
  thin, though: 587 s and 579 s for 600 s of audio, so 2–4% under the limit.
- **Hub mode: yes for English, no for French.** Transcribe English with `base`: it is 2.5 times
  faster, uses about 240 MiB less memory, and its WER is 6.0% against small's 3.5%. Keep French
  on `small`, because base and tiny French output can't be used. When French is slow, queue the
  jobs instead of downgrading the model. Run one transcription at a time and check free memory
  before each job.
- **Before fixing the hub default, re-run this benchmark on the real hub laptop.** Small has
  almost no time headroom on this CPU, and a slower CPU or a portable build will push it past
  real time.

## Setup

| | |
|---|---|
| Machine | Intel Core i7-6600U (2 cores, 4 threads, 2.6–3.4 GHz, AVX2), 7.6 GiB RAM, Arch Linux |
| Toolchain | Rust 1.98.1, CMake 4.4.4, FFmpeg 9.0.2 |
| Library | `whisper-rs` 0.16.0, which bundles whisper.cpp 1.8.3 (`whisper-rs-sys` 0.15.0). CPU only, release build, `GGML_NATIVE=ON`, no OpenMP or BLAS |
| Models | `ggml-tiny-q5_1.bin`, `ggml-base-q5_1.bin` and `ggml-small-q5_1.bin` (32, 60 and 190 MB), checked against their SHA-256 hashes |
| Decoding | Greedy decoding, 4 threads, language set to `en` or `fr`, token timestamps on, one word per segment (`max_len = 1`, `split_on_word`) |
| VAD | Silero v6.2.0 (`ggml-silero-v6.2.0.bin`, 0.9 MB) with whisper.cpp's default settings (threshold 0.5, 30 ms padding), 1 thread |
| English audio | LibriVox, *Alice's Adventures in Wonderland*, chapter 1, read by Kristen McQuillin. Reference text: Project Gutenberg #11 |
| French audio | LibriVox, *Lettres de mon moulin*, "Le secret de maître Cornille", read by Naf. Reference text: Project Gutenberg #36780 |
| Clips | First 600 s of each recording, converted with FFmpeg to 16 kHz mono 16-bit WAV |
| Memory guard | Each run gets its own systemd scope with `MemoryMax=1536M` and no swap |

The measurements are defined as follows:

- **Wall clock:** the whole process, from start to exit. It includes loading the model (under
  1 s), VAD (about 6 s) and writing the results.
- **Peak RSS:** `ru_maxrss` from `wait4`, which is the figure GNU `time -v` reports. GNU `time`
  isn't installed on this machine, so the harness also reads `VmHWM` itself. The two figures
  agree to within 1 MiB.
- **WER:** computed after normalizing case and punctuation. Numbers are not normalized. Two
  windows are scored, both hand-checked: about 2 minutes of English (452 words, 196–322 s) and
  about 2 minutes of French (395 words, 231–347 s). The whole clip is scored too, story text
  only, without the LibriVox intro.

The machine was not idle during the runs. The Copilot desktop app used about 0.7 of a CPU core
and about 3 GB of RAM the whole time, and the 1-minute load average was mostly 5–6.5 on 4
logical CPUs. On an idle machine the times would be lower. No compiler ran during any of the
six main runs.

## Results

Pass rule: 600 s of audio transcribed in 600 s or less, with peak RSS below 1.5 GB. The table
treats 1.5 GB as 1536 MiB; every model is far below it under either reading.

| Model | Language | Wall clock | Real-time factor | Peak RSS | WER, 2-min window | WER, whole clip | Verdict |
|---|---|---|---|---|---|---|---|
| small | English | 587 s | 0.98 | 694 MiB | 3.5% | 2.8% | pass, 2% margin |
| small | French | 579 s | 0.96 | 693 MiB | 10.6% | 12.4% | pass, 4% margin |
| base | English | 230 s | 0.38 | 458 MiB | 6.0% | 4.1% | pass |
| base | French | 215 s | 0.36 | 452 MiB | 27.3% | 40.4% | pass |
| tiny | English | 119 s | 0.20 | 385 MiB | 10.4% | 6.9% | pass |
| tiny | French | 120 s | 0.20 | 381 MiB | 42.0% | 39.9% | pass |

VAD kept 539 s of the English clip and 520 s of the French clip.

Two extra tiny runs without VAD serve as the reference for the timestamp check. Without VAD,
tiny scored 10.2% and 6.0% WER in English and 41.3% and 37.9% in French (window, then whole
clip), with a peak RSS of 354 MiB. Their times can't be compared with the main runs: one
overlapped the scoring scripts and the other overlapped a compiler run in another session.

The hand-check of the error lists shows the following patterns:

- **Small, French:** the errors are rare words and names. For example, "des vaïles" became
  "des bails" and "la taillole en lambeaux" became "la taille au l'ambo". The transcript
  remains readable.
- **Base, French:** the grammar breaks down ("que noms me lui renondent de mettre cornie"), and
  the model invents text. It inserted 74 words in 12 runs, for example "et je ne devais pas
  avoir un peu de temps à faire un".
- **Tiny, French:** sound-alike nonsense ("comme un bremboémi").
- **English, all models:** the transcripts are usable. Names are the weak spot: "Dinah" came
  out as "Dino" in every model.

The small/base decision for hub mode, with numbers:

| | small | base | tiny |
|---|---|---|---|
| A class of 30 learners × 3 minutes = 90 minutes of audio, on this CPU | about 88 min | about 33 min | about 18 min |
| Peak RSS against a 2.5 GB hub laptop that also runs the OS, the Wi-Fi hub and FFmpeg | 694 MiB | 455 MiB | 383 MiB |
| WER, English and French windows | 3.5% and 10.6% | 6.0% and 27.3% | 10.4% and 42.0% |

## Word timestamps

Every word gets the start and end times of its whisper.cpp token timestamps, mapped back to the
original timeline after VAD. These times were checked in two ways:

- **Against the audio:** at every point where speech resumes after a pause of at least 300 ms
  (157 in English, 105 in French), the first word's start time was compared with the energy
  onset.
- **Against the no-VAD run:** word start times were compared with those of the run without VAD.

| Run | Median error at speech onsets | 90th percentile | Median offset | First half / second half | Words with midpoint in a pause | Against no-VAD run |
|---|---|---|---|---|---|---|
| small, English | 110 ms | 280 ms | +80 ms | 110 / 100 ms | 7.3% | – |
| base, English | 30 ms | 230 ms | +10 ms | 20 / 40 ms | 6.8% | – |
| tiny, English | 100 ms | 270 ms | +20 ms | 105 / 100 ms | 7.5% | median 150 ms, 90th percentile 510 ms |
| small, French | 170 ms | 500 ms | +115 ms | 170 / 165 ms | 3.9% | – |
| base, French | 205 ms | 590 ms | +50 ms | 195 / 210 ms | 3.7% | – |
| tiny, French | 195 ms | 510 ms | +105 ms | 200 / 180 ms | 3.7% | median 160 ms, 90th percentile 500 ms |
| tiny, English, no VAD | 90 ms | 280 ms | −30 ms | 115 / 80 ms | 9.6% | – |
| tiny, French, no VAD | 90 ms | 290 ms | −40 ms | 90 / 100 ms | 6.9% | – |

Here are some sample checks from small, English. Speech resumes at 173.54 s and "you" starts at
173.52 s. At 300.65 s, "the" starts at 300.56 s. At 457.67 s, "never" starts at 457.71 s.

- **Start times line up with the audio.** In English the median error is 30–110 ms, and 90% of
  words fall within 230–280 ms. The first and second halves of each file match, so the VAD time
  map does not drift.
- **With VAD, French words start about 0.1 s late,** and small reaches up to 0.4 s late in the
  samples. Without VAD, tiny is 40 ms early. The cause is that Silero clips French phrase
  onsets:
  - 40 of 101 French VAD segments start more than 50 ms after the energy onset, with a 90th
    percentile of +530 ms.
  - In English this happens for only 10 of 151 segments.

  The clipping also costs some accuracy: tiny's whole-clip WER rises from 37.9% to 39.9% in
  French and from 6.0% to 6.9% in English. The `analyze` crate should test more padding, such
  as `speech_pad_ms = 200`, or a lower threshold for French.
- **End times stretch into pauses.** 4–8% of words have their midpoint inside a pause. Most of
  them start in speech, and their end time runs on into the following silence (96 of 151 words
  for small, English). Caption timing should end each word at the end of the speech, using the
  VAD segment end, instead of trusting Whisper's end time.
- **This is good enough for phrase-level captions.** Word-by-word highlighting, which PRD feature F6
  needs, will need sharper timing. The harness has a `--dtw` flag for whisper.cpp's dynamic time
  warping (DTW) word timestamps, but it is untested.

## What surprised me

1. **VAD settings in `whisper-rs` 0.16 do nothing.** `FullParams::enable_vad` is silently
   ignored. In whisper.cpp 1.8.3, VAD runs only inside `whisper_full()` and
   `whisper_full_parallel()`, but `WhisperState::full` calls `whisper_full_with_state()`, which
   skips it. There is no error and no log line. The first "VAD" run transcribed all 600 s, and
   its last word ended at 601.4 s. The harness now runs whisper.cpp's Silero model itself
   through `WhisperVadContext`, then joins the speech segments and maps every timestamp back
   ([`src/vad.rs`](../../spikes/whisper-bench/src/vad.rs)). The `analyze` crate needs the same
   approach, or a patched `whisper-rs`.
2. **VAD needs 1 thread.** With 4 threads, Silero took 90 s for 10 minutes of audio, because its
   threads competed with Whisper's. With 1 thread it took 4–6 s.
3. **Memory, not CPU, is what broke the laptop.** The first attempt froze the desktop and ended
   in a power-key reboot at 18:33. The small run (0.7 GB) overlapped a cargo build from another
   session, the Copilot app was using about 3 GB, and swap was already full. On a 2.5 GB hub
   laptop, the engine must run one transcription at a time and check free memory first. The
   harness now caps each run at 1.5 GiB and waits for compilers to finish before it starts.
4. **French is where model size matters.** From small down to tiny, English WER roughly triples
   (3.5% to 10.4%), while French WER quadruples (10.6% to 42%). In French, base also invents
   text.
5. **Most of the memory is not the model.** Tiny's weights are 31 MB, yet the process peaks at
   385 MiB. The rest is compute buffers, 10 minutes of audio held twice as 32-bit floats, and
   the mel spectrogram. Feeding audio in chunks would reduce this.
6. **Audiobooks have little silence.** VAD removed only 10–13% of the audio here. Phone clips
   with pauses should gain more.
7. **These are native-build numbers.** `GGML_NATIVE=ON` tuned whisper.cpp for this CPU's AVX2
   instructions. A build shipped to unknown hub laptops must target a baseline CPU or use ggml's
   runtime CPU detection, which may be slower. Re-measure with that build.

Not tested here, but worth trying in the `analyze` crate: whisper.cpp's flash-attention option
and the `openmp` feature for speed, an initial prompt listing names (to fix "Dinah"), and the
English-only `base.en` model for English hub jobs.

## Data downloaded

The benchmark downloaded at most about 299.4 MB in total, with no speech datasets:

- **`fetch.sh`, 293.6 MB:**
  - Models: 281.9 MB (tiny 32.2 MB, base 59.7 MB, small 190.1 MB).
  - Silero VAD: 0.9 MB.
  - Two LibriVox MP3s: 10.3 MB.
  - Two Project Gutenberg texts: 0.5 MB.
- **Crates: at most 5.8 MB.** Some of them were probably already cached.

One model download was interrupted; it resumed instead of starting over.

## Reproduce

```sh
cd spikes/whisper-bench
./fetch.sh                  # about 294 MB of models, VAD and audio; skips files already present
cargo build --release       # builds whisper.cpp with CMake, about 4 minutes
python3 make_refs.py        # writes refs/en.txt and refs/fr.txt from the Gutenberg texts
python3 bench.py            # small, base and tiny × English and French with VAD, about 35 minutes
python3 bench.py --models tiny --vad off --tag novad   # optional no-VAD baseline
python3 summarize.py        # prints the tables in this document
```

`bench.py` can be resumed: if you run it again, it skips runs that have already finished. Run
outputs go to `out/`, which is ignored by git along with `models/`, `audio/` and `target/`.
Each `out/<run>.words.tsv` lists every word with its start and end times.
