#!/usr/bin/env python3
"""Runs `sabicut plan` on the benchmark readings with different settings and checks each
plan against the audio with plan_check.py. Used for docs/bench/plan.md.

The word timings are the 10-minute LibriVox readings that `sabicut analyze` transcribed
with the `small` model (full-small-en.json, full-small-fr.json) and the sample fixes
(fixes-en.toml, fixes-fr.toml), in $SABICUT_BENCH/words; the audio is en.wav and fr.wav in
$SABICUT_BENCH/audio. SABICUT_BENCH defaults to ~/.cache/sabicut/bench. Set SABICUT_BIN to
use another build of sabicut than target/release/sabicut.

usage:
  plan_sweep.py            prints one Markdown table row per setting and language
"""

import json
import os
import subprocess
import tomllib
from pathlib import Path

from plan_check import Audio, check

HERE = Path(__file__).resolve().parent
SABICUT = Path(os.environ.get("SABICUT_BIN", HERE.parents[1] / "target" / "release" / "sabicut"))
BENCH = Path(os.environ.get("SABICUT_BENCH", Path.home() / ".cache" / "sabicut" / "bench"))

DEFAULTS = {"max-pause": 0.4, "pad-before": 0.15, "pad-after": 0.1, "min-cut": 0.2, "vad-reach": 1.0}

SWEEPS = [
    ("max-pause", [0.3, 0.4, 0.5, 0.75, 1.0]),
    ("pads", [(0.08, 0.08), (0.12, 0.08), (0.15, 0.1), (0.2, 0.1), (0.2, 0.2)]),
    ("min-cut", [0.1, 0.2, 0.3]),
    ("vad-reach", [0.0, 0.3, 1.0]),
]


def plan(language: str, settings: dict) -> dict:
    words = BENCH / "words"
    args = [str(SABICUT), "plan", str(BENCH / "audio" / f"{language}.wav"),
            "--words", str(words / f"full-small-{language}.json"),
            "--fixes", str(words / f"fixes-{language}.toml"), "-o", "-"]
    for key, value in settings.items():
        args += [f"--{key}", str(value)]
    result = subprocess.run(args, capture_output=True, text=True, check=True)
    return tomllib.loads(result.stdout)


def row(language: str, label: str, result: dict) -> str:
    points = result["inner_points"]
    onset, tail = result["onset_margin_s"], result["tail_margin_s"]
    return (f"| {language} | {label} | {result['cuts']} | {result['removed_s']:.1f} s | "
            f"{points['silent']} / {points['noise']} / {points['voiced']} | "
            f"{result['voiced_removed_s']:.2f} s | {onset['p10']:.2f} / {onset['median']:.2f} | "
            f"{onset['tight']} | {tail['p10']:.2f} / {tail['median']:.2f} | {tail['tight']} | "
            f"{result['mid_word']} |")


def main() -> None:
    print("| Language | Setting | Cuts | Removed | Cut points: silent / noise / voiced | "
          "Voice cut | Onset margin p10 / median | Tight onsets | "
          "Tail margin p10 / median | Tight tails | Mid-word |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for language in ("en", "fr"):
        audio = Audio(str(BENCH / "audio" / f"{language}.wav"))
        with open(BENCH / "words" / f"full-small-{language}.json", encoding="utf-8") as fh:
            words = json.load(fh)
        for name, values in SWEEPS:
            for value in values:
                settings = dict(DEFAULTS)
                if name == "pads":
                    settings["pad-before"], settings["pad-after"] = value
                    label = f"pads {value[0]} / {value[1]}"
                else:
                    settings[name] = value
                    label = f"{name} {value}"
                print(row(language, label, check(plan(language, settings), words, audio)), flush=True)


if __name__ == "__main__":
    main()
