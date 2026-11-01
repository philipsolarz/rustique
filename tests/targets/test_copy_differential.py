"""Differential tests: the copy / dataclasses kernels against CPython's own `copy.py` and
`dataclasses.py` (fresh unpatched copies), on random object graphs with shared
references, cycles, and every hook protocol `copy` honours. Compared: result structure and
identities, `sys.getsizeof` of built lists/dicts, each hook call with the memo contents it
saw (labelled) and the recursion budget it had, exceptions (type and message).
"""

from __future__ import annotations

import copyreg
import dataclasses
import enum
import sys
from typing import Any

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import _copy_support as S
from _copy_support import Ctx, budget


@pytest.fixture(scope="module", autouse=True)
def _enabled():
    S.enable()
    S.A.stats(True, True)
    yield
    st = S.A.stats(False, True)
    S.disable()
    assert st["copy.deepcopy"]["hits"] > 0, st["copy.deepcopy"]
    for name in S.ALL_TARGETS:
        # Explicit non-default arguments fall back; calls that reach user code restart.
        fb = {k: v for k, v in st[name]["fallback"].items() if v}
        assert set(fb) <= {"args", "deopt"}, (name, fb)


# ---------------------------------------------------------------------------------------
# Object kinds
# ---------------------------------------------------------------------------------------


class Plain:
    def m(self):
        return self


class Slotted:
    __slots__ = ("a", "b", "__weakref__")


class SlotDict:
    __slots__ = ("a", "__dict__")


@dataclasses.dataclass
class DC:
    x: Any = None
    y: Any = None


@dataclasses.dataclass(frozen=True)
class FDC:
    x: Any = None


class Color(enum.Enum):
    RED = 1
    BLUE = 2


class ListSub(list):
    pass


class DictSub(dict):
    pass


def log(*ev):
    Ctx.log.append(ev)


class Hook:
    """`__deepcopy__` / `__copy__` with a behaviour."""

    def __init__(self, mode, child=None):
        self.mode = mode
        self.child = child

    def __deepcopy__(self, memo):
        lab = Ctx.lab
        log("deepcopy", lab.obj(self), self.mode, lab.memo(memo))
        m = self.mode
        if m == "self":
            return self
        if m == "raise":
            raise ValueError("boom")
        if m == "budget":
            log("budget", budget())
            return self
        if m == "mutate":
            memo[id(self.child)] = "MARK"
        if m == "del":
            for k in list(memo):
                if k != id(memo):
                    del memo[k]
                    break
        if m == "ka":
            memo[id(memo)] = ("not", "a", "list")
        new = Hook(m)
        memo[id(self)] = new
        new.child = Ctx.cp.deepcopy(self.child, memo)
        log("after", lab.obj(self), lab.memo(memo))
        return new

    def __copy__(self):
        log("copy", Ctx.lab.obj(self), self.mode)
        if self.mode == "raise":
            raise ValueError("boom")
        return Hook(self.mode, self.child)


class Red:
    """`__reduce_ex__` returning each shape `_reconstruct` accepts (and some it rejects)."""

    def __init__(self, mode, child=None):
        self.mode = mode
        self.child = child

    @staticmethod
    def _make(mode, child):
        log("make", mode, Ctx.lab.obj(child))
        return Red(mode, child)

    @staticmethod
    def _new():
        return Red.__new__(Red)

    def __reduce_ex__(self, proto):
        log("reduce", Ctx.lab.obj(self), self.mode, proto)
        m = self.mode
        if m == "args":
            return (Red._make, (m, self.child))
        if m == "state":
            return (Red._new, (), {"mode": m, "child": self.child})
        if m == "slots":
            return (Red._new, (), (None, {"mode": m, "child": self.child}))
        if m == "setstate":
            return (RedS, (), {"mode": m, "child": self.child})
        if m == "listiter":
            return (RList, (), None, iter([self.child, 1, self.child]))
        if m == "dictiter":
            return (RDict, (), None, None, iter([("k", self.child), (2, [self.child])]))
        if m == "str":
            return "Red"
        if m == "short":
            return (Red._new,)
        if m == "long":
            return (Red._new, (), None, None, None, None)
        if m == "list":
            return [Red._new, (), {"mode": m}]
        if m == "none":
            return None
        if m == "raise":
            raise KeyError("reduce")
        raise AssertionError(m)


