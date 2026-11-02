"""Callgrind cells of the data-model groups (`enum`, `generated`, `contextlib`): Ir per
call, plain CPython vs enabled, fast paths, fallback paths and guard rejects, class
creation and install (docs/targets/datamodel.md).

    python benches/targets/datamodel_cells.py [--only substr] [--jobs 4] [--json out.json]

Classes whose generated methods get kernels must be created after `enable()`, so each
cell's setup enables first (variant "on"), then defines its classes.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(__file__))
from _callgrind import per_call, region  # noqa: E402

GROUPS = ["enum", "generated", "contextlib", "functools"]

ENUM = """
import enum
class Color(enum.Enum):
    RED = 1
    GREEN = 2
    BLUE = 3
class Perm(enum.Flag):
    R = 4
    W = 2
    X = 1
class Num(enum.IntEnum):
    A = 1
    B = 2
class Bits(enum.IntFlag):
    A = 1
    B = 2
class Meta(enum.EnumType):
    pass
class M(enum.Enum, metaclass=Meta):
    A = 1
class Own(enum.Enum):
    A = 1
    @enum.property
    def extra(self):
        return 1
class K:
    def __hash__(self): return 7
    def __eq__(self, o): return False
class Bad(enum.Enum):
    A = 1
Bad._value2member_map_[K()] = Bad.A
m = Color.GREEN
rw = Perm.R | Perm.W
r = Perm.R
d = {Color.RED: 1, Color.GREEN: 2}
def miss():
    try:
        Color(99)
    except ValueError:
        pass
"""

NT = """
import collections, typing
NT = collections.namedtuple("NT", "a b c")
TN = typing.NamedTuple("TN", [("x", int), ("y", int)])
class Sub(NT):
    pass
nt = NT(1, 2, 3)
sub = Sub(1, 2, 3)
t = (1, 2, 3)
l = [1, 2, 3]
"""

DC = """
import dataclasses
@dataclasses.dataclass(frozen=True)
class F:
    x: int
    y: int
@dataclasses.dataclass(frozen=True, slots=True, kw_only=True)
class E:
    a: int
    b: str = "s"
@dataclasses.dataclass(frozen=True, slots=True)
class S:
    a: int
    b: int
@dataclasses.dataclass(frozen=True)
class L:
    a: int
    items: list = dataclasses.field(default_factory=list)
@dataclasses.dataclass(frozen=True)
class U:
    a: int
    items: list = dataclasses.field(default_factory=lambda: [])
@dataclasses.dataclass
class P:
    x: int
    y: int
@dataclasses.dataclass(frozen=True)
class Post:
    x: int
    def __post_init__(self):
        pass
"""

CTX = """
import contextlib
@contextlib.contextmanager
def cm():
    yield 1
@contextlib.contextmanager
def cm2(a, b=2, *, k=3):
    yield a
@contextlib.contextmanager
def notgen():
    return iter([1])
def gen():
    yield 1
"""

SD = """
import functools
@functools.singledispatch
def fmt(x):
    return 0
@fmt.register(int)
def _(x):
    return 1
class Meta(type):
    def __eq__(cls, o): return type.__eq__(cls, o)
    __hash__ = type.__hash__
class WM(metaclass=Meta):
    pass
wm = WM()
class H:
    @functools.singledispatchmethod
    def handle(self, x):
        return 0
    @handle.register
    def _(self, x: int):
        return 1
    @functools.singledispatchmethod
    @classmethod
    def cm(cls, x):
        return 2
