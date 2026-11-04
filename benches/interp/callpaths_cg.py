"""Callgrind cells: what a Python call costs when the interpreter inlines the frame push
versus when it goes through C (the C->Python re-entry the M2 overrides could remove).

    python benches/interp/callpaths_cg.py [-o out.json]
"""
import argparse, json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
from _callgrind import per_call

DEFS = '''
import functools
def f(a, b=0):
    return a
class O:
    def m(self, a, b=0):
        return a
    def __call__(self, a):
        return a
class P:
    def __init__(self, a, b=0):
        self.a = a
o = O()
bm = o.m
part = functools.partial(f, 1)
args = (1,)
kw = {"b": 2}
'''
CELLS = [
    ("f(*args)  [CALL_EX_PY: inlined]", "f(*args)"),
    ("bm(*args) [bound method: C->Py]", "bm(*args)"),
    ("f(*args, **kw)  [inlined]", "f(*args, **kw)"),
    ("o.m(*args, **kw) [bound method: C->Py]", "o.m(*args, **kw)"),
    ("P(1)      [CALL_ALLOC_AND_ENTER_INIT: inlined]", "P(1)"),
    ("P(1, b=2) [CALL_KW -> type_call: C->Py]", "P(1, b=2)"),
    ("P(*args)  [CALL_EX -> type_call: C->Py]", "P(*args)"),
    ("o.m(1)    [inlined]", "o.m(1)"),
    ("o(1)      [__call__: slot_tp_call C->Py]", "o(1)"),
    ("f(1, 2)   [inlined]", "f(1, 2)"),
    ("part(2)   [functools.partial: C->Py]", "part(2)"),
]


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("-o")
    ap.add_argument("--swap", help="DIR:MOD:OP[,OP] handlers swapped in for a second column")
    a = ap.parse_args()
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rqeval")
    res = {}
    for cell, stmt in CELLS:
        row = {"plain": per_call(stmt, DEFS, n=200)}
        if a.swap:
            d, m, ops = a.swap.split(":")
            sw = (f"import sys as _s; _s.path.insert(0, {here!r}); import rqpatch as _rp; "
                  f"_rp.swap({d!r}, {m!r}, {ops.split(',')!r})")
            row["swap"] = per_call(stmt, DEFS + "\n" + sw, n=200)
        res[cell] = row
        print(f"{cell:52s} " + "  ".join(f"{k}={v:7.0f} ({v / row['plain']:.3f})" for k, v in row.items()), flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
