#!/usr/bin/env python3
"""Rough word error rate (WER) for whisper-bench transcripts.

The reference file holds the text spoken in the whole clip. Text before a `<<<STORY` line
(the LibriVox intro) is used for alignment but not scored. Lines `<<<WINDOW` and `WINDOW>>>`
mark the hand-checked 2-minute window. The hypothesis is aligned against the whole reference,
so the window score does not depend on timestamps.

Normalization: NFKC, lowercase, apostrophes, hyphens and punctuation become spaces, and
accents are kept. Numbers are not normalized ("4,000" against "four thousand" counts as errors),
so the windows were chosen to contain no numbers.
"""

import argparse
import json
import re
import sys
import unicodedata
from pathlib import Path

STORY = "<<<STORY"
WINDOW_OPEN = "<<<WINDOW"
WINDOW_CLOSE = "WINDOW>>>"


def normalize(text: str) -> list[str]:
    text = unicodedata.normalize("NFKC", text).lower()
    text = text.replace("œ", "oe").replace("æ", "ae")
    text = re.sub(r"['’‘`´ʼ_]", " ", text)
    text = re.sub(r"[^\w\s]", " ", text)
    return text.split()


def read_reference(path: Path) -> tuple[list[str], int, int, int]:
    """Returns reference words, the scored start, and the [start, end) range of the window."""
    raw = path.read_text(encoding="utf-8")
    if WINDOW_OPEN not in raw or WINDOW_CLOSE not in raw:
        raise SystemExit(f"{path}: missing {WINDOW_OPEN} / {WINDOW_CLOSE} markers")
    intro, body = raw.split(STORY, 1) if STORY in raw else ("", raw)
    before, rest = body.split(WINDOW_OPEN, 1)
    window, after = rest.split(WINDOW_CLOSE, 1)
    intro_w, pre, win, post = (normalize(t) for t in (intro, before, window, after))
    story_start = len(intro_w)
    w_start = story_start + len(pre)
    return intro_w + pre + win + post, story_start, w_start, w_start + len(win)


def align(ref: list[str], hyp: list[str]) -> list[tuple[str, int | None, int | None]]:
    """Levenshtein alignment as (op, ref_index, hyp_index); op is ok, sub, del or ins."""
    n, m = len(ref), len(hyp)
    back = [bytearray(m + 1) for _ in range(n + 1)]
    back[0][1:] = b"\x02" * m
    prev = list(range(m + 1))
    for i in range(1, n + 1):
        cur = [i] + [0] * m
        back[i][0] = 1
        word = ref[i - 1]
        row = back[i]
        for j in range(1, m + 1):
            best, move = prev[j - 1] + (word != hyp[j - 1]), 0
            if prev[j] + 1 < best:
                best, move = prev[j] + 1, 1
            if cur[j - 1] + 1 < best:
                best, move = cur[j - 1] + 1, 2
            cur[j] = best
            row[j] = move
        prev = cur

    ops = []
    i, j = n, m
    while i > 0 or j > 0:
        move = back[i][j]
        if i > 0 and j > 0 and move == 0:
            ops.append(("ok" if ref[i - 1] == hyp[j - 1] else "sub", i - 1, j - 1))
            i, j = i - 1, j - 1
        elif i > 0 and (move == 1 or j == 0):
            ops.append(("del", i - 1, None))
            i -= 1
        else:
            ops.append(("ins", None, j - 1))
            j -= 1
    ops.reverse()
    return ops


def score(ops, start: int, end: int) -> dict:
    """Counts errors whose reference position lies in [start, end).

    An insertion belongs to the window only if it falls strictly between two window words.
    """
    counts = {"sub": 0, "del": 0, "ins": 0}
    next_ref = 0
    for op, ref_i, _ in ops:
        if op == "ins":
            if start < next_ref < end:
                counts["ins"] += 1
            continue
        if start <= ref_i < end and op != "ok":
            counts[op] += 1
        next_ref = ref_i + 1
    n_ref = end - start
    errors = counts["sub"] + counts["del"] + counts["ins"]
    return {**counts, "n_ref": n_ref, "wer": errors / n_ref if n_ref else 0.0}


def evaluate(ref_path: Path, hyp_path: Path) -> dict:
    ref, story_start, w_start, w_end = read_reference(ref_path)
    hyp = normalize(hyp_path.read_text(encoding="utf-8"))
    ops = align(ref, hyp)
    return {
        "window": score(ops, w_start, w_end),
        "clip": score(ops, story_start, len(ref)),
        "ops": ops,
        "ref": ref,
        "hyp": hyp,
        "window_range": (w_start, w_end),
    }


def show_window(result: dict) -> None:
    """Prints the window alignment, one error per line, for hand-checking."""
    start, end = result["window_range"]
    ref, hyp = result["ref"], result["hyp"]
    next_ref = 0
    for op, ref_i, hyp_i in result["ops"]:
        position = ref_i if ref_i is not None else next_ref
        if ref_i is not None:
            next_ref = ref_i + 1
        if not start <= position < end or op == "ok":
            continue
        ref_word = ref[ref_i] if ref_i is not None else "-"
        hyp_word = hyp[hyp_i] if hyp_i is not None else "-"
        context = " ".join(ref[max(start, position - 4):position + 5])
        print(f"{op:3}  ref={ref_word!r:18} hyp={hyp_word!r:18} … {context}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Rough WER over a clip and its 2-minute window")
    parser.add_argument("reference", type=Path)
    parser.add_argument("hypothesis", type=Path, nargs="+")
    parser.add_argument("--show", action="store_true", help="list window errors for hand-checking")
    args = parser.parse_args()

    for hyp_path in args.hypothesis:
        result = evaluate(args.reference, hyp_path)
        summary = {"hyp": str(hyp_path), "window": result["window"], "clip": result["clip"]}
        json.dump(summary, sys.stdout)
        print()
        if args.show:
            show_window(result)


if __name__ == "__main__":
    main()
