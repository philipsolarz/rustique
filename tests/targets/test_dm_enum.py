"""Group `enum` (docs/targets/datamodel.md): differential tests (CPython's enum code vs the
kernels, same scenario run plain and enabled), mechanism tests (slots restored, tools,
subclasses and other metaclasses, mutation of the value map), free-threaded stress."""

from __future__ import annotations

import enum
import pickle
import sys
import threading

import pytest
from _dm_support import A, Counting, both, disable, enable, outcome
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

NAMES = ["enum.property.__get__", "enum.EnumType.__call__", "enum.Enum.__hash__", "enum.Flag.operators"]


@pytest.fixture(autouse=True)
def _off():
    yield
    disable()


def test_installed():
    enable(["enum"])
    st_ = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "enum"}
    assert sorted(st_) == sorted(NAMES)
    for n in NAMES:
        assert st_[n]["state"] == "installed", st_[n]


# ---------------------------------------------------------------------------------------
# Targeted differential scenarios
# ---------------------------------------------------------------------------------------


def scenario_basic():
    log = []

    class Color(enum.Enum):
        RED = 1
        GREEN = "g"
        BLUE = 2.5
        TUP = (1, 2)
        ALIAS = 1

    class Num(enum.IntEnum):
        A = 1
        B = 2

    class S(enum.StrEnum):
        X = "x"

    for v in [1, "g", 2.5, (1, 2), 3, "nope", True, 1.0, [1], None, Color.RED, Num.A, S.X, b"x"]:
        log.append(("Color", repr(v), outcome(lambda v=v: Color(v))))
    for v in [1, 2, 3, True, "1", Num.B, 1.0]:
        log.append(("Num", repr(v), outcome(lambda v=v: Num(v))))
    for v in ["x", "y", S.X]:
        log.append(("S", repr(v), outcome(lambda v=v: S(v))))
    for m in [Color.RED, Color.GREEN, Num.A, S.X]:
        log.append((outcome(lambda m=m: m.value), outcome(lambda m=m: m.name), outcome(lambda m=m: hash(m) == hash(m._name_))))
    log.append(outcome(lambda: Color.RED is Color.ALIAS))
    log.append(outcome(lambda: {Color.RED: 1, Num.A: 2}[Color.RED]))
    log.append(outcome(lambda: Color("RED", "nope")))  # functional API with members: error path
    log.append(outcome(lambda: list(enum.Enum("Dyn", "A B C"))))
    return log


def test_basic_differential():
    with Counting(*NAMES) as c:
        plain, fast = both(scenario_basic, ["enum"])
    assert plain == fast
    assert c.hits("enum.EnumType.__call__") > 0
    assert c.hits("enum.property.__get__") > 0
    assert c.hits("enum.Enum.__hash__") > 0


def scenario_flags():
    log = []
    calls = []

    class Perm(enum.Flag):
        R = 4
        W = 2
        X = 1
        RW = 6

    class Bits(enum.IntFlag):
        A = 1
        B = 2

    class Strict(enum.Flag, boundary=enum.STRICT):
        P = 1
        Q = 2

    class Logged(enum.Flag):
        L = 1
        M = 2

        @classmethod
        def _missing_(cls, value):
            calls.append(("missing", value))
            return super()._missing_(value)

    ops = [
        lambda: Perm.R | Perm.W,
        lambda: Perm.R | Perm.X,
        lambda: Perm.R | Perm.X,  # second time: composite cached
        lambda: Perm.RW & Perm.R,
        lambda: Perm.RW ^ Perm.W,
        lambda: ~Perm.R,
        lambda: ~Perm.R,
        lambda: ~(Perm.R | Perm.X),
        lambda: Perm.R in Perm.RW,
        lambda: Perm.X in Perm.RW,
        lambda: 1 in Perm.RW,
        lambda: Perm.R | 4,
        lambda: Perm.R | Bits.A,
        lambda: Bits.A | Bits.B,
        lambda: Bits.A | 2,
        lambda: 2 | Bits.A,
        lambda: ~Bits.A,
        lambda: Bits.A in Bits(3),
        lambda: Strict.P | Strict.Q,
        lambda: Strict(4),
        lambda: Logged.L | Logged.M,
        lambda: Logged.L | Logged.M,
        lambda: Logged(8),
        lambda: Perm(0),
        lambda: Perm(7),
        lambda: Perm(16),
    ]
    for op in ops:
        log.append(outcome(op))
    log.append(tuple(calls))
    return log


