#!/usr/bin/env python3
"""Callgrind cells (Ir per operation) for CPython machinery, plain and with prototypes.

    python benches/machinery/cells.py [--only REGEX] [--variants plain,proto] [-j 8] [-o out.json]

Each cell is (name, setup, stmt); Ir per execution of `stmt` measured in a fresh process,
region-instrumented, warmed, Ir(2n)-Ir(n) over n with the statement unrolled 10x
(benches/targets/_callgrind.py). Variant `proto` runs `import rustique._rustique as R;
R.proto_enable(<groups>)` at the end of the setup (prototype kernels of this slice).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "targets"))
from _callgrind import per_call  # noqa: E402

ENUM = """
import enum
class Color(enum.Enum):
    RED = 1; GREEN = 2; BLUE = 3
class Perm(enum.Flag):
    R = 4; W = 2; X = 1
class Prio(enum.IntEnum):
    LOW = 1; HIGH = 2
class Kind(enum.StrEnum):
    A = 'a'; B = 'b'
m = Color.GREEN; rw = Perm.R | Perm.W; r = Perm.R; w = Perm.W; x = Perm.X; ip = Prio.HIGH; ks = Kind.B
d = {Color.RED: 1, Color.GREEN: 2, Color.BLUE: 3}
one = 1; two = 2
"""

DC = """
import dataclasses
from dataclasses import dataclass, field
@dataclass
class P:
    x: int
    y: int
    z: int = 0
@dataclass(frozen=True)
class F:
    x: int
    y: int
    z: int = 0
@dataclass(slots=True)
class S:
    x: int
    y: int
    z: int = 0
@dataclass
class L:
    x: int
    tags: list = field(default_factory=list)
@dataclass
class PI:
    x: int
    y: int
    def __post_init__(self):
        pass
@dataclass(frozen=True, slots=True, kw_only=True)
class E:
    a: int
    b: str
    c: float = 0.0
@dataclass(order=True, frozen=True)
class O:
    a: int
    b: str
p = P(1, 2); q = P(1, 2); f = F(1, 2); g = F(1, 3); o1 = O(1, 'a'); o2 = O(1, 'b')
one = 1; two = 2
"""

NT = """
import collections, typing
NT = collections.namedtuple('NT', 'x y z')
class TN(typing.NamedTuple):
    x: int
    y: int
    z: int = 0
t = (1, 2, 3); nt = NT(1, 2, 3); tn = TN(1, 2)
one = 1; two = 2; three = 3
"""

CTX = """
import contextlib
@contextlib.contextmanager
def cm():
    yield 1
@contextlib.contextmanager
def cm2(a, b):
    yield a
one = 1; two = 2
"""

SD = """
import functools
@functools.singledispatch
def fmt(v):
    return 'obj'
@fmt.register
def _(v: int):
    return 'int'
class H:
    @functools.singledispatchmethod
    def handle(self, v):
        return 'obj'
    @handle.register
    def _(self, v: int):
        return 'int'
h = H(); one = 1
fmt(1); h.handle(1)
dispatch = fmt.dispatch
"""

FT = """
import functools
class C:
    def __init__(self, v): self.v = v
    @functools.cached_property
    def p(self): return self.v
@functools.total_ordering
class T:
    __slots__ = ('v',)
    def __init__(self, v): self.v = v
    def __lt__(self, o): return self.v < o.v
    def __eq__(self, o): return self.v == o.v
a = T(1); b = T(2)
"""

TY = """
import typing, collections.abc
@typing.runtime_checkable
class Closer(typing.Protocol):
    def close(self): ...
class Res:
    def close(self): pass
class Plain:
    pass
