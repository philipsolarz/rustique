"""Callgrind cells of the `core` workstream's wave-2 changes (plain vs enabled, Ir per
call, benches/targets/_callgrind.py method):

    python benches/targets/core_cells.py json      # lazy default= / cls=JSONEncoder
    python benches/targets/core_cells.py uuid      # uuid.UUID in deepcopy / asdict
    python benches/targets/core_cells.py contains  # class and tuple needles in `in`

Setups come from w2-floors' prototype cells (`docs/atlas/w2-floors.md` §2.2).
"""

from __future__ import annotations

import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import per_call  # noqa: E402

JSON_SETUP = r"""
import json, datetime, decimal, uuid
from json import dumps
rec = {"id": 12345, "name": "Ada Lovelace", "email": "ada@example.com", "active": True,
       "score": 98.5, "tags": ["admin", "math"], "age": 36, "ratio": 0.3333333333333333,
       "manager": None, "city": "London"}
rec_dt = dict(rec, created=datetime.datetime(2026, 11, 1, 12, 30), price=decimal.Decimal("9.99"))
def encode(o):
    if isinstance(o, datetime.datetime): return o.isoformat()
    if isinstance(o, decimal.Decimal): return str(o)
    raise TypeError(o)
payload = {"ts": "2026-11-02T12:00:00", "level": "INFO", "logger": "app.http", "msg": "GET /api/items 200 12.5ms",
           "module": "handlers", "line": 42, "request_id": "r-000123", "user": "u42"}
class Enc(json.JSONEncoder):
    def default(self, o):
        return encode(o)
"""
JSON = [
    ("dumps(rec)", "dumps(rec)"),
    ("dumps(rec, default=encode) [hook not needed]", "dumps(rec, default=encode)"),
    ("dumps(payload, default=str) [logging_service]", "dumps(payload, default=str)"),
    ("dumps(rec, cls=json.JSONEncoder)", "dumps(rec, cls=json.JSONEncoder)"),
    ("dumps(rec, default=encode, indent=2, sort_keys=True)", "dumps(rec, default=encode, indent=2, sort_keys=True)"),
    ("dumps(rec_dt, default=encode) [hook needed: restart]", "dumps(rec_dt, default=encode)"),
    ("dumps(rec_dt, cls=Enc) [subclass: fallback]", "dumps(rec_dt, cls=Enc)"),
    ("dumps(rec_dt, default=encode, ensure_ascii=False, separators) [json_api]",
     "dumps(rec_dt, default=encode, ensure_ascii=False, separators=(',', ':'))"),
]

ASDICT_SETUP = r"""
import sys
sys.modules.setdefault(__name__, type(sys)(__name__))
import dataclasses, datetime as dt, enum, uuid, copy
from decimal import Decimal
class Status(enum.Enum):
    NEW = "new"
@dataclasses.dataclass
class Customer:
    id: object
    name: str
    email: str
    marketing_opt_in: bool = False
@dataclasses.dataclass
class Line:
    sku: str
    qty: int
    price: Decimal
@dataclasses.dataclass
class Order:
    id: int
    customer: Customer
    city: str
    country: str
    currency: str
    lines: list
    placed_at: dt.datetime
    status: Status
    total: Decimal
    coupon: object = None
def mk(cid):
    return Order(7, Customer(cid, "Ada", "ada@example.com"), "Oslo", "NO", "EUR",
                 [Line(f"SKU-{i:05d}", i + 1, Decimal("9.99")) for i in range(4)],
                 dt.datetime(2026, 11, 2, 12, 30), Status.NEW, Decimal("39.96"))
o_uuid = mk(uuid.UUID(int=12345))
o_int = mk(12345)
u = uuid.UUID(int=12345)

c_uuid = Customer(u, "Ada", "ada@example.com")
"""
UUID = [
    ("asdict(Order, customer.id=UUID) [json_api]", "dataclasses.asdict(o_uuid)"),
    ("asdict(Order, customer.id=int)", "dataclasses.asdict(o_int)"),
    ("asdict(Customer with UUID)", "dataclasses.asdict(c_uuid)"),
    ("deepcopy(UUID)", "copy.deepcopy(u)"),
    ("deepcopy([UUID, UUID])", "copy.deepcopy([u, u])"),
    ("deepcopy(UUID, memo={})", "copy.deepcopy(u, {})"),
]

