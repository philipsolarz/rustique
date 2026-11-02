"""Callgrind cells for the generic LOAD_ATTR path and the megamorphic attribute cache override.

    python benches/interp/attr_cg.py [--swap DIR:MOD] [-o out.json]

--swap swaps the whole LOAD_ATTR family of the core for MOD's handlers (so deopts of the
specialized forms land in MOD's generic LOAD_ATTR); `plain` is untouched CPython.
"""
import argparse, json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
from _callgrind import per_call

DEFS = '''
import functools, math
class A:
    def __init__(self): self.x = 1
    def m(self): return 1
class B:
    def __init__(self): self.y = 0; self.x = 2
    def m(self): return 2
class C(A):
    pass
class D:
    k = 5
    def __init__(self): self.v = 1
    def m(self): return 1
    @functools.cached_property
    def cp(self): return 3
class G:
    def __init__(self): self.x = 1
    def __getattr__(self, n): return 7
class S0:
    __slots__ = ("args", "k")
    def __init__(self): self.args = 1; self.k = 2
class S1(S0):
    __slots__ = ()
class S2(S0):
    __slots__ = ()
sl = [S0(), S1(), S2(), S1()]
objs = [A(), B(), C(), A()]
a = A()
d = D(); d.cp; d.__dict__
g = G()
'''
CELLS = {
    "polymorphic o.x (3 types, per 4 loads)": "for _o in objs: _o.x",
    "polymorphic o.m() (3 types, per 4 calls)": "for _o in objs: _o.m()",
    "polymorphic slot o.args (3 types, per 4 loads)": "for _o in sl: _o.args",
    "d.m() (materialized __dict__)": "d.m()",
    "d.v (materialized __dict__)": "d.v",
    "d.cp (cached_property, cached)": "d.cp",
    "d.k (class attr, materialized __dict__)": "d.k",
    "a.x (monomorphic, specialized)": "a.x",
    "a.m() (monomorphic, specialized)": "a.m()",
    "math.pi (module attr)": "math.pi",
    "g.x (__getattr__ class, found)": "g.x",
    "a.zz miss -> AttributeError caught": "try:\n            a.zz\n        except AttributeError: pass",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--swap")
    ap.add_argument("-o")
    ap.add_argument("--generic-only", action="store_true", help="swap only the generic LOAD_ATTR handler")
    a = ap.parse_args()
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rqeval")
    res = {}
    for cell, stmt in CELLS.items():
        row = {"plain": per_call(stmt, DEFS, n=200)}
        if a.swap:
            d, m = a.swap.split(":")
            ops = "['LOAD_ATTR']" if a.generic_only else "['LOAD_ATTR'] + list(_om._specializations['LOAD_ATTR'])"
            sw = (f"import sys as _s; _s.path.insert(0, {here!r}); import rqpatch as _rp; import _opcode_metadata as _om; "
                  f"_rp.swap({d!r}, {m!r}, {ops})")
            row["swap"] = per_call(stmt, DEFS + "\n" + sw, n=200)
        res[cell] = row
        print(f"{cell:44s} " + "  ".join(f"{k}={v:7.0f} ({v / row['plain']:.3f})" for k, v in row.items()), flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
