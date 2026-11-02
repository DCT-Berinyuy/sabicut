#!/usr/bin/env python3
"""Runs the whisper-bench matrix, one process per run, and appends results to out/results.jsonl.

Safety: each run gets its own systemd scope with MemoryMax=1536M and no swap. The cap equals
the pass threshold (peak RSS below 1.5 GB), so a run that hits it fails, and the kernel kills
only that run instead of pushing the whole laptop into swap. Before a run starts, the runner
waits until no compiler (cargo, rustc, cc1plus, cc1) is running and enough RAM is free; during
the run it samples free memory and compiler processes so contaminated timings are visible.
Finished runs are skipped, so the matrix can be resumed after an interruption.

Peak RSS comes from wait4()'s ru_maxrss, the number GNU `time -v` reports as "Maximum
resident set size" (GNU time is not installed on the benchmark machine). The load average
and free memory are recorded with every run because the machine is shared with other work.
"""

import argparse
import json
import os
import signal
import subprocess
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BIN = HERE / "target" / "release" / "whisper-bench"
VAD_MODEL = HERE / "models" / "ggml-silero-v6.2.0.bin"
OUT = HERE / "out"
RESULTS = OUT / "results.jsonl"

CAP_MB = 1536
# MemAvailable needed before a run starts: the expected peak plus a margin for the desktop.
MIN_AVAILABLE_MB = {"tiny": 900, "base": 1000, "small": 1300}
BUILD_PROCESSES = {"cargo", "rustc", "cc1plus", "cc1"}
WAIT_POLL_S = 15
WAIT_LIMIT_S = 600
BUILD_WAIT_LIMIT_S = 1800
SAMPLE_S = 5


def mem_available_mb() -> int:
    with open("/proc/meminfo", encoding="ascii") as fh:
        for line in fh:
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) // 1024
    return 0


def build_processes() -> set[str]:
    found = set()
    for entry in os.scandir("/proc"):
        if not entry.name.isdigit():
            continue
        try:
            with open(f"/proc/{entry.name}/comm", encoding="utf-8") as fh:
                comm = fh.read().strip()
        except OSError:
            continue
        if comm in BUILD_PROCESSES:
            found.add(comm)
    return found


def wait_for_no_builds() -> float:
    started = time.monotonic()
    while running := build_processes():
        waited = time.monotonic() - started
        if waited > BUILD_WAIT_LIMIT_S:
            raise SystemExit(f"compilers still running after {waited:.0f} s: {sorted(running)}")
        print(f"waiting for builds to finish: {', '.join(sorted(running))}", flush=True)
        time.sleep(WAIT_POLL_S)
    return time.monotonic() - started


def wait_for_memory(model: str) -> int:
    need = MIN_AVAILABLE_MB.get(model, max(MIN_AVAILABLE_MB.values()))
    deadline = time.monotonic() + WAIT_LIMIT_S
    while (available := mem_available_mb()) < need:
        if time.monotonic() > deadline:
            raise SystemExit(
                f"only {available} MiB free, {model} needs {need} MiB: close some apps and "
                "re-run (finished runs are kept)"
            )
        print(f"waiting for memory: {available} MiB free, {model} needs {need} MiB", flush=True)
        time.sleep(WAIT_POLL_S)
    return available


def finished_runs() -> set[str]:
    if not RESULTS.exists():
        return set()
    lines = RESULTS.read_text(encoding="utf-8").splitlines()
    return {json.loads(line)["run"] for line in lines if line.strip()}


def bench_command(model: str, lang: str, vad: bool, threads: int, prefix: Path) -> list[str]:
    cmd = [
        "systemd-run", "--user", "--scope", "--quiet", "--collect",
        "-p", f"MemoryMax={CAP_MB}M", "-p", "MemorySwapMax=0", "--",
        str(BIN),
        "--model", str(HERE / "models" / f"ggml-{model}-q5_1.bin"),
        "--audio", str(HERE / "audio" / f"{lang}.wav"),
        "--lang", lang,
        "--out", str(prefix),
        "--threads", str(threads),
    ]
    if vad:
        cmd += ["--vad-model", str(VAD_MODEL), "--vad-threads", "1"]
    return cmd