res = Res(); pl = Plain(); dd = {}
cast = typing.cast
"""

RE = """
import re
s = 'hello world 12345'
re.match(r'\\w+', s); re.search(r'\\d+', s); re.sub(r'o', '0', s); re.split(r'\\s+', s)
"""

MISC = """
import html, uuid, base64, os, inspect
s = 'Hello, world'; s2 = '<a href="x">Tom & Jerry</a>'
hx = '12345678123456781234567812345678'; u = uuid.UUID(hx); b = b'0123456789abcdef0123'
def fn(a, b, c=1, *args, d=2, **kw): pass
sig = inspect.signature(fn)
class A: pass
class B: pass
a = A()
"""

WR = """
import weakref
class K: pass
ks = [K() for _ in range(10)]
wkd = weakref.WeakKeyDictionary((k, i) for i, k in enumerate(ks))
wvd = weakref.WeakValueDictionary((i, k) for i, k in enumerate(ks))
k0 = ks[3]
"""

CELLS = [
    # enum
    ("enum.value", ENUM, "m.value"),
    ("enum.name", ENUM, "m.name"),
    ("intenum.value", ENUM, "ip.value"),
    ("enum.call", ENUM, "Color(two)"),
    ("enum.call_member", ENUM, "Color(m)"),
    ("enum.hash", ENUM, "hash(m)"),
    ("enum.dict_lookup", ENUM, "d[m]"),
    ("flag.or", ENUM, "r | w"),
    ("flag.and", ENUM, "rw & r"),
    ("flag.contains", ENUM, "r in rw"),
    ("flag.invert", ENUM, "~r"),
    ("strenum.str", ENUM, "str(ks)"),
    # dataclasses
    ("dc.init_pos", DC, "P(one, two)"),
    ("dc.init_kw", DC, "P(x=one, y=two)"),
    ("dc.init_frozen", DC, "F(one, two)"),
    ("dc.init_slots", DC, "S(one, two)"),
    ("dc.init_frozen_kw", DC, "F(x=one, y=two)"),
    ("dc.init_frozen_slots_kwonly", DC, "E(a=one, b='s')"),
    ("dc.init_factory", DC, "L(one)"),
    ("dc.init_postinit", DC, "PI(one, two)"),
    ("dc.eq", DC, "p == q"),
    ("dc.hash_frozen", DC, "hash(f)"),
    ("dc.repr", DC, "repr(p)"),
    ("dc.lt_order", DC, "o1 < o2"),
    # namedtuple
    ("nt.new", NT, "NT(one, two, three)"),
    ("nt.new_kw", NT, "NT(x=one, y=two, z=three)"),
    ("tn.new_default", NT, "TN(one, two)"),
    ("nt.make", NT, "NT._make(t)"),
    ("nt.replace", NT, "nt._replace(x=two)"),
    ("nt.repr", NT, "repr(nt)"),
    ("nt.asdict", NT, "nt._asdict()"),
    # contextlib
    ("ctx.create", CTX, "cm()"),
    ("ctx.with", CTX, "with cm(): pass"),
    ("ctx.with_args", CTX, "with cm2(one, two): pass"),
    # singledispatch
    ("sd.call", SD, "fmt(one)"),
    ("sd.dispatch", SD, "dispatch(int)"),
    ("sdm.call", SD, "h.handle(one)"),
    ("sdm.get", SD, "h.handle"),
    # functools
    ("cached_property.first", FT, "C(1).p"),
    ("total_ordering.ge", FT, "a >= b"),
    # typing / abc
    ("protocol.isinstance_pos", TY, "isinstance(res, Closer)"),
    ("protocol.isinstance_neg", TY, "isinstance(pl, Closer)"),
    ("abc.isinstance_mapping", TY, "isinstance(dd, collections.abc.Mapping)"),
    ("abc.isinstance_mapping_neg", TY, "isinstance(pl, collections.abc.Mapping)"),
    ("typing.cast", TY, "cast(int, res)"),
    # isinstance
    ("isinstance.pos", MISC, "isinstance(a, A)"),
    ("isinstance.neg", MISC, "isinstance(a, B)"),
    # re
    ("re.match", RE, "re.match(r'\\w+', s)"),
    ("re.search", RE, "re.search(r'\\d+', s)"),
    ("re.sub", RE, "re.sub(r'o', '0', s)"),
    ("re.split", RE, "re.split(r'\\s+', s)"),
    ("re.sub_template_fallback", RE, "re.sub(r'(o)', r'<\\1>', s)"),
    ("re.match_flag", RE, "re.match(r'\\w+', s, re.I)"),
    # misc stdlib
    ("html.escape_plain", MISC, "html.escape(s)"),
    ("html.escape_special", MISC, "html.escape(s2)"),
    ("uuid.parse", MISC, "uuid.UUID(hx)"),
    ("uuid.str", MISC, "str(u)"),
    ("base64.b64encode", MISC, "base64.b64encode(b)"),
    ("os.environ.get", MISC, "os.environ.get('HOME')"),
    ("os.environ[]", MISC, "os.environ['HOME']"),
    ("inspect.getfullargspec", MISC, "inspect.getfullargspec(fn)"),
    ("inspect.signature", MISC, "inspect.signature(fn)"),
    ("inspect.bind", MISC, "sig.bind(1, 2)"),
    # weakref
    ("wkd.getitem", WR, "wkd[k0]"),
    ("wvd.getitem", WR, "wvd[3]"),
]


def run(cell, variant, groups):
    name, setup, stmt = cell
    if variant == "proto":
        setup = setup + (f"\nimport rustique._rustique as _R\n_R.proto_enable({groups!r})\n"
                         "import rustique._proto_machinery as _PM\n_PM.adopt_all(globals())\n")
    try:
        return per_call(stmt, setup, n=200)
    except Exception as e:  # noqa: BLE001
        return f"ERR {str(e)[-300:]}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only")
    ap.add_argument("--variants", default="plain")
    ap.add_argument("--groups", default="all")
    ap.add_argument("-j", type=int, default=8)
    ap.add_argument("-o")
    a = ap.parse_args()
    cells = [c for c in CELLS if not a.only or re.search(a.only, c[0])]
    variants = a.variants.split(",")
    jobs = [(c, v) for c in cells for v in variants]
    with ThreadPoolExecutor(a.j) as ex:
        res = list(ex.map(lambda cv: run(cv[0], cv[1], a.groups), jobs))
    out = {}
    for (c, v), r in zip(jobs, res):
        out.setdefault(c[0], {})[v] = r
    for name, vs in out.items():
        base = vs.get("plain")
        parts = []
        for v, r in vs.items():
            if isinstance(r, str):
                parts.append(f"{v}={r}")
            else:
                ratio = f" ({r / base:.3f})" if isinstance(base, float) and v != "plain" else ""
                parts.append(f"{v}={r:,.0f}{ratio}")
        print(f"{name:30s} " + "  ".join(parts), flush=True)
    if a.o:
        json.dump(out, open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