class RedS:
    def __setstate__(self, state):
        log("setstate", sorted((k, Ctx.lab.obj(v)) for k, v in state.items()))
        self.__dict__.update(state)


class RList(list):
    def append(self, x):
        log("append", Ctx.lab.obj(x))
        super().append(x)


class RDict(dict):
    def __setitem__(self, k, v):
        log("setitem", Ctx.lab.obj(k), Ctx.lab.obj(v))
        super().__setitem__(k, v)


class Getattr:
    """Instance `__getattr__` sees every optional lookup copy performs."""

    def __getattr__(self, name):
        log("getattr", name)
        raise AttributeError(name)


class Reg:
    """Reduced through `copyreg.dispatch_table`."""

    def __init__(self, child=None):
        self.child = child


def _reduce_reg(o):
    log("reg", Ctx.lab.obj(o))
    return (Reg, (o.child,))


copyreg.pickle(Reg, _reduce_reg)


class Disp:
    """Copied by a copier registered in `copy._deepcopy_dispatch`."""

    def __init__(self, child=None):
        self.child = child


def _copy_disp(x, memo):
    log("disp", Ctx.lab.obj(x), Ctx.lab.memo(memo))
    y = Disp()
    memo[id(x)] = y
    y.child = Ctx.cp.deepcopy(x.child, memo)
    return y


S.register_copier(Disp, _copy_disp)

HOOK_MODES = ["new", "self", "raise", "budget", "mutate", "del", "ka"]
RED_MODES = ["args", "state", "slots", "setstate", "listiter", "dictiter", "str", "short", "long", "list", "none", "raise"]
KINDS = (
    ["list", "tuple", "dict", "set", "frozenset", "plain", "slotted", "slotdict", "dc", "fdc", "enum"]
    + ["listsub", "dictsub", "method", "getattr", "reg", "disp", "bytearray", "budget"]
    + [f"hook:{m}" for m in HOOK_MODES]
    + [f"red:{m}" for m in RED_MODES]
    + (["frozendict"] if S.PY315 else [])
)
MUTABLE = {"list", "dict", "plain", "slotdict", "dc", "listsub", "dictsub", "reg", "disp"}

atoms = st.one_of(
    st.integers(-5, 5),
    st.integers(2**40, 2**70),
    st.text(max_size=3),
    st.floats(allow_nan=False),
    st.none(),
    st.booleans(),
    st.binary(max_size=3),
)
KEYS = ["a", "b", "c", 1, 2, (1, "x")]


@st.composite
def graphs(draw, kinds=KINDS, max_nodes=10):
    n = draw(st.integers(1, max_nodes))
    nodes = []
    for i in range(n):
        kind = draw(st.sampled_from(kinds))
        k = draw(st.integers(0, 3))
        kids = [
            ("n", draw(st.integers(0, i - 1))) if i and draw(st.booleans()) else ("a", draw(atoms)) for _ in range(k)
        ]
        nodes.append((kind, kids))
    backs = draw(st.lists(st.tuples(st.integers(0, n - 1), st.integers(0, n - 1)), max_size=3))
    return nodes, backs


