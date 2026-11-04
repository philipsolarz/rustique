"""Callgrind cells of the w2-floors prototypes (docs/atlas/w2-floors.md §2): plain vs the
group enabled, Ir per call (benches/targets/_callgrind.py method).

    python benches/floors/proto_cells.py contains [-j 6]
    python benches/floors/proto_cells.py keys
    python benches/floors/proto_cells.py json_default
"""
import os, sys
from concurrent.futures import ThreadPoolExecutor
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
from _callgrind import per_call  # noqa: E402

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
    ("3 in (1, 2, 3) [scalar path, unchanged]", "3 in (1, 2, 3)"),
]

KEYS_SETUP = r"""
from operator import itemgetter, attrgetter
words = ['delta', 'Alpha', 'charlie', 'bravo', 'echo', 'Foxtrot', 'golf', 'hotel', 'India', 'juliet']
pairs = [(w, i) for i, w in enumerate(words)]
class R:
    def __init__(self, n, k): self.name = n; self.k = k
recs = [R(w, i % 3) for i, w in enumerate(words)]
key_lambda = lambda p: p[1]
g1 = itemgetter(1)
ga = attrgetter('name')
keys_only = [p[1] for p in pairs]
"""
KEYS = [
    ("sorted(words)", "sorted(words)"),
    ("sorted(words, key=str.lower)", "sorted(words, key=str.lower)"),
    ("sorted(words, key=len)", "sorted(words, key=len)"),
    ("sorted(pairs, key=lambda)", "sorted(pairs, key=key_lambda)"),
    ("sorted(pairs, key=itemgetter(1))", "sorted(pairs, key=g1)"),
    ("sorted(recs, key=attrgetter('name'))", "sorted(recs, key=ga)"),
    ("[key_lambda(p) for p in pairs] (key calls alone)", "[key_lambda(p) for p in pairs]"),
    ("list(map(g1, pairs)) (C key calls alone)", "list(map(g1, pairs))"),
    ("sorted(keys_only) (sort of the keys alone)", "sorted(keys_only)"),
    ("max(pairs, key=itemgetter(1))", "max(pairs, key=g1)"),
    ("min(words, key=len)", "min(words, key=len)"),
]

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
JSONC = [
    ("dumps(rec)", "dumps(rec)"),
    ("dumps(rec, default=encode) [no hook call]", "dumps(rec, default=encode)"),
    ("dumps(rec_dt, default=encode) [2 hook calls]", "dumps(rec_dt, default=encode)"),
    ("dumps(rec_dt, default=encode, ensure_ascii=False, separators) [json_api]", "dumps(rec_dt, default=encode, ensure_ascii=False, separators=(',', ':'))"),
    ("dumps(rec_dt, cls=Enc)", "dumps(rec_dt, cls=Enc)"),
    ("dumps(payload, default=str) [logging_service]", "dumps(payload, default=str)"),
    ("dumps(rec, cls=json.JSONEncoder)", "dumps(rec, cls=json.JSONEncoder)"),
    ("encode(dt)+encode(dec) (the hooks alone)", "encode(rec_dt['created']); encode(rec_dt['price'])"),
    ("json.JSONEncoder(default=encode) (encoder construction alone)", "json.JSONEncoder(default=encode)"),
]

SORT_SETUP = r"""
import random
random.seed(1)
def mk(kind, n, order):
    if kind == "int": base = list(range(n))
    elif kind == "float": base = [i / 7 for i in range(n)]
    else: base = [f"k{i:05d}" for i in range(n)]
    if order == "asc": return base
    if order == "desc": return base[::-1]
    if order == "rand": s = base[:]; random.shuffle(s); return s
    if order == "halves": h = n // 2; return base[h:] + base[:h]
    if order == "nearly":
        s = base[:]
        for _ in range(max(1, n // 50)):
            i, j = random.randrange(n), random.randrange(n); s[i], s[j] = s[j], s[i]
        return s
    if order == "dups": return [base[i % 3] for i in range(n)] if n else []
"""
SORT = [(f"sorted {k} n={n} {o}", f"sorted(D_{k}_{n}_{o})") for k in ("int", "float", "str") for n in (2, 5, 20, 100, 1000)
        for o in ("asc", "desc", "rand", "halves", "nearly", "dups")]
SORT_SETUP += "".join(f"D_{k}_{n}_{o} = mk({k!r}, {n}, {o!r})\n" for k in ("int", "float", "str") for n in (2, 5, 20, 100, 1000)
                      for o in ("asc", "desc", "rand", "halves", "nearly", "dups"))
SORT += [("max(strs20)", "max(D_str_20_rand)"), ("min(strs1000)", "min(D_str_1000_rand)"), ("max(strs20 asc)", "max(D_str_20_asc)"),
         ("min(ints1000 desc)", "min(D_int_1000_desc)"), ("max(floats100)", "max(D_float_100_rand)")]

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
"""
ASDICT = [("asdict(Order, customer.id=UUID) [json_api]", "dataclasses.asdict(o_uuid)"),
          ("asdict(Order, customer.id=int)", "dataclasses.asdict(o_int)"),
          ("deepcopy(UUID)", "copy.deepcopy(u)")]

SETS = {"asdict": (ASDICT_SETUP, ASDICT, ["copy", "dataclasses", "datetime"]), "sort": (SORT_SETUP, SORT, ["builtins", "containers"]), "contains": (CONTAINS_SETUP, CONTAINS, ["containers"]), "keys": (KEYS_SETUP, KEYS, ["builtins", "containers"]),
        "json_default": (JSON_SETUP, JSONC, ["float", "json", "datetime"])}

def main():
    which = sys.argv[1]
    jobs = int(sys.argv[sys.argv.index("-j") + 1]) if "-j" in sys.argv else 6
    setup, cells, groups = SETS[which]
    if "-k" in sys.argv:
        sub = sys.argv[sys.argv.index("-k") + 1]
        cells = [c for c in cells if sub in c[0]]
    def one(c):
        name, stmt = c
        a = per_call(stmt, setup)
        b = per_call(stmt, setup, enable=groups)
        return name, a, b
    with ThreadPoolExecutor(jobs) as ex:
        for name, a, b in ex.map(one, cells):
            print(f"{name:72s} {a:9.0f} {b:9.0f} {b / a:6.3f}", flush=True)

if __name__ == "__main__":
    main()
