"""Callgrind cells: cost of isinstance() paths on 3.15 (Ir per call).

    python benches/interp/isinstance_cg.py [-o out.json]

Negative isinstance(x, C) with C a plain class always looks up x.__class__ through
x's tp_getattro (object_isinstance, Objects/abstract.c:2610); a class with a custom
metaclass also goes through _PyObject_LookupSpecial(cls, '__instancecheck__') and a call.
"""
import argparse, json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
from _callgrind import per_call

DEFS = '''
import abc
class A: pass
class B(A): pass
class Other: pass
class Meta(type): pass
class MA(metaclass=Meta): pass
class MB(MA): pass
class Abs(abc.ABC): pass
class G:
    def __getattr__(self, n): raise AttributeError(n)
class Slots:
    __slots__ = ("x",)
a = A(); b = B(); mb = MB(); g = G(); s = Slots(); n = 7
'''
CELLS = {
    "isinstance(a, A) exact": "isinstance(a, A)",
    "isinstance(b, A) subclass (positive)": "isinstance(b, A)",
    "isinstance(a, Other) negative": "isinstance(a, Other)",
    "isinstance(s, Other) negative, __slots__": "isinstance(s, Other)",
    "isinstance(n, Other) negative, int": "isinstance(n, Other)",
    "isinstance(g, Other) negative, __getattr__ class": "isinstance(g, Other)",
    "isinstance(mb, MA) metaclass, positive": "isinstance(mb, MA)",
    "isinstance(a, MA) metaclass, negative": "isinstance(a, MA)",
    "isinstance(a, Abs) ABCMeta, negative": "isinstance(a, Abs)",
    "isinstance(a, (Other, A)) tuple": "isinstance(a, (Other, A))",
    "type(a) is Other (floor of a negative)": "type(a) is Other",
}

SWAP = ("import sys as _s; _s.path.insert(0, {here!r}); import rqpatch as _rp; "
        "_rp.swap({d!r}, {m!r}, ['CALL_ISINSTANCE'])")


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("-o")
    ap.add_argument("--swap", help="DIR:MOD of an _rqeval build whose CALL_ISINSTANCE handler is swapped in")
    a = ap.parse_args()
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rqeval")
    res = {}
    for cell, stmt in CELLS.items():
        row = {"plain": per_call(stmt, DEFS, n=200)}
        if a.swap:
            d, m = a.swap.split(":")
            row["swap"] = per_call(stmt, DEFS + "\n" + SWAP.format(here=here, d=d, m=m), n=200)
        res[cell] = row
        print(f"{cell:52s} " + "  ".join(f"{k}={v:6.0f} ({v / row['plain']:.3f})" for k, v in row.items()), flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)

if __name__ == "__main__":
    main()
