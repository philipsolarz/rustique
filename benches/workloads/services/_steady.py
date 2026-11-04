"""Steady-state harness for long-running workloads: time, RSS and GC over many iterations.

Not a workload itself (leading underscore). Runs one workload in this process for many
iterations of ``run()`` and records, per iteration: wall and CPU time, current RSS
(``VmRSS``), peak RSS (``VmHWM``), the number of collections per GC generation
(``gc.get_stats()``) and the digest (which must stay constant). Prints one JSON object:

    python benches/workloads/services/_steady.py services/job_worker [--iters 60] [--seconds 120]
        [--enable] [--out steady.json]

Summary fields: ``warm_s`` (median of iterations 2-6), ``steady_s``/``steady_cpu_s``
(median wall/CPU time of the second half of the iterations), ``rss_mb`` after setup / after the first iteration / at the end, ``rss_growth_mb_per_100``
(least-squares slope of RSS over the second half of the iterations, per 100 iterations),
``peak_rss_mb`` (``VmHWM``), ``gc_per_iter`` (collections per iteration per generation,
second half), ``digest_constant``. Linux only (reads /proc/self/status).
"""

from __future__ import annotations

import argparse
import gc
import json
import os
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def _status_kb(field):
    with open("/proc/self/status") as f:
        for line in f:
            if line.startswith(field + ":"):
                return int(line.split()[1])
    return 0


def _slope(xs, ys):
    if len(xs) < 2:
        return 0.0
    mx, my = statistics.fmean(xs), statistics.fmean(ys)
    den = sum((x - mx) ** 2 for x in xs)
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / den if den else 0.0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workload", help="family/name, e.g. services/job_worker")
    ap.add_argument("--iters", type=int, default=60)
    ap.add_argument("--seconds", type=float, default=0, help="stop early once this much time has passed")
    ap.add_argument("--enable", action="store_true", help="rustique.enable() before importing the workload")
    ap.add_argument("--out")
    a = ap.parse_args(argv)
    if a.enable:
        import rustique

        rustique.enable()
    sys.path.insert(0, os.path.dirname(HERE))
    import run as runner  # benches/workloads/run.py

    mod = runner.load(a.workload)
    t0 = time.perf_counter()
    state = mod.setup()
    setup_s = time.perf_counter() - t0
    rss_setup = _status_kb("VmRSS")
    rows = []
    digests = set()
    start = time.perf_counter()
    for i in range(a.iters):
        g0 = [s["collections"] for s in gc.get_stats()]
        c0, w0 = time.process_time(), time.perf_counter()
        res = mod.run(state)
        w1, c1 = time.perf_counter(), time.process_time()
        g1 = [s["collections"] for s in gc.get_stats()]
        digests.add(mod.digest(res))
        rows.append({"i": i, "wall_s": w1 - w0, "cpu_s": c1 - c0, "rss_kb": _status_kb("VmRSS"),
                     "hwm_kb": _status_kb("VmHWM"), "gc": [b - x for x, b in zip(g0, g1)]})
        if a.seconds and time.perf_counter() - start > a.seconds and i >= 10:
            break
    n = len(rows)
    half = rows[n // 2:]
    out = {
        "workload": a.workload, "enabled": a.enable, "python": sys.version.split()[0]
        + ("t" if not getattr(sys, "_is_gil_enabled", lambda: True)() else ""),
        "iters": n, "setup_s": round(setup_s, 3),
        "warm_s": statistics.median(r["wall_s"] for r in rows[1:6]),
        "steady_s": statistics.median(r["wall_s"] for r in half),
        "steady_cpu_s": statistics.median(r["cpu_s"] for r in half),
        "cpu_over_wall": round(sum(r["cpu_s"] for r in half) / sum(r["wall_s"] for r in half), 3),
        "rss_mb": {"setup": round(rss_setup / 1024, 1), "iter1": round(rows[0]["rss_kb"] / 1024, 1),
                   "end": round(rows[-1]["rss_kb"] / 1024, 1)},
        "rss_growth_mb_per_100": round(_slope([r["i"] for r in half], [r["rss_kb"] / 1024 for r in half]) * 100, 2),
        "peak_rss_mb": round(rows[-1]["hwm_kb"] / 1024, 1),
        "gc_per_iter": [round(statistics.fmean(r["gc"][g] for r in half), 2) for g in range(len(half[0]["gc"]))],
        "digest_constant": len(digests) == 1,
        "rows": rows,
    }
    text = json.dumps(out)
    if a.out:
        with open(a.out, "w") as f:
            f.write(text)
    summary = {k: v for k, v in out.items() if k != "rows"}
    print(json.dumps(summary), flush=True)
    os._exit(0)  # server threads/loops of the workloads are daemonic or idle; do not wait for them


if __name__ == "__main__":
    main()
