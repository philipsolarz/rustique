"""Exhaustive-style identity check of the float core against CPython, in process.

Each job compares `rustique_core::float_repr` with `PyOS_double_to_string(x, 'r', 0,
Py_DTSF_ADD_DOT_0)` -- the function CPython's `float_repr` calls -- on a family of values
(see `_t_float._selfcheck`). Jobs run in parallel processes.

    python benches/targets/float_selfcheck.py [--scale 1.0] [-j 4] [-o out.json]
"""

import argparse
import json
import struct
import sys
import time
from concurrent.futures import ProcessPoolExecutor


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def jobs(scale):
    m = 1_000_000
    s = lambda n: max(1, int(n * scale))  # noqa: E731
    out = []
    for i in range(8):
        out.append(("bits", s(25 * m), 0x9E3779B97F4A7C15 + i))
    for i in range(2):
        out.append(("subnormal", s(25 * m), 0xD1B54A32D192ED03 + i))
    for i in range(4):
        out.append(("dyadic", s(25 * m), 0x94D049BB133111EB + i))
    for i in range(2):
        out.append(("decimal", s(10 * m), 0xBF58476D1CE4E5B9 + i))
    # Consecutive bit patterns across the layout cut-offs and the extremes.
    for x in (1e16, 1e-4, 1e-5, 1e15, 1.0, 2.0**53, 5e-324, 2.2250738585072014e-308, 1.7976931348623157e308):
        n = s(10 * m)
        start = max(0, bits(x) - n // 2)
        out.append(("sweep", n, start))
    out.append(("sweep", s(10 * m), 0))  # smallest subnormals
    return out


def run(job):
    from rustique._rustique import _accel

    _t_float = _accel._t_float

    fam, n, seed = job
    t0 = time.time()
    checked, bad, ex = _t_float._selfcheck(fam, n, seed)
    return {"family": fam, "n": n, "seed": seed, "checked": checked, "mismatches": bad, "examples": ex,
            "seconds": round(time.time() - t0, 1)}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--scale", type=float, default=1.0)
    p.add_argument("-j", type=int, default=4)
    p.add_argument("-o")
    a = p.parse_args()
    results = []
    with ProcessPoolExecutor(a.j) as ex:
        for r in ex.map(run, jobs(a.scale)):
            results.append(r)
            print(json.dumps(r), flush=True)
    total = sum(r["checked"] for r in results)
    bad = sum(r["mismatches"] for r in results)
    print(f"python {sys.version.split()[0]}: {total:,} values checked, {bad} mismatches")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "total": total, "mismatches": bad, "jobs": results}, f, indent=1)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