CONTAINS_SETUP = r"""
import ast
class A: pass
class B: pass
class C: pass
class D: pass
class Eq:
    def __eq__(self, o): return False
    __hash__ = object.__hash__
class M(type):
    def __eq__(s, o): return False
    __hash__ = type.__hash__
class X(metaclass=M): pass
CLS = (A, B, C)
node = ast.Name('x')
NODE_TYPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda, ast.Name)
NODE_TYPES_L = list(NODE_TYPES)
o = A(); objs = [A(), A(), o]; objs_miss = [A(), A(), A()]
e = Eq(); eqs = (Eq(), Eq())
MIXED = (A, 1, 'x', None, C)
WITH_META = (B, X)
WITH_EQ = (A(), Eq())

pairs = [(1, 'a'), (2, 'b'), (3, 'c'), (4, 'd'), (5, 'e')]
pair_hit = (4, 'd'); pair_miss = (9, 'z')
tpairs = tuple(pairs)
"""
CONTAINS = [
    ("type(node) in NODE_TYPES (hit at 5th)", "type(node) in NODE_TYPES"),
    ("type(node) in NODE_TYPES_L (list)", "type(node) in NODE_TYPES_L"),
    ("C in CLS (hit at 3rd)", "C in CLS"),
    ("D in CLS (miss)", "D in CLS"),
    ("type(o) in CLS (hit at 1st)", "type(o) in CLS"),
    ("o in objs (list, hit at 3rd)", "o in objs"),
    ("o in objs_miss (list miss)", "o in objs_miss"),
    ("D in MIXED (classes + scalars, miss)", "D in MIXED"),
    ("e in eqs [fallback: needle __eq__]", "e in eqs"),
    ("A in WITH_META [fallback after 1 item: metaclass __eq__]", "A in WITH_META"),
    ("o in WITH_EQ [fallback after 1 item: item __eq__]", "o in WITH_EQ"),
    ("3 in (1, 2, 3) [scalar path]", "3 in (1, 2, 3)"),
    ("pair_hit in pairs (tuple needle, list)", "pair_hit in pairs"),
    ("pair_miss in pairs (tuple needle, miss)", "pair_miss in pairs"),
    ("pair_hit in tpairs (tuple needle, tuple)", "pair_hit in tpairs"),
    ("pairs.count(pair_hit)", "pairs.count(pair_hit)"),
    ("pairs.index(pair_hit)", "pairs.index(pair_hit)"),
    ("objs.count(o)", "objs.count(o)"),
]

SETS = {
    "json": (JSON_SETUP, JSON, ["float", "json", "datetime"]),
    "uuid": (ASDICT_SETUP, UUID, ["copy", "dataclasses", "datetime"]),
    "contains": (CONTAINS_SETUP, CONTAINS, ["containers"]),
}


def main() -> None:
    which = sys.argv[1]
    jobs = int(sys.argv[sys.argv.index("-j") + 1]) if "-j" in sys.argv else 6
    setup, cells, groups = SETS[which]
    if "-k" in sys.argv:
        sub = sys.argv[sys.argv.index("-k") + 1]
        cells = [c for c in cells if sub in c[0]]

    def one(c):
        name, stmt = c
        return name, per_call(stmt, setup), per_call(stmt, setup, enable=groups)

    out = []
    with ThreadPoolExecutor(jobs) as ex:
        for name, a, b in ex.map(one, cells):
            out.append({"cell": name, "plain": a, "enabled": b, "ratio": b / a})
            flag = "  <-- over 1.05" if b / a > 1.05 else ""
            print(f"{name:72s} {a:9.0f} {b:9.0f} {b / a:6.3f}{flag}", flush=True)
    if "-o" in sys.argv:
        with open(sys.argv[sys.argv.index("-o") + 1], "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
