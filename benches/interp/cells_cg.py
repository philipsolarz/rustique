"""Callgrind cells of the interp tier (ACCELERATION §7.4): instructions per operation, plain vs
the tier on (its own targets only), each cell in its own fresh process, region-instrumented
(benches/targets/_callgrind.py). Fast paths, the fallbacks of every override, the specialized
LOAD_ATTR forms (our verbatim copies run instead of the core's when the family is swapped),
and operations whose handlers are not swapped (cost 0 by construction).

    python benches/interp/cells_cg.py [-o out.json] [--only substring] [--jit]
"""
from __future__ import annotations

import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "targets"))
from _callgrind import per_call  # noqa: E402

SETUP = f"import sys; sys.path.insert(0, {HERE!r}); from _cells import *; import math, dataclasses"
TIER = "; import _tier; _tier.enable_tier()"

# (group, cell, statement); "per 4" cells iterate over 4 objects of 3 types.
CELLS = [
    ("isinstance", "exact", "isinstance(o_a, A)"),
    ("isinstance", "subclass, positive", "isinstance(poly[1], A)"),
    ("isinstance", "negative, plain class", "isinstance(o_a, Other)"),
    ("isinstance", "negative, int instance", "isinstance(1, A)"),
    ("isinstance", "metaclass, positive", "isinstance(o_m, MA)"),
    ("isinstance", "metaclass, negative", "isinstance(o_a, MA)"),
    ("isinstance", "tuple (Other, A)", "isinstance(o_a, (Other, A))"),
    ("isinstance", "__getattr__ class, negative (fallback)", "isinstance(o_g, Other)"),
    ("isinstance", "ABCMeta negative (fallback)", "isinstance(o_a, Abc)"),
    ("isinstance", "union (fallback)", "isinstance(o_a, int | str)"),
    ("attr", "polymorphic o.x (per 4)", "for o in poly: o.x"),
    ("attr", "polymorphic o.m() (per 4)", "for o in poly: o.m()"),
    ("attr", "polymorphic __slots__ (per 4)", "for o in slots: o.args"),
    ("attr", "materialized __dict__ attribute", "mat.x"),
    ("attr", "cached_property, cached", "cached.v"),
    ("attr", "specialized a.x", "o_a.x"),
    ("attr", "specialized a.m()", "o_a.m()"),
    ("attr", "specialized class attribute", "o_a.cls_attr"),
    ("attr", "specialized module attribute math.pi", "math.pi"),
    ("attr", "class attribute on a class C.x", "A.cls_attr"),
    ("attr", "Python data descriptor (miss, hint)", "wd.d"),
    ("attr", "__getattr__ hook (fallback)", "o_g.anything"),
    ("attr", "AttributeError path (fallback)", "getattr(o_o, 'missing', None)"),
    ("attr", "AttributeError raised (fallback)", "try: o_o.missing\nexcept AttributeError: pass"),
    ("setattr", "frozen dataclass F(1, 'x', 2.0)", "Frozen(1, 'x', 2.0)"),
    ("setattr", "object.__setattr__(o, 'x', 1)", "object.__setattr__(o_a, 'x', 1)"),
    ("setattr", "object.__setattr__(type) (fallback, raises)", "try: object.__setattr__(Other, 'x', 1)\nexcept TypeError: pass"),
    ("setattr", "dict.__setitem__ (other wrapper, fallback)", "dict.__setitem__(d0, 'a', 1)"),
    ("setattr", "str.join via descriptor (fallback)", "str.join('-', l3)"),
    ("kwinit", "P(1, b=2)", "P(1, b=2)"),
    ("kwinit", "P(1) (CALL_ALLOC_AND_ENTER_INIT, not ours)", "P(1)"),
    ("kwinit", "dict(a=1) (builtin class, fallback)", "dict(a=1)"),
    ("kwinit", "f(1, b=2) (CALL_KW_PY, not ours)", "kwf(1, b=2)"),
    ("none", "ident(1)", "ident(1)"),
    ("none", "len(l3)", "len(l3)"),
    ("none", "generator sum(100)", "sum(x for x in r100)"),
]
EXTRA = "; d0 = {}; l3 = ['a', 'b', 'c']; r100 = range(100)\ndef ident(x): return x\ndef kwf(a, b=0): return a"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("-o")
    ap.add_argument("--only")
    ap.add_argument("--jit", action="store_true")
    a = ap.parse_args()
    rows = []
    for group, cell, stmt in CELLS:
        if a.only and a.only not in cell and a.only != group:
            continue
        stmt1 = stmt
        if "\n" in stmt:   # a compound statement: wrap as a one-liner function call
            stmt1 = f"_c{len(rows)}()"
            body = "\n".join("    " + line for line in stmt.splitlines())
            extra = f"\ndef _c{len(rows)}():\n{body}"
        else:
            extra = ""
        setup = SETUP + EXTRA + extra
        plain = per_call(stmt1, setup, jit=a.jit)
        tier = per_call(stmt1, setup + "\n" + TIER.lstrip("; "), jit=a.jit)
        rows.append({"group": group, "cell": cell, "plain": plain, "tier": tier, "ratio": tier / plain})
        print(f"{group:10s} {cell:50s} {plain:9.1f} {tier:9.1f}  {tier / plain:6.3f}", flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "jit": a.jit, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