def build(spec):
    nodes, backs = spec

    def source():
        objs: list[Any] = []

        def val(c):
            return objs[c[1]] if c[0] == "n" else c[1]

        def hashable(c):
            v = val(c)
            try:
                hash(v)
            except TypeError:
                return False
            # Identity-hashed objects would iterate in run-dependent order.
            return c[0] == "a"

        for kind, kids in nodes:
            vals = [val(c) for c in kids]
            first = vals[0] if vals else None
            if kind == "list":
                o = list(vals)
            elif kind == "tuple":
                o = tuple(vals)
            elif kind == "dict":
                o = dict(zip(KEYS, vals))
            elif kind == "frozendict":
                o = frozendict(zip(KEYS, vals))  # noqa: F821
            elif kind == "set":
                o = {val(c) for c in kids if hashable(c)}
            elif kind == "frozenset":
                o = frozenset(val(c) for c in kids if hashable(c))
            elif kind == "plain":
                o = Plain()
                for key, v in zip("xyz", vals):
                    setattr(o, key, v)
            elif kind == "slotted":
                o = Slotted()
                if vals:
                    o.a = first
            elif kind == "slotdict":
                o = SlotDict()
                o.a = first
                o.z = vals[1:]
            elif kind == "dc":
                o = DC(first, vals[1:])
            elif kind == "fdc":
                o = FDC(first if not isinstance(first, (list, dict, set)) else None)
            elif kind == "enum":
                o = Color.RED
            elif kind == "listsub":
                o = ListSub(vals)
                o.attr = first
            elif kind == "dictsub":
                o = DictSub(zip(KEYS, vals))
                o.attr = first
            elif kind == "method":
                p = Plain()
                p.v = first
                o = p.m
            elif kind == "getattr":
                o = Getattr()
                o.v = first
            elif kind == "reg":
                o = Reg(first)
            elif kind == "disp":
                o = Disp(first)
            elif kind == "bytearray":
                o = bytearray(b"ab")
            elif kind == "budget":
                o = Hook("budget")
            elif kind.startswith("hook:"):
                o = Hook(kind[5:], first)
            elif kind.startswith("red:"):
                o = Red(kind[4:], first)
            else:
                raise AssertionError(kind)
            objs.append(o)
        for i, j in backs:
            kind = nodes[i][0]
            o, t = objs[i], objs[j]
            if kind in ("list", "listsub"):
                o.append(t)
            elif kind in ("dict", "dictsub"):
                o["back"] = t
            elif kind in MUTABLE:
                o.back = t
        named = {f"N{i}": o for i, o in enumerate(objs)}
        return objs[-1], named

    return source


def deepcopy_root(cp, dc, root):
    return cp.deepcopy(root)


def deepcopy_memo(cp, dc, root):
    memo: dict = {}
    try:
        y = cp.deepcopy(root, memo)
    except Exception as e:  # noqa: BLE001
        return ("raised", type(e).__name__, str(e), Ctx.lab.memo(memo))
    return (y, Ctx.lab.memo(memo))


def copy_root(cp, dc, root):
    return cp.copy(root)


@settings(max_examples=300)
@given(graphs())
def test_deepcopy(spec):
    S.compare(deepcopy_root, build(spec))


@settings(max_examples=200)
@given(graphs())
def test_deepcopy_explicit_memo(spec):
    S.compare(deepcopy_memo, build(spec))


@settings(max_examples=200)
@given(graphs())
def test_copy(spec):
    S.compare(copy_root, build(spec))


DC_KINDS = ["list", "tuple", "dict", "dc", "fdc", "plain", "enum", "listsub", "dictsub", "set", "hook:new", "red:args"]


def asdict_root(cp, dc, root):
    return dc.asdict(DC(root, [root]))


def astuple_root(cp, dc, root):
    return dc.astuple(DC(root, (root,)))


@settings(max_examples=200)
@given(graphs(kinds=DC_KINDS))
def test_asdict(spec):
    S.compare(asdict_root, build(spec))


@settings(max_examples=200)
@given(graphs(kinds=DC_KINDS))
def test_astuple(spec):
    S.compare(astuple_root, build(spec))


# ---------------------------------------------------------------------------------------
# Recursion depth: RecursionError at the same level, and the budget hooks see.
# ---------------------------------------------------------------------------------------


def nested(kind, depth):
    def source():
        leaf: Any = Hook("budget")
        x = leaf
        for i in range(depth):
            if kind == "list":
                x = [x]
            elif kind == "dict":
                x = {"k": x}
            elif kind == "tuple":
                x = (x, i)
            elif kind == "plain":
                p = Plain()
                p.v = x
                x = p
            elif kind == "red":
                x = Red("args", x)
            elif kind == "dc":
                x = DC(x)
            elif kind == "mixed":
                x = [{"k": (DC(x), i)}]
        return x, {"root": x}

    return source