def test_flags_differential():
    with Counting(*NAMES) as c:
        plain, fast = both(scenario_flags, ["enum"])
    assert plain == fast
    assert c.hits("enum.Flag.operators") > 0


def scenario_user_hooks():
    """Overrides and unusual members: every one must give CPython's behaviour."""
    log = []
    seen = []

    class Weird:
        def __init__(self, v):
            self.v = v

        def __hash__(self):
            seen.append(("hash", self.v))
            return hash(self.v)

        def __eq__(self, other):
            seen.append(("eq", self.v))
            return isinstance(other, Weird) and other.v == self.v

        def __repr__(self):
            return f"Weird({self.v})"

    class W(enum.Enum):
        A = Weird(1)
        B = 1

    class Own(enum.Enum):
        A = 1

        @property
        def value(self):
            seen.append("own value")
            return 42

        def __hash__(self):
            seen.append("own hash")
            return 7

    class Meta(enum.EnumType):
        def __call__(cls, *a, **k):
            seen.append(("meta call", a))
            return super().__call__(*a, **k)

    class M(enum.Enum, metaclass=Meta):
        A = 1

    class Sub(enum.EnumType):
        pass

    class N(enum.Enum, metaclass=Sub):
        A = 1
        B = 2

    class Gattr(enum.Enum):
        A = 1

        def __getattr__(self, name):
            seen.append(("getattr", name))
            raise AttributeError(name)

    for op in [
        lambda: W(1),
        lambda: W(Weird(1)),
        lambda: W(2),
        lambda: Own(1),
        lambda: Own.A.value,
        lambda: hash(Own.A),
        lambda: M(1),
        lambda: M(5),
        lambda: N(2),
        lambda: N(3),
        lambda: N.B.value,
        lambda: Gattr(1).value,
        lambda: hash(Gattr.A) == hash("A"),
    ]:
        log.append(outcome(op))
    log.append(tuple(seen))
    return log


def test_user_hooks_differential():
    plain, fast = both(scenario_user_hooks, ["enum"])
    assert plain == fast


def scenario_mutation():
    log = []

    class C(enum.Enum):
        A = 1
        B = 2

    log.append(outcome(lambda: C(1)))
    # Private-state tampering: a key with a user type (and the same hash) must not run its
    # __eq__ from native code: the facts are recomputed after the dict changes.
    calls = []

    class K:
        def __hash__(self):
            return hash(2)

        def __eq__(self, other):
            calls.append(other)
            return False

    C._value2member_map_[K()] = C.A
    log.append(outcome(lambda: C(2)))
    log.append(outcome(lambda: C(3)))
    log.append(len(calls))
    C._add_member_("D", C.A) if hasattr(C, "_add_member_") else None
    log.append(outcome(lambda: C(1)))
    C._value2member_map_.clear()
    log.append(outcome(lambda: C(1)))
    return log


def test_value_map_mutation():
    plain, fast = both(scenario_mutation, ["enum"])
    assert plain == fast


def test_property_subclass_and_descriptor_get():
    def scenario():
        log = []

        class MyProp(enum.property):
            def __get__(self, inst, owner=None):
                log.append("myprop")
                return super().__get__(inst, owner)

        class E(enum.Enum):
            A = 1

            @MyProp
            def extra(self):
                return "extra"

        log.append(outcome(lambda: E.A.extra))
        log.append(outcome(lambda: E.A.value))
        log.append(outcome(lambda: type(E).__dict__.get("value")))
        log.append(outcome(lambda: enum.Enum.__dict__["value"].__get__(E.A)))
        log.append(outcome(lambda: enum.Enum.__dict__["value"].__get__(None, E)))
        return log

    plain, fast = both(scenario, ["enum"])
    assert plain == fast


