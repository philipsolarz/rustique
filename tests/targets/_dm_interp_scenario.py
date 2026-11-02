"""Run in a fresh process by test_dm_interp_compose.py: one scenario under a mode given in
argv[1] (plain | dm | interp | both); prints a JSON line with the observable log and the
counters of the data-model and interp targets."""

import collections
import contextlib
import dataclasses
import enum
import functools
import json
import re
import sys
import traceback

mode = sys.argv[1]
DM = ["enum", "generated", "contextlib", "functools"]
if mode != "plain":
    import rustique

    rustique.stats(True)
    only = (DM if mode in ("dm", "both") else []) + (["interp"] if mode in ("interp", "both") else [])
    rustique.enable(only=only, tiers=["interp"] if mode in ("interp", "both") else None)


def outcome(fn):
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001
        return ["raise", type(e).__name__, re.sub(r" at 0x[0-9a-f]+", "", str(e)),
                [f.name for f in traceback.extract_tb(e.__traceback__)[1:]]]
    return ["ok", re.sub(r" at 0x[0-9a-f]+", "", repr(r))]


log = []


@dataclasses.dataclass(frozen=True)
class F:
    x: int
    y: int = 0


@dataclasses.dataclass(frozen=True, slots=True, kw_only=True)
class S:
    a: int
    b: list = dataclasses.field(default_factory=list)


@dataclasses.dataclass(frozen=True)
class Post:
    x: int

    def __post_init__(self):
        log.append(("post", self.x))


class Plain:
    def __init__(self, a, b=1):
        self.a, self.b = a, b

    def __repr__(self):
        return f"Plain({self.a}, {self.b})"


class Weird:
    def __setattr__(self, name, value):
        log.append(("setattr", name))
        object.__setattr__(self, name, value)


class Color(enum.Enum):
    RED = 1
    GREEN = 2


NT = collections.namedtuple("NT", "a b")


@contextlib.contextmanager
def cm(v):
    yield v


@functools.singledispatch
def f(x):
    return "o"


@f.register(int)
def _(x):
    return "i"


for _ in range(50):  # warm the specializer and the overrides
    for op in [
        lambda: F(1),
        lambda: F(1, 2),
        lambda: F(x=1, y=2),
        lambda: F(),
        lambda: F(1, 2, 3),
        lambda: S(a=1),
        lambda: S(a=1, b=[2]),
        lambda: Post(3),
        lambda: Post(x=4),
        lambda: Plain(1),
        lambda: Plain(1, b=2),
        lambda: Plain(a=1, c=2),
        lambda: object.__setattr__(Weird(), "q", 1),
        lambda: object.__setattr__(F(1), "x", 9),
        lambda: F(1).__init__(5),
        lambda: (Color(2), Color.RED.value, Color.GREEN.name, hash(Color.RED) == hash("RED")),
        lambda: (NT(1, 2), NT._make([3, 4]), NT(1, 2)._replace(b=5)),
        lambda: [v for v in [cm(1).__enter__()]],
        lambda: (f(1), f("s")),
    ]:
        log.append(outcome(op))

counters = {}
if mode != "plain":
    for name, v in rustique.stats().items():
        if name.startswith(("interp.", "dataclasses.", "collections.", "enum.", "contextlib.", "functools.")):
            counters[name] = [v["hits"], sum(v["fallback"].values())]
print(json.dumps({"log": log, "counters": counters}))