@pytest.mark.parametrize("kind", ["list", "dict", "tuple", "plain", "red", "dc", "mixed"])
@pytest.mark.parametrize("fn", [deepcopy_root, deepcopy_memo, copy_root, asdict_root])
def test_recursion_depth(kind, fn):
    old = sys.getrecursionlimit()
    try:
        sys.setrecursionlimit(160)
        for depth in (0, 1, 3, 10, 20, 30, 40, 60, 100):
            S.compare(fn, nested(kind, depth))
    finally:
        sys.setrecursionlimit(old)


@pytest.mark.parametrize("kind", ["list", "dict", "plain"])
def test_recursion_high_limit(kind):
    """A structure deeper than any C stack could recurse: the kernel's work stack is on
    the heap, as CPython's frames are."""
    old = sys.getrecursionlimit()
    try:
        sys.setrecursionlimit(400_000)
        S.compare(deepcopy_root, nested(kind, 50_000))
    finally:
        sys.setrecursionlimit(old)


# ---------------------------------------------------------------------------------------
# Targeted scenarios
# ---------------------------------------------------------------------------------------


class Mutator:
    """Hooks that mutate the *source* container while it is being iterated."""

    def __init__(self, target, how):
        self.target, self.how = target, how

    def __deepcopy__(self, memo):
        t, how = self.target, self.how
        log("mutator", how)
        if how == "grow-list":
            t.append(1)
        elif how == "shrink-list":
            t.pop()
        elif how == "grow-dict":
            t["new"] = 1
        elif how == "replace-dict":
            k = next(iter(t))
            del t[k]
            t["other"] = 1
        elif how == "clear-dict":
            t.clear()
        return self


@pytest.mark.parametrize("how", ["grow-list", "shrink-list"])
def test_list_mutated_during_copy(how):
    def source():
        x: list = [1, 2]
        x.insert(1, Mutator(x, how))
        return x, {"x": x}

    S.compare(deepcopy_root, source)


@pytest.mark.parametrize("how", ["grow-dict", "replace-dict", "clear-dict"])
def test_dict_mutated_during_copy(how):
    def source():
        x: dict = {"a": 1}
        x["m"] = Mutator(x, how)
        x["z"] = 2
        return x, {"x": x}

    S.compare(deepcopy_root, source)


def test_value_copied_before_key():
    class K:
        def __init__(self, n):
            self.n = n

        def __hash__(self):
            return hash(self.n)

        def __eq__(self, o):
            return isinstance(o, K) and o.n == self.n

        def __deepcopy__(self, memo):
            log("key", self.n, Ctx.lab.memo(memo))
            return K(self.n)

    def source():
        v = Hook("new", [1])
        x = {K(1): v, K(2): Hook("new", v)}
        return x, {"x": x, "v": v}

    S.compare(deepcopy_root, source)


def test_shared_and_cycles_identity():
    def source():
        a: list = [1]
        d = {"a": a, "b": a}
        a.append(d)
        t = (a, d, a)
        p = Plain()
        p.t = t
        p.me = p
        root = [t, p, (1, 2, "s"), t]
        return root, {"a": a, "d": d, "t": t, "p": p}

    S.compare(deepcopy_root, source)
    S.compare(deepcopy_memo, source)


def test_tuple_recursive_through_list():
    def source():
        t = ([],)
        t[0].append(t)
        return t, {"t": t}

    S.compare(deepcopy_memo, source)


def test_uncopyable():
    class NoReduce:
        __reduce_ex__ = None
        __reduce__ = None

    S.compare(deepcopy_root, lambda: ([1, NoReduce()], {}))
    S.compare(copy_root, lambda: (NoReduce(), {}))


def test_metaclass_hash_runs_user_code():
    class Meta(type):
        def __hash__(cls):
            log("meta-hash", cls.__name__)
            return 7

        def __eq__(cls, other):
            log("meta-eq", cls.__name__)
            return cls is other

    class W(metaclass=Meta):
        pass

    S.compare(deepcopy_root, lambda: ([W(), {"k": W()}], {}))
    S.compare(copy_root, lambda: (W(), {}))


def test_atomic_and_types():
    def source():
        def f():
            pass

        x = [int, str, f, len, range(3), property(), ..., NotImplemented, 1j, b"x", (lambda: 0).__code__]
        return x, {"x": x}

    S.compare(deepcopy_root, source)
    S.compare(copy_root, source)