# ---------------------------------------------------------------------------------------
# Hypothesis: random enums, values and lookups
# ---------------------------------------------------------------------------------------

values = st.one_of(
    st.integers(-5, 40),
    st.text(max_size=3),
    st.floats(allow_nan=True, allow_infinity=False, width=16),
    st.booleans(),
    st.tuples(st.integers(0, 3), st.integers(0, 3)),
)


@settings(max_examples=60)
@given(st.lists(values, min_size=1, max_size=8, unique_by=lambda v: (type(v), repr(v))), st.lists(values, max_size=10))
def test_hypothesis_lookup(member_values, probes):
    def scenario():
        E = enum.Enum("E", [(f"M{i}", v) for i, v in enumerate(member_values)])
        log = []
        for p in probes + member_values:
            log.append(outcome(lambda p=p: E(p)))
            log.append(outcome(lambda p=p: E(p).value))
        for m in E:
            log.append((outcome(lambda m=m: m.name), outcome(lambda m=m: hash(m) == hash(m.name))))
        return log

    plain, fast = both(scenario, ["enum"])
    assert plain == fast


@settings(max_examples=60)
@given(
    st.lists(st.integers(0, 6), min_size=1, max_size=5, unique=True),
    st.lists(st.tuples(st.sampled_from("|&^~i"), st.integers(0, 4), st.integers(0, 4)), max_size=15),
    st.sampled_from(["flag", "intflag", "strict", "conform"]),
)
def test_hypothesis_flags(bits, ops, kind):
    def scenario():
        base, boundary = {
            "flag": (enum.Flag, None),
            "intflag": (enum.IntFlag, None),
            "strict": (enum.Flag, enum.STRICT),
            "conform": (enum.Flag, enum.CONFORM),
        }[kind]
        kw = {"boundary": boundary} if boundary else {}
        F = base("F", [(f"B{b}", 1 << b) for b in bits], **kw)
        members = list(F)
        log = []
        for op, i, j in ops:
            a = members[i % len(members)]
            b = members[j % len(members)]
            fn = {
                "|": lambda: a | b,
                "&": lambda: a & b,
                "^": lambda: a ^ b,
                "~": lambda: ~a,
                "i": lambda: a in (a | b),
            }[op]
            log.append(outcome(fn))
        return log

    plain, fast = both(scenario, ["enum"])
    assert plain == fast


# ---------------------------------------------------------------------------------------
# Mechanisms
# ---------------------------------------------------------------------------------------


class Planet(enum.Enum):
    MERCURY = 1
    VENUS = 2


def test_slots_restored_by_disable():
    prop_get = type(enum.Enum.__dict__["value"]).__get__
    before = (enum.property.__dict__["__get__"], enum.EnumType.__dict__["__call__"])
    enable(["enum"])
    with Counting(*NAMES) as c:
        assert Planet(2) is Planet.VENUS and Planet.VENUS.value == 2
    assert c.hits("enum.EnumType.__call__") == 1 and c.hits("enum.property.__get__") == 1
    disable()
    with Counting(*NAMES) as c:
        assert Planet(2) is Planet.VENUS and Planet.VENUS.value == 2
    assert c.hits("enum.EnumType.__call__") == 0 and c.hits("enum.property.__get__") == 0
    assert (enum.property.__dict__["__get__"], enum.EnumType.__dict__["__call__"]) == before
    assert prop_get is enum.property.__get__
    for f in (enum.Enum.__hash__, enum.Flag.__or__, enum.EnumType.__call__):
        assert A.vectorcall_is_original(f)


