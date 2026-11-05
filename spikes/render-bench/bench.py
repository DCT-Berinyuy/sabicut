#!/usr/bin/env python3
"""Render benchmark: a 10-minute 1080p30 phone-like source cut into a ~60 s short with
20 cuts (some out of order, some zoomed), rendered with `sabicut render`.

Measures wall time per stage, peak memory, frame exactness and output loudness, and
appends one JSON line per run to out/results.jsonl.

Memory: a sampler thread adds up VmRSS of `sabicut` and every process below it (the
ffmpeg runs) every 0.1 s; the peak of that sum is the number reported against the PRD's
800 MB budget. It counts the ffmpeg libraries once per process, so the peak sum of PSS
(shared pages split between the processes mapping them) is recorded too, as is
wait4()'s ru_maxrss (largest single process, what GNU `time -v` calls "Maximum resident
set size").

Safety: each run gets its own systemd scope with MemoryMax and no swap, so a runaway
render is killed instead of pushing the shared laptop into swap. A run starts only when
no compiler is running and enough memory is free.

With --captions WORDS.json, the plan also gets word-by-word captions from those word
timings (read as the source's speech; the bench transcripts are 10 minutes long too) and
a 2 s hook, to measure what captions cost.

Usage: spikes/render-bench/bench.py [--preset medium --preset veryfast] [--ffmpeg PATH]
                                    [--captions WORDS.json]
"""

import argparse
import json
import os
import re
import subprocess
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
OUT = HERE / "out"
RESULTS = OUT / "results.jsonl"

SOURCE_SECONDS = 600
FPS = 30
CUTS = 20
CAP_MB = 1200
MIN_AVAILABLE_MB = 1500
BUILD_PROCESSES = {"cargo", "rustc", "cc1plus", "cc1"}
SAMPLE_S = 0.1


def available_mb():
    with open("/proc/meminfo") as meminfo:
        for line in meminfo:
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) // 1024
    return 0


def building():
    names = set()
    for pid in os.listdir("/proc"):
        if pid.isdigit():
            try:
                names.add(Path(f"/proc/{pid}/comm").read_text().strip())
            except OSError:
                pass
    return bool(names & BUILD_PROCESSES)


def wait_until_quiet():
    while building() or available_mb() < MIN_AVAILABLE_MB:
        print(f"waiting: {available_mb()} MB available, compiler running: {building()}", flush=True)
        time.sleep(15)


def make_source(work, ffmpeg):
    """10 min of 1920x1080 at 30 fps, H.264 with a 1 s GOP like a phone, a frame-number
    patch in the middle (grey 16 + 7 * (N mod 32)) and a slowly varying 220 Hz tone."""
    path = work / "source-10min-1080p30.mp4"
    if path.exists():
        return path
    partial = work / "partial-source.mp4"
    subprocess.run(
        [
            ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", f"testsrc2=s=1920x1080:r={FPS}:d={SOURCE_SECONDS}",
            "-f", "lavfi", "-i",
            f"nullsrc=s=16x16:r={FPS}:d={SOURCE_SECONDS},geq=lum='16+7*mod(N\\,32)':cb=128:cr=128,scale=480:480:flags=neighbor",
            "-f", "lavfi", "-i",
            f"sine=f=220:r=48000:d={SOURCE_SECONDS},volume='0.1+0.05*sin(2*PI*t/7)':eval=frame,aformat=channel_layouts=stereo",
            "-filter_complex", "[0:v][1:v]overlay=(W-w)/2:(H-h)/2,format=yuv420p[v]",
            "-map", "[v]", "-map", "2:a", "-c:v", "libx264", "-preset", "veryfast", "-crf", "23",
            "-g", str(FPS), "-c:a", "aac", "-b:a", "128k", "-f", "mp4", str(partial),
        ],
        check=True,
    )
    partial.rename(path)
    return path


