#!/usr/bin/env python3
"""Per-workload rows of one policy vs default on both builds (deterministic proxies first).

    python benches/runtime/policyrows.py DIR [DIR2 ...] --policy adaptive [--min-gc 0.01]

Rows: workloads whose automatic GC share (default) is >= --min-gc on either build. Columns per
build: GC share (default), GC work ratio (objects examined), largest collection ratio, peak RSS
ratio, wall ratio (dev box: noisy, median of paired rounds)."""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gctable as T  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("dirs", nargs="+")
ap.add_argument("--policy", default="adaptive")
ap.add_argument("--base", default="default")
ap.add_argument("--min-gc", type=float, default=0.01)
a = ap.parse_args()
by = {}
for d in a.dirs:
    for (py, wl, pol), rounds in T.load(d).items():
        if pol in (a.policy, a.base):
            s = T.summarize_cell(rounds)
            if s:
                by.setdefault(wl, {}).setdefault(py, {})[pol] = s


def cell(b, p):
    if not b or not p:
        return " | | | | "
    w = p["work"] / b["work"] if b["work"] else float("nan")
    sz = p["size_max"] / b["size_max"] if b["size_max"] else float("nan")
    m = p["hwm_kb"] / b["hwm_kb"] if b["hwm_kb"] > 0 else float("nan")
    r = T.paired_ratio(b, p)[0]
    return f"{100 * b['gc_share']:.1f} | {w:.2f} | {sz:.2f} | {m:.2f} | {r:.2f}"


print(f"workload | GC % (3.15.1) | work × | largest coll. × | peak RSS × | wall × (noisy) | GC % (t) | work × | largest coll. × | peak RSS × | wall ×")
print("---|---|---|---|---|---|---|---|---|---|---")
for wl in sorted(by):
    rows = by[wl]
    g = max(rows.get(py, {}).get(a.base, {}).get("gc_share", 0) for py in ("3.15.1", "3.15.1t"))
    if g < a.min_gc:
        continue
    cs = [cell(rows.get(py, {}).get(a.base), rows.get(py, {}).get(a.policy)) for py in ("3.15.1", "3.15.1t")]
    print(f"{wl} | {cs[0]} | {cs[1]}")
