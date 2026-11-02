#!/usr/bin/env python3
"""User-space instructions per steady-state run() of the workloads the `text` workstream
touches, plain vs `rustique.enable()` (all default groups, or `--only`).

    python benches/targets/text_wl.py [workload ...] [--only logging,datetime] [-k 3] [-r 1] [-o out.json]

For each workload and side, fresh processes under `perf stat -e instructions:u` run
`setup()`, one warm-up `run()` and then 1 or K more `run()`s; instructions per run =
(I_K - I_1) / (K - 1) (startup, imports, setup and the warm-up cancel). The digest of the
last run must be identical on both sides (a mismatch is a bug: exit 1). Hardware counters:
dev box only. Default workloads: the nine of the wave-2 `text` brief.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.join(os.path.dirname(HERE), "workloads")
DEFAULT = [
    "web/logging_service", "services/job_worker", "threads/shared_cache", "data/fs_inventory",
    "data/tz_events", "data/log_analytics", "data/report_render", "web/requests_local",
    "custom/fulfilment_sim",
]


def driver(name: str, iters: int, only: str) -> None:
    if only != "-":
        import rustique

        kw = {"only": only.split(",")} if only else {}
        rustique.enable(**kw)
    import importlib.util

    path = os.path.join(WL, name + ".py")
    sys.path.insert(0, os.path.dirname(path))
    spec = importlib.util.spec_from_file_location("wl_" + name.replace("/", "_"), path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    st = mod.setup()
    res = mod.run(st)
    for _ in range(iters):
        res = mod.run(st)
    print("DIGEST", mod.digest(res))


def measure(name: str, iters: int, only: str) -> tuple[int, str]:
    cmd = ["perf", "stat", "-x,", "-e", "instructions:u", "--", sys.executable, __file__,
           "--driver", name, str(iters), only]
    r = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, PYTHONHASHSEED="0"))
    if r.returncode:
        raise RuntimeError(f"{name} {only!r}: {r.stderr[-3000:]}")
    dig = [ln for ln in r.stdout.splitlines() if ln.startswith("DIGEST")][-1]
    ins = [ln for ln in r.stderr.splitlines() if "instructions" in ln][-1]
    return int(ins.split(",")[0]), dig


def per_run(name: str, k: int, only: str, reps: int) -> tuple[float, str]:
    vals = []
    dig = ""
    for _ in range(reps):
        i1, _d1 = measure(name, 1, only)
        ik, dig = measure(name, k, only)
        vals.append((ik - i1) / (k - 1))
    return statistics.median(vals), dig


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--driver":
        driver(sys.argv[2], int(sys.argv[3]), sys.argv[4])
        return
    ap = argparse.ArgumentParser()
    ap.add_argument("workloads", nargs="*")
    ap.add_argument("--only", default="")
    ap.add_argument("-k", type=int, default=3)
    ap.add_argument("-r", type=int, default=1)
    ap.add_argument("-o")
    a = ap.parse_args()
    rows = []
    bad = False
    for w in a.workloads or DEFAULT:
        try:
            p, dp = per_run(w, a.k, "-", a.r)
            e, de = per_run(w, a.k, a.only, a.r)
        except RuntimeError as exc:
            print(f"{w:28s} FAILED {exc}", flush=True)
            bad = True
            continue
        same = dp == de
        bad |= not same
        rows.append({"workload": w, "plain": p, "enabled": e, "ratio": e / p, "same": same})
        print(f"{w:28s} plain {p / 1e6:9.1f}M  enabled {e / 1e6:9.1f}M  ratio {e / p:6.3f}  "
              f"{'same' if same else 'DIGEST MISMATCH'}", flush=True)
    if rows:
        gm = statistics.geometric_mean(r["ratio"] for r in rows)
        print(f"{'geometric mean':28s} {'':41s} ratio {gm:6.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "only": a.only, "rows": rows}, f, indent=1)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
