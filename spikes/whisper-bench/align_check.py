#!/usr/bin/env python3
"""Spot-checks whisper-bench word timestamps against the audio itself.

1. Onset check: finds pauses of at least 300 ms in the WAV (10 ms RMS frames). The silence
   threshold adapts to the recording: its noise floor (10th-percentile frame) plus 6 dB.
   The first word after each pause should start where speech resumes; reports the median and
   90th-percentile |word start - speech onset| and the median signed offset (bias).
2. Drift check: the onset error in the first and second half of the file. A time-mapping bug
   (for example, after VAD removes silence) would make the error grow along the file.
3. Silence check: share of words whose midpoint falls inside one of those pauses.
4. Optional cross-check (--against): aligns the words with another run of the same audio
   (for example, without VAD) and reports how far matched word start times differ.
"""

import argparse
import array
import csv
import json
import math
import statistics
import wave
from pathlib import Path

from wer import align, normalize

FRAME_S = 0.01
MIN_PAUSE_S = 0.3
NOISE_MARGIN_DB = 6.0
ONSET_WINDOW_S = 1.0


def frame_db(wav_path: Path) -> list[float]:
    """10 ms frame energies in dB relative to loud speech (95th-percentile frame)."""
    with wave.open(str(wav_path), "rb") as wav:
        if wav.getsampwidth() != 2 or wav.getnchannels() != 1:
            raise SystemExit(f"{wav_path}: expected 16-bit mono WAV")
        rate = wav.getframerate()
        samples = array.array("h", wav.readframes(wav.getnframes()))
    hop = int(rate * FRAME_S)
    energies = []
    for i in range(0, len(samples) - hop + 1, hop):
        chunk = samples[i:i + hop]
        energies.append(sum(s * s for s in chunk) / hop)
    loud = sorted(energies)[int(len(energies) * 0.95)] or 1.0
    return [10 * math.log10(e / loud) if e > 0 else -120.0 for e in energies]


def silence_threshold_db(db: list[float]) -> float:
    return sorted(db)[len(db) // 10] + NOISE_MARGIN_DB


def pauses(db: list[float], threshold_db: float) -> list[tuple[float, float]]:
    """Returns (start_s, end_s) for silent runs of at least MIN_PAUSE_S."""
    found, run_start = [], None
    for i, value in enumerate(db + [0.0]):
        silent = value < threshold_db
        if silent and run_start is None:
            run_start = i
        elif not silent and run_start is not None:
            if (i - run_start) * FRAME_S >= MIN_PAUSE_S:
                found.append((run_start * FRAME_S, i * FRAME_S))
            run_start = None
    return found


def read_words(tsv_path: Path) -> list[dict]:
    with open(tsv_path, encoding="utf-8", newline="") as fh:
        rows = list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))
    return [
        {"t0": int(r["seg_t0_ms"]) / 1000, "t1": int(r["seg_t1_ms"]) / 1000, "word": r["word"]}
        for r in rows
    ]


def percentile(values: list[float], q: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(q * len(ordered)))]


def median_ms(values: list[float]) -> int | None:
    return round(statistics.median(values)) if values else None


def onset_errors(words: list[dict], gaps: list[tuple[float, float]]) -> list[tuple[float, float]]:
    """(onset_s, word_start - onset_s) for each pause followed by a word within ONSET_WINDOW_S."""
    starts = [w["t0"] for w in words]
    errors, k = [], 0
    for _, onset in gaps:
        while k < len(starts) and starts[k] < onset - ONSET_WINDOW_S:
            k += 1
        nearby = [s for s in starts[k:k + 4] if abs(s - onset) <= ONSET_WINDOW_S]
        if nearby:
            best = min(nearby, key=lambda s: abs(s - onset))
            errors.append((onset, best - onset))
    return errors


def words_in_pauses(words: list[dict], gaps: list[tuple[float, float]]) -> float:
    inside = 0
    for w in words:
        mid = (w["t0"] + w["t1"]) / 2
        if any(a + 0.05 < mid < b - 0.05 for a, b in gaps):
            inside += 1
    return inside / len(words) if words else 0.0


def tokens_with_times(words: list[dict]) -> tuple[list[str], list[float]]:
    tokens, times = [], []
    for w in words:
        for token in normalize(w["word"]):
            tokens.append(token)
            times.append(w["t0"])
    return tokens, times


def cross_check(words: list[dict], other: list[dict]) -> dict:
    a_tokens, a_times = tokens_with_times(words)
    b_tokens, b_times = tokens_with_times(other)
    deltas = [
        abs(a_times[i] - b_times[j]) * 1000
        for op, i, j in align(a_tokens, b_tokens)
        if op == "ok"
    ]
    return {
        "matched": len(deltas),
        "median_abs_ms": median_ms(deltas),
        "p90_abs_ms": round(percentile(deltas, 0.9)) if deltas else None,
        "over_500ms_pct": round(100 * sum(d > 500 for d in deltas) / len(deltas), 2)
        if deltas
        else None,
    }


def check(
    wav: Path,
    tsv: Path,
    against: Path | None = None,
    db: list[float] | None = None,
) -> dict:
    words = read_words(tsv)
    db = db if db is not None else frame_db(wav)
    threshold = silence_threshold_db(db)
    gaps = pauses(db, threshold)
    errors = onset_errors(words, gaps)
    abs_ms = [abs(e) * 1000 for _, e in errors]
    half = len(abs_ms) // 2
    result = {
        "words": str(tsv),
        "n_words": len(words),
        "silence_threshold_db": round(threshold, 1),
        "pauses": len(gaps),
        "onsets_matched": len(errors),
        "onset_median_abs_ms": median_ms(abs_ms),
        "onset_p90_abs_ms": round(percentile(abs_ms, 0.9)) if abs_ms else None,
        "onset_bias_ms": median_ms([e * 1000 for _, e in errors]),
        "first_half_median_abs_ms": median_ms(abs_ms[:half]),
        "second_half_median_abs_ms": median_ms(abs_ms[half:]),
        "words_in_pauses_pct": round(100 * words_in_pauses(words, gaps), 2),
    }
    if against:
        result["vs_" + against.stem.replace(".words", "")] = cross_check(words, read_words(against))
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description="Spot-check word timestamps against the audio")
    parser.add_argument("wav", type=Path)
    parser.add_argument("words", type=Path, nargs="+", help="*.words.tsv from whisper-bench")
    parser.add_argument("--against", type=Path, help="another run's *.words.tsv to compare with")
    args = parser.parse_args()
    for tsv in args.words:
        print(json.dumps(check(args.wav, tsv, args.against)))


if __name__ == "__main__":
    main()