class RunMonitor:
    """Samples free memory and compiler processes in the background while a run is going."""

    def __init__(self, mem_before: int) -> None:
        self.min_available_mb = mem_before
        self.builds_seen: set[str] = set()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._sample, daemon=True)

    def _sample(self) -> None:
        while not self._stop.wait(SAMPLE_S):
            self.min_available_mb = min(self.min_available_mb, mem_available_mb())
            self.builds_seen |= build_processes()

    def __enter__(self) -> "RunMonitor":
        self._thread.start()
        return self

    def __exit__(self, *_exc) -> None:
        self._stop.set()
        self._thread.join()


def run_one(model: str, lang: str, vad: bool, threads: int, tag: str, name: str) -> dict:
    prefix = OUT / name
    build_wait_s = wait_for_no_builds()
    mem_before = wait_for_memory(model)
    load_before = os.getloadavg()[0]
    with open(f"{prefix}.log", "w", encoding="utf-8") as log, RunMonitor(mem_before) as monitor:
        start = time.monotonic()
        # systemd-run --scope execs the benchmark in place, so this PID is the benchmark itself.
        proc = subprocess.Popen(
            bench_command(model, lang, vad, threads, prefix),
            stdout=subprocess.PIPE, stderr=log, text=True,
        )
        stdout = proc.stdout.read()
        _, status, usage = os.wait4(proc.pid, 0)
        wall_s = time.monotonic() - start
    exit_code = os.waitstatus_to_exitcode(status)

    if exit_code == 0:
        result = json.loads(stdout.strip().splitlines()[-1])
        result["status"] = "ok"
    elif exit_code == -signal.SIGKILL:
        result = {"status": f"killed (memory cap {CAP_MB} MiB)", "lang": lang, "vad": vad}
    else:
        raise SystemExit(f"{name} failed with exit code {exit_code}; see {prefix}.log")

    result.update(
        run=name,
        tag=tag,
        model=model,
        wall_s=round(wall_s, 2),
        maxrss_kb=usage.ru_maxrss,
        cpu_user_s=round(usage.ru_utime, 2),
        cpu_sys_s=round(usage.ru_stime, 2),
        build_wait_s=round(build_wait_s),
        builds_during_run=sorted(monitor.builds_seen),
        mem_available_mb_before=mem_before,
        mem_available_mb_min=monitor.min_available_mb,
        mem_available_mb_after=mem_available_mb(),
        load1_before=round(load_before, 2),
        load1_after=round(os.getloadavg()[0], 2),
    )
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--models", nargs="+", default=["small", "base", "tiny"])
    parser.add_argument("--langs", nargs="+", default=["en", "fr"])
    parser.add_argument("--vad", choices=["on", "off", "both"], default="on")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--tag", default="main")
    args = parser.parse_args()

    OUT.mkdir(exist_ok=True)
    done = finished_runs()
    vad_modes = {"on": [True], "off": [False], "both": [True, False]}[args.vad]
    for model in args.models:
        for lang in args.langs:
            for vad in vad_modes:
                name = f"{args.tag}-{model}-{lang}-{'vad' if vad else 'novad'}-t{args.threads}"
                if name in done:
                    print(f"{name}: already done, skipping", flush=True)
                    continue
                result = run_one(model, lang, vad, args.threads, args.tag, name)
                with open(RESULTS, "a", encoding="utf-8") as fh:
                    fh.write(json.dumps(result) + "\n")
                print(
                    f"{name}: {result['status']}, wall {result['wall_s']:.1f} s, "
                    f"peak RSS {result['maxrss_kb'] / 1024:.0f} MiB, "
                    f"load {result['load1_before']}->{result['load1_after']}",
                    flush=True,
                )


if __name__ == "__main__":
    main()
