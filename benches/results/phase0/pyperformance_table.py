"""Combine the pyperformance replicates of one build into one table.

    python pyperformance_table.py <results>/pyperformance 3.15.1 [--md out.md]

Reads plain-<b>.json / enabled-<b>.json (replicate A: plain first), plain-<b>-B.json /
enabled-<b>-B.json (replicate B: enabled first, another L3 domain) and, if present,
plain-<b>-ports.json / enabled-<b>-ports.json (benchmarks that bind fixed ports, re-run
alone). Prints per-benchmark plain wall time and the enabled/plain ratio of each replicate,
geometric means, and the benchmarks that moved by > 2% the same way in both replicates.
"""

from __future__ import annotations

import argparse
import math
import os
import statistics

import pyperf


def load(path: str) -> dict[str, pyperf.Benchmark]:
    if not os.path.exists(path):
        return {}
    return {b.get_name(): b for b in pyperf.BenchmarkSuite.load(path).get_benchmarks()}


def fmt_t(s: float) -> str:
    for unit, f in (("s", 1), ("ms", 1e3), ("µs", 1e6), ("ns", 1e9)):
        if s * f >= 1:
            return f"{s * f:.3g} {unit}"
    return f"{s * 1e9:.3g} ns"


def gm(xs: list[float]) -> float:
    return math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("build")
    args = ap.parse_args()
    d, b = args.dir, args.build
    reps = {}
    for tag, name in (("", "A"), ("-B", "B")):
        p, e = load(f"{d}/plain-{b}{tag}.json"), load(f"{d}/enabled-{b}{tag}.json")
        if p and e:
            reps[name] = (p, e)
    ports = (load(f"{d}/plain-{b}-ports.json"), load(f"{d}/enabled-{b}-ports.json"))
    names = sorted(set().union(*[set(p) | set(e) for p, e in reps.values()], set(ports[0]) | set(ports[1])))
    rows = []
    for n in names:
        ratios, plains = {}, []
        for r, (p, e) in reps.items():
            if n in p and n in e:
                ratios[r] = e[n].mean() / p[n].mean()
            if n in p:
                plains.append(p[n].mean())
        if n in ports[0] and n in ports[1]:
            ratios["ports"] = ports[1][n].mean() / ports[0][n].mean()
            plains.append(ports[0][n].mean())
        rows.append((n, plains, ratios))
    print(f"# pyperformance {b}: replicates {sorted(reps)}" + (" + ports re-run" if ports[0] else ""))
    for r, (p, e) in reps.items():
        common = sorted(set(p) & set(e))
        print(f"replicate {r}: plain {len(p)} results, enabled {len(e)}, common {len(common)}, "
              f"geomean enabled/plain {gm([e[n].mean() / p[n].mean() for n in common]):.4f}")
        missing = sorted((set(p) ^ set(e)) | (set(names) - set(p) - set(e)))
        if missing:
            print(f"   missing in one or both: {', '.join(missing)}")
    best = []
    for n, plains, ratios in rows:
        vals = [v for k, v in ratios.items() if k != "ports"] or list(ratios.values())
        if vals:
            best.append(gm(vals))
    print(f"all benchmarks, geomean over replicates (ports re-run where the replicates lack it): "
          f"{gm(best):.4f} over {len(best)}")
    both = [(n, ratios) for n, _, ratios in rows if "A" in ratios and "B" in ratios]
    up = [(n, r) for n, r in both if r["A"] > 1.02 and r["B"] > 1.02]
    down = [(n, r) for n, r in both if r["A"] < 0.98 and r["B"] < 0.98]
    print(f"slower by > 2% in both replicates ({len(up)}): " + ", ".join(f"{n} {r['A']:.3f}/{r['B']:.3f}" for n, r in sorted(up, key=lambda x: -x[1]['A'])))
    print(f"faster by > 2% in both replicates ({len(down)}): " + ", ".join(f"{n} {r['A']:.3f}/{r['B']:.3f}" for n, r in sorted(down, key=lambda x: x[1]['A'])))
    ab = sorted(r["B"] / r["A"] for _, r in both)
    if ab:
        print(f"replicate agreement: ratio B/A median {statistics.median(ab):.4f}, "
              f"p05 {ab[len(ab) // 20]:.3f}, p95 {ab[len(ab) * 19 // 20]:.3f}")
    print()
    print("| benchmark | plain wall time (mean of runs) | ratio A | ratio B | ratio ports |")
    print("|---|---:|---:|---:|---:|")
    for n, plains, ratios in rows:
        pt = fmt_t(statistics.fmean(plains)) if plains else "failed"
        cell = lambda k: f"{ratios[k]:.3f}" if k in ratios else "—"  # noqa: E731
        print(f"| {n} | {pt} | {cell('A')} | {cell('B')} | {cell('ports')} |")


if __name__ == "__main__":
    main()
