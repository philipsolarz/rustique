"""Parity scoreboard: every builtin container operation, Rustique vs the builtin.

Dimensions
    type     list, tuple, dict, set, frozenset (Rustique: List, Tuple, Dict, Set, FrozenSet;
             a type Rustique does not provide yet is reported as `missing`)
    storage  int, float, str, mixed, object (tuples)
    size     0, 1, 8, 100, 10k, 1M
    build    whatever interpreter runs the script (run it once per build: GIL and 3.15t)
    op       literals/displays, comprehensions, unpacking, `in`, iteration, `len`, `==`,
             hashing, indexing, mutation, json, pickle, sorted/sum/min/max/any/all/
             enumerate/zip, ...

Each cell is measured for both implementations with the *same* statement, compiled into a
fresh function whose locals hold the inputs (so the specialising interpreter sees ordinary
code). Constructs that only builtins can express (literals, displays, comprehensions) use
the provisional rewrite form on the Rustique side (e.g. `List((a, b, c))`), until the
takeover mechanism (docs/TAKEOVER.md) produces them for real.

Implementations (`--impls`, the builtin is always measured as the reference)
    rustique  the Rustique types (`rs.List`, ...) running the same statements, in a process
              where `rustique.enable()` is active when it exists (the whole product: builtin
              functions such as `sum` dispatch to native kernels)
    enabled   the builtin types in a process where `rustique.enable()` is active (the
              in-place acceleration tier, docs/TAKEOVER.md); compared against plain CPython
              in a separate process, results cross-checked by digest

Metrics
    time    pyperf (`bench_time_func`, worker processes, calibrated loops). Mutating
            operations get fresh inputs prepared outside the timed region.
    instr   instructions per operation under callgrind (valgrind), measuring exactly the
            timed region via client requests; immune to machine load. Computed from two
            runs (L and 2L loops) so fixed costs cancel. Default sizes 0..100. Workers run
            with PYTHONHASHSEED=0 and at most `--chunk` cells each (callgrind's memory grows
            per measured cell); keep `--shards` (concurrent workers) at 2-3.

Every cell is verified first: both implementations must produce equal results (Rustique
containers are converted to builtins for the comparison). `error` / `mismatch` cells are
reported, never dropped.

Usage
    python benches/scoreboard.py list [--type list --op 'x[i]' --storage int --size 8]
    python benches/scoreboard.py check  [filters]                 # verification only
    python benches/scoreboard.py instr  [filters] -o instr.json   # callgrind
    python benches/scoreboard.py time   [filters] -o time.json [--shards 6]   # pyperf
    python benches/scoreboard.py report time*.json instr*.json [--impl enabled] [-o board.md]

Filters take comma-separated values; `--op` matches substrings of op names.
Build with optimisations first (`maturin develop --release`).
"""

from __future__ import annotations

import argparse
import copy as _copy
import gc
import glob
import itertools
import json
import os
import pickle
import platform
import random
import shutil
import subprocess
import sys
import sysconfig
import tempfile
import time
from dataclasses import dataclass, field
from typing import Any, Callable

import rustique as rs

TYPES = ("list", "tuple", "dict", "set", "frozenset")
STORAGES = ("int", "float", "str", "mixed", "object")
SIZES = (0, 1, 8, 100, 10_000, 1_000_000)
SMALL_SIZES = (0, 1, 8, 100)
SEQ = ("list", "tuple")
HASHED = ("dict", "set", "frozenset")
SETS = ("set", "frozenset")
ALL = TYPES

BUILTIN_CLASS = {"list": list, "tuple": tuple, "dict": dict, "set": set, "frozenset": frozenset}
RUSTIQUE_NAME = {"list": "List", "tuple": "Tuple", "dict": "Dict", "set": "Set", "frozenset": "FrozenSet"}
ORDERED = ("int", "float", "str", "object")  # storages whose values are mutually orderable
NUMERIC = ("int", "float")


def rustique_class(type_name: str) -> Any:
    return getattr(rs, RUSTIQUE_NAME[type_name], None)


# --------------------------------------------------------------------------------------
# Data
# --------------------------------------------------------------------------------------


def values(storage: str, n: int, seed: int, *, unique: bool) -> list[Any]:
    """`n` deterministic values of one storage kind. `unique` values are distinct (keys).
    Large inputs are cached on disk: every pyperf worker process needs them."""
    if n < 10_000:
        return _values(storage, n, seed, unique=unique)
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-scoreboard", "data")
    path = os.path.join(cache, f"{storage}-{n}-{seed}-{int(unique)}.pickle")
    try:
        with open(path, "rb") as f:
            return pickle.load(f)
    except (OSError, EOFError, pickle.UnpicklingError):
        data = _values(storage, n, seed, unique=unique)
        os.makedirs(cache, exist_ok=True)
        tmp = f"{path}.{os.getpid()}.tmp"
        with open(tmp, "wb") as f:
            pickle.dump(data, f, protocol=pickle.HIGHEST_PROTOCOL)
        os.replace(tmp, path)
        return data


def _values(storage: str, n: int, seed: int, *, unique: bool) -> list[Any]:
    rng = random.Random(seed)
    if storage == "int":
        return rng.sample(range(-(10**6), 10**6), n) if unique else [rng.randrange(-(10**6), 10**6) for _ in range(n)]
    if storage == "float":
        return [v + 0.5 for v in rng.sample(range(-(10**6), 10**6), n)] if unique else [rng.uniform(-1e6, 1e6) for _ in range(n)]
    if storage == "str":
        return [f"s{v}" for v in rng.sample(range(10**7), n)]
    if storage == "object":
        return [(v, f"o{v}") for v in rng.sample(range(10**7), n)]
    if storage == "mixed":
        base = rng.sample(range(10**6), n)
        if unique:
            kinds = (lambda v: v * 3, lambda v: v * 3 + 1.5, lambda v: f"m{v * 3 + 2}")
            return [kinds[i % 3](v) for i, v in enumerate(base)]
        kinds = (lambda v: v, lambda v: v + 0.25, lambda v: f"m{v}", lambda v: None)
        return [kinds[i % 4](v) for i, v in enumerate(base)]
    raise ValueError(storage)


MISS = {"int": 10**7 + 1, "float": 1e7 + 0.5, "str": "absent", "mixed": "absent", "object": (10**8, "absent")}


