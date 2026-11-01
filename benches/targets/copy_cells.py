"""Callgrind cells for groups `copy` and `dataclasses` (docs/targets/copy.md):
Ir per call, plain vs enabled, region-instrumented (benches/targets/_callgrind.py).

    python benches/targets/copy_cells.py [-o out.json] [--only substr] [--jit]

Cells: small realistic inputs (a 10-field record dict, a small dataclass, a list of 5
dicts, ...), every hook and fallback path (`__deepcopy__`, registered copiers, reductors,
subclasses, non-dict memo, extra arguments), shallow copies and the dataclasses helpers.
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import per_call  # noqa: E402

SETUP = r"""
import sys
# 3.14's dataclasses looks the defining module up in sys.modules.
sys.modules.setdefault(__name__, type(sys)(__name__))
import copy, copyreg, dataclasses, datetime, decimal, enum, uuid, collections
from copy import deepcopy
class E(enum.Enum):
    A = 1
@dataclasses.dataclass
class P:
    x: int
    y: float
    name: str
@dataclasses.dataclass
class Order:
    id: int
    items: list
    tags: tuple
    when: datetime.datetime
    state: E
class Plain:
    def __init__(self, i):
        self.a = i; self.b = [i, i + 1]; self.c = f"s{i}"
class Hook:
    def __init__(self, v): self.v = v
    def __deepcopy__(self, memo):
        return Hook(copy.deepcopy(self.v, memo))
    def __copy__(self):
        return Hook(self.v)
class Disp:
    def __init__(self, v): self.v = v
def _copy_disp(x, memo):
    return Disp(copy.deepcopy(x.v, memo))
copy._deepcopy_dispatch[Disp] = _copy_disp
class Reg:
    def __init__(self, v): self.v = v
copyreg.pickle(Reg, lambda o: (Reg, (o.v,)))
class DictSub(dict):
    pass
class Slotted:
    __slots__ = ("a", "b")
    def __init__(self): self.a = 1; self.b = "x"
rec = {"id": 1, "name": "user1", "email": "u1@x.org", "score": 0.5, "active": True,
       "age": 33, "city": "Oslo", "zip": "0150", "note": None, "rank": 7}
recs = [dict(rec, id=i) for i in range(5)]
cfg = {"a": 1, "b": [1, 2, 3], "c": {"d": "x"}}
p = P(1, 2.0, "p")
order = Order(1, [P(i, 1.0, "i") for i in range(3)], ("a", "b"), datetime.datetime(2024, 1, 2, 3, 4), E.A)
plain = Plain(1)
tup = (1, "a", 2.5, None, (3, "b"))
fs = frozenset({"a", "b", "c", "d"})
s = {"a", "b", "c"}
dt = datetime.datetime(2024, 1, 2, 3, 4)
dec = decimal.Decimal("1.25")
u = uuid.UUID(int=12345)
hook = Hook([1, 2])
disp = Disp([1, 2])
reg = Reg([1, 2])
dsub = DictSub(a=1, b=[2])
slotted = Slotted()
od = collections.OrderedDict(a=1)
lst100 = list(range(100))
dct100 = dict.fromkeys(range(100))
"""

CELLS = [
    # deepcopy, native paths
    ("deepcopy int", "deepcopy(7)"),
    ("deepcopy str", "deepcopy('abc')"),
    ("deepcopy record (10 fields)", "deepcopy(rec)"),
    ("deepcopy list of 5 records", "deepcopy(recs)"),
    ("deepcopy cfg (3 keys, nested)", "deepcopy(cfg)"),
    ("deepcopy tuple of immutables", "deepcopy(tup)"),
    ("deepcopy small dataclass", "deepcopy(p)"),
    ("deepcopy dataclass tree", "deepcopy(order)"),
    ("deepcopy plain object", "deepcopy(plain)"),
    ("deepcopy slotted object", "deepcopy(slotted)"),
    ("deepcopy enum member", "deepcopy(E.A)"),
    ("deepcopy frozenset", "deepcopy(fs)"),
    ("deepcopy set", "deepcopy(s)"),
    ("deepcopy datetime", "deepcopy(dt)"),
    ("deepcopy uuid", "deepcopy(u)"),
    # hook paths
    ("deepcopy Decimal (C __deepcopy__)", "deepcopy(dec)"),
    ("deepcopy __deepcopy__ hook", "deepcopy(hook)"),
    ("deepcopy registered copier", "deepcopy(disp)"),
    ("deepcopy copyreg reductor", "deepcopy(reg)"),
    ("deepcopy dict subclass", "deepcopy(dsub)"),
    ("deepcopy OrderedDict", "deepcopy(od)"),
    # fallback paths
    ("deepcopy memo=OrderedDict (fallback args)", "deepcopy(cfg, collections.OrderedDict())"),
    ("deepcopy explicit {} memo", "deepcopy(cfg, {})"),
    # copy.copy
    ("copy int", "copy.copy(7)"),
    ("copy list100", "copy.copy(lst100)"),
    ("copy dict100", "copy.copy(dct100)"),
    ("copy record", "copy.copy(rec)"),
    ("copy plain object", "copy.copy(plain)"),
    ("copy small dataclass", "copy.copy(p)"),
    ("copy enum member", "copy.copy(E.A)"),
    ("copy __copy__ hook", "copy.copy(hook)"),
    ("copy set", "copy.copy(s)"),
    ("copy datetime", "copy.copy(dt)"),
    ("copy (fallback args: kw)", "copy.copy(x=rec)"),
    # dataclasses
    ("asdict small dataclass", "dataclasses.asdict(p)"),
    ("asdict dataclass tree", "dataclasses.asdict(order)"),
    ("astuple small dataclass", "dataclasses.astuple(p)"),
    ("replace small dataclass", "dataclasses.replace(p, x=2)"),
    ("replace no changes", "dataclasses.replace(p)"),
    ("asdict (fallback: dict_factory)", "dataclasses.asdict(p, dict_factory=collections.OrderedDict)"),
    # Fallbacks before adaptive suspension: each fallback is followed by a hit, which
    # resets the streak; fallback overhead = (pair cell) - (the hit cell's saving).
    ("pair: deepcopy memo=OrderedDict + deepcopy int", "deepcopy(cfg, collections.OrderedDict()); deepcopy(7)"),
    ("pair: copy kw + copy int", "copy.copy(x=rec); copy.copy(7)"),
    ("pair: asdict factory + asdict", "dataclasses.asdict(p, dict_factory=collections.OrderedDict); dataclasses.asdict(p)"),
]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("-o", "--out")
    ap.add_argument("--only")
    ap.add_argument("--jit", action="store_true")
    a = ap.parse_args()
    rows = []
    for name, stmt in CELLS:
        if a.only and a.only not in name:
            continue
        plain = per_call(stmt, setup=SETUP, jit=a.jit)
        enabled = per_call(stmt, setup=SETUP, enable=["copy", "dataclasses"], jit=a.jit)
        r = enabled / plain
        rows.append({"cell": name, "stmt": stmt, "plain": plain, "enabled": enabled, "ratio": r})
        print(f"{name:45s} {plain:10.0f} {enabled:10.0f} {r:7.3f}", flush=True)
    if a.out:
        with open(a.out, "w") as f:
            json.dump({"python": sys.version, "jit": a.jit, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
