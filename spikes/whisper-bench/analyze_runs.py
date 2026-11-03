#!/usr/bin/env python3
"""Runs `sabicut analyze` on the benchmark clips, one transcription at a time, and scores
each run with analyze_check.py. Used for docs/bench/analyze.md.

Before each run it waits until MemAvailable is at least 1.5 GiB and no compiler or ffmpeg
is running; each run gets its own systemd scope with MemoryMax=1536M and no swap. While a
run goes on, competing processes and the load average are sampled, so contended timings
are visible. Results are appended to out/analyze/runs.jsonl.

Clips are cut from audio/en.wav and audio/fr.wav (see fetch.sh, or point --audio at a
folder that has them): en = 190-330 s and fr = 224-354 s, which cover the hand-checked
WER windows; en60 = 190-250 s; en10 and fr10 = the whole 10 minutes.

usage: analyze_runs.py NAME CLIP [sabicut analyze options...]
  e.g. analyze_runs.py small-fr-pad200 fr --lang fr --vad-pad 200
"""

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

import analyze_check

HERE = Path(__file__).resolve().parent
SABICUT = HERE.parents[1] / "target" / "release" / "sabicut"
AUDIO = Path(os.environ.get("BENCH_AUDIO", HERE / "audio"))
OUT = HERE / "out" / "analyze"
CLIPS = {  # name: (source, start s, length s or None, reference)
    "en": ("en", 190, 140, "refs/en.txt"),
    "fr": ("fr", 224, 130, "refs/fr.txt"),
    "en60": ("en", 190, 60, None),
    "en10": ("en", 0, None, "refs/en.txt"),
    "fr10": ("fr", 0, None, "refs/fr.txt"),
}
NEED_MIB = 1536
COMPETITORS = {"cargo", "rustc", "cc1plus", "cc1", "ld", "cmake", "ffmpeg"}


def mem_available_mib() -> int:
    for line in open("/proc/meminfo", encoding="ascii"):
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) // 1024
    return 0


def competitors() -> set[str]:
    found = set()
    for entry in os.scandir("/proc"):
        if entry.name.isdigit():
            try:
                comm = open(f"/proc/{entry.name}/comm", encoding="utf-8").read().strip()
            except OSError:
                continue
            if comm in COMPETITORS:
                found.add(comm)
    return found


def wait_until_quiet() -> dict:
    started = time.monotonic()
    while True:
        running, free = competitors(), mem_available_mib()
        if not running and free >= NEED_MIB:
            return {"waited_s": round(time.monotonic() - started), "mem_available_mib": free}
        if time.monotonic() - started > 3600:
            raise SystemExit(f"gave up waiting: {free} MiB free, running {sorted(running)}")
        print(f"waiting: {free} MiB free, running {sorted(running)}", flush=True)
        time.sleep(15)


def clip_path(name: str) -> tuple[Path, Path | None]:
    source, start, length, ref = CLIPS[name]
    full = AUDIO / f"{source}.wav"
    if length is None:
        return full, HERE / ref if ref else None
    path = OUT / "clips" / f"{name}.wav"
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-ss", str(start),
                        "-t", str(length), "-i", str(full), "-c:a", "pcm_s16le", str(path)],
                       check=True)
    return path, HERE / ref if ref else None


def main() -> None:
    name, clip, extra = sys.argv[1], sys.argv[2], sys.argv[3:]
    wav, ref = clip_path(clip)
    out = OUT / f"{name}.json"
    before = wait_until_quiet()
    seen: set[str] = set()
    loads: list[float] = []
    stop = threading.Event()

    def sample() -> None:
        while not stop.wait(5):
            seen.update(competitors())
            loads.append(os.getloadavg()[0])

    cmd = ["systemd-run", "--user", "--scope", "--quiet", "--collect",
           "-p", f"MemoryMax={NEED_MIB}M", "-p", "MemorySwapMax=0", "--",
           str(SABICUT), "analyze", str(wav), "-o", str(out), *extra]
    monitor = threading.Thread(target=sample, daemon=True)
    started = time.monotonic()
    monitor.start()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    wall = time.monotonic() - started
    stop.set()
    monitor.join()
    record = {
        "name": name, "clip": clip, "args": extra, "wall_s": round(wall, 1),
        "exit": proc.returncode, "summary": proc.stdout.strip().splitlines(),
        "stderr_tail": proc.stderr.strip().splitlines()[-3:],
        "competitors_seen": sorted(seen),
        "load1_mean": round(sum(loads) / len(loads), 2) if loads else None, **before,
    }
    if proc.returncode == 0:
        record["check"] = analyze_check.check("words", wav, out, ref)
    with open(OUT / "runs.jsonl", "a", encoding="utf-8") as fh:
        fh.write(json.dumps(record) + "\n")
    print(json.dumps(record), flush=True)


if __name__ == "__main__":
    main()