def make_env(type_name: str, storage: str, n: int, impl: str) -> dict[str, Any] | None:
    """Inputs for one cell. `None` when the implementation does not exist."""
    cls = BUILTIN_CLASS[type_name] if impl in ("builtin", "enabled") else rustique_class(type_name)
    if cls is None:
        return None
    hashed = type_name in HASHED
    src = values(storage, n, seed=1000 + n, unique=hashed)
    other_new = values(storage, n, seed=2000 + n, unique=hashed)
    if hashed:
        # Second operand overlapping half of the first (set algebra, dict merge).
        seen = set(src)
        other_new = [v for v in other_new if v not in seen]
        while len(other_new) < n - n // 2:  # top up from a disjoint range
            other_new.append(MISS[storage] if not other_new else _bump(other_new[-1]))
    other = src[n // 2 :] + other_new[: n - len(src[n // 2 :])]
    env: dict[str, Any] = {
        "C": cls,
        "src": src,
        "n": n,
        "i": n // 2,
        "hit": src[n // 2] if n else None,
        "miss": MISS[storage],
        "v": src[0] if n else MISS[storage],
        "json": json,
        "pickle": pickle,
        "copy": _copy,
        "g": _sink,
    }
    if type_name == "dict":
        vals = list(range(n))
        bd = dict(zip(src, vals))
        bo = dict(zip(other, vals))
        env.update(keys=src, vals=vals, bd=bd, x=cls(bd), y=cls(bd), t=cls(bo))
    else:
        env.update(x=cls(src), y=cls(src), t=cls(other))
    return env


_ENABLED: bool | None = None


def enable_rustique() -> bool:
    """Activate `rustique.enable()` in this process (once). False if it does not exist."""
    global _ENABLED
    if _ENABLED is None:
        fn = getattr(rs, "enable", None)
        if fn is not None:
            fn()
        _ENABLED = fn is not None
    return _ENABLED


def digest(result: Any) -> str:
    """Process-independent fingerprint of a (normalised) result, for cross-process checks."""
    import hashlib

    return hashlib.sha1(repr(normalize(result)).encode("utf-8", "surrogatepass")).hexdigest()


def _bump(v: Any) -> Any:
    if isinstance(v, tuple):
        return (v[0] + 10**8, v[1] + "+")
    if isinstance(v, str):
        return v + "+"
    return v + 10**8


def _sink(*args: Any, **kwargs: Any) -> None:
    return None


# --------------------------------------------------------------------------------------
# Operations
# --------------------------------------------------------------------------------------


@dataclass
class Op:
    name: str
    types: tuple[str, ...]
    stmt: str | Callable[[int, str], str]  # statement using the env names; or a generator
    rs_stmt: str | Callable[[int, str], str] | None = None  # Rustique-side form, if different
    storages: tuple[str, ...] = STORAGES
    min_n: int = 0
    max_n: int = 10**9
    only_n: tuple[int, ...] | None = None
    prep: str | None = None  # per-iteration fresh input bound to `c` (mutating ops)
    inner: int = 1  # statement copies per loop iteration (cheap ops)
    result: str | None = None  # expression checked after one run (default: the statement)
    needs: tuple[str, ...] = ()  # extra env names that must exist
    group: str = ""

    def applies(self, type_name: str, storage: str, n: int) -> bool:
        if type_name not in self.types or storage not in self.storages:
            return False
        if self.only_n is not None:
            return n in self.only_n
        return self.min_n <= n <= self.max_n

    def code(self, impl: str, n: int, storage: str) -> str:
        stmt = self.rs_stmt if (impl == "rustique" and self.rs_stmt is not None) else self.stmt
        return stmt(n, storage) if callable(stmt) else stmt


def elems(n: int) -> str:
    return ", ".join(f"e{k}" for k in range(n))


def seq_display(open_: str, close: str, rs_wrap: bool) -> Callable[[int, str], str]:
    def gen(n: int, storage: str) -> str:
        body = elems(n) + ("," if n == 1 and open_ == "(" else "")
        if rs_wrap:
            return f"C(({body}{',' if n == 1 and open_ != '(' else ''}))"
        return f"{open_}{body}{close}"

    return gen


def set_display(rs_wrap: bool) -> Callable[[int, str], str]:
    def gen(n: int, storage: str) -> str:
        if rs_wrap:
            return f"C(({elems(n)}{',' if n == 1 else ''}))"
        return "{" + elems(n) + "}" if n else "set()"

    return gen


def frozenset_display(rs_wrap: bool) -> Callable[[int, str], str]:
    def gen(n: int, storage: str) -> str:
        return f"C(({elems(n)}{',' if n == 1 else ''}))" if rs_wrap else f"frozenset(({elems(n)}{',' if n == 1 else ''}))"

    return gen


def dict_display(rs_wrap: bool) -> Callable[[int, str], str]:
    def gen(n: int, storage: str) -> str:
        body = "{" + ", ".join(f"e{k}: {k}" for k in range(n)) + "}"
        return f"C({body})" if rs_wrap else body

    return gen


def unpack_exact(n: int, storage: str) -> str:
    return ", ".join(f"a{k}" for k in range(n)) + ("," if n == 1 else "") + " = x"


OPS: list[Op] = [
    # --- construction -------------------------------------------------------------------
    Op("literal/display", ("list",), seq_display("[", "]", False), seq_display("[", "]", True), max_n=100, group="build"),
    Op("literal/display", ("tuple",), seq_display("(", ")", False), seq_display("(", ")", True), max_n=100, group="build"),
    Op("literal/display", ("set",), set_display(False), set_display(True), max_n=100, group="build"),
    Op("literal/display", ("frozenset",), frozenset_display(False), frozenset_display(True), max_n=100, group="build"),
    Op("literal/display", ("dict",), dict_display(False), dict_display(True), max_n=100, group="build"),
    Op("comprehension", ("list",), "[e for e in src]", "C([e for e in src])", group="build"),
    Op("comprehension", ("set",), "{e for e in src}", "C({e for e in src})", group="build"),
    Op("comprehension", ("dict",), "{k: 1 for k in keys}", "C({k: 1 for k in keys})", group="build"),
    Op("genexpr ingest", SEQ + SETS, "C(e for e in src)", group="build"),
    Op("from list", SEQ + SETS, "C(src)", group="build"),
    Op("from dict", ("dict",), "C(bd)", group="build"),
    Op("from pairs", ("dict",), "C(zip(keys, vals))", group="build"),
    Op("copy C(x)", ALL, "C(x)", group="build"),
    Op("x.copy()", ("list", "dict", "set", "frozenset"), "x.copy()", group="build"),
    Op("append loop", ("list",), "c = C()\nfor e in src: c.append(e)", result="c", group="build"),
    Op("add loop", ("set",), "c = C()\nfor e in src: c.add(e)", result="c", group="build"),
    Op("setitem loop", ("dict",), "c = C()\nfor k in keys: c[k] = 1", result="c", group="build"),
    Op("extend", ("list",), "c = C()\nc.extend(src)", result="c", group="build"),
    Op("x + y", SEQ, "x + y", group="build"),
    Op("x * 2", SEQ, "x * 2", group="build"),
    Op("[*x, *y]", ("list",), "[*x, *y]", "C((*x, *y))", max_n=10_000, group="build"),
    Op("(*x, *y)", ("tuple",), "(*x, *y)", "C((*x, *y))", max_n=10_000, group="build"),
    Op("{**x, **t}", ("dict",), "{**x, **t}", "C({**x, **t})", max_n=10_000, group="build"),
    Op("x | t", ("dict",), "x | t", group="build"),
    # --- access ---------------------------------------------------------------------------
    Op("len(x)", ALL, "len(x)", inner=10, group="access"),
    Op("bool(x)", ALL, "if x: pass", inner=10, result="bool(x)", group="access"),
    Op("x[i]", SEQ, "x[i]", min_n=1, inner=10, group="access"),
    Op("x[-1]", SEQ, "x[-1]", min_n=1, inner=10, group="access"),
    Op("x[i] = v", ("list",), "x[i] = v", min_n=1, inner=10, result="x", group="access"),
    Op("x[k]", ("dict",), "x[hit]", min_n=1, inner=10, group="access"),
    Op("x[k] = v", ("dict",), "x[hit] = 1", min_n=1, inner=10, result="x", group="access"),
    Op("x.get(k)", ("dict",), "x.get(hit)", min_n=1, inner=10, group="access"),
    Op("x.get(miss)", ("dict",), "x.get(miss)", inner=10, group="access"),
    Op("x.setdefault(k)", ("dict",), "x.setdefault(hit, 0)", min_n=1, inner=10, result="x", group="access"),
    Op("hit in x", ALL, "hit in x", min_n=1, inner=10, group="access"),
    Op("miss in x", ALL, "miss in x", inner=10, group="access"),
    Op("x[a:b]", SEQ, "x[n // 4 : 3 * n // 4]", min_n=1, group="access"),
    Op("x.index(hit)", SEQ, "x.index(hit)", min_n=1, group="access"),
    Op("x.count(hit)", SEQ, "x.count(hit)", min_n=1, group="access"),
    Op("unpack exact", SEQ, unpack_exact, only_n=(1, 8), inner=10, result="x", group="access"),
    Op("a, *b = x", SEQ, "a, *b = x", min_n=1, max_n=10_000, result="(a, b)", group="access"),
    Op("g(*x)", SEQ + SETS, "g(*x)", max_n=10_000, group="access"),
    Op("g(**x)", ("dict",), "g(**x)", storages=("str",), max_n=10_000, group="access"),
    # --- iteration ------------------------------------------------------------------------
    Op("for v in x", ALL, "for e in x: pass", group="iter"),
    Op("for k, v in x.items()", ("dict",), "for k, w in x.items(): pass", group="iter"),
    Op("for v in x.values()", ("dict",), "for w in x.values(): pass", group="iter"),
    Op("list(x)", ALL, "list(x)", group="iter"),
    Op("tuple(x)", ALL, "tuple(x)", group="iter"),
    Op("enumerate", ALL, "for k, e in enumerate(x): pass", group="iter"),
    Op("zip", ALL, "for a, b in zip(x, y): pass", group="iter"),
    Op("reversed", SEQ + ("dict",), "for e in reversed(x): pass", group="iter"),
    # --- comparison / hashing -------------------------------------------------------------
    Op("x == y", ALL, "x == y", group="compare"),
    Op("x == t", ALL, "x == t", group="compare"),
    Op("x < y", SEQ, "x < y", storages=ORDERED, group="compare"),
    Op("hash(x)", ("tuple", "frozenset"), "hash(x)", group="compare"),
    # --- set algebra ----------------------------------------------------------------------
    Op("x | t", SETS, "x | t", group="setops"),
    Op("x & t", SETS, "x & t", group="setops"),
    Op("x - t", SETS, "x - t", group="setops"),
    Op("x ^ t", SETS, "x ^ t", group="setops"),
    Op("x <= t", SETS, "x <= t", group="setops"),
    Op("x.isdisjoint(t)", SETS, "x.isdisjoint(t)", group="setops"),
    Op("x.keys() & t.keys()", ("dict",), "x.keys() & t.keys()", group="setops"),
    # --- builtin functions ----------------------------------------------------------------
    Op("sorted(x)", ALL, "sorted(x)", storages=ORDERED, group="builtins"),
    Op("sum(x)", ALL, "sum(x)", storages=NUMERIC, group="builtins"),
    Op("min(x)", ALL, "min(x)", storages=ORDERED, min_n=1, group="builtins"),
    Op("max(x)", ALL, "max(x)", storages=ORDERED, min_n=1, group="builtins"),
    Op("any(x)", ALL, "any(x)", group="builtins"),
    Op("all(x)", ALL, "all(x)", group="builtins"),
    # --- mutation (fresh input per iteration) ---------------------------------------------
    Op("x.sort()", ("list",), "c.sort()", storages=ORDERED, prep="C(src)", result="c", group="mutate"),
    Op("x.reverse()", ("list",), "c.reverse()", prep="C(src)", result="c", group="mutate"),
    Op("pop() loop", ("list",), "while c: c.pop()", prep="C(src)", result="c", group="mutate"),
    Op("pop() loop", SETS[:1], "while c: c.pop()", prep="C(src)", result="c", group="mutate"),
    Op("popitem() loop", ("dict",), "while c: c.popitem()", prep="C(bd)", result="c", group="mutate"),
    Op("del x[k] loop", ("dict",), "for k in keys: del c[k]", prep="C(bd)", result="c", group="mutate"),
    Op("discard loop", ("set",), "for e in src: c.discard(e)", prep="C(src)", result="c", group="mutate"),
    Op("insert(0, v)", ("list",), "c.insert(0, v)", prep="C(src)", result="c", max_n=10_000, group="mutate"),
    Op("x.update(t)", ("dict", "set"), "c.update(t)", prep="C(x)", result="c", group="mutate"),
    Op("x.clear()", ("list", "dict", "set"), "c.clear()", prep="C(x)", result="c", group="mutate"),
    # --- serialisation / repr -------------------------------------------------------------
    Op("repr(x)", ALL, "repr(x)", group="serial"),
    Op("json.dumps", ("list", "tuple"), "json.dumps(x)", group="serial"),
    Op("json.dumps", ("dict",), "json.dumps(x)", storages=("int", "float", "str"), group="serial"),
    Op("pickle.dumps", ALL, "pickle.dumps(x)", group="serial"),
    Op("pickle round trip", ALL, "pickle.loads(pickle.dumps(x))", group="serial"),
    Op("copy.deepcopy", ALL, "copy.deepcopy(x)", max_n=10_000, group="serial"),
]


# --------------------------------------------------------------------------------------
# Cells
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Cell:
    type: str
    op: Op = field(compare=False)
    storage: str
    n: int

    @property
    def key(self) -> str:
        return f"{self.type}/{self.op.name}/{self.storage}/{self.n}"


def all_cells() -> list[Cell]:
    cells = []
    for t in TYPES:
        for op in OPS:
            for s in STORAGES:
                for n in SIZES:
                    if op.applies(t, s, n):
                        cells.append(Cell(t, op, s, n))
    return cells


def select(cells: list[Cell], args: argparse.Namespace) -> list[Cell]:
    def split(v: str | None) -> list[str] | None:
        return None if not v else [p.strip() for p in v.split(",") if p.strip()]

    types, storages, ops, groups = split(args.type), split(args.storage), split(args.op), split(args.group)
    sizes = [int(float(s)) for s in split(args.size)] if args.size else None
    out = []
    for c in cells:
        if types and c.type not in types:
            continue
        if storages and c.storage not in storages:
            continue
        if sizes is not None and c.n not in sizes:
            continue
        if ops and not any(o in c.op.name for o in ops):
            continue
        if groups and c.op.group not in groups:
            continue
        out.append(c)
    if getattr(args, "shard", None):
        k, total = (int(p) for p in args.shard.split("/"))
        out = [c for idx, c in enumerate(out) if idx % total == k]
    return out


# --------------------------------------------------------------------------------------
# Code generation
# --------------------------------------------------------------------------------------

ENV_NAMES = ("C", "src", "n", "i", "hit", "miss", "v", "x", "y", "t", "keys", "vals", "bd", "json", "pickle", "copy", "g")


def _indent(code: str, level: int) -> str:
    pad = "    " * level
    return "\n".join(pad + line for line in code.splitlines())


def build_function(cell: Cell, impl: str, env: dict[str, Any]) -> Callable[..., Any]:
    """`bench(loops, start, stop)`: runs the statement `loops` times between `start()` and
    `stop(t0)`; inputs are locals. Mutating ops get one fresh `c` per iteration, prepared
    before `start()`."""
    op = cell.op
    stmt = op.code(impl, cell.n, cell.storage)
    binds = [f"{name} = E[{name!r}]" for name in ENV_NAMES if name in env]
    if "e0" in stmt or cell.op.name == "literal/display":
        binds += [f"e{k} = src[{k}]" for k in range(cell.n)]
    lines = ["def bench(loops, start, stop, E=E):", _indent("\n".join(binds), 1)]
    if op.prep:
        lines += [
            f"    pool = [{op.prep} for _ in range(loops)]",
            "    t0 = start()",
            "    for c in pool:",
            _indent(stmt, 2),
            "    dt = stop(t0)",
            "    del pool",
            "    return dt",
        ]
    else:
        body = "\n".join([stmt] * op.inner)
        lines += ["    t0 = start()", "    for _ in range(loops):", _indent(body, 2), "    return stop(t0)"]
    src = "\n".join(lines) + "\n"
    ns: dict[str, Any] = {"E": env, "__builtins__": __builtins__}
    exec(compile(src, f"<cell {cell.key} {impl}>", "exec"), ns)
    return ns["bench"]


def build_check(cell: Cell, impl: str, env: dict[str, Any]) -> Callable[[], Any]:
    """Run the statement once on a private copy of the inputs and return its result."""
    op = cell.op
    stmt = op.code(impl, cell.n, cell.storage)
    binds = [f"{name} = E[{name!r}]" for name in ENV_NAMES if name in env]
    binds += [f"e{k} = src[{k}]" for k in range(cell.n)] if "e0" in stmt else []
    lines = ["def check(E=E):", _indent("\n".join(binds), 1)]
    if op.prep:
        lines.append(f"    c = {op.prep}")
    is_expr = op.result is None and "\n" not in stmt and _is_expression(stmt)
    if is_expr:
        lines.append(f"    return ({stmt})")
    else:
        lines.append(_indent(stmt, 1))
        lines.append(f"    return ({op.result or 'None'})")
    src = "\n".join(lines) + "\n"
    ns: dict[str, Any] = {"E": env, "__builtins__": __builtins__}
    exec(compile(src, f"<check {cell.key} {impl}>", "exec"), ns)
    return ns["check"]


def _is_expression(code: str) -> bool:
    try:
        compile(code, "<expr>", "eval")
        return True
    except SyntaxError:
        return False


# --------------------------------------------------------------------------------------
# Verification
# --------------------------------------------------------------------------------------

_RS_TO_BUILTIN = [(rs_name, py) for py_name, rs_name in RUSTIQUE_NAME.items() for py in [BUILTIN_CLASS[py_name]]]


def normalize(x: Any) -> Any:
    for rs_name, py in _RS_TO_BUILTIN:
        cls = getattr(rs, rs_name, None)
        if cls is not None and isinstance(x, cls) and type(x) is not py:
            if py is dict:
                return {normalize(k): normalize(v) for k, v in x.items()}
            return py(normalize(v) for v in x) if py is not dict else None
    if type(x) in (list, tuple):
        return type(x)(normalize(v) for v in x)
    if type(x) is dict:
        return {k: normalize(v) for k, v in x.items()}
    return x


def same(a: Any, b: Any) -> bool:
    if type(a) is not type(b):
        return False
    if isinstance(a, (list, tuple)):
        return len(a) == len(b) and all(same(p, q) for p, q in zip(a, b))
    if isinstance(a, dict):
        return list(a) == list(b) and all(same(a[k], b[k]) for k in a)
    if isinstance(a, float):
        return a == b or (a != a and b != b)
    return a == b


def verify(cell: Cell) -> tuple[str, str]:
    """('ok' | 'mismatch' | 'error' | 'missing', detail)."""
    benv = make_env(cell.type, cell.storage, cell.n, "builtin")
    renv = make_env(cell.type, cell.storage, cell.n, "rustique")
    if renv is None:
        return "missing", f"rustique.{RUSTIQUE_NAME[cell.type]} does not exist"
    assert benv is not None
    try:
        expected = normalize(build_check(cell, "builtin", benv)())
    except Exception as e:  # the builtin side must work: a harness bug
        return "error", f"builtin failed: {type(e).__name__}: {e}"
    try:
        got = normalize(build_check(cell, "rustique", renv)())
    except Exception as e:
        return "error", f"{type(e).__name__}: {str(e)[:120]}"
    if cell.op.name.startswith(("pickle.dumps",)):
        return "ok", ""  # byte streams legitimately differ; the round trip cell checks values
    if cell.type in SETS and cell.op.name in ("repr(x)",):
        # Set iteration order is unspecified; compare as sets of element reprs.
        return ("ok", "") if _set_repr_equal(expected, got) else ("mismatch", f"{expected[:60]!r} vs {got[:60]!r}")
    if cell.type in SETS and isinstance(expected, list) and isinstance(got, list) and cell.op.name in (
        "list(x)", "for v in x", "enumerate", "zip"):
        return ("ok", "") if sorted(map(repr, expected)) == sorted(map(repr, got)) else ("mismatch", "elements differ")
    if cell.type in SETS and isinstance(expected, tuple) and isinstance(got, tuple):
        return ("ok", "") if sorted(map(repr, expected)) == sorted(map(repr, got)) else ("mismatch", "elements differ")
    if same(expected, got):
        return "ok", ""
    return "mismatch", f"{_short(expected)} vs {_short(got)}"


def _set_repr_equal(a: str, b: str) -> bool:
    return a == b


def _short(v: Any) -> str:
    r = repr(v)
    return r if len(r) < 70 else r[:67] + "..."


# --------------------------------------------------------------------------------------
# Instruction counts (callgrind)
# --------------------------------------------------------------------------------------

ICOUNT_C = r"""
#include <Python.h>
#include <valgrind/callgrind.h>
/* Measured regions run instrumented; everything else (setup, verification, compiling cells)
   runs uninstrumented, so callgrind never accumulates cost centres outside the regions. */
static PyObject *zero(PyObject *m, PyObject *a) {
    CALLGRIND_START_INSTRUMENTATION;
    CALLGRIND_ZERO_STATS;
    Py_RETURN_NONE;
}
static PyObject *dump(PyObject *m, PyObject *a) {
    CALLGRIND_DUMP_STATS;
    CALLGRIND_STOP_INSTRUMENTATION;
    Py_RETURN_NONE;
}
static PyObject *on(PyObject *m, PyObject *a) { return PyLong_FromLong(RUNNING_ON_VALGRIND); }
static PyMethodDef methods[] = {{"zero", zero, METH_NOARGS, 0}, {"dump", dump, METH_NOARGS, 0},
                                {"on_valgrind", on, METH_NOARGS, 0}, {0}};
static PyModuleDef_Slot slots[] = {
#ifdef Py_mod_gil
    {Py_mod_gil, Py_MOD_GIL_NOT_USED},
#endif
    {0, NULL}};
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_icount_v2", 0, 0, methods, slots};
PyMODINIT_FUNC PyInit__icount_v2(void) { return PyModuleDef_Init(&def); }
"""


def icount_module_dir() -> str:
    """Build (once per interpreter ABI) the tiny callgrind client-request extension."""
    suffix = sysconfig.get_config_var("EXT_SUFFIX")
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-scoreboard")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_icount_v2" + suffix)
    if not os.path.exists(target):
        if not os.path.exists("/usr/include/valgrind/callgrind.h"):
            sys.exit("valgrind headers not found (/usr/include/valgrind/callgrind.h)")
        c_file = os.path.join(cache, "icount.c")
        with open(c_file, "w") as f:
            f.write(ICOUNT_C)
        include = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{include}", "-o", target, c_file])
    return cache


class Callgrind:
    def __init__(self, out_dir: str) -> None:
        sys.path.insert(0, icount_module_dir())
        import _icount_v2  # type: ignore[import-not-found]

        self.ic = _icount_v2
        self.out_dir = out_dir

    def start(self) -> int:
        self.ic.zero()
        return 0

    def stop(self, _t0: int) -> int:
        self.ic.dump()
        files = glob.glob(os.path.join(self.out_dir, "cg.out.*"))
        latest = max(files, key=os.path.getmtime)
        total = None
        with open(latest) as f:
            for line in f:
                if line.startswith("totals:"):
                    total = int(line.split()[1])
                    break
        os.remove(latest)
        if total is None:
            raise RuntimeError(f"no totals in {latest}")
        return total


def instr_loops(cell: Cell) -> int:
    if cell.op.prep:
        return max(2, min(64, 20_000 // max(1, cell.n)))
    per_iter = max(1, cell.n) if cell.op.group in ("iter", "build", "builtins", "serial", "compare", "setops") else 1
    return max(2, min(400, 40_000 // (per_iter * cell.op.inner)))


def run_instr_worker(args: argparse.Namespace) -> None:
    impls = args.impls.split(",")
    enabled_ok = False
    if "enabled" in impls or "rustique" in impls:
        # Process-wide tiers: never in the same process as the builtin reference.
        assert len(impls) == 1, "rustique/enabled are measured in their own processes"
        enabled_ok = enable_rustique()
    cg = Callgrind(args.cg_dir)
    if args.keys_file:
        with open(args.keys_file) as f:
            wanted_keys = set(f.read().split("\n"))
        cells = [c for c in all_cells() if c.key in wanted_keys]
    else:
        cells = select(all_cells(), args)
    results = []
    for cell in cells:
        entry: dict[str, Any] = {"key": cell.key, "metric": "instr"}
        for impl in impls:
            if impl == "rustique":
                entry["status"], entry["detail"] = verify(cell)
                if entry["status"] in ("missing", "error"):
                    continue
            if impl == "enabled" and not enabled_ok:  # (rustique runs either way)
                entry["status_enabled"], entry["detail_enabled"] = "missing", "rustique.enable() does not exist"
                continue
            try:
                check_env = make_env(cell.type, cell.storage, cell.n, impl)
                assert check_env is not None
                entry[f"digest_{impl}"] = digest(build_check(cell, impl, check_env)())
                env = make_env(cell.type, cell.storage, cell.n, impl)
                assert env is not None
                fn = build_function(cell, impl, env)
                loops = instr_loops(cell)
                gc.collect()
                gc.disable()
                try:
                    fn(max(2, loops // 4), lambda: 0, lambda t: 0)  # warm up / specialise
                    a = fn(loops, cg.start, cg.stop)
                    b = fn(2 * loops, cg.start, cg.stop)
                finally:
                    gc.enable()
                entry[impl] = (b - a) / (loops * cell.op.inner)
            except Exception as e:
                key = "status" if impl == "rustique" else f"status_{impl}"
                entry[key] = "error"
                entry["detail" if impl == "rustique" else f"detail_{impl}"] = f"{type(e).__name__}: {str(e)[:150]}"
        results.append(entry)
        shown = " / ".join(f"{i}={entry.get(i, '-')}" for i in impls)
        print(f"{cell.key}: {shown}", file=sys.stderr, flush=True)
    with open(args.output, "w") as f:
        json.dump({"meta": metadata("instr"), "cells": results}, f, indent=0)


def run_instr(args: argparse.Namespace) -> None:
    """Callgrind workers over bounded chunks of cells.

    Memory: callgrind's call tracking grows with the instructions it instruments on
    CPython 3.15's tail-calling interpreter and is never returned (measured: ~15 MB per
    cell for dict/str cells; 8.8 GB when instrumenting a whole run). So only the measured
    regions are instrumented (`--instr-atstart=no` + client requests) and each worker
    process measures at most `--chunk` cells (~150-300 MB peak at the default 10)."""
    if args.size is None:
        args.size = ",".join(map(str, SMALL_SIZES))
    wanted = [i.strip() for i in args.impls.split(",") if i.strip()]
    groups = [["builtin"]] + [[i] for i in ("rustique", "enabled") if i in wanted]
    keys = [c.key for c in select(all_cells(), args)]
    chunks = [keys[i : i + args.chunk] for i in range(0, len(keys), args.chunk)]
    jobs = [(g, group, k, chunk) for g, group in enumerate(groups) for k, chunk in enumerate(chunks)]
    tmp = tempfile.mkdtemp(prefix="scoreboard-cg-")
    outs: list[str] = []
    running: list[subprocess.Popen[bytes]] = []
    failed: list[int] = []
    done = 0
    for g, group, k, chunk in jobs:
        while len(running) >= max(1, args.shards):
            running, finished = _reap(running)
            failed += [rc for rc in finished if rc]
            done += len(finished)
        cg_dir = os.path.join(tmp, f"g{g}c{k}")
        os.makedirs(cg_dir)
        keys_file = os.path.join(cg_dir, "keys.txt")
        with open(keys_file, "w") as f:
            f.write("\n".join(chunk))
        out = os.path.join(tmp, f"instr{g}-{k}.json")
        outs.append(out)
        cmd = [
            "valgrind", "--tool=callgrind", "--instr-atstart=no", f"--callgrind-out-file={cg_dir}/cg.out",
            "--dump-instr=no", "--dump-line=no", "--compress-strings=no", "-q", sys.executable,
            os.path.abspath(__file__), "_instr_worker", "--impls", ",".join(group), "--keys-file", keys_file,
            "--cg-dir", cg_dir, "-o", out,
        ]
        # Fixed str hashing: dict/set probe sequences (and so instruction counts) are
        # otherwise randomised per process (~3% run-to-run drift on str keys).
        env = {**os.environ, "PYTHONHASHSEED": "0"}
        running.append(subprocess.Popen(cmd, stderr=subprocess.DEVNULL if args.quiet else None, env=env))
    while running:
        running, finished = _reap(running)
        failed += [rc for rc in finished if rc]
    if failed:
        sys.exit(f"instr workers failed: {failed}")
    merged: dict[str, dict[str, Any]] = {}
    meta = None
    for out in outs:
        with open(out) as f:
            data = json.load(f)
        meta = meta or data["meta"]
        for c in data["cells"]:
            merged.setdefault(c["key"], {}).update(c)
    for c in merged.values():
        if "digest_enabled" in c and "digest_builtin" in c and "status_enabled" not in c:
            same_result = c["digest_enabled"] == c["digest_builtin"]
            c["status_enabled"] = "ok" if same_result else "mismatch"
    meta = meta or metadata("instr")
    meta["impls"] = wanted
    with open(args.output, "w") as f:
        json.dump({"meta": meta, "cells": list(merged.values())}, f, indent=0)
    shutil.rmtree(tmp, ignore_errors=True)
    print(f"wrote {args.output} ({len(merged)} cells)")


def _reap(running: list[subprocess.Popen[bytes]]) -> tuple[list[subprocess.Popen[bytes]], list[int]]:
    """Wait until at least one worker exits; returns (still running, exit codes)."""
    while True:
        still, finished = [], []
        for p in running:
            rc = p.poll()
            (still if rc is None else finished).append(p if rc is None else rc)
        if finished or not still:
            return still, finished
        time.sleep(0.05)


def filter_cli(args: argparse.Namespace) -> list[str]:
    """The filter options of `args` as command-line arguments (for worker processes)."""
    cmd: list[str] = []
    for name in ("type", "storage", "op", "group", "size", "impls"):
        value = getattr(args, name, None)
        if value:
            cmd.extend((f"--{name}", value))
    return cmd


# --------------------------------------------------------------------------------------
# Wall time (pyperf)
# --------------------------------------------------------------------------------------


def run_time_worker() -> None:
    """Executed by `time` (master and pyperf workers): registers every selected cell."""
    import pyperf

    def add_cmdline_args(cmd: list[str], args: argparse.Namespace) -> None:
        cmd.append("_time_worker")
        for name in ("type", "storage", "op", "group", "size", "shard", "impls", "status_file"):
            value = getattr(args, name)
            if value:
                cmd.extend((f"--{name.replace('_', '-')}", value))

    runner = pyperf.Runner(add_cmdline_args=add_cmdline_args, metadata={"rustique_build": metadata("time")["build"]})
    add_filters(runner.argparser)
    runner.argparser.add_argument("--shard")
    runner.argparser.add_argument("--impls", default="rustique")
    runner.argparser.add_argument("--status-file")
    # pyperf appends `add_cmdline_args` after its own worker options, so the marker can be
    # anywhere in argv.
    args = runner.parse_args([a for a in sys.argv[1:] if a != "_time_worker"])
    cells = select(all_cells(), args)
    impls = ["builtin"] + [i for i in args.impls.split(",") if i in ("rustique", "enabled")]
    # pyperf numbers benchmarks by registration order, so master and workers must register
    # exactly the same ones: the skip decision comes from one precomputed status file.
    statuses: dict[str, list[str]] = {}
    if args.status_file:
        with open(args.status_file) as f:
            statuses = json.load(f)
    for cell in cells:
        for impl in impls:
            name = f"{cell.key}/{impl}"
            state: dict[str, Any] = {}

            def time_func(loops: int, cell: Cell = cell, impl: str = impl, state: dict[str, Any] = state) -> float:
                # Each pyperf worker process runs exactly one benchmark, so enabling the
                # process-wide tier here never affects a builtin measurement.
                if impl == "enabled" and not enable_rustique():
                    raise RuntimeError("rustique.enable() does not exist")
                if impl == "rustique":
                    enable_rustique()  # the whole product, when it exists
                if "fn" not in state:
                    env = make_env(cell.type, cell.storage, cell.n, impl)
                    if env is None:
                        raise RuntimeError("missing implementation")
                    state["fn"] = build_function(cell, impl, env)
                gc.collect()
                gc.disable()
                try:
                    return state["fn"](loops, time.perf_counter, lambda t0: time.perf_counter() - t0)
                finally:
                    gc.enable()

            if impl == "rustique" and rustique_class(cell.type) is None:
                continue
            if impl == "enabled" and getattr(rs, "enable", None) is None:
                continue
            if impl == "rustique" and statuses.get(cell.key, ["ok"])[0] in ("missing", "error"):
                continue
            runner.bench_time_func(name, time_func, inner_loops=cell.op.inner)


def small_variant(cell: Cell) -> Cell:
    """The same cell at the largest applicable size <= 100: errors and missing types do not
    depend on the size, and verifying 1M-element inputs would dominate the run."""
    if cell.n <= 100:
        return cell
    for n in reversed(SMALL_SIZES):
        if cell.op.applies(cell.type, cell.storage, n):
            return Cell(cell.type, cell.op, cell.storage, n)
    return cell


def write_statuses(cells: list[Cell], path: str) -> None:
    statuses = {c.key: list(verify(small_variant(c))) for c in cells}
    with open(path, "w") as f:
        json.dump(statuses, f)


def run_time(args: argparse.Namespace) -> None:
    shards = max(1, args.shards)
    # pyperf refuses to overwrite its output: drop results of an earlier run.
    for stale in glob.glob(glob.escape(args.output) + "*"):
        if stale == args.output or stale.startswith(args.output + ".shard") or stale.endswith(".status.json"):
            os.remove(stale)
    args.status_file = args.output + ".status.json"
    write_statuses(select(all_cells(), args), args.status_file)
    if shards == 1 and args.affinity is None:
        os.execv(sys.executable, [sys.executable, os.path.abspath(__file__), "_time_worker", *time_cli(args, None)])
    procs, outs = [], []
    cpus = sorted(os.sched_getaffinity(0))
    for k in range(shards):
        out = f"{args.output}.shard{k}.json"
        outs.append(out)
        cpu = cpus[(k * 2 + 1) % len(cpus)] if args.affinity is None else int(args.affinity.split(",")[k % len(args.affinity.split(","))])
        cmd = [sys.executable, os.path.abspath(__file__), "_time_worker", *time_cli(args, f"{k}/{shards}", out),
               f"--affinity={cpu}"]
        procs.append(subprocess.Popen(cmd, stdout=subprocess.DEVNULL if args.quiet else None))
    rc = [p.wait() for p in procs]
    if any(rc):
        print(f"warning: some shards failed: {rc}", file=sys.stderr)
    print("shard outputs:", " ".join(outs))


def time_cli(args: argparse.Namespace, shard: str | None, out: str | None = None) -> list[str]:
    cmd = filter_cli(args)
    if shard:
        cmd.extend(("--shard", shard))
    if getattr(args, "status_file", None):
        cmd.extend(("--status-file", args.status_file))
    cmd.extend(("-o", out or args.output, "-p", str(args.processes), "-n", str(args.values), "-w", "1",
                "--min-time", str(args.min_time), "--quiet"))
    return cmd


# --------------------------------------------------------------------------------------
# Report
# --------------------------------------------------------------------------------------


def size_label(n: int) -> str:
    return {10_000: "10k", 1_000_000: "1M"}.get(n, str(n))


def load_results(paths: list[str]) -> tuple[dict[tuple[str, str], dict[str, Any]], list[dict[str, Any]]]:
    """{(build, key): {"instr": {impl: Ir/op}, "time": {impl: s}, "noise": {impl: rel std},
    "status": rustique verification, "status_enabled": enabled-tier verification}}"""
    cells: dict[tuple[str, str], dict[str, Any]] = {}
    metas = []
    for path in paths:
        if path.endswith(".status.json"):
            continue  # verification sidecar of a pyperf result, read with it
        with open(path) as f:
            data = json.load(f)
        if "cells" in data:  # instr
            build = data["meta"]["build"]
            metas.append(data["meta"])
            for c in data["cells"]:
                e = cells.setdefault((build, c["key"]), {})
                for k in ("status", "detail", "status_enabled", "detail_enabled"):
                    if k in c:
                        e[k] = c[k]
                for impl in IMPLS:
                    if impl in c:
                        e.setdefault("instr", {})[impl] = c[impl]
        else:  # pyperf suite
            import pyperf

            suite = pyperf.BenchmarkSuite.load(path)
            meta = suite.get_metadata()
            build = meta.get("rustique_build") or _build_from_meta(meta)
            metas.append({"build": build, **{k: str(v) for k, v in meta.items() if k in ("python_version", "hostname")}})
            statuses: dict[str, list[str]] = {}
            for sp in [path + ".status.json", path.split(".shard")[0] + ".status.json"]:
                if os.path.exists(sp):
                    with open(sp) as f:
                        statuses.update(json.load(f))
            for b in suite.get_benchmarks():
                key, impl = b.get_name().rsplit("/", 1)
                e = cells.setdefault((build, key), {})
                e.setdefault("time", {})[impl] = b.median()
                e.setdefault("noise", {})[impl] = _rel_std(b)
            for key, st in statuses.items():
                e = cells.setdefault((build, key), {})
                e.setdefault("status", st[0])
                e.setdefault("detail", st[1])
    return cells, metas


IMPLS = ("builtin", "rustique", "enabled")


def cell_ratio(e: dict[str, Any], impl: str, n: int) -> tuple[float | None, str | None, str]:
    """(ratio vs builtin, status, metric) of one cell for `impl`."""
    status = e.get("status") if impl == "rustique" else e.get("status_enabled")
    for metric in (("instr", "time") if n in SMALL_SIZES else ("time", "instr")):
        values = e.get(metric, {})
        if impl in values and values.get("builtin"):
            return values[impl] / values["builtin"], status, metric
    return None, status, ""


def _rel_std(bench: Any) -> float:
    try:
        return bench.stdev() / bench.mean()
    except Exception:
        return 0.0


def _build_from_meta(meta: dict[str, Any]) -> str:
    v = str(meta.get("python_version", ""))
    return v.split()[0] if v else "?"


def metadata(metric: str) -> dict[str, Any]:
    gil = getattr(sys, "_is_gil_enabled", lambda: True)()
    ft = bool(sysconfig.get_config_var("Py_GIL_DISABLED"))
    return {
        "metric": metric,
        "build": f"{platform.python_version()}{'t' if ft else ''}",
        "gil_enabled": gil,
        "rustique": rs.__version__,
        "machine": platform.machine(),
        "cpus": os.cpu_count(),
        "git": _git_rev(),
    }


def _git_rev() -> str:
    try:
        return subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=os.path.dirname(__file__),
                                       text=True, stderr=subprocess.DEVNULL).strip()
    except Exception:
        return "?"


def fmt_ratio(r: float | None, status: str | None) -> str:
    if status == "missing":
        return "—"
    if status == "error":
        return "err"
    if r is None:
        return " "
    mark = "**" if r > 1.05 else ""
    s = f"{r:.2f}" if r < 10 else f"{r:.0f}"
    if status == "mismatch":
        s += "≠"
    return f"{mark}{s}{mark}"


def report(args: argparse.Namespace) -> None:
    cells, metas = load_results(args.files)
    impl = args.impl
    builds = sorted({b for b, _ in cells})
    all_by_key = {c.key: c for c in all_cells()}
    what = ("Rustique types ÷ builtin types" if impl == "rustique"
            else "builtin types under `rustique.enable()` ÷ plain CPython")
    out: list[str] = []
    out.append(f"Ratio = {what} (lower is faster); **bold** = above the 1.05 parity bar; `—` = missing; "
               "`err` = raised; `≠` = result differs from the builtin. Sizes 0–100 show instruction-count "
               "ratios (callgrind) when measured, larger sizes pyperf median wall-time ratios.")
    out.append("")
    summary_rows = []
    order = {op.name: i for i, op in enumerate(OPS)}
    for build in builds:
        for t in TYPES:
            keys = [k for (b, k) in cells if b == build and k.split("/")[0] == t]
            rows: dict[tuple[str, str], dict[int, str]] = {}
            at_parity = total = 0
            for k in keys:
                cell = all_by_key.get(k)
                if cell is None:
                    continue
                r, st, _ = cell_ratio(cells[(build, k)], impl, cell.n)
                if r is None and st not in ("missing", "error"):
                    continue
                rows.setdefault((cell.op.name, cell.storage), {})[cell.n] = fmt_ratio(r, st)
                total += 1
                if st not in ("missing", "error", "mismatch") and r is not None and r <= 1.05:
                    at_parity += 1
            if not rows:
                continue
            summary_rows.append((build, t, at_parity, total))
            out.append(f"### {t} — {build}  ({at_parity}/{total} cells at parity)")
            out.append("")
            out.append("| op | storage | " + " | ".join(size_label(n) for n in SIZES) + " |")
            out.append("|---|---|" + "---:|" * len(SIZES))
            for (opname, storage), by_n in sorted(rows.items(), key=lambda kv: (order.get(kv[0][0], 999), STORAGES.index(kv[0][1]))):
                name = opname.replace("|", "\\|")
                out.append(f"| {name} | {storage} | " + " | ".join(by_n.get(n, "") for n in SIZES) + " |")
            out.append("")
    head = ["| build | type | cells at parity |", "|---|---|---:|"]
    head += [f"| {b} | {t} | {p}/{n} |" for b, t, p, n in summary_rows]
    text = "\n".join(head + [""] + out) + "\n"
    if args.output:
        with open(args.output, "w") as f:
            f.write(text)
        print(f"wrote {args.output}")
    else:
        sys.stdout.write(text)


# --------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------


def add_filters(p: argparse.ArgumentParser) -> None:
    p.add_argument("--type", help="comma-separated: " + ",".join(TYPES))
    p.add_argument("--storage", help="comma-separated: " + ",".join(STORAGES))
    p.add_argument("--op", help="comma-separated substrings of op names")
    p.add_argument("--group", help="build,access,iter,compare,setops,builtins,mutate,serial")
    p.add_argument("--size", help="comma-separated sizes, e.g. 0,8,1e4")


def main() -> None:
    if "_time_worker" in sys.argv[1:]:
        run_time_worker()
        return
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("list", "check", "instr", "time", "_instr_worker"):
        sp = sub.add_parser(name)
        add_filters(sp)
        sp.add_argument("--shard")
        sp.add_argument("--impls", default="rustique", help="rustique,enabled (the builtin is always measured)")
        if name in ("instr", "time", "_instr_worker"):
            sp.add_argument("-o", "--output", default=f"scoreboard-{name.strip('_').split('_')[0]}.json")
            sp.add_argument("--shards", type=int, default=2,
                            help="concurrent worker processes (instr: ~150-300 MB each)")
            sp.add_argument("--quiet", action="store_true")
        if name == "_instr_worker":
            sp.add_argument("--cg-dir", required=True)
            sp.add_argument("--keys-file")
        if name == "instr":
            sp.add_argument("--chunk", type=int, default=10, help="cells per callgrind process (bounds memory)")
        if name == "time":
            sp.add_argument("-p", "--processes", type=int, default=2)
            sp.add_argument("-n", "--values", type=int, default=3)
            sp.add_argument("--min-time", type=float, default=0.05)
            sp.add_argument("--affinity", help="CPU list for the shards (default: odd CPUs)")
    rp = sub.add_parser("report")
    rp.add_argument("files", nargs="+")
    rp.add_argument("-o", "--output")
    rp.add_argument("--impl", default="rustique", choices=("rustique", "enabled"))
    args = p.parse_args()

    if args.cmd == "list":
        for c in select(all_cells(), args):
            print(c.key)
    elif args.cmd == "check":
        counts: dict[str, int] = {}
        for c in select(all_cells(), args):
            status, detail = verify(c)
            counts[status] = counts.get(status, 0) + 1
            if status != "ok":
                print(f"{status:8} {c.key}: {detail}")
        print(counts)
    elif args.cmd == "instr":
        run_instr(args)
    elif args.cmd == "_instr_worker":
        run_instr_worker(args)
    elif args.cmd == "time":
        run_time(args)
    elif args.cmd == "report":
        report(args)


if __name__ == "__main__":
    main()
