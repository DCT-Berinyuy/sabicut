#!/usr/bin/env python3
"""Checks the word timings that `sabicut analyze` writes (words.json) against the audio.

Used for docs/bench/analyze.md. It builds on align_check.py and wer.py, so its numbers
compare with docs/bench/whisper.md:

- Onsets (as in the benchmark): the end of each pause of at least 300 ms, where the energy
  rises 6 dB above the noise floor. The first word after each pause should start there.
- Voice onsets: after each such pause, the first 10 ms frame within 35 dB of loud speech.
  In a very quiet recording the benchmark onset fires on breaths before a phrase; the voice
  onset does not.
- Short pauses: the same, after pauses of at least 120 ms, which includes many boundaries
  inside phrases.
- Words in pauses, and how far word ends run into the pause that follows them.
- VAD coverage: how much of each onset and offset the speech segments cut off.
- WER on the hand-checked window (wer.py), when --ref is given.

usage:
  analyze_check.py words <wav> <words.json> [--ref refs/en.txt]
  analyze_check.py vad <wav> <words.json>
  analyze_check.py spike <wav> <run.words.tsv> --start S --end E [--ref refs/en.txt]
      (a whisper-bench run, cut to S..E seconds and shifted to the clip's time)
"""

import argparse
import json
import statistics
import tempfile
from pathlib import Path

import align_check
import wer
from align_check import frame_db, median_ms, onset_errors, pauses, percentile, silence_threshold_db
from align_check import words_in_pauses

VOICE_DB = -35.0
SHORT_PAUSE_S = 0.12
WINDOW_S = 1.0


def energy_gaps(db: list[float], min_pause_s: float) -> list[tuple[float, float]]:
    saved, align_check.MIN_PAUSE_S = align_check.MIN_PAUSE_S, min_pause_s
    try:
        return pauses(db, silence_threshold_db(db))
    finally:
        align_check.MIN_PAUSE_S = saved


def voice_onsets(db: list[float], gaps: list[tuple[float, float]]) -> list[float]:
    onsets = []
    for _, end in gaps:
        i = int(round(end * 100))
        limit = min(len(db), i + int(WINDOW_S * 100))
        while i < limit and db[i] < VOICE_DB:
            i += 1
        if i < limit:
            onsets.append(i / 100)
    return onsets


def stats_ms(values: list[float]) -> dict:
    if not values:
        return {"n": 0}
    ms = [v * 1000 for v in values]
    return {"n": len(ms), "median": round(statistics.median(ms)), "p90": round(percentile(ms, 0.9)),
            "max": round(max(ms))}


def onset_stats(words: list[dict], onsets: list[float]) -> dict:
    errors = onset_errors(words, [(None, onset) for onset in onsets])
    abs_ms = [abs(e) * 1000 for _, e in errors]
    return {
        "n": len(abs_ms),
        "median": median_ms(abs_ms),
        "p90": round(percentile(abs_ms, 0.9)) if abs_ms else None,
        "bias": median_ms([e * 1000 for _, e in errors]),
    }


def overrun(words: list[dict], gaps: list[tuple[float, float]]) -> dict:
    runs = []
    for w in words:
        for start, end in gaps:
            if w["t0"] < start < w["t1"]:
                runs.append(min(w["t1"], end) - start)
                break
    return stats_ms(runs)


def coverage(speech: list[dict], points: list[float], forward: bool) -> dict:
    """How much the speech segments cut off at onsets (forward) or offsets."""
    clipped, missed = [], 0
    for point in points:
        if any(s["start"] <= point <= s["end"] for s in speech):
            clipped.append(0.0)
            continue
        if forward:
            cut = [s["start"] - point for s in speech if 0 < s["start"] - point <= WINDOW_S]
        else:
            cut = [point - s["end"] for s in speech if 0 < point - s["end"] <= WINDOW_S]
        if cut:
            clipped.append(min(cut))
        else:
            missed += 1
    return {"clipped_over_50ms": sum(c > 0.05 for c in clipped), "of": len(points),
            "missed": missed, "clip": stats_ms(clipped)}


def window_wer(ref: Path, text: str) -> dict:
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as fh:
        fh.write(text)
        hyp = Path(fh.name)
    try:
        result = wer.evaluate(ref, hyp)
    finally:
        hyp.unlink()
    return {"window_pct": round(100 * result["window"]["wer"], 1),
            "clip_pct": round(100 * result["clip"]["wer"], 1)}


def from_words_json(path: Path) -> tuple[dict, list[dict]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    return data, [{"t0": w["start"], "t1": w["end"], "word": w["text"]} for w in data["words"]]


def from_tsv(path: Path, start: float, end: float) -> list[dict]:
    words = []
    for line in path.read_text(encoding="utf-8").splitlines()[1:]:
        cols = line.split("\t")
        t0, t1 = int(cols[0]) / 1000, int(cols[1]) / 1000
        if start <= t0 < end:
            words.append({"t0": t0 - start, "t1": min(t1, end) - start, "word": cols[6]})
    return words


def check(mode: str, wav: Path, source: Path, ref: Path | None = None,
          start: float = 0.0, end: float = 1e9) -> dict:
    db = frame_db(wav)
    gaps = energy_gaps(db, 0.3)
    short = energy_gaps(db, SHORT_PAUSE_S)
    voiced = voice_onsets(db, gaps)
    result: dict = {"source": source.name}
    if mode == "spike":
        words = from_tsv(source, start, end)
    else:
        data, words = from_words_json(source)
        speech = data["speech"]
        result["language"] = data["language"]
        result["segments"] = len(speech)
        result["speech_s"] = round(sum(s["end"] - s["start"] for s in speech), 1)
        result["vad_onsets"] = coverage(speech, [e for _, e in gaps], forward=True)
        result["vad_voice_onsets"] = coverage(speech, voiced, forward=True)
        result["vad_offsets"] = coverage(speech, [s for s, _ in gaps], forward=False)
    if mode != "vad":
        result["words"] = len(words)
        result["onset"] = onset_stats(words, [e for _, e in gaps])
        result["voice_onset"] = onset_stats(words, voiced)
        result["short_voice_onset"] = onset_stats(words, voice_onsets(db, short))
        result["words_in_pauses_pct"] = round(100 * words_in_pauses(words, gaps), 2)
        result["into_pause"] = overrun(words, gaps)
        if ref:
            result["wer"] = window_wer(ref, " ".join(w["word"] for w in words))
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description="Check sabicut analyze word timings")
    parser.add_argument("mode", choices=["words", "vad", "spike"])
    parser.add_argument("wav", type=Path)
    parser.add_argument("source", type=Path)
    parser.add_argument("--ref", type=Path)
    parser.add_argument("--start", type=float, default=0.0)
    parser.add_argument("--end", type=float, default=1e9)
    args = parser.parse_args()
    print(json.dumps(check(args.mode, args.wav, args.source, args.ref, args.start, args.end)))


if __name__ == "__main__":
    main()