def test_replace():
    @dataclasses.dataclass
    class R:
        a: int
        b: list = dataclasses.field(default_factory=list)
        c: dataclasses.InitVar[int] = 0
        d: int = dataclasses.field(default=5, init=False)

        def __post_init__(self, c):
            log("post_init", self.a, c)
            self.d = self.a + c

    @dataclasses.dataclass
    class NeedInit:
        a: int
        c: dataclasses.InitVar[int]

        def __post_init__(self, c):
            pass

    cases = [
        (lambda cp, dc, r: dc.replace(r), lambda: (R(1, [2], 3), {})),
        (lambda cp, dc, r: dc.replace(r, a=7, c=1), lambda: (R(1, [2], 3), {})),
        (lambda cp, dc, r: dc.replace(r, d=1), lambda: (R(1, [2], 3), {})),
        (lambda cp, dc, r: dc.replace(r, zz=1), lambda: (R(1, [2], 3), {})),
        (lambda cp, dc, r: dc.replace(r, a=2), lambda: (NeedInit(1, 2), {})),
        (lambda cp, dc, r: dc.replace(r, a=2, c=3), lambda: (NeedInit(1, 2), {})),
        (lambda cp, dc, r: dc.replace(r), lambda: (Plain(), {})),
        (lambda cp, dc, r: dc.replace(r, x=4), lambda: (FDC(3), {})),
    ]
    for fn, src in cases:
        S.compare(fn, src)


def test_asdict_errors_and_factories():
    S.compare(lambda cp, dc, r: dc.asdict(r), lambda: (Plain(), {}))
    S.compare(lambda cp, dc, r: dc.astuple(r), lambda: ([1], {}))
    S.compare(lambda cp, dc, r: dc.asdict(r, dict_factory=list), lambda: (DC(1, [DC(2)]), {}))
    S.compare(lambda cp, dc, r: dc.astuple(r, tuple_factory=list), lambda: (DC(1, [DC(2)]), {}))


# ---------------------------------------------------------------------------------------
# Plain objects: the native `object.__reduce_ex__(4)` and its exclusions
# ---------------------------------------------------------------------------------------


class Meta(type):
    pass


class WithMeta(metaclass=Meta):
    pass


class GetState:
    def __getstate__(self):
        log("getstate", sorted(self.__dict__))
        return {"g": 1, **self.__dict__}


class NewArgs:
    def __new__(cls, *a):
        log("new", a)
        return super().__new__(cls)

    def __getnewargs__(self):
        log("getnewargs")
        return (1, 2)


class WeakSlots:
    __slots__ = ("a", "__weakref__")


class Base:
    __slots__ = ("s",)


class Derived(Base):
    pass


def _plain_cases():
    def emptied():
        p = Plain()
        p.x = 1
        _ = p.__dict__
        del p.x
        return p

    def shadow_reduce_ex():
        p = Plain()
        p.__reduce_ex__ = lambda proto: (log("inst-reduce", proto), (Plain, ()))[1]
        return p

    def shadow_getstate():
        p = Plain()
        p.v = 1
        p.__getstate__ = lambda: (log("inst-getstate"), {"z": 2})[1]
        return p

    def shadow_reduce():
        p = Plain()
        p.__reduce__ = lambda: (log("inst-reduce"), (Plain, ()))[1]
        p.v = [1]
        return p

    def single_empty_dict():
        p = Plain()
        p.d = {}
        return p

    def weak():
        w = WeakSlots()
        w.a = [1]
        return w

    def derived():
        d = Derived()
        d.s = [1]
        d.x = (2,)
        return d

    def fresh_slots_class():
        class Fresh:
            __slots__ = ("a", "b")

        f = Fresh()
        f.a = [1]
        return f

    def meta():
        w = WithMeta()
        w.v = [1]
        return w

    def getstate():
        g = GetState()
        g.v = [1]
        return g

    return [
        Plain,
        emptied,
        shadow_reduce_ex,
        shadow_getstate,
        shadow_reduce,
        single_empty_dict,
        weak,
        derived,
        fresh_slots_class,
        meta,
        getstate,
        NewArgs,
        lambda: [Plain(), Plain()],
    ]


