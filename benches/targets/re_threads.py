"""CPU time per call of the `re` kernels at 1 and 8 threads (DECISIONS §10.9: at 8 threads on
3.15.1t, ≤ 1.05× CPython's original): each cell runs in fresh processes, plain and with the
group installed, interleaved; process CPU time (`time.process_time`) over all threads divided
by the calls; median of `-r` rounds.

    python benches/targets/re_threads.py [-t 8] [-r 5] [-n 20000]
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys

CHILD = r"""
import json, re, sys, threading, time
from re import _compiler
threads, n, stmt, mode = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]
if mode != "0":
    import rustique
if mode == "1":
    rustique.enable(only=["re"])
SMALL = r"(\d+)-(\d+)"
S = "abc 12-34 def"
def cb(m):
    return "#"
code = compile("for _ in range(n):\n    " + stmt, "<cell>", "exec")
def work():
    exec(code, {"re": re, "_compiler": _compiler, "SMALL": SMALL, "S": S, "cb": cb, "n": n})
work()  # warm-up (and first-call validation)
ts = [threading.Thread(target=work) for _ in range(threads)]
t0 = time.process_time()
for t in ts:
    t.start()
for t in ts:
    t.join()
print(json.dumps((time.process_time() - t0) / (threads * n)))
"""

CELLS = {
    "re.match hit": "re.match(SMALL, S)",
    "re.sub template hit": "re.sub(SMALL, r'\\2-\\1', S)",
    "re.compile hit": "re.compile(SMALL)",
    "re.sub callable (fallback)": "re.sub(SMALL, cb, S)",
    "re.sub callable + literal (fallback+hit)": "re.sub(SMALL, cb, S); re.sub(SMALL, '#', S)",
    "compile (no cache)": "_compiler.compile(SMALL, 0)",
}


def run(threads: int, n: int, stmt: str, mode: str) -> float:
    """mode "0": plain; "1": the group installed; "c": control (rustique imported, nothing
    enabled: the noise floor of the comparison)."""
    p = subprocess.run([sys.executable, "-c", CHILD, str(threads), str(n), stmt, mode],
                       capture_output=True, text=True, check=True)
    return json.loads(p.stdout)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("-t", type=int, default=8)
    ap.add_argument("-r", type=int, default=5)
    ap.add_argument("-n", type=int, default=20000)
    ap.add_argument("-o")
    ap.add_argument("--only")
    ap.add_argument("--control", action="store_true", help="compare plain with plain (noise floor)")
    a = ap.parse_args()
    res = {}
    for name, stmt in CELLS.items():
        if a.only and a.only not in name:
            continue
        n = a.n // 20 if "compile (no cache)" in name else a.n
        for threads in (1, a.t):
            plain, rq = [], []
            for _ in range(a.r):
                plain.append(run(threads, n, stmt, "0"))
                rq.append(run(threads, n, stmt, "c" if a.control else "1"))
            p, q = statistics.median(plain), statistics.median(rq)
            res[f"{name} x{threads}"] = {"plain_us": p * 1e6, "rq_us": q * 1e6, "ratio": q / p}
            print(f"{name:32s} threads={threads}  plain={p * 1e6:8.2f} us  rq={q * 1e6:8.2f} us  ratio={q / p:.3f}",
                  flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "cells": res}, f, indent=1)


if __name__ == "__main__":
    main()
