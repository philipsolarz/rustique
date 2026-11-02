"""Callgrind cells (Ir per operation) for PEP 523 eval-frame variants on 3.15.

    python benches/interp/hook_cg.py [-o out.json] [--jit] [--rq DIR:MOD ...] [--no-record]
                                     [--cells name,name]

Variants:
  none    no eval-frame function
  record  classic PEP 523 hook (_testinternalcapi.set_eval_frame_record: calls
          _PyEval_EvalFrameDefault; specialization stays off)
  pbs     the generated interpreter copy that ships inside the PBS binary
          (_testinternalcapi.set_eval_frame_interp(), specialization allowed)
  MOD     each --rq DIR:MOD: an _rqeval build (benches/interp/rqeval) installed with
          specialization allowed
"""
import argparse, json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
from _callgrind import per_call

DEFS = '''
import asyncio
def ident(x):
    return x
def kw(a, b=2, *, c=3):
    return a
def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)
class O:
    def m(self, x):
        return x
class P:
    def __init__(self, a, b):
        self.a = a; self.b = b
class E:
    def __eq__(self, other):
        return True
def gen(n):
    for i in range(n):
        yield i
async def leaf():
    return 1
async def coro(n):
    s = 0
    for _ in range(n):
        s += await leaf()
    return s
def raiser():
    raise ValueError("x")
def catch():
    try:
        raiser()
    except ValueError:
        return 1
def catch_local():
    try:
        raise KeyError(1)
    except KeyError:
        return 1
def closure():
    y = 1
    def inner(x):
        return x + y
    return inner
inner = closure()
o = O(); e1 = E(); e2 = E()
L = list(range(100))
D = {i: i for i in range(10)}
'''
CELLS = {
    "ident(1)": "ident(1)",
    "kw(1, c=4)": "kw(1, c=4)",
    "o.m(1)": "o.m(1)",
    "inner(1) (closure)": "inner(1)",
    "P(1, 2) (instantiation)": "P(1, 2)",
    "e1 == e2 (user __eq__, C->Py)": "e1 == e2",
    "fib(10) (177 calls)": "fib(10)",
    "for x in gen(100) (100 items)": "for _x in gen(100): pass",
    "coroutine 20 awaits (coro(20).send)": "_c = coro(20)\n        try:\n            _c.send(None)\n        except StopIteration: pass",
    "[x*x for x in L] (100 items)": "[_x * _x for _x in L]",
    "sum(x for x in L) (genexpr, 100)": "sum(_x for _x in L)",
    "catch() (raise in callee, except)": "catch()",
    "catch_local() (raise+except)": "catch_local()",
    "D[k] KeyError caught": "try:\n            D[99]\n        except KeyError: pass",
    "d.get / str ops mix": "D.get(3); 'a,b'.split(','); len(L)",
}


def variants(a):
    v = {"none": ""}
    if not a.no_record:
        v["record"] = "import _testinternalcapi as _t; _rec = []; _t.set_eval_frame_record(_rec)"
    if not a.no_pbs:
        v["pbs"] = "import _testinternalcapi as _t; _t.set_eval_frame_interp()"
    for spec in a.rq or []:
        d, mod = spec.split(":")
        v[mod] = f"import sys as _s; _s.path.insert(0, {d!r}); import {mod} as _r; _r.install()"
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rqeval")
    for spec in a.swapall or []:
        d, mod = spec.split(":")
        v["swapall:" + mod] = (f"import sys as _s; _s.path.insert(0, {here!r}); import rqpatch as _rp; "
                              f"_rp.swap_all({d!r}, {mod!r})")
    return v


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-o")
    ap.add_argument("--jit", action="store_true")
    ap.add_argument("--rq", action="append")
    ap.add_argument("--no-record", action="store_true")
    ap.add_argument("--swapall", action="append", help="DIR:MOD: swap every core handler for MOD's")
    ap.add_argument("--no-pbs", action="store_true")
    ap.add_argument("--cells")
    ap.add_argument("--n", type=int, default=100)
    a = ap.parse_args()
    cells = CELLS if not a.cells else {k: v for k, v in CELLS.items() if any(c in k for c in a.cells.split(","))}
    res = {}
    for cell, stmt in cells.items():
        row = {}
        for v, h in variants(a).items():
            if v == "record" and ("coro" in stmt or "gen(" in stmt):
                continue
            try:
                row[v] = per_call(stmt, DEFS + "\n" + h, jit=a.jit, n=a.n)
            except RuntimeError as e:
                row[v] = None
                print("  failed", v, str(e)[-300:], file=sys.stderr)
        res[cell] = row
        base = row["none"]
        print(f"{cell:40s} " + "  ".join(
            f"{k}={x:,.0f} ({x / base:.3f})" if x else f"{k}=n/a" for k, x in row.items()), flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
