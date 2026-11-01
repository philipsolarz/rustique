"""callgrind cells (Ir per call, region-instrumented, fresh process each) for the
retrofitted `containers` and `builtins` groups: fast paths, fallback paths and guard-reject
paths, plain CPython vs `enable(only=[group])` (docs/ACCELERATION.md §5.2, §7.4).

Usage: python benches/targets/containers.py [--jit] [--only substring] [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import per_call  # noqa: E402

SETUP = """
class O:
    pass
o = O()
ints = [5, 3, 9, 1, 7, 3, 8, 2]
small = [1, 2, 3]
objs = [o, O(), O()]
mixed = [1, 2, o]
strs = ['b', 'a', 'c', 'd']
t = (5, 3, 9, 1)
tobjs = (o, O())
d = {'a': 1, 'b': 2}
fl = [1.5, 2.5, 3.25]
"""

# (cell, statement, group, kind)
CELLS = [
    ("3 in ints", "3 in ints", "containers", "fast"),
    ("4 in ints (miss)", "4 in ints", "containers", "fast"),
    ("o in objs (first is not a scalar)", "o in objs", "containers", "fallback"),
    ("1 in mixed", "1 in mixed", "containers", "fast"),
    ("o in mixed (needle not a scalar)", "o in mixed", "containers", "fallback"),
    ("ints.index(7)", "ints.index(7)", "containers", "fast"),
    ("ints.index(7, 1) (extra arg)", "ints.index(7, 1)", "containers", "fallback"),
    ("objs.index(o)", "objs.index(o)", "containers", "fallback"),
    ("ints.count(3)", "ints.count(3)", "containers", "fast"),
    ("objs.count(o)", "objs.count(o)", "containers", "fallback"),
    ("t.index(1)", "t.index(1)", "containers", "fast"),
    ("tobjs.count(o)", "tobjs.count(o)", "containers", "fallback"),
    ("9 in t", "9 in t", "containers", "fast"),
    ("ints[:].sort()", "ints[:].sort()", "containers", "fast"),
    ("strs[:].sort(key=len) (key)", "strs[:].sort(key=len)", "containers", "fallback"),
    ("sum(small)", "sum(small)", "builtins", "fast"),
    ("sum(fl)", "sum(fl)", "builtins", "fast"),
    ("min(3, 4)", "min(3, 4)", "builtins", "fast"),
    ("max(ints)", "max(ints)", "builtins", "fast"),
    ("max(ints, key=abs) (key)", "max(ints, key=abs)", "builtins", "fallback"),
    ("sorted(small)", "sorted(small)", "builtins", "fallback"),
    ("any(small)", "any(small)", "builtins", "fast"),
    ("any(d) (dict: direct fallback)", "any(d)", "builtins", "fallback"),
    ("all(strs)", "all(strs)", "builtins", "fast"),
    ("sum(range(3)) (iterator: direct fallback)", "sum(range(3))", "builtins", "fallback"),
]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--jit", action="store_true")
    ap.add_argument("--only")
    ap.add_argument("-o")
    a = ap.parse_args()
    rows = {}
    for cell, stmt, group, kind in CELLS:
        if a.only and a.only not in cell:
            continue
        plain = per_call(stmt, SETUP, jit=a.jit)
        enabled = per_call(stmt, SETUP, enable=[group], jit=a.jit)
        rows[cell] = {"group": group, "kind": kind, "plain": plain, "enabled": enabled, "ratio": enabled / plain}
        print(f"{cell:45s} {kind:8s} {plain:9.1f} {enabled:9.1f}  {enabled / plain:.3f}", flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "jit": a.jit, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