@pytest.mark.parametrize("make", _plain_cases(), ids=lambda f: getattr(f, "__name__", "case"))
@pytest.mark.parametrize("fn", [deepcopy_root, deepcopy_memo, copy_root])
def test_plain_reduce_variants(make, fn):
    def source():
        x = make()
        return x, {"x": x}

    S.compare(fn, source)


def test_replace_instance_shadows_fields():
    def source():
        p = DC(1, [2])
        p.__dict__["__dataclass_fields__"] = ["not", "a", "dict"]
        return p, {"p": p}

    S.compare(lambda cp, dc, r: dc.replace(r, x=2), source)


# ---------------------------------------------------------------------------------------
# Memos that are not exact dicts: every operation copy.py performs on them, in order.
# ---------------------------------------------------------------------------------------


class LoggingMemo(dict):
    def __contains__(self, k):
        log("memo-in", Ctx.lab.key(k))
        return super().__contains__(k)

    def __getitem__(self, k):
        log("memo-get", Ctx.lab.key(k))
        return super().__getitem__(k)

    def __setitem__(self, k, v):
        log("memo-set", Ctx.lab.key(k), Ctx.lab.obj(v))
        super().__setitem__(k, v)

    def get(self, k, default=None):
        log("memo-.get", Ctx.lab.key(k))
        return super().get(k, default)


def deepcopy_memo_of(factory):
    def fn(cp, dc, root):
        memo = factory()
        Ctx.lab.ids[id(memo)] = "MEMO"
        y = cp.deepcopy(root, memo)
        return (y, Ctx.lab.memo(dict(memo)))

    return fn


@settings(max_examples=150)
@given(graphs())
def test_deepcopy_logging_memo(spec):
    S.compare(deepcopy_memo_of(LoggingMemo), build(spec))


@settings(max_examples=100)
@given(graphs())
def test_deepcopy_ordereddict_memo(spec):
    import collections

    S.compare(deepcopy_memo_of(collections.OrderedDict), build(spec))


# ---------------------------------------------------------------------------------------
# Hooks run under CPython's own frames (the kernel restarts before any user code)
# ---------------------------------------------------------------------------------------


class FrameHook:
    def __init__(self, child=None):
        self.child = child

    def __deepcopy__(self, memo):
        import traceback

        log("stack", [f.name for f in traceback.extract_stack()][-8:-1])
        return FrameHook(Ctx.cp.deepcopy(self.child, memo))

    def __copy__(self):
        import traceback

        log("stack", [f.name for f in traceback.extract_stack()][-8:-1])
        return FrameHook(self.child)


class RaisingHook:
    def __deepcopy__(self, memo):
        raise ValueError("boom")


def _frames_case(fn):
    def source():
        x = [{"a": (1, [FrameHook({"k": [1, 2]})])}, Plain()]
        return x, {"x": x}

    return S.compare(fn, source)


@pytest.mark.parametrize("fn", [deepcopy_root, copy_root])
def test_hook_sees_cpython_frames(fn):
    _frames_case(fn)


def test_hook_exception_traceback():
    import traceback

    def fn(cp, dc, root):
        try:
            cp.deepcopy(root)
        except ValueError as e:
            return [f.name for f in traceback.extract_tb(e.__traceback__)]
        return None

    S.compare(fn, lambda: ([{"a": [1, RaisingHook()]}], {}))


class TracingHook:
    """Turns a tracer on from inside the copy: the rest of the call must be traced as in
    CPython (the kernel is not running anymore)."""

    def __deepcopy__(self, memo):
        events = Ctx.events

        def tracer(frame, event, arg):
            if event == "call":
                events.append(frame.f_code.co_name)
            return None

        sys.settrace(tracer)
        return self


def test_tool_attached_by_hook_sees_the_rest():
    def fn(cp, dc, root):
        Ctx.events = []
        try:
            cp.deepcopy(root)
        finally:
            sys.settrace(None)
        return list(Ctx.events)

    S.compare(fn, lambda: ([TracingHook(), {"a": [1, (2, Plain())]}, Plain()], {}))
