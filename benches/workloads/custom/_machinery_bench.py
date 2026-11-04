#!/usr/bin/env python3
"""Instructions per operation of the CPython machinery the `custom` workloads lean on.

    python _machinery_bench.py [--n 400000] [--only name,...] [--json out.json]

Each case runs its statement ``n`` times in a fresh process under
``perf stat -e instructions:u``; the same process with ``n = 0`` is subtracted, and so is the
empty-loop cost, so the result is user-space instructions per operation (noise-robust on
the shared dev box). Each "machinery" case is paired with the plain-Python construct that
is its floor (what an exact native fast path could approach). Not a workload.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile

SETUP = r'''
import abc, contextlib, enum, functools, inspect, typing, weakref
from dataclasses import dataclass
from collections import ChainMap
from typing import Protocol, runtime_checkable

@dataclass(frozen=True, slots=True)
class F2:
    a: int
    b: str = "x"
@dataclass(slots=True)
class M2:
    a: int
    b: str = "x"
class P2:
    __slots__ = ("a", "b")
    def __init__(self, a, b="x"):
        self.a = a; self.b = b
@dataclass(frozen=True, slots=True, kw_only=True)
class F6:
    stream: str; version: int; at: int; amount: int; reference: str; kind: str = "x"
@dataclass(slots=True, kw_only=True)
class M6:
    stream: str; version: int; at: int; amount: int; reference: str; kind: str = "x"
@dataclass(frozen=True)
class FD2:
    a: int
    b: str = "x"
@dataclass(frozen=True, slots=True, order=True)
class O2:
    a: int
    b: str
class T2:
    __slots__ = ("a", "b")
    def __init__(s, a, b): s.a = a; s.b = b
    def __eq__(s, o): return (s.a, s.b) == (o.a, o.b)
    def __hash__(s): return hash((s.a, s.b))
f1, f2 = F2(1, "y"), F2(1, "y"); o1, o2 = O2(1, "a"), O2(1, "b"); t1, t2 = T2(1, "y"), T2(1, "y")

class C(enum.Enum):
    X = 1; Y = 2
class S(enum.StrEnum):
    A = "a"; B = "b"
class Fl(enum.Flag):
    NONE = 0; P = enum.auto(); Q = enum.auto(); R = enum.auto()
fl = Fl.P | Fl.Q
class Plain:
    __slots__ = ("value", "name")
    def __init__(s, v, n): s.value = v; s.name = n
plain = Plain(2, "Y")
cmap = {1: C.X, 2: C.Y}
d_enum = {C.X: 1, C.Y: 2}
d_int = {1: 1, 2: 2}

class Base: pass
class A(Base): pass
@functools.singledispatch
def sd(x): return 0
@sd.register
def _(x: A): return 1
def plainf(x): return 1
class K:
    @functools.singledispatchmethod
    def m(self, x): return 0
    @m.register
    def _(self, x: A): return 1
    def p(self, x): return 1
a = A(); k = K()

@contextlib.contextmanager
def cm():
    yield 1
class CM:
    def __enter__(s): return 1
    def __exit__(s, *e): return None
cmo = CM()

@runtime_checkable
class Proto(Protocol):
    name: str
    def run(self) -> int: ...
class Impl:
    name = "x"
    def run(self): return 1
class AB(abc.ABC):
    @abc.abstractmethod
    def run(self): ...
class ABImpl(AB):
    def run(self): return 1
impl, abimpl = Impl(), ABImpl()

class CP:
    @functools.cached_property
    def v(self): return 1
    @property
    def pv(self): return 1

wv = weakref.WeakValueDictionary(); wk = weakref.WeakKeyDictionary(); dd = {}
class Obj: pass
keep = [Obj() for _ in range(64)]
for i, o in enumerate(keep): wv[i] = o; wk[o] = i; dd[i] = o
ko = keep[5]

def sigf(a: int, b: str = "x", *, c: float = 1.0) -> int: return a
cmap2 = ChainMap({"loop": 1}, {"x": 2}, {"company": 3}); pd = {"loop": 1, "x": 2, "company": 3}
@functools.total_ordering
class TO:
    __slots__ = ("k",)
    def __init__(s, k): s.k = k
    def __lt__(s, o): return s.k < o.k
    def __eq__(s, o): return s.k == o.k
to1, to2 = TO(1), TO(2)
'''

CASES = [
    # (name, statement, group)
    ("loop: pass", "pass", "baseline"),
    ("dataclass frozen+slots 2 fields: F2(1,'y')", "F2(1, 'y')", "dataclass"),
    ("dataclass slots 2 fields: M2(1,'y')", "M2(1, 'y')", "dataclass"),
    ("plain slotted class: P2(1,'y')", "P2(1, 'y')", "dataclass"),
    ("dataclass frozen (dict) 2 fields: FD2(1,'y')", "FD2(1, 'y')", "dataclass"),
    ("dataclass frozen+slots 6 kw fields", "F6(stream='a', version=1, at=2, amount=3, reference='r')", "dataclass"),
    ("dataclass slots 6 kw fields", "M6(stream='a', version=1, at=2, amount=3, reference='r')", "dataclass"),
    ("dataclass __eq__: f1 == f2", "f1 == f2", "dataclass"),
    ("hand-written __eq__: t1 == t2", "t1 == t2", "dataclass"),
    ("tuple ==: (1,'y') == (1,'y')", "(f1.a, f1.b) == (f2.a, f2.b)", "dataclass"),
    ("dataclass __hash__: hash(f1)", "hash(f1)", "dataclass"),
    ("hand-written __hash__: hash(t1)", "hash(t1)", "dataclass"),
    ("dataclass order=True __lt__: o1 < o2", "o1 < o2", "dataclass"),
    ("enum by value: C(2)", "C(2)", "enum"),
    ("StrEnum by value: S('b')", "S('b')", "enum"),
    ("dict lookup (floor of C(2)): cmap[2]", "cmap[2]", "enum"),
    ("enum .value", "C.Y.value", "enum"),
    ("slot attribute (floor of .value)", "plain.value", "enum"),
    ("hash(enum member)", "hash(C.Y)", "enum"),
    ("dict[enum] lookup", "d_enum[C.Y]", "enum"),
    ("dict[int] lookup", "d_int[2]", "enum"),
    ("Flag |", "Fl.P | Fl.Q", "enum"),
    ("Flag &", "fl & Fl.Q", "enum"),
    ("Flag in", "Fl.Q in fl", "enum"),
    ("singledispatch call", "sd(a)", "functools"),
    ("plain function call", "plainf(a)", "functools"),
    ("singledispatchmethod call", "k.m(a)", "functools"),
    ("plain method call", "k.p(a)", "functools"),
    ("cached_property first access (new instance)", "CP().v", "functools"),
    ("property access (new instance)", "CP().pv", "functools"),
    ("total_ordering __ge__", "to1 >= to2", "functools"),
    ("direct __lt__", "to1 < to2", "functools"),
    ("with @contextmanager", "with cm() as x: pass", "contextlib"),
    ("with class-based CM", "with cmo as x: pass", "contextlib"),
    ("isinstance(x, runtime_checkable Protocol)", "isinstance(impl, Proto)", "typing/abc"),
    ("isinstance(x, ABC subclass)", "isinstance(abimpl, AB)", "typing/abc"),
    ("isinstance(x, plain class)", "isinstance(impl, Impl)", "typing/abc"),
    ("WeakValueDictionary.get", "wv.get(5)", "weakref"),
    ("WeakKeyDictionary[k]", "wk[ko]", "weakref"),
    ("dict.get", "dd.get(5)", "weakref"),
    ("inspect.signature(simple function)", "inspect.signature(sigf)", "inspect"),
    ("typing.get_type_hints(simple function)", "typing.get_type_hints(sigf)", "inspect"),
    ("ChainMap[k] (3 maps, hit in last)", "cmap2['company']", "collections"),
    ("dict[k]", "pd['company']", "collections"),
]

DRIVER = r'''
import sys
n = int(sys.argv[1])
SETUP_CODE = {setup!r}
exec(SETUP_CODE, globals())
# the loop runs inside a function: globals are LOAD_GLOBAL (specialized), the loop var is fast
exec(compile("def _bench(n):\n    for _ in range(n):\n        " + {stmt!r}, "<bench>", "exec"),
     globals())
_bench(n)
'''


def instructions(py: str, setup: str, stmt: str, n: int, tmp: str) -> float:
    path = os.path.join(tmp, "drv.py")
    with open(path, "w") as f:
        f.write(DRIVER.format(setup=setup, stmt=stmt))
    out = os.path.join(tmp, "stat.csv")
    subprocess.run(["perf", "stat", "-x", ",", "-e", "instructions:u", "-o", out, "--", py, path,
                    str(n)], check=True, stdout=subprocess.DEVNULL)
    for line in open(out):
        parts = line.strip().split(",")
        if len(parts) > 2 and parts[0].isdigit():
            return float(parts[0])
    raise RuntimeError("no instructions count")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=1_000_000)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--only")
    ap.add_argument("--json")
    a = ap.parse_args()
    tmp = tempfile.mkdtemp()
    py = sys.executable
    base0 = min(instructions(py, SETUP, "pass", 0, tmp) for _ in range(a.reps))
    loop = (min(instructions(py, SETUP, "pass", a.n, tmp) for _ in range(a.reps)) - base0) / a.n
    rows = []
    for name, stmt, group in CASES:
        if a.only and group not in a.only.split(","):
            continue
        tot = min(instructions(py, SETUP, stmt, a.n, tmp) for _ in range(a.reps))
        per = (tot - base0) / a.n - loop
        rows.append({"case": name, "group": group, "instructions_per_op": round(per, 1)})
        print(f"{group:12s} {name:50s} {per:9.1f}", flush=True)
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"python": sys.version, "n": a.n, "loop_overhead": loop, "rows": rows}, f,
                      indent=1)


if __name__ == "__main__":
    main()
