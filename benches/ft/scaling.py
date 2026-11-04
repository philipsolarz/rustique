"""Throughput scaling of multi-threaded workloads: plain vs `rustique.enable()`, 1..N threads.

    python benches/ft/scaling.py run threads [-t 1,2,4,8] [-n 3] [-p 3] [--modes plain,enable]
                                 [--only json,copy] [--exclude json] [-o out.json]
    python benches/ft/scaling.py table out-3.15.1.json out-3.15.1t.json [...]

`run` uses the current interpreter. For every workload, repeat `p` times: for every thread
count and mode (interleaved, so drift hits all cells alike) start a fresh process
`benches/workloads/run.py one <w> -n <n> [--enable]` with `WL_THREADS=<t>` (each process:
`setup()`, one warm-up `run()`, `n` timed runs). Cell value: median of the `p * n` timed
runs, with min/max. Digests must agree across every cell of a workload on one interpreter
(a mismatch is a bug: exit 1). Pin the whole command to a CPU set (`taskset`) with at least
max(t) cores. `--only/--exclude` restrict the enabled side to some groups (bisecting a
scaling problem). `table` prints Markdown: time per cell, speedup over the same mode at
1 thread (scaling), enabled/plain per thread count, and plain t-build / GIL-build when both
builds are given.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import sysconfig

HERE = os.path.dirname(os.path.abspath(__file__))
RUN = os.path.join(HERE, "..", "workloads", "run.py")
sys.path.insert(0, os.path.join(HERE, "..", "workloads"))


def build_name() -> str:
    v = sys.version_info
    return f"{v.major}.{v.minor}.{v.micro}" + ("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else "")


def one(w: str, t: int, mode: str, n: int, only: str | None, exclude: str | None) -> dict:
    env = dict(os.environ, WL_THREADS=str(t))
    cmd = [sys.executable, RUN, "one", w, "-n", str(n)]
    if mode == "enable":
        cmd.append("--enable")
        if only:
            cmd += ["--only", only]
        if exclude:
            env["RUSTIQUE_EXCLUDE"] = exclude
    r = subprocess.run(cmd, capture_output=True, text=True, env=env)
    if r.returncode != 0:
        raise SystemExit(f"{w} t={t} {mode}: FAILED\n{r.stderr[-3000:]}")
    return json.loads(r.stdout.strip().splitlines()[-1])


def cmd_run(a: argparse.Namespace) -> None:
    import run as wl_run  # benches/workloads/run.py

    threads = [int(x) for x in a.threads.split(",")]
    modes = a.modes.split(",")
    out = {"build": build_name(), "threads": threads, "modes": modes, "n": a.n, "p": a.p,
           "only": a.only, "exclude": a.exclude, "affinity": sorted(os.sched_getaffinity(0)), "cells": []}
    bad = False
    for w in wl_run.select(a.workloads):
        times: dict[tuple[int, str], list[float]] = {}
        digests = set()
        for _ in range(a.p):
            for t in threads:
                for m in modes:
                    d = one(w, t, m, a.n, a.only, a.exclude)
                    times.setdefault((t, m), []).extend(d["times"])
                    digests.add(d["digest"])
        same = len(digests) == 1
        bad |= not same
        for (t, m), ts in sorted(times.items()):
            out["cells"].append({"workload": w, "threads": t, "mode": m, "median_s": statistics.median(ts),
                                 "min_s": min(ts), "max_s": max(ts), "times": ts, "digest": sorted(digests)[0],
                                 "same_digest": same})
        row = "  ".join(f"t{t}:{statistics.median(times[(t, m)]) * 1e3:7.1f}{m[0]}" for t in threads for m in modes)
        print(f"{out['build']:8s} {w:28s} {row}  {'same' if same else 'DIGEST MISMATCH'}", flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump(out, f, indent=1)
    if bad:
        sys.exit(1)


def cmd_table(a: argparse.Namespace) -> None:
    runs = [json.load(open(p)) for p in a.files]
    by = {}
    for r in runs:
        for c in r["cells"]:
            by[(r["build"], c["workload"], c["threads"], c["mode"])] = c
    builds = [r["build"] for r in runs]
    workloads = list(dict.fromkeys(c["workload"] for r in runs for c in r["cells"]))
    threads = sorted({c["threads"] for r in runs for c in r["cells"]})
    print("| workload | build | mode | " + " | ".join(f"{t} thr ms" for t in threads) + " | "
          + " | ".join(f"x{t}" for t in threads[1:]) + " |")
    print("|---|---|---|" + "---:|" * (2 * len(threads) - 1))
    for w in workloads:
        for b in builds:
            for m in ("plain", "enable"):
                cells = [by.get((b, w, t, m)) for t in threads]
                if not any(cells):
                    continue
                base = cells[0]["median_s"] if cells[0] else None
                ms = " | ".join(f"{c['median_s'] * 1e3:.0f}" if c else "" for c in cells)
                sp = " | ".join(f"{base / c['median_s']:.2f}" if c and base else "" for c in cells[1:])
                print(f"| {w} | {b} | {m} | {ms} | {sp} |")
    print()
    print("enabled / plain (lower is better):")
    print()
    print("| workload | build | " + " | ".join(f"{t} thr" for t in threads) + " |")
    print("|---|---|" + "---:|" * len(threads))
    for w in workloads:
        for b in builds:
            vals = []
            for t in threads:
                p, e = by.get((b, w, t, "plain")), by.get((b, w, t, "enable"))
                vals.append(f"{e['median_s'] / p['median_s']:.3f}" if p and e else "")
            print(f"| {w} | {b} | " + " | ".join(vals) + " |")
    gil = [b for b in builds if not b.endswith("t")]
    ft = [b for b in builds if b.endswith("t")]
    if gil and ft:
        print()
        print(f"t build / GIL build ({ft[0]} / {gil[0]}), same mode and thread count:")
        print()
        print("| workload | mode | " + " | ".join(f"{t} thr" for t in threads) + " |")
        print("|---|---|" + "---:|" * len(threads))
        for w in workloads:
            for m in ("plain", "enable"):
                vals = []
                for t in threads:
                    g, f = by.get((gil[0], w, t, m)), by.get((ft[0], w, t, m))
                    vals.append(f"{f['median_s'] / g['median_s']:.3f}" if g and f else "")
                print(f"| {w} | {m} | " + " | ".join(vals) + " |")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("workloads", nargs="+")
    r.add_argument("-t", "--threads", default="1,2,4,8")
    r.add_argument("-n", type=int, default=3)
    r.add_argument("-p", type=int, default=3)
    r.add_argument("--modes", default="plain,enable")
    r.add_argument("--only")
    r.add_argument("--exclude")
    r.add_argument("-o")
    t = sub.add_parser("table")
    t.add_argument("files", nargs="+")
    a = ap.parse_args()
    {"run": cmd_run, "table": cmd_table}[a.cmd](a)


if __name__ == "__main__":
    main()
