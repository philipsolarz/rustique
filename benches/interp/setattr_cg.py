"""Callgrind cells for the object.__setattr__ override (CALL_NON_PY_GENERAL).

    python benches/interp/setattr_cg.py [--swap DIR:MOD] [-o out.json]
"""
import argparse, json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
from _callgrind import per_call

DEFS = '''
import dataclasses
@dataclasses.dataclass(frozen=True)
class F:
    a: int
    b: str
    c: float
class O:
    pass
class T:
    pass
o = O()
d = {}
'''
CELLS = {
    "F(1, 'x', 2.0) (frozen dataclass, 3 fields)": "F(1, 'x', 2.0)",
    "object.__setattr__(o, 'x', 1)": "object.__setattr__(o, 'x', 1)",
    "object.__setattr__(T, 'x', 1) (type: fallback, raises)": "try:\n            object.__setattr__(T, 'x', 1)\n        except TypeError: pass",
    "dict.__setitem__(d, 'a', 1) (other wrapper)": "dict.__setitem__(d, 'a', 1)",
    "other CALL_NON_PY_GENERAL: str.join via descriptor": "str.join('-', ('a', 'b'))",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--swap")
    ap.add_argument("-o")
    a = ap.parse_args()
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rqeval")
    res = {}
    for cell, stmt in CELLS.items():
        row = {"plain": per_call(stmt, DEFS, n=200)}
        if a.swap:
            d, m = a.swap.split(":")
            sw = (f"import sys as _s; _s.path.insert(0, {here!r}); import rqpatch as _rp; "
                  f"_rp.swap({d!r}, {m!r}, ['CALL_NON_PY_GENERAL'])")
            row["swap"] = per_call(stmt, DEFS + "\n" + sw, n=200)
        res[cell] = row
        print(f"{cell:46s} " + "  ".join(f"{k}={v:7.0f} ({v / row['plain']:.3f})" for k, v in row.items()), flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
