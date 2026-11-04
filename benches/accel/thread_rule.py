"""The 8-thread rule (docs/ACCELERATION.md §7.4, DECISIONS §10.9): on a free-threaded build,
CPU time per call at 8 threads with the kernel <= 1.05x CPython's original at 8 threads, for
per-thread (`local`) and shared (`shared`) inputs. A reusable harness for every target's
benchmarks, built on benches/ft/kernel_scaling.py's cells (weak scaling: every thread makes the
same K calls from a barrier; CPU time = the workers' `thread_time()`, which counts spinning on
locks and cache-line bouncing but not other processes on the CPU set).

    python benches/accel/thread_rule.py --targets json --cell "json.dumps" \\
        [--cell "NAME=SETUP::STATEMENT" ...] [-t 8] [-r 5] [--secs 0.15] [-o out.json]

Each side runs in a fresh process (plain: nothing enabled; enabled: `_accel.enable(only=
targets)` validated eagerly before any import). `--cell` takes a cell name of
kernel_scaling.CELLS or `NAME=SETUP::STATEMENT` (`;` separates statements). Prints the ratio
per cell and mode and exits non-zero when one exceeds `--limit` (1.05). Run on 3.15.1t, the
workspace pinned to one L3 domain.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "ft"))


def parse_cell(spec: str) -> tuple[str, str, str]:
    import kernel_scaling as ks

    if "=" in spec and "::" in spec:
        name, rest = spec.split("=", 1)
        setup, stmt = rest.split("::", 1)
        return name, setup, stmt
    setup, stmt = ks.CELLS[spec]
    return spec, setup, stmt


def child(a: argparse.Namespace) -> None:
    """One side, in this process: CPU ns per call at each thread count and mode."""
    import kernel_scaling as ks

    if a.targets:
        from rustique import _rustique

        A = _rustique._accel
        A._set_validation("eager")
        A.enable(a.targets.split(","))
    out = []
    threads = [int(x) for x in a.threads.split(",")]
    for spec in a.cell:
        name, setup, stmt = parse_cell(spec)
        k = 200
        while True:
            dt = ks.run_cell(setup, stmt, 1, k, False)
            if dt >= a.secs / 4 or k > 50_000_000:
                break
            k *= 4
        k = max(1, int(k * a.secs / dt))
        for mode in ("local", "shared"):
            row = {"cell": name, "mode": mode, "k": k, "cpu_ns": {}}
            for t in threads:
                cpus = []
                for _ in range(a.r):
                    ks.run_cell(setup, stmt, t, k, mode == "shared")
                    cpus.append(ks.run_cell.cpu / (t * k) * 1e9)
                row["cpu_ns"][t] = statistics.median(cpus)
            out.append(row)
    print(json.dumps(out))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--targets", default="", help="groups or targets to enable (empty: plain)")
    ap.add_argument("--cell", action="append", default=[])
    ap.add_argument("-t", "--threads", default="1,8")
    ap.add_argument("-r", type=int, default=5)
    ap.add_argument("--secs", type=float, default=0.15)
    ap.add_argument("--limit", type=float, default=1.05)
    ap.add_argument("-o")
    ap.add_argument("--child", action="store_true", help=argparse.SUPPRESS)
    a = ap.parse_args()
    if a.child:
        child(a)
        return
    if not a.cell or not a.targets:
        sys.exit("--targets and at least one --cell are required")
    if getattr(sys, "_is_gil_enabled", lambda: True)():
        print("note: GIL build; the rule is measured on free-threaded builds", file=sys.stderr)
    base = [sys.executable, os.path.abspath(__file__), "--child", "-t", a.threads, "-r", str(a.r), "--secs", str(a.secs)]
    for c in a.cell:
        base += ["--cell", c]
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    sides = {}
    for side, extra in (("plain", []), ("enabled", ["--targets", a.targets])):
        r = subprocess.run(base + extra, capture_output=True, text=True, env=env)
        if r.returncode:
            sys.exit(f"{side} side failed:\n{r.stderr[-3000:]}")
        sides[side] = json.loads(r.stdout.strip().splitlines()[-1])
    worst = 0.0
    rows = []
    t = max(int(x) for x in a.threads.split(","))
    print(f"{'cell':28} {'mode':6} {'plain ns':>9} {'enabled ns':>10} {'ratio@' + str(t):>9}")
    for p, e in zip(sides["plain"], sides["enabled"]):
        pt, et = p["cpu_ns"][str(t)] if str(t) in p["cpu_ns"] else p["cpu_ns"][t], e["cpu_ns"].get(str(t), e["cpu_ns"].get(t))
        ratio = et / pt
        worst = max(worst, ratio)
        rows.append({"cell": p["cell"], "mode": p["mode"], "plain": pt, "enabled": et, "ratio": ratio})
        flag = "" if ratio <= a.limit else "  > limit"
        print(f"{p['cell']:28} {p['mode']:6} {pt:9.0f} {et:10.0f} {ratio:9.3f}{flag}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"threads": t, "rows": rows, "sides": sides}, f, indent=1)
    sys.exit(0 if worst <= a.limit else 1)


if __name__ == "__main__":
    main()
