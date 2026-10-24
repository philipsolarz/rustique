"""Per-element hot paths of `rustique.List` against builtin baselines.

Honest methodology (ARCHITECTURE §1.8): every repetition builds fresh inputs, the GC is
disabled only while timing, and every result is checked against the builtin result.
Each case reports best-of-N and median; ratios use the best times.

Two baselines:
- `list`: the builtin (often specialised by the interpreter, holds ready-made objects);
- `array`: `array.array` with the same values -- a C container with native storage that
  must materialise a Python object per element read, i.e. the floor for an extension type.

Usage:
    python benches/list_hotpaths.py [--n 1000000] [--reps 7] [--only name,name] [--json out.json]
"""

from __future__ import annotations

import argparse
import array
import gc
import json
import random
import statistics
import sys
import time

from rustique import List


def ints(n: int, rep: int) -> list[int]:
    rng = random.Random(rep)
    return [rng.randrange(-(10**9), 10**9) for _ in range(n)]


def small_ints(n: int, rep: int) -> list[int]:
    rng = random.Random(rep)
    return [rng.randrange(0, 1000) for _ in range(n)]


def floats(n: int, rep: int) -> list[float]:
    rng = random.Random(rep)
    return [rng.random() * 1e6 for _ in range(n)]


def strs(n: int, rep: int) -> list[str]:
    rng = random.Random(rep)
    return [f"s{rng.randrange(10**6)}" for _ in range(n)]


def loop_index(x):
    s = 0
    for i in range(len(x)):
        s += x[i]
    return s


def loop_index_neg(x):
    s = 0
    for i in range(-len(x), 0):
        s += x[i]
    return s


def loop_iter(x):
    s = 0
    for v in x:
        s += v
    return s


def loop_iter_str(x):
    n = 0
    for v in x:
        n += len(v)
    return n


def loop_append(x, n):
    app = x.append
    for i in range(n):
        app(i)
    return x


def loop_append_float(x, n):
    app = x.append
    for i in range(n):
        app(i * 0.5)
    return x


TYPECODE = {ints: "q", small_ints: "q", floats: "d"}

# name -> (data generator, operation, kind)
#   kind "read": op(container) on a container built from fresh data (not timed);
#   kind "build": op(type, data) constructs; the builtin side uses `list`.
#   kind "append": op(empty container, n).
CASES = {
    "x[i] int": (ints, loop_index, "read"),
    "x[-i] int": (ints, loop_index_neg, "read"),
    "x[i] small int": (small_ints, loop_index, "read"),
    "x[i] float": (floats, loop_index, "read"),
    "for v in x int": (ints, loop_iter, "read"),
    "for v in x float": (floats, loop_iter, "read"),
    "for v in x str": (strs, loop_iter_str, "read"),
    "list(x) int": (ints, list, "read"),
    "list(x) float": (floats, list, "read"),
    "list(x) str": (strs, list, "read"),
    "tuple(x) int": (ints, tuple, "read"),
    "to_list() int": (ints, "to_list", "read"),
    "to_list() float": (floats, "to_list", "read"),
    "to_list() str": (strs, "to_list", "read"),
    "append int": (None, loop_append, "append"),
    "append float": (None, loop_append_float, "append"),
    "build list[int]": (ints, None, "build"),
    "build list[float]": (floats, None, "build"),
    "build list[str]": (strs, None, "build"),
    "build tuple[int]": (ints, tuple, "build"),
    "build generator": (ints, "gen", "build"),
    "build range": (None, "range", "build"),
}


def timed(fn, *args):
    gc.collect()
    gc.disable()
    try:
        t0 = time.perf_counter()
        out = fn(*args)
        t1 = time.perf_counter()
    finally:
        gc.enable()
    return t1 - t0, out


def as_builtin(out):
    if isinstance(out, List):
        return out.to_list()
    if isinstance(out, array.array):
        return out.tolist()
    return out


def run_case(name: str, n: int, reps: int) -> dict:
    gen, op, kind = CASES[name]
    sides = {"list": [], "List": [], "array": []}
    for rep in range(reps):
        results = {}
        if kind == "read":
            data = gen(n, rep)
            containers = {"list": list(data), "List": List(data)}
            if gen in TYPECODE:
                containers["array"] = array.array(TYPECODE[gen], data)
            for side, c in containers.items():
                if op == "to_list":
                    fn = {"list": list.copy, "List": List.to_list, "array": array.array.tolist}[side]
                else:
                    fn = op
                t, out = timed(fn, c)
                sides[side].append(t)
                results[side] = as_builtin(out)
            del containers, data
        elif kind == "append":
            ctors = {"list": list, "List": List}
            if op is loop_append:
                ctors["array"] = lambda: array.array("q")
            else:
                ctors["array"] = lambda: array.array("d")
            for side, ctor in ctors.items():
                t, out = timed(op, ctor(), n)
                sides[side].append(t)
                results[side] = as_builtin(out)
        else:  # build
            for side in ("list", "List"):
                cls = list if side == "list" else List
                if op == "range":
                    src = range(rep, n + rep)
                elif op == "gen":
                    src = (i * 3 for i in range(rep, n + rep))
                else:
                    src = gen(n, rep)
                    if op is tuple:
                        src = tuple(src)
                t, out = timed(cls, src)
                sides[side].append(t)
                results[side] = as_builtin(out)
                del out, src
        expected = results["list"]
        for side, got in results.items():
            if side == "list":
                continue
            if isinstance(expected, (tuple, list)) and not isinstance(got, type(expected)):
                got = type(expected)(got)
            assert got == expected, f"{name}: {side} result differs from builtin"
    row = {"case": name, "n": n}
    for side, ts in sides.items():
        if ts:
            row[side] = {"best": min(ts), "median": statistics.median(ts)}
    return row


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--n", type=int, default=1_000_000)
    ap.add_argument("--reps", type=int, default=7)
    ap.add_argument("--only", default="", help="comma-separated substrings of case names")
    ap.add_argument("--json", default="", help="write the rows to this file")
    args = ap.parse_args()
    only = [s for s in args.only.split(",") if s]
    gil = getattr(sys, "_is_gil_enabled", lambda: True)()
    print(f"python {sys.version.split()[0]} gil={gil} n={args.n} reps={args.reps}")
    header = f"{'case':20} {'list ms':>9} {'List ms':>9} {'med':>8} {'array ms':>9} {'ns/elem':>8} {'/list':>6} {'/array':>7}"
    print(header)
    rows = []
    for name in CASES:
        if only and not any(o in name for o in only):
            continue
        row = run_case(name, args.n, args.reps)
        rows.append(row)
        b, r = row["list"]["best"], row["List"]["best"]
        a = row.get("array", {}).get("best")
        print(
            f"{name:20} {b * 1e3:9.2f} {r * 1e3:9.2f} {row['List']['median'] * 1e3:8.2f} "
            f"{(a * 1e3 if a else float('nan')):9.2f} {r / args.n * 1e9:8.2f} {r / b:6.2f} "
            f"{(r / a if a else float('nan')):7.2f}",
            flush=True,
        )
    if args.json:
        with open(args.json, "w") as f:
            json.dump({"python": sys.version, "gil": gil, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
