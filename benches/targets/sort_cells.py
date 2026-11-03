"""Callgrind cells of the sort family (`list.sort`, `sorted`, `min`, `max`): plain vs the
`builtins` + `containers` groups enabled, Ir per call (benches/targets/_callgrind.py
method: region-instrumented, fresh process per cell).

    python benches/targets/sort_cells.py sort [-j 6] [-k substr] [-o out.json]
    python benches/targets/sort_cells.py family      # reverse, in place, tuples, keys, ...
    python benches/targets/sort_cells.py minmax      # min/max on str/int/float, every size

`sort` is w2-floors' 90-cell grid (`docs/atlas/w2-floors.md` §6.3): int/float/str x
n in {2, 5, 20, 100, 1000} x {asc, desc, rand, halves, nearly, dups}, through `sorted`.
"""

from __future__ import annotations

import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import per_call  # noqa: E402

SETUP = r"""
import random
random.seed(1)
def mk(kind, n, order):
    if kind == "int": base = list(range(n))
    elif kind == "float": base = [i / 7 for i in range(n)]
    elif kind == "ustr": base = [f"ā{i:05d}" for i in range(n)]
    elif kind == "lstr": base = [f"key-{i:08d}-x" for i in range(n)]
    else: base = [f"k{i:05d}" for i in range(n)]
    if order == "asc": return base
    if order == "desc": return base[::-1]
    if order == "rand": s = base[:]; random.shuffle(s); return s
    if order == "halves": h = n // 2; return base[h:] + base[:h]
    if order == "nearly":
        s = base[:]
        for _ in range(max(1, n // 50)):
            i, j = random.randrange(n), random.randrange(n); s[i], s[j] = s[j], s[i]
        return s
    if order == "dups": return [base[i % 3] for i in range(n)] if n else []
"""
KINDS = ("int", "float", "str")
SIZES = (2, 5, 20, 100, 1000)
ORDERS = ("asc", "desc", "rand", "halves", "nearly", "dups")
SETUP += "".join(
    f"D_{k}_{n}_{o} = mk({k!r}, {n}, {o!r})\n" for k in KINDS + ("ustr", "lstr") for n in SIZES + (1, 3, 64) for o in ORDERS
)
SETUP += "T_int_20_rand = tuple(D_int_20_rand); T_str_100_rand = tuple(D_str_100_rand)\n"
SETUP += "pairs = [(w, i) for i, w in enumerate(D_str_20_rand)]\nS_int = set(D_int_100_rand)\n"
SETUP += "mixed = D_int_20_rand[:10] + [0.5] + D_int_20_rand[10:]\nbig = [2**70 + i for i in D_int_20_rand]\n"
SETUP += "nan = D_float_20_rand[:10] + [float('nan')] + D_float_20_rand[10:]\na, b = 'alpha', 'beta'\n"

SORT = [(f"sorted {k} n={n} {o}", f"sorted(D_{k}_{n}_{o})") for k in KINDS for n in SIZES for o in ORDERS]

FAMILY = [
    *[(f"sorted {k} n={n} {o} reverse", f"sorted(D_{k}_{n}_{o}, reverse=True)") for k in KINDS for n in (5, 20, 1000) for o in ("asc", "desc", "rand")],
    *[(f"x[:].sort() {k} n={n} {o}", f"D_{k}_{n}_{o}[:].sort()") for k in KINDS for n in (2, 20, 1000) for o in ("asc", "desc", "rand")],
    *[(f"sorted ustr n={n} {o}", f"sorted(D_ustr_{n}_{o})") for n in (20, 100, 1000) for o in ("asc", "desc", "rand")],
    *[(f"sorted lstr n={n} {o}", f"sorted(D_lstr_{n}_{o})") for n in (20, 100, 1000) for o in ("asc", "desc", "rand")],
    *[(f"sorted str n=64 {o}", f"sorted(D_str_64_{o})") for o in ORDERS],
    ("sorted(tuple int 20)", "sorted(T_int_20_rand)"),
    ("sorted(tuple str 100)", "sorted(T_str_100_rand)"),
    ("sorted(set int 100) [fallback: set]", "sorted(S_int)"),
    ("sorted(pairs) [fallback: tuples]", "sorted(pairs)"),
    ("sorted(pairs, key=len) [fallback: key]", "sorted(pairs, key=len)"),
    ("sorted(mixed int/float) [decline after copy]", "sorted(mixed)"),
    ("sorted(big ints) [decline after copy]", "sorted(big)"),
    ("sorted(floats with nan) [decline after copy]", "sorted(nan)"),
    ("sorted([]) ", "sorted([])"),
    ("sorted int n=1", "sorted(D_int_1_asc)"),
    ("sorted(gen) [fallback]", "sorted(x for x in D_int_5_rand)"),
    ("pairs[:].sort() [fallback: tuples]", "pairs[:].sort()"),
]

MINMAX = [
    *[(f"{f}({k} n={n} {o})", f"{f}(D_{k}_{n}_{o})") for f in ("min", "max") for k in ("str", "ustr", "lstr") for n in (1, 2, 3, 5, 20, 1000) for o in ("asc", "desc", "rand")],
    *[(f"{f}({k} n={n} rand)", f"{f}(D_{k}_{n}_rand)") for f in ("min", "max") for k in ("int", "float") for n in (1, 2, 5, 20, 1000)],
    ("min(a, b) str", "min(a, b)"),
    ("max(a, b) str", "max(a, b)"),
    ("min(a, b, 'c') str", "min(a, b, 'c')"),
    ("max(str 20, key=len) [fallback]", "max(D_str_20_rand, key=len)"),
    ("min(str 20, default='') ", "min(D_str_20_rand, default='')"),
]

# `list.sort()` in place on fresh copies (the scoreboard's `x.sort()` shape: the copy is made
# before the measured region), around the short-str hand-off threshold.
SMALL_N = (1, 2, 3, 4, 5, 6, 8, 12, 16, 20, 24, 31, 32)
SMALL = [(f"inplace sort {k} n={n} {o}", "P.pop().sort()", f"P = [mk({k!r}, {n}, {o!r}) for _ in range(16000)]")
         for k in ("str", "int", "float") for n in SMALL_N for o in ("asc", "desc", "rand")]

GROUPS = ["builtins", "containers"]
SETS = {"sort": SORT, "family": FAMILY, "minmax": MINMAX, "small": SMALL}


def main() -> None:
    which = sys.argv[1]
    jobs = int(sys.argv[sys.argv.index("-j") + 1]) if "-j" in sys.argv else 6
    cells = SETS[which]
    if "-k" in sys.argv:
        sub = sys.argv[sys.argv.index("-k") + 1]
        cells = [c for c in cells if sub in c[0]]

    def one(c: tuple) -> tuple[str, float, float]:
        name, stmt, *extra = c
        setup = SETUP + ("\n" + extra[0] if extra else "")
        return name, per_call(stmt, setup), per_call(stmt, setup, enable=GROUPS)

    out = []
    with ThreadPoolExecutor(jobs) as ex:
        for name, a, b in ex.map(one, cells):
            out.append({"cell": name, "plain": a, "enabled": b, "ratio": b / a})
            flag = "  <-- over 1.05" if b / a > 1.05 else ""
            print(f"{name:58s} {a:9.0f} {b:9.0f} {b / a:6.3f}{flag}", flush=True)
    if "-o" in sys.argv:
        with open(sys.argv[sys.argv.index("-o") + 1], "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