def make_plan(work, source, words=None):
    """20 cuts of 2.4 to 3.6 s spread over the 10 minutes; cuts 7 and 8 swapped (a retake
    used out of order); a punch-in on every third cut, and an in-and-out on cut 10. With
    `words`, captions from those word timings and a hook."""
    cuts = []
    for k in range(CUTS):
        start = 12.0 + 29.0 * k + 0.37 * (k % 3)
        length = 2.4 + 0.4 * (k % 4)
        zooms = []
        if k % 3 == 0:
            zooms.append((start + 1.0, 1.2 + 0.05 * (k % 4)))
        if k == 10:
            zooms = [(start + 0.5, 1.5), (start + 1.8, 1.0)]
        cuts.append((start, start + length, zooms))
    cuts[7], cuts[8] = cuts[8], cuts[7]
    lines = [
        "schema = 1",
        "[meta]",
        f"fps = {FPS}",
        "out = { w = 1080, h = 1920 }",
        "[[source]]",
        'id = "a"',
        f'path = "{source.name}"',
        "crop_x = 0.5",
    ]
    for start, end, zooms in cuts:
        lines += ["[[cut]]", 'src = "a"', f"in = {start:.3f}", f"out = {end:.3f}"]
        if zooms:
            inner = ", ".join(f"{{ at = {at:.3f}, scale = {scale:.2f} }}" for at, scale in zooms)
            lines.append(f"zoom = [{inner}]")
    name = "plan.toml"
    if words:
        # No font, colour or outline: the defaults (the bundled Anton, gold, 10 px).
        lines += [
            "[hook]",
            'text = "Ten minutes in sixty seconds"',
            "dur = 2.0",
            "[captions]",
            f"words = {json.dumps(str(Path(words).resolve()))}",
            'keywords = ["Alice", "rabbit"]',
        ]
        name = "plan-captions.toml"
    plan = work / name
    plan.write_text("\n".join(lines) + "\n")
    return plan


def descendants(root):
    children = {}
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            stat = Path(f"/proc/{pid}/stat").read_text()
        except OSError:
            continue
        ppid = int(stat.rsplit(")", 1)[1].split()[1])
        children.setdefault(ppid, []).append(int(pid))
    found, todo = [], [root]
    while todo:
        pid = todo.pop()
        found.append(pid)
        todo.extend(children.get(pid, []))
    return found


def rss_mb(pid):
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) / 1024
    except OSError:
        pass
    return 0.0


def pss_mb(pid):
    """Proportional set size: shared pages (the ffmpeg libraries) split between the
    processes that map them, so a sum over processes counts them once."""
    try:
        for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
            if line.startswith("Pss:"):
                return int(line.split()[1]) / 1024
    except OSError:
        pass
    return 0.0


def run_render(sabicut, plan, output, preset, env):
    command = [
        "systemd-run", "--user", "--scope", "--quiet", "-p", f"MemoryMax={CAP_MB}M",
        "-p", "MemorySwapMax=0", str(sabicut), "render", str(plan), "-o", str(output),
        "--preset", preset,
    ]
    started = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
    peak = {"sum": 0.0, "pss": 0.0, "min_available": available_mb(), "stage": "start", "by_stage": {}}
    done = threading.Event()

    def sample():
        while not done.is_set():
            pids = descendants(process.pid)
            total = sum(rss_mb(pid) for pid in pids)
            peak["sum"] = max(peak["sum"], total)
            peak["pss"] = max(peak["pss"], sum(pss_mb(pid) for pid in pids))
            stage = peak["stage"]
            peak["by_stage"][stage] = round(max(peak["by_stage"].get(stage, 0), total))
            peak["min_available"] = min(peak["min_available"], available_mb())
            time.sleep(SAMPLE_S)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    # Stage boundaries, from the progress lines on stderr.
    stages = []
    for line in process.stderr:
        for prefix, name in [
            ("cutting", "audio"),
            ("measuring", "loudness"),
            ("encoding", "video"),
            ("checking", "check"),
            ("writing", "cover"),
        ]:
            if line.startswith(prefix) and not any(s[0] == name for s in stages):
                stages.append((name, time.monotonic() - started))
                peak["stage"] = name
    stdout = process.stdout.read()
    _, status, usage = os.wait4(process.pid, 0)
    elapsed = time.monotonic() - started
    done.set()
    sampler.join()
    if status != 0:
        raise SystemExit(f"render failed ({status}):\n{stdout}")
    durations = {}
    for (name, at), (_, next_at) in zip(stages, stages[1:] + [("end", elapsed)]):
        durations[name] = round(next_at - at, 1)
    return {
        "wall_s": round(elapsed, 1),
        "stages_s": durations,
        "peak_tree_rss_mb": round(peak["sum"]),
        "peak_rss_by_stage_mb": peak["by_stage"],
        "peak_tree_pss_mb": round(peak["pss"]),
        "max_single_rss_mb": round(usage.ru_maxrss / 1024),
        "min_available_mb": peak["min_available"],
        "stdout": stdout.strip(),
    }