h = H()
fmt(1); fmt("s"); fmt(wm); h.handle(1)
"""

CELLS: dict[str, tuple[str, str]] = {
    # enum: fast paths
    "enum m.value": (ENUM, "m.value"),
    "enum m.name": (ENUM, "m.name"),
    "enum Color(2)": (ENUM, "Color(2)"),
    "enum Color(m)": (ENUM, "Color(m)"),
    "enum Num(2)": (ENUM, "Num(2)"),
    "enum Perm.R | Perm.W": (ENUM, "Perm.R | Perm.W"),
    "enum rw & r": (ENUM, "rw & r"),
    "enum rw ^ r": (ENUM, "rw ^ r"),
    "enum ~r": (ENUM, "~r"),
    "enum r in rw": (ENUM, "r in rw"),
    "enum hash(m)": (ENUM, "hash(m)"),
    "enum d[m]": (ENUM, "d[m]"),
    "enum Bits.A | Bits.B": (ENUM, "Bits.A | Bits.B"),
    # enum: fallbacks and rejects
    "enum miss Color(99) (fallback)": (ENUM, "miss()"),
    "enum Color('RED') (fallback)": (ENUM, "Color.__members__['RED']"),
    "enum Bits.A | 2 (fallback)": (ENUM, "Bits.A | 2"),
    "enum M(1) metaclass subclass": (ENUM, "M(1)"),
    "enum Own.A.extra (user property)": (ENUM, "Own.A.extra"),
    "enum Bad(1) (unsafe keys)": (ENUM, "Bad(1)"),
    "enum Num.A.value": (ENUM, "Num.A.value"),
    # namedtuple
    "nt NT(1, 2, 3)": (NT, "NT(1, 2, 3)"),
    "nt NT(a=1, b=2, c=3)": (NT, "NT(a=1, b=2, c=3)"),
    "nt TN(1, 2)": (NT, "TN(1, 2)"),
    "nt NT._make(t)": (NT, "NT._make(t)"),
    "nt NT._make(l)": (NT, "NT._make(l)"),
    "nt nt._replace(b=5)": (NT, "nt._replace(b=5)"),
    "nt Sub(1, 2, 3)": (NT, "Sub(1, 2, 3)"),
    "nt Sub._make(t)": (NT, "Sub._make(t)"),
    "nt sub._replace(a=1) (fallback: dict)": (NT, "sub._replace(a=1)"),
    "nt NT._make(iter(t)) (fallback)": (NT, "NT._make(iter(t))"),
    "nt nt._asdict() (untouched)": (NT, "nt._asdict()"),
    # dataclass
    "dc F(x=1, y=2) frozen kw": (DC, "F(x=1, y=2)"),
    "dc F(1, 2) frozen positional": (DC, "F(1, 2)"),
    "dc E(a=1, b='t') frozen slots kw_only": (DC, "E(a=1, b='t')"),
    "dc S(1, 2) frozen slots positional": (DC, "S(1, 2)"),
    "dc L(1) builtin factory": (DC, "L(1)"),
    "dc U(1) user factory (fallback)": (DC, "U(1)"),
    "dc P(x=1, y=2) non-frozen (untouched)": (DC, "P(x=1, y=2)"),
    "dc Post(x=1) __post_init__ (untouched)": (DC, "Post(x=1)"),
    # contextmanager
    "ctx cm()": (CTX, "cm()"),
    "ctx with cm(): pass": (CTX, "with cm(): pass"),
    "ctx cm2(1, k=2)": (CTX, "cm2(1, k=2)"),
    "ctx notgen() (fallback)": (CTX, "notgen()"),
    "ctx contextmanager(gen) (decorate)": (CTX, "contextlib.contextmanager(gen)"),
    # singledispatch
    "sd fmt(1)": (SD, "fmt(1)"),
    "sd fmt.dispatch(int)": (SD, "fmt.dispatch(int)"),
    "sd fmt(wm) metaclass __eq__ (fallback)": (SD, "fmt(wm)"),
    "sd h.handle(1)": (SD, "h.handle(1)"),
    "sd h.handle (get)": (SD, "h.handle"),
    "sd H.cm(1) classmethod func (fallback)": (SD, "H.cm(1)"),
    "sd singledispatch(f) (decorate)": (SD, "functools.singledispatch(H)"),
}

# Class creation in steady state: one class of each kind is created and called in the setup,
# so first-call validation (accel D: targets are validated at their first call) is not in
# the region; the "first use" regions measure that one-time validation.
_WARM = (
    "import collections, dataclasses, enum\n"
    "collections.namedtuple('W', 'a')(1)\n"
    "@dataclasses.dataclass(frozen=True)\nclass W:\n    x: int\nW(x=1)\n"
    "class WE(enum.Enum):\n    A = 1\nWE(1); WE.A.value; hash(WE.A)\n"
)
REGIONS: dict[str, tuple[str, str]] = {
    "create namedtuple + 1 call": (_WARM, "P = collections.namedtuple('P', 'a b c'); P(1, 2, 3)"),
    "create frozen dataclass + 1 call": (
        _WARM,
        "@dataclasses.dataclass(frozen=True)\nclass F:\n    x: int\n    y: int = 0\nF(x=1)",
    ),
    "create enum (5 members)": (_WARM, "class E(enum.Enum):\n    A=1\n    B=2\n    C=3\n    D=4\n    E=5\nE(3)"),
    "first use incl. validation: namedtuple": ("import collections", "P = collections.namedtuple('P', 'a b c'); P(1, 2, 3)"),
    "first use incl. validation: dataclass": (
        "import dataclasses",
        "@dataclasses.dataclass(frozen=True)\nclass F:\n    x: int\n    y: int = 0\nF(x=1)",
    ),
    "first use incl. validation: enum": ("import enum", "class E(enum.Enum):\n    A=1\n    B=2\nE(2); E.A.value; hash(E.A)"),
}


def setup_for(body: str, on: bool) -> str:
    pre = "from rustique import _takeover as _T\n_T.enable(only=%r)\n" % GROUPS if on else ""
    return pre + body


def run_cell(name: str) -> tuple[str, dict[str, float]]:
    body, stmt = CELLS[name]
    out = {}
    for v, on in (("plain", False), ("on", True)):
        out[v] = per_call(stmt, setup_for(body, on))
    return name, out


def run_region(name: str) -> tuple[str, dict[str, float]]:
    setup, stmt = REGIONS[name]
    out = {}
    for v, on in (("plain", False), ("on", True)):
        out[v] = float(region(stmt, setup_for(setup, on)))
    return name, out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--json")
    a = ap.parse_args()
    cells = [c for c in CELLS if a.only in c]
    regions = [r for r in REGIONS if a.only in r]
    with ThreadPoolExecutor(a.jobs) as ex:
        res = dict(ex.map(run_cell, cells))
        res.update(dict(ex.map(run_region, regions)))
    worst = 0.0
    for name in cells + regions:
        v = res[name]
        ratio = v["on"] / v["plain"]
        worst = max(worst, ratio)
        print(f"{name:44s} plain={v['plain']:>9,.0f}  on={v['on']:>9,.0f}  ({ratio:.3f})")
    print(f"worst ratio: {worst:.3f}")
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"python": sys.version, "cells": res}, f, indent=1)


if __name__ == "__main__":
    main()
