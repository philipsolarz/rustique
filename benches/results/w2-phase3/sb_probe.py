"""Does Rustique cause the non-kernel scoreboard cells that read > 1.05 alone? (Phase 0 §7.2)

    python sb_probe.py run --cells 'tuple/for v in x/int/10000;...' --cpus 2,10,20,30 -o probe.json
    python sb_probe.py one --cell KEY --mode MODE --loops L [--values 5]     (one process; JSON on stdout)

(cwd = the repository root, in a venv with Rustique.) The scoreboard (benches/scoreboard.py)
times a cell's statement in a pyperf process that has imported `rustique` (the "builtin"
side) and in one that also called `rustique.enable()` (the "enabled" side). Phase 0 found ten
cells of code no target changes that stay 1.06-1.22 on the enabled side when re-measured
alone. This probe separates the possible causes, with the same statement and inputs as the
board (`make_env`, `build_function`), each mode in its own fresh processes:

    stub       `rustique` replaced by an empty module before the scoreboard is imported:
               the extension is never loaded (plain CPython)
    stub2      the same again: the plain-vs-plain control (process-to-process spread)
    imported   the real `import rustique` (the extension loaded, nothing enabled): the board's
               builtin side, and what an installed-but-not-enabled process has once imported
    enabled    `rustique.enable()` too: the board's enabled side

Heap offsets (`--offsets`, core's pool-boundary finding, sb_heap.py): stub, imported and
enabled also run with k junk objects of each of nine small size classes allocated before the
timed region; the ratios of the minima over the offsets are reported next to the defaults.

Wall time: `--procs` processes per mode (modes interleaved, order rotating), each `--values`
timed values of `loops` iterations after one warm-up value (gc disabled around each value, as
the board); per-op time = median over processes of each process's median. Hardware counters:
`perf stat` on whole processes at `loops` and 2 x `loops` (`--counter-procs` each); per-op
events = (median at 2L - median at L) / L, so setup and teardown cancel.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import gc
import importlib.util
import json
import os
import queue
import statistics
import subprocess
import sys
import time
import types

REPO = os.getcwd()
SB = os.path.join(REPO, "benches", "scoreboard.py")
EVENTS = "instructions,cycles,branch-misses,dTLB-load-misses,cache-misses,page-faults"  # 6 events: no multiplexing on this box
MODES = ("stub", "stub2", "imported", "enabled")


def load_sb(stub: bool):
    if stub:
        sys.modules["rustique"] = types.ModuleType("rustique")  # no extension, no enable
    spec = importlib.util.spec_from_file_location("scoreboard", SB)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["scoreboard"] = mod
    spec.loader.exec_module(mod)
    return mod


def find_cell(sb, key: str):
    for c in sb.all_cells():
        if c.key == key:
            return c
    raise SystemExit(f"no cell {key!r}")


def cmd_one(a) -> None:
    sb = load_sb(a.mode.startswith("stub"))
    impl = "builtin"
    if a.mode == "enabled":
        import rustique

        rustique.enable()
        impl = "enabled"
    cell = find_cell(sb, a.cell)
    env = sb.make_env(cell.type, cell.storage, cell.n, impl)
    fn = sb.build_function(cell, impl, env)
    # heap offset: k junk objects of each of nine small size classes, kept alive (sb_heap.py)
    _junk = [bytearray(s) for s in (8, 24, 40, 56, 88, 120, 184, 248, 504) for _ in range(a.offset)]
    ext = [m for m in sys.modules if "rustique" in m and m != "rustique"]
    vals = []
    for k in range(a.values + 1):
        gc.collect()
        gc.disable()
        try:
            dt = fn(a.loops, time.perf_counter, lambda t0: time.perf_counter() - t0)
        finally:
            gc.enable()
        if k:  # the first value warms up
            vals.append(dt / (a.loops * cell.op.inner) * 1e9)
    print(json.dumps({"cell": a.cell, "mode": a.mode, "loops": a.loops, "inner": cell.op.inner, "offset": a.offset,
                      "ns": vals, "extension_loaded": ext, "junk": len(_junk)}))


def one(py: str, cpu: int, cell: str, mode: str, loops: int, values: int, perf: bool = False, offset: int = 0) -> dict:
    cmd = [py, os.path.abspath(__file__), "one", "--cell", cell, "--mode", mode, "--loops", str(loops),
           "--values", str(values), "--offset", str(offset)]
    if perf:
        cmd = ["perf", "stat", "-x", ";", "-e", EVENTS, "--", *cmd]
    r = subprocess.run(["taskset", "-c", str(cpu), *cmd], capture_output=True, text=True, cwd=REPO)
    if r.returncode:
        raise RuntimeError(f"{cell} {mode}: {r.stderr[-1500:]}")
    d = json.loads(r.stdout.strip().splitlines()[-1])
    if perf:
        ev = {}
        for line in r.stderr.splitlines():
            parts = line.split(";")
            if len(parts) > 3 and parts[0].replace(".", "").isdigit():
                ev[parts[2]] = float(parts[0])
        d["events"] = ev
    return d


def probe_cell(py: str, cpu: int, cell: str, a) -> dict:
    t0 = time.time()
    # calibrate: one value of about --target seconds in the board's builtin mode
    loops = 1
    while True:
        d = one(py, cpu, cell, "imported", loops, 1)
        per = statistics.median(d["ns"]) * 1e-9 * d["inner"]
        if per * loops >= a.target / 4 or loops > 1e8:
            loops = max(1, int(a.target / max(per, 1e-12)))
            break
        loops *= 10
    offs = [int(x) for x in a.offsets.split(",")]
    wall = {m: [] for m in MODES}
    woff = {m: {k: [] for k in offs} for m in ("stub", "imported", "enabled")}
    for r in range(a.procs):
        order = MODES[r % len(MODES):] + MODES[: r % len(MODES)]
        for m in order:
            for k in (offs if m in woff else [0]):
                d = one(py, cpu, cell, m, loops, a.values, offset=k)
                if m in woff:
                    woff[m][k].append(statistics.median(d["ns"]))
                if k == 0:
                    wall[m].append(statistics.median(d["ns"]))
                if m.startswith("stub") and d["extension_loaded"]:
                    raise RuntimeError(f"stub mode loaded {d['extension_loaded']}")
    counters = {}
    for m in ("stub", "imported", "enabled"):
        runs = {1: [], 2: []}
        for _ in range(a.counter_procs):
            for f in (1, 2):
                runs[f].append(one(py, cpu, cell, m, loops * f, a.values, perf=True)["events"])
        per = {}
        for ev in runs[1][0]:
            v1 = statistics.median(x.get(ev, float("nan")) for x in runs[1])
            v2 = statistics.median(x.get(ev, float("nan")) for x in runs[2])
            per[ev] = (v2 - v1) / (loops * (a.values + 1)) / d["inner"]  # per statement executed
        counters[m] = per
    med = {m: statistics.median(v) for m, v in wall.items()}
    by_off = {m: {k: statistics.median(v) for k, v in d_.items()} for m, d_ in woff.items()}
    mins = {m: min(v.values()) for m, v in by_off.items()}
    res = {"cell": cell, "cpu": cpu, "loops": loops, "wall_ns": wall, "median_ns": med,
           "ratios": {"stub2/stub (control)": med["stub2"] / med["stub"],
                      "imported/stub (extension loaded)": med["imported"] / med["stub"],
                      "enabled/imported (the board's ratio)": med["enabled"] / med["imported"],
                      "enabled/stub": med["enabled"] / med["stub"],
                      "min over heap offsets: enabled/imported": mins["enabled"] / mins["imported"],
                      "min over heap offsets: enabled/stub": mins["enabled"] / mins["stub"]},
           "median_ns_by_offset": by_off,
           "counters_per_op": counters, "elapsed_s": time.time() - t0}
    return res


def cmd_run(a) -> None:
    cells = list(dict.fromkeys(c for c in a.cells.split(";") if c))
    cpus: queue.Queue[int] = queue.Queue()
    for c in a.cpus.split(","):
        cpus.put(int(c))
    out = []

    def work(cell: str) -> dict:
        cpu = cpus.get()
        try:
            return probe_cell(sys.executable, cpu, cell, a)
        finally:
            cpus.put(cpu)

    with cf.ThreadPoolExecutor(len(a.cpus.split(","))) as ex:
        for res in ex.map(work, cells):
            out.append(res)
            r = res["ratios"]
            c = res["counters_per_op"]
            print(f"{res['cell']:40s} cpu {res['cpu']:2d}  " + "  ".join(f"{k} {v:.3f}" for k, v in r.items()), flush=True)
            for m in ("stub", "imported", "enabled"):
                print(f"{'':40s}   {m:9s} " + "  ".join(f"{k} {v:,.2f}" for k, v in c[m].items()), flush=True)
    with open(a.o, "w") as f:
        json.dump({"python": sys.version, "events": EVENTS, "procs": a.procs, "values": a.values, "rows": out}, f, indent=1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    o = sub.add_parser("one")
    o.add_argument("--cell", required=True)
    o.add_argument("--mode", choices=MODES, required=True)
    o.add_argument("--loops", type=int, required=True)
    o.add_argument("--values", type=int, default=5)
    o.add_argument("--offset", type=int, default=0)
    r = sub.add_parser("run")
    r.add_argument("--cells", required=True, help="cell keys (type/op/storage/n), ';'-separated")
    r.add_argument("--cpus", required=True)
    r.add_argument("--procs", type=int, default=10)
    r.add_argument("--values", type=int, default=5)
    r.add_argument("--counter-procs", type=int, default=3)
    r.add_argument("--target", type=float, default=0.1, help="seconds per timed value")
    r.add_argument("--offsets", default="0,1,2,3", help="heap offsets for stub/imported/enabled (0 = the default)")
    r.add_argument("-o", required=True)
    a = ap.parse_args()
    cmd_one(a) if a.cmd == "one" else cmd_run(a)


if __name__ == "__main__":
    main()