def test_introspection_unchanged():
    def snap():
        return (
            repr(enum.property),
            enum.property.__get__.__qualname__,
            enum.EnumType.__call__.__code__,
            pickle.loads(pickle.dumps(Planet.VENUS)) is Planet.VENUS,
            enum.Flag.__or__ is enum.Flag.__dict__["__or__"],
        )

    a = snap()
    enable(["enum"])
    b = snap()
    disable()
    assert a == b


def test_reassigning_slot_function_falls_back():
    enable(["enum"])
    orig = enum.EnumType.__call__
    seen = []

    def spy(cls, *a, **k):
        seen.append(a)
        return orig(cls, *a, **k)

    try:
        enum.EnumType.__call__ = spy  # CPython rewrites the slot over the kernel
        assert Planet(1) is Planet.MERCURY
        assert seen == [(1,)]
    finally:
        enum.EnumType.__call__ = orig
        disable()
    enable(["enum"])
    with Counting("enum.EnumType.__call__") as c:
        assert Planet(1) is Planet.MERCURY
    assert c.hits("enum.EnumType.__call__") == 1


def test_tools_see_original():
    enable(["enum"])
    code_new = enum.Enum.__new__.__code__
    seen = []

    def tracer(frame, event, arg):
        if event == "call" and frame.f_code is code_new:
            seen.append(1)

    with Counting(*NAMES) as c:
        sys.settrace(tracer)
        try:
            assert Planet(1) is Planet.MERCURY
        finally:
            sys.settrace(None)
    assert seen and c.hits("enum.EnumType.__call__") == 0
    with Counting(*NAMES) as c:
        assert Planet(1) is Planet.MERCURY
    assert c.hits("enum.EnumType.__call__") == 1


def test_local_monitoring_on_new_falls_back():
    enable(["enum"])
    mon = sys.monitoring
    tool = 4
    mon.use_tool_id(tool, "dm-test")
    seen = []
    mon.register_callback(tool, mon.events.PY_START, lambda code, off: seen.append(code))
    mon.set_local_events(tool, enum.Enum.__new__.__code__, mon.events.PY_START)
    try:
        with Counting(*NAMES) as c:
            assert Planet(2) is Planet.VENUS
        assert c.hits("enum.EnumType.__call__") == 0
        assert enum.Enum.__new__.__code__ in seen
    finally:
        mon.set_local_events(tool, enum.Enum.__new__.__code__, 0)
        mon.register_callback(tool, mon.events.PY_START, None)
        mon.free_tool_id(tool)


def test_patched_hash_builtin_falls_back():
    from unittest import mock

    enable(["enum"])
    with Counting("enum.Enum.__hash__") as c:
        hash(Planet.MERCURY)
    assert c.hits("enum.Enum.__hash__") == 1
    calls = []
    with mock.patch.object(enum, "hash", lambda x: calls.append(x) or 5, create=True):
        assert hash(Planet.MERCURY) == 5
        assert calls == ["MERCURY"]
    for _ in range(2000):
        pass
    with Counting("enum.Enum.__hash__") as c:
        hash(Planet.MERCURY)
    assert c.hits("enum.Enum.__hash__") == 1


def test_threads_stress():
    class T(enum.Flag):
        A = 1
        B = 2
        C = 4

    def work():
        r = []
        for _ in range(200):
            r.append(((T.A | T.B) & T.B, T(4), T.C.value, T.A.name, hash(T.A) == hash("A"), T.A in (T.A | T.C)))
        return r[-1]

    expected = work()
    errors = []
    stop = threading.Event()

    def worker():
        while not stop.is_set():
            try:
                if work() != expected:
                    errors.append("wrong")
            except Exception as e:  # noqa: BLE001
                errors.append(repr(e))

    ts = [threading.Thread(target=worker) for _ in range(4)]
    for t in ts:
        t.start()
    try:
        for _ in range(30):
            enable(["enum"])
            disable()
    finally:
        stop.set()
        for t in ts:
            t.join()
    assert not errors, errors[:3]
