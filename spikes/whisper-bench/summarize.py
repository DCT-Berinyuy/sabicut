#!/usr/bin/env python3
"""Prints the Markdown result tables for docs/bench/whisper.md from out/results.jsonl.

For every finished run it adds the rough WER (2-minute window and whole clip, see wer.py)
and the word-timestamp spot-check (see align_check.py). When a no-VAD run of the same model
and language exists (tag "novad"), the timestamps are also compared word by word against it.
"""

import json
from pathlib import Path

import align_check
import wer

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
RESULTS = OUT / "results.jsonl"
PASS_WALL_S = 600
PASS_RSS_MIB = 1536


def load_results() -> list[dict]:
    lines = RESULTS.read_text(encoding="utf-8").splitlines()
    return [json.loads(line) for line in lines if line.strip()]


def pct(value: float) -> str:
    return f"{100 * value:.1f}%"


def verdict(r: dict) -> str:
    if r["status"] != "ok":
        return "**fail** (memory cap)"
    fast = r["wall_s"] <= PASS_WALL_S
    lean = r["maxrss_kb"] / 1024 < PASS_RSS_MIB
    return "pass" if fast and lean else "**fail** (" + ("time" if lean else "memory") + ")"


def novad_words(r: dict) -> Path | None:
    path = OUT / f"novad-{r['model']}-{r['lang']}-novad-t{r.get('threads', 4)}.words.tsv"
    return path if path.exists() and r.get("vad") else None


def score(results: list[dict]) -> list[dict]:
    db_cache: dict[str, list[float]] = {}
    scored = []
    for r in results:
        row = dict(r)
        if r["status"] == "ok":
            lang, name = r["lang"], r["run"]
            ref = HERE / "refs" / f"{lang}.txt"
            row["wer"] = wer.evaluate(ref, OUT / f"{name}.txt")
            wav = HERE / "audio" / f"{lang}.wav"
            db = db_cache.setdefault(lang, align_check.frame_db(wav))
            row["ts"] = align_check.check(wav, OUT / f"{name}.words.tsv", novad_words(r), db)
        scored.append(row)
    return scored


def main_table(rows: list[dict]) -> None:
    print("| Model | Lang | VAD | Speech kept | Wall clock | Real-time factor | Peak RSS "
          "| WER, 2-min window | WER, 10-min clip | Verdict |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        rss = f"{r['maxrss_kb'] / 1024:.0f} MiB"
        if r["status"] != "ok":
            print(f"| {r['model']} | {r['lang']} | {'on' if r['vad'] else 'off'} | – "
                  f"| {r['wall_s']:.0f} s | – | ≥{rss} | – | – | {verdict(r)} |")
            continue
        w = r["wer"]
        print(
            f"| {r['model']} | {r['lang']} | {'on' if r['vad'] else 'off'} "
            f"| {r['speech_s']:.0f} s | {r['wall_s']:.0f} s | {r['wall_s'] / r['audio_s']:.2f} "
            f"| {rss} | {pct(w['window']['wer'])} ({w['window']['n_ref']} words) "
            f"| {pct(w['clip']['wer'])} | {verdict(r)} |"
        )


def conditions_table(rows: list[dict]) -> None:
    print("| Run | Load (1 min) before → after | Free RAM before / lowest during "
          "| Compilers seen during run |")
    print("|---|---|---|---|")
    for r in rows:
        builds = ", ".join(r.get("builds_during_run", [])) or "none"
        print(
            f"| {r['run']} | {r['load1_before']} → {r['load1_after']} "
            f"| {r['mem_available_mb_before']} / {r.get('mem_available_mb_min', '?')} MiB "
            f"| {builds} |"
        )


def timestamp_table(rows: list[dict]) -> None:
    print("| Run | Onsets checked | Median abs. error | 90th pct | Median bias "
          "| First half / second half | Words inside pauses | Against no-VAD run |")
    print("|---|---|---|---|---|---|---|---|")
    for r in rows:
        if r["status"] != "ok":
            continue
        t = r["ts"]
        cross = next((v for k, v in t.items() if k.startswith("vs_")), None)
        cross_text = (
            f"median {cross['median_abs_ms']} ms, p90 {cross['p90_abs_ms']} ms "
            f"({cross['matched']} words)" if cross else "–"
        )
        print(
            f"| {r['run']} | {t['onsets_matched']} of {t['pauses']} "
            f"| {t['onset_median_abs_ms']} ms | {t['onset_p90_abs_ms']} ms "
            f"| {t['onset_bias_ms']:+d} ms "
            f"| {t['first_half_median_abs_ms']} / {t['second_half_median_abs_ms']} ms "
            f"| {t['words_in_pauses_pct']}% | {cross_text} |"
        )


def main() -> None:
    rows = score(load_results())
    for title, table in (
        ("Results", main_table),
        ("Machine conditions per run", conditions_table),
        ("Word-timestamp spot-check", timestamp_table),
    ):
        print(f"### {title}\n")
        table(rows)
        print()


if __name__ == "__main__":
    main()