def timeline_frames(sabicut, plan):
    """(first source frame, frame count) per cut, from `sabicut timeline`."""
    text = subprocess.run([str(sabicut), "timeline", str(plan)], capture_output=True, text=True, check=True).stdout
    cuts = []
    for line in text.splitlines():
        match = re.match(r"\s*\d+\s+\S+\s+([\d.]+) -\s+[\d.]+\s+[\d.]+ -\s+[\d.]+\s+(\d+)\s", line)
        if match:
            cuts.append((round(float(match.group(1)) * FPS), int(match.group(2))))
    return cuts


def frame_exact(ffmpeg, output, cuts):
    """Frames read back from the patch, and how many show the wrong source frame."""
    raw = subprocess.run(
        [
            ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error", "-i", str(output),
            "-map", "0:v:0", "-fps_mode", "passthrough",
            "-vf", "crop=64:64:(iw-64)/2:(ih-64)/2,scale=1:1:flags=area,format=rgb24", "-f", "rawvideo", "-",
        ],
        capture_output=True, check=True,
    ).stdout
    got = [round(raw[i] * 219 / (7 * 255)) % 32 for i in range(0, len(raw), 3)]
    want = [(first + n) % 32 for first, count in cuts for n in range(count)]
    wrong = sum(1 for g, w in zip(got, want) if g != w) + abs(len(got) - len(want))
    return len(got), wrong


def loudness(ffmpeg, output):
    log = subprocess.run(
        [ffmpeg, "-nostdin", "-hide_banner", "-nostats", "-i", str(output), "-map", "0:a:0",
         "-af", "ebur128=framelog=quiet", "-f", "null", "-"],
        capture_output=True, text=True, check=True,
    ).stderr
    summary = log[log.rfind("Summary:"):]
    return float(re.search(r"I:\s+(-?[\d.]+) LUFS", summary).group(1))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--preset", action="append", help="x264 presets to run (default: medium, veryfast)")
    parser.add_argument("--ffmpeg", help="ffmpeg binary to render with (sets SABICUT_FFMPEG)")
    parser.add_argument("--work", default=str(Path.home() / ".cache" / "sabicut-render-bench"))
    parser.add_argument("--sabicut", default=str(ROOT / "target" / "release" / "sabicut"))
    parser.add_argument("--captions", help="words.json to caption the plan with")
    args = parser.parse_args()
    presets = args.preset or ["medium", "veryfast"]
    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(exist_ok=True)
    env = dict(os.environ)
    if args.ffmpeg:
        env["SABICUT_FFMPEG"] = args.ffmpeg
    ffmpeg = args.ffmpeg or "ffmpeg"
    version = subprocess.run([ffmpeg, "-hide_banner", "-version"], capture_output=True, text=True).stdout.split()[2]

    wait_until_quiet()
    source = make_source(work, "ffmpeg")
    plan = make_plan(work, source, args.captions)
    cuts = timeline_frames(args.sabicut, plan)
    for preset in presets:
        wait_until_quiet()
        suffix = "-captions" if args.captions else ""
        output = work / f"out-{preset}-{version}{suffix}.mp4"
        print(f"rendering with preset {preset}, ffmpeg {version}...", flush=True)
        load_before = os.getloadavg()[0]
        result = run_render(args.sabicut, plan, output, preset, env)
        frames, wrong = frame_exact(ffmpeg, output, cuts)
        result.update(
            preset=preset,
            captions=bool(args.captions),
            ffmpeg=version,
            cuts=len(cuts),
            frames=frames,
            frames_wrong=wrong,
            lufs=loudness(ffmpeg, output),
            output_mb=round(output.stat().st_size / 1e6, 1),
            cpus=os.cpu_count(),
            load_before=round(load_before, 2),
        )
        with RESULTS.open("a") as results:
            results.write(json.dumps(result) + "\n")
        print(json.dumps(result, indent=2), flush=True)


if __name__ == "__main__":
    main()
