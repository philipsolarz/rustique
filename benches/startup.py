"""Startup cost of Rustique (never-slower includes startup, docs/ACCELERATION.md §5.2).

Cells (each a whole `python -c ...` process):
  pass        python -c pass                                (baseline)
  import      python -c "import rustique"
  enable      python -c "import rustique; rustique.enable()"
  ext         python -c "import rustique._rustique"        (the extension alone)

Two measurements:
  callgrind   total instructions of the whole process (deterministic; one run per cell),
              plus the share of the dynamic loader, libpython and the extension;
  wall        interleaved repeats (hyperfine-style), median and p10–p90 in ms.

Usage: python benches/startup.py [--wall N] [--no-callgrind] [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time

CELLS = {
    "pass": "pass",
    "ext": "import rustique._rustique",
    "import": "import rustique",
    "enable": "import rustique; rustique.enable()",
    "enable_quiet": "import rustique._rustique as r; r._accel.enable(quiet=True)",
}


def callgrind(code: str) -> dict[str, int]:
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        subprocess.run(
            ["valgrind", "--tool=callgrind", f"--callgrind-out-file={out}", "-q", sys.executable, "-c", code],
            check=True,
            env={**os.environ, "PYTHONHASHSEED": "0"},
        )
        r = subprocess.run(["callgrind_annotate", "--inclusive=no", "--threshold=100", out], capture_output=True, text=True)
    total = 0
    by_obj: dict[str, int] = {}
    for line in r.stdout.splitlines():
        line = line.strip()
        if line.endswith("PROGRAM TOTALS"):
            total = int(line.split()[0].replace(",", ""))
            continue
        parts = line.split(None, 2)
        if len(parts) >= 3 and parts[0].replace(",", "").isdigit() and ":" in parts[-1]:
            ir = int(parts[0].replace(",", ""))
            path = parts[-1].split(":", 1)[0].strip()
            obj = os.path.basename(path.split()[-1]) if path else "?"
            by_obj[obj] = by_obj.get(obj, 0) + ir
    return {"total": total, **dict(sorted(by_obj.items(), key=lambda kv: -kv[1])[:8])}


def faults(code: str, n: int = 5) -> dict[str, float]:
    """`perf stat` page faults (median of n runs)."""
    vals: dict[str, list[float]] = {}
    for _ in range(n):
        r = subprocess.run(["perf", "stat", "-x,", "-e", "page-faults,minor-faults,major-faults", sys.executable, "-c", code],
                           capture_output=True, text=True)
        for line in r.stderr.splitlines():
            parts = line.split(",")
            if len(parts) > 2 and parts[0].replace(".", "").isdigit():
                vals.setdefault(parts[2], []).append(float(parts[0]))
    return {k: statistics.median(v) for k, v in vals.items()}


def wall(n: int) -> dict[str, list[float]]:
    times: dict[str, list[float]] = {k: [] for k in CELLS}
    for _ in range(n):
        for k, code in CELLS.items():
            t = time.perf_counter()
            subprocess.run([sys.executable, "-c", code], check=True)
            times[k].append((time.perf_counter() - t) * 1e3)
    return times


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--wall", type=int, default=30)
    ap.add_argument("--no-callgrind", action="store_true")
    ap.add_argument("--faults", action="store_true")
    ap.add_argument("-o")
    a = ap.parse_args()
    out: dict[str, object] = {"python": sys.version}
    if not a.no_callgrind:
        cg = {k: callgrind(code) for k, code in CELLS.items()}
        base = cg["pass"]["total"]
        print("callgrind (whole process, Ir):")
        for k, v in cg.items():
            print(f"  {k:12s} {v['total']:>14,d}  {v['total'] / base:.3f}")
        out["callgrind"] = cg
    if a.wall:
        w = wall(a.wall)
        base = statistics.median(w["pass"])
        print(f"wall ({a.wall} interleaved runs, ms):")
        for k, v in w.items():
            q = statistics.quantiles(v, n=10)
            print(f"  {k:12s} {statistics.median(v):7.2f}  ({q[0]:.2f}–{q[-1]:.2f})  {statistics.median(v) / base:.3f}")
        out["wall"] = w
    if a.faults:
        f = {k: faults(code) for k, code in CELLS.items()}
        print("perf stat (median of 5):")
        for k, v in f.items():
            print(f"  {k:12s} " + "  ".join(f"{e}={n:.0f}" for e, n in v.items()))
        out["faults"] = f
    if a.o:
        with open(a.o, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
