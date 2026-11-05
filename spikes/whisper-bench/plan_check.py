#!/usr/bin/env python3
"""Checks a plan written by `sabicut plan` against its word timings and the audio.

Used for docs/bench/plan.md. It uses align_check.py's 10 ms frame energies, relative to
loud speech (the 95th-percentile frame):

- Silence: below the noise floor (10th-percentile frame) plus 6 dB, as in the benchmark.
- Voice: at least max(-35 dB, noise floor + 12 dB). The English recording's noise floor
  (-31 dB) is above analyze_check.py's -35 dB voice level, hence the floor-relative part.
- Cut points: each cut's `in` and `out`, except the start and the end of the output. A
  cut point is "silent" when every frame within 20 ms of it is silence, "voiced" when one
  is voice, and "noise" otherwise (breath, room tone, clicks).
- Mid-word: a cut point inside a word's [start, end], ends included; the start and the
  end of the media do not count.
- Voiced audio removed: voice frames outside every cut.
- Onset and tail margins: from a cut's `in` to its first voice frame, and from its last
  voice frame to its `out`. Cuts that start at 0 or end with the media are left out, as
  they cannot be padded. A margin under 20 ms is "tight": a clipped phrase start, or a
  breath or click above the voice level right at the cut.

usage:
  plan_check.py <plan.toml> <words.json> <audio.wav>
"""

import json
import sys
import tomllib

from align_check import frame_db, percentile

NOISE_MARGIN_DB = 6.0
VOICE_DB = -35.0
VOICE_MARGIN_DB = 12.0
POINT_WINDOW_FRAMES = 2
TIGHT_S = 0.02


class Audio:
    """Frame energies of a recording, with its silence and voice levels."""

    def __init__(self, wav_path: str):
        self.db = frame_db(wav_path)
        floor = sorted(self.db)[len(self.db) // 10]
        self.silent_below = floor + NOISE_MARGIN_DB
        self.voiced_from = max(VOICE_DB, floor + VOICE_MARGIN_DB)


def check(plan: dict, words: dict, audio: Audio) -> dict:
    db = audio.db
    fps = plan["meta"]["fps"]
    end_of_media = int(words["duration"] * fps + 1e-9) / fps
    cuts = [(cut["in"], cut["out"]) for cut in plan["cut"]]

    mid_word = 0
    inner_points = []
    for i, (start, end) in enumerate(cuts):
        for t, edge in ((start, start <= 0.0), (end, end >= end_of_media - 1e-6)):
            if not edge and any(w["start"] - 1e-9 <= t <= w["end"] + 1e-9 for w in words["words"]):
                mid_word += 1
        if i > 0:
            inner_points.append(start)
        if i < len(cuts) - 1:
            inner_points.append(end)

    points = {"silent": 0, "noise": 0, "voiced": 0}
    for t in inner_points:
        frame = int(round(t * 100))
        loudest = max(db[max(0, frame - POINT_WINDOW_FRAMES):frame + POINT_WINDOW_FRAMES + 1])
        if loudest < audio.silent_below:
            points["silent"] += 1
        elif loudest >= audio.voiced_from:
            points["voiced"] += 1
        else:
            points["noise"] += 1

    kept = [False] * len(db)
    for start, end in cuts:
        for frame in range(int(start * 100), min(len(db), int(end * 100))):
            kept[frame] = True
    voiced_removed = sum(1 for f, v in enumerate(db) if not kept[f] and v >= audio.voiced_from) / 100

    onsets, tails = [], []
    for start, end in cuts:
        lo, hi = int(start * 100), min(len(db), int(end * 100))
        first = next((f for f in range(lo, hi) if db[f] >= audio.voiced_from), None)
        last = next((f for f in range(hi - 1, lo - 1, -1) if db[f] >= audio.voiced_from), None)
        if first is not None and start > 0.0:
            onsets.append((first - lo) / 100)
        if last is not None and end < end_of_media - 1e-6:
            tails.append((hi - 1 - last) / 100)

    output = sum(end - start for start, end in cuts)
    return {
        "cuts": len(cuts),
        "input_s": words["duration"],
        "output_s": round(output, 3),
        "removed_s": round(words["duration"] - output, 3),
        "cut_points": 2 * len(cuts),
        "mid_word": mid_word,
        "inner_points": points,
        "voiced_removed_s": round(voiced_removed, 2),
        "onset_margin_s": margins(onsets),
        "tail_margin_s": margins(tails),
    }


def margins(values: list[float]) -> dict:
    if not values:
        return {"n": 0}
    return {"n": len(values), "p10": percentile(values, 0.1), "median": percentile(values, 0.5),
            "tight": sum(1 for x in values if x < TIGHT_S)}


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    plan_path, words_path, wav_path = sys.argv[1:4]
    with open(plan_path, "rb") as fh:
        plan = tomllib.load(fh)
    with open(words_path, encoding="utf-8") as fh:
        words = json.load(fh)
    print(json.dumps(check(plan, words, Audio(wav_path))))


if __name__ == "__main__":
    main()
