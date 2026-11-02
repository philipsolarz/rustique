"""Thread scaling of single operations (Rustique kernels vs CPython's originals).

    python benches/ft/kernel_scaling.py [--enable] [-t 1,2,4,8] [-r 3] [--cells a,b] [--secs 0.15] [-o out.json]
    python benches/ft/kernel_scaling.py table plain.json enabled.json

Weak scaling: every thread runs the same number of calls K (calibrated so that one thread
takes ~`--secs`), all threads start at a barrier, and the cell's time is the wall time until
the last thread finishes (median of `-r` repeats). Efficiency = time(1 thread) / time(T
threads): 1.0 is perfect scaling, 1/T is fully serialized. Each cell has two input modes:
`local` (every thread builds its own inputs, so CPython's biased reference counting stays on
the owner's fast path) and `shared` (one input object built by the main thread and used by
every thread: atomic reference counts and, for containers, per-object locks). Run once
plain and once with `--enable` (a fresh process each, `rustique.enable()` before any import)
on the same CPU set, then `table` compares. The fallback cells (`*_fb`) call a kernel with
inputs it sends to CPython's original while the main thread waits in `join()`.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import sysconfig
import threading
import time

# name: (setup, statement). Setup runs in the thread that owns the inputs (main thread for
# `shared`, each worker for `local`); the statement runs K times per thread.
CELLS = {
    "json.dumps": ("import json; x = {'id': 7, 'name': 'Ana Silva', 'tags': ['a', 'b'], 'score': 3.25, 'ok': True, 'meta': {'k': None}}",
                   "json.dumps(x)"),
    "json.loads": ("import json; x = '{\"id\": 7, \"name\": \"Ana Silva\", \"tags\": [\"a\", \"b\"], \"score\": 3.25, \"ok\": true, \"meta\": {\"k\": null}}'",
                   "json.loads(x)"),
    "copy.deepcopy": ("import copy; x = {'a': [1, 2, {'b': 'c'}], 'd': ('e', 3.5), 'f': {'g': [None, True]}}",
                      "copy.deepcopy(x)"),
    "deepcopy(frozenset)": ("import copy; x = {'tags': frozenset({'a', 'b', 'c'}), 'n': [1, 2]}", "copy.deepcopy(x)"),
    "dataclasses.asdict": ("import dataclasses\n@dataclasses.dataclass\nclass P:\n    x: int\n    y: list\n    z: str = 'q'\nx = P(1, [1, 2, 3])\nasdict = dataclasses.asdict",
                           "asdict(x)"),
    "datetime.strptime": ("from datetime import datetime; s = '2026-11-02 18:21:47'", "datetime.strptime(s, '%Y-%m-%d %H:%M:%S')"),
    "datetime.isoformat": ("from datetime import datetime; x = datetime(2026, 11, 2, 18, 21, 47, 123456)", "x.isoformat()"),
    "date.strftime": ("from datetime import date; x = date(2026, 11, 2)", "x.strftime('%Y-%m-%d %a')"),
    "posixpath.join": ("import posixpath", "posixpath.join('/srv/app', 'static', 'css', 'site.css')"),
    "posixpath.join_fb": ("import posixpath, pathlib; p = pathlib.PurePosixPath('/srv/app')", "posixpath.join(p, 'static')"),
    "fnmatch.fnmatch": ("import fnmatch", "fnmatch.fnmatch('src/pkg/module.py', '*.py')"),
    "pathlib.join": ("import pathlib; p = pathlib.PurePosixPath('/srv/app')", "p / 'static' / 'x.css'"),
    "logging.format": ("import logging; f = logging.Formatter('%(asctime)s %(levelname)s %(name)s: %(message)s'); "
                       "r = logging.LogRecord('svc.api', 20, '/srv/app.py', 10, 'user %s logged in', ('ana',), None)",
                       "f.format(r)"),
    "logging.LogRecord": ("import logging; L = logging.getLogger('svc.api')",
                          "L.makeRecord('svc.api', 20, '/srv/app.py', 10, 'user %s logged in', ('ana',), None)"),
    "urllib.quote": ("from urllib.parse import quote", "quote('/a b/c?d=é&x=1')"),
    "urllib.urlsplit": ("from urllib.parse import urlsplit", "urlsplit('https://example.com:8443/a/b?q=1&r=2#frag')"),
    "random.randint": ("import random", "random.randint(1, 1000)"),
    "Random().randint": ("import random; R = random.Random(5)", "R.randint(1, 1000)"),
    "textwrap.wrap": ("import textwrap; s = 'The quick brown fox jumps over the lazy dog ' * 3", "textwrap.wrap(s, 30)"),
    "statistics.mean": ("import statistics; x = [1.5, 2.25, 3.0, 4.75, 5.5, 6.0, 7.25, 8.5]", "statistics.mean(x)"),
    "float.repr": ("x = 1234.5678", "repr(x)"),
    "sorted(50 ints)": ("import random; x = random.Random(1).sample(range(1000), 50)", "sorted(x)"),
    "list.sort(50 str)": ("import random; x = [str(i) for i in random.Random(1).sample(range(1000), 50)]", "y = x[:]; y.sort()"),
    "x in list(100)": ("x = list(range(100))", "99 in x"),
    "list.index(100)": ("x = list(range(100))", "x.index(99)"),
    "x in tuple(8)": ("x = ('GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS', 'TRACE')", "'TRACE' in x"),
    "sum(list 100)": ("x = [i * 0.5 for i in range(100)]", "sum(x)"),
    "max(list 100)": ("x = [i * 0.5 for i in range(100)]", "max(x)"),
    "min(dict 50)": ("x = {f'k{i}': i for i in range(50)}", "min(x)"),
    # CPython serialization points with no Rustique kernel (candidate evidence)
    "functools.cache hit": ("import functools\n@functools.cache\ndef f(x):\n    return x * 2\nf(3)", "f(3)"),
    "lru_cache(128) hit": ("import functools\n@functools.lru_cache(maxsize=128)\ndef f(x):\n    return x * 2\nf(3)", "f(3)"),
    "re.match (module cache)": ("import re", "re.match(r'\\d+-\\w+', '123-abc')"),
    "isinstance(d, Mapping)": ("import collections.abc as A; x = {'a': 1}", "isinstance(x, A.Mapping)"),
    "isinstance(f, Number)": ("import numbers; x = 1.5", "isinstance(x, numbers.Number)"),
    "isinstance(d, dict) (ref)": ("x = {'a': 1}", "isinstance(x, dict)"),
    "Pattern.match (compiled)": ("import re; p = re.compile(r'\\d+-\\w+')", "p.match('123-abc')"),
}


def build_name() -> str:
    v = sys.version_info
    return f"{v.major}.{v.minor}.{v.micro}" + ("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else "")


def make(setup: str, stmt: str):
    ns: dict = {}
    exec(setup, ns)
    src = "def _loop(n):\n    for _ in range(n):\n" + "".join(f"        {s.strip()}\n" for s in stmt.split(";"))
    exec(src, ns)
    return ns["_loop"]


def run_cell(setup: str, stmt: str, t: int, k: int, shared: bool) -> float:
    shared_loop = make(setup, stmt) if shared else None
    loops: list = [None] * t
    barrier = threading.Barrier(t + 1)
    done = [0.0] * t
    cpu = [0.0] * t

    def worker(i: int) -> None:
        loops[i] = shared_loop if shared else make(setup, stmt)
        loops[i](10)  # warm (specialize) before timing
        barrier.wait()
        c0 = time.thread_time()
        loops[i](k)
        cpu[i] = time.thread_time() - c0
        done[i] = time.perf_counter()

    ts = [threading.Thread(target=worker, args=(i,)) for i in range(t)]
    for th in ts:
        th.start()
    barrier.wait()
    t0 = time.perf_counter()
    for th in ts:
        th.join()
    run_cell.cpu = sum(cpu)  # CPU seconds of the workers (spinning counts, sleeping does not)
    return max(done) - t0


def cmd_run(a: argparse.Namespace) -> None:
    if a.enable:
        import rustique

        rustique.enable()
    threads = [int(x) for x in a.threads.split(",")]
    cells = (a.cells.split("|") if "|" in a.cells else a.cells.split(",")) if a.cells else list(CELLS)
    out = {"build": build_name(), "enabled": a.enable, "gil": getattr(sys, "_is_gil_enabled", lambda: True)(),
           "affinity": sorted(os.sched_getaffinity(0)), "cells": []}
    for name in cells:
        setup, stmt = CELLS[name]
        # calibrate K at one thread
        k = 200
        while True:
            dt = run_cell(setup, stmt, 1, k, False)
            if dt >= a.secs / 4 or k > 50_000_000:
                break
            k *= 4
        k = max(1, int(k * a.secs / dt))
        for mode in ("local", "shared"):
            row = {"cell": name, "mode": mode, "k": k, "times": {}}
            row["cpu"] = {}
            for t in threads:
                walls, cpus = [], []
                for _ in range(a.r):
                    walls.append(run_cell(setup, stmt, t, k, mode == "shared"))
                    cpus.append(run_cell.cpu)
                row["times"][t] = statistics.median(walls)
                row["cpu"][t] = statistics.median(cpus) / (t * k) * 1e9  # CPU ns per call
            base = row["times"][threads[0]]
            row["ns_per_call_1t"] = base / k * 1e9
            row["efficiency"] = {t: base / row["times"][t] for t in threads}
            out["cells"].append(row)
            print(f"{out['build']:8s} {'en' if a.enable else 'pl'} {name:22s} {mode:6s} {row['ns_per_call_1t']:8.0f} ns/call  "
                  + "  ".join(f"eff{t}={row['efficiency'][t]:.2f}" for t in threads)
                  + "  cpu ns/call " + " ".join(f"{row['cpu'][t]:.0f}" for t in threads), flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump(out, f, indent=1)


def cmd_table(a: argparse.Namespace) -> None:
    """Compare per-call times (time / K: each process calibrates its own K)."""
    runs = [json.load(open(p)) for p in a.files]
    plain = next(r for r in runs if not r["enabled"])
    en = next((r for r in runs if r["enabled"]), None)
    key = lambda r: {(c["cell"], c["mode"]): c for c in r["cells"]}
    P, E = key(plain), key(en) if en else {}
    threads = sorted(int(t) for t in plain["cells"][0]["times"])
    t1, tmax = threads[0], threads[-1]
    ns = lambda c, t: c["times"][str(t)] / c["k"] * 1e9  # wall ns per call per thread
    eff = lambda c: c["times"][str(t1)] / c["times"][str(tmax)]
    print(f"build {plain['build']}; ns = wall time per call per thread; eff = time(1 thread) / time({tmax} threads) "
          f"with every thread doing the same work (1.00 = perfect scaling, {1 / tmax:.2f} = serialized); "
          f"enabled/plain < 1 = Rustique faster")
    print()
    cpu = lambda c, t: c["cpu"][str(t)] if "cpu" in c else float("nan")
    print("CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) "
          f"raises it at {tmax} threads; a lock that parks the thread raises wall time only. Wall time is "
          "sensitive to other processes on the CPU set; CPU time is not.")
    print()
    print(f"| cell | inputs | plain ns ({t1} thr) | enabled/plain ({t1} thr) | plain CPU ns x{tmax}/x{t1} | "
          f"enabled CPU ns x{tmax}/x{t1} | enabled/plain CPU ({tmax} thr) | plain eff | enabled eff | enabled/plain wall ({tmax} thr) |")
    print("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for (cell, mode), p in P.items():
        e = E.get((cell, mode))
        if not e:
            continue
        print(f"| {cell} | {mode} | {ns(p, t1):.0f} | {ns(e, t1) / ns(p, t1):.3f} | "
              f"{cpu(p, tmax) / cpu(p, t1):.2f} | {cpu(e, tmax) / cpu(e, t1):.2f} | {cpu(e, tmax) / cpu(p, tmax):.3f} | "
              f"{eff(p):.2f} | {eff(e):.2f} | {ns(e, tmax) / ns(p, tmax):.3f} |")


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "table":
        ap = argparse.ArgumentParser()
        ap.add_argument("cmd")
        ap.add_argument("files", nargs="+")
        cmd_table(ap.parse_args())
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--enable", action="store_true")
    ap.add_argument("-t", "--threads", default="1,2,4,8")
    ap.add_argument("-r", type=int, default=3)
    ap.add_argument("--cells")
    ap.add_argument("--secs", type=float, default=0.15)
    ap.add_argument("-o")
    cmd_run(ap.parse_args())


if __name__ == "__main__":
    main()
