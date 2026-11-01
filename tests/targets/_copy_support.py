"""Shared helpers for the copy / dataclasses target tests.

The reference is CPython's own code: fresh, unpatched copies of `Lib/copy.py` and
`Lib/dataclasses.py` loaded under other names (`REF`, `REF_DC`). A scenario runs once
against the reference and once against the patched modules, each time on a freshly built
object graph, and everything observable is compared: the result's structure and identity
relations, every hook call with the memo contents it saw (keys and values labelled by
which object they are), the remaining recursion budget hooks see, and exceptions.
"""

from __future__ import annotations

import copy
import copyreg
import dataclasses
import enum
import importlib.util
import sys
import types
from typing import Any

from rustique import _rustique

A = _rustique._accel
COPY_TARGETS = ["copy.deepcopy", "copy.copy"]
DC_TARGETS = ["dataclasses.asdict", "dataclasses.astuple", "dataclasses.replace"]
ALL_TARGETS = COPY_TARGETS + DC_TARGETS
PY315 = sys.version_info >= (3, 15)


def _load(name: str, path: str):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


REF = _load("_rustique_copy_ref", copy.__file__)
REF_DC = _load("_rustique_dataclasses_ref", dataclasses.__file__)
REF_DC.copy = REF
# The reference must recognise fields made by the real `dataclasses.dataclass`.
for _k in ("_FIELD", "_FIELD_CLASSVAR", "_FIELD_INITVAR", "MISSING", "_FIELDS", "KW_ONLY"):
    setattr(REF_DC, _k, getattr(dataclasses, _k))


def enable() -> None:
    from rustique import _takeover as rt

    rt.enable(only=["copy", "dataclasses"])
    states = {t["name"]: t for t in rt.status()["targets"]}
    for n in ALL_TARGETS:
        assert states[n]["state"] == "installed", states[n]


def disable() -> None:
    A.disable(ALL_TARGETS)


# The module under test inside hooks: REF or the patched `copy`.
class Ctx:
    cp: Any = copy
    dc: Any = dataclasses
    log: list = []
    lab: Labeler | None = None


def budget() -> int:
    """How many more Python frames can be pushed here before RecursionError."""
    n = 0

    def r():
        nonlocal n
        n += 1
        r()

    try:
        r()
    except RecursionError:
        pass
    return n


class Labeler:
    """Stable labels for objects (and ids) seen during one run, by order of appearance."""

    def __init__(self, sources: dict[int, str]):
        self.ids = dict(sources)  # id -> label
        self.keep: list = []  # keep labelled objects alive (ids stay unique)
        self.n = 0

    def obj(self, o: Any) -> str:
        if isinstance(o, (int, float, str, bytes, bool, type(None), complex)) and not isinstance(o, enum.Enum):
            return repr(o)
        i = id(o)
        if i not in self.ids:
            self.n += 1
            self.ids[i] = f"T{self.n}:{type(o).__name__}"
            self.keep.append(o)
        return self.ids[i]

    def key(self, k: int) -> str:
        return self.ids.get(k, "?id")

    def memo(self, memo: dict) -> list:
        out = []
        ka = memo.get(id(memo))
        if isinstance(ka, list):
            # Label keep-alive objects first: memo keys of temporaries refer to them.
            for o in ka:
                self.obj(o)
        for k, v in list(memo.items()):
            if k == id(memo):
                out.append(("KA", [self.obj(o) for o in v] if isinstance(v, list) else self.obj(v)))
            else:
                out.append((self.key(k), self.obj(v)))
        return out


def canon(x: Any, lab: Labeler) -> Any:
    """The result graph as nested data: types, contents, identities, list/dict sizes."""
    seen: dict[int, str] = {}
    out = []

    def walk(o: Any) -> str:
        if isinstance(o, (int, float, str, bytes, bool, type(None), complex, range)) and not isinstance(o, enum.Enum):
            return f"{type(o).__name__}:{o!r}"
        i = id(o)
        if i in seen:
            return seen[i]
        name = lab.obj(o)
        seen[i] = name
        t = type(o)
        if t in (list, tuple) or isinstance(o, (list, tuple)):
            body = [walk(v) for v in o]
            extra = [sys.getsizeof(o)] if t is list else []
        elif isinstance(o, dict) or (PY315 and isinstance(o, frozendict)):  # noqa: F821
            body = [(walk(k), walk(v)) for k, v in o.items()]
            extra = [sys.getsizeof(o)] if t is dict else []
        elif isinstance(o, (set, frozenset)):
            body = sorted(walk(v) for v in o)
            extra = []
        elif isinstance(o, enum.Enum):
            body, extra = [o.name], []
        elif hasattr(o, "__dict__") or hasattr(type(o), "__slots__"):
            d = getattr(o, "__dict__", {})
            body = [(k, walk(v)) for k, v in d.items()]
            for s in getattr(type(o), "__slots__", ()):
                if s not in ("__weakref__", "__dict__") and hasattr(o, s):
                    body.append((s, walk(getattr(o, s))))
            extra = []
        elif isinstance(o, types.MethodType):
            body, extra = [o.__func__.__qualname__, walk(o.__self__)], []
        else:
            r = repr(o)
            body, extra = [r if " at 0x" not in r else t.__name__], []
        out.append((name, t.__name__, body, extra))
        return name

    root = walk(x)
    return root, out


def run(fn, cp: Any, dc: Any, sources_fn) -> tuple:
    """Build the sources, run `fn(cp, dc, root)`; return everything observable."""
    root, named = sources_fn()
    lab = Labeler({id(o): n for n, o in named.items()})
    Ctx.cp, Ctx.dc, Ctx.log, Ctx.lab = cp, dc, [], lab
    try:
        res = fn(cp, dc, root)
        outcome = ("ok", canon(res, lab))
    except Exception as e:  # noqa: BLE001
        outcome = ("raise", type(e).__name__, str(e))
    log = Ctx.log
    Ctx.cp, Ctx.dc, Ctx.log, Ctx.lab = copy, dataclasses, [], None
    return outcome, log, named, root


def compare(fn, sources_fn) -> tuple:
    a = run(fn, REF, REF_DC, sources_fn)
    b = run(fn, copy, dataclasses, sources_fn)
    assert a[0] == b[0], f"outcome differs:\nref  {a[0]}\nrust {b[0]}"
    assert a[1] == b[1], f"hook log differs:\nref  {a[1]}\nrust {b[1]}"
    return a, b


def register_reducer(cls, fn) -> None:
    copyreg.pickle(cls, fn)


def register_copier(cls, fn) -> None:
    copy._deepcopy_dispatch[cls] = fn
    REF._deepcopy_dispatch[cls] = fn
