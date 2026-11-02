"""Group `generated` (docs/targets/datamodel.md): the methods `collections.namedtuple` and
`@dataclass` generate. Differential tests (CPython's generated code vs the kernels:
results, errors with their tracebacks, side effects of user hooks), recognition limits
(only exactly what the generator emits), mechanisms (disable restores, tools, adoption of
classes created before enable(), dependency pins), free-threaded stress."""

from __future__ import annotations

import collections
import copy
import dataclasses
import pickle
import sys
import threading
import typing
from unittest import mock

import pytest
from _dm_support import FREE_THREADED, A, Counting, both, disable, enable, ft_only_skip, outcome
from hypothesis import given, settings
from hypothesis import strategies as st

NT = "collections.namedtuple"
DC = "dataclasses.__init__"


@pytest.fixture(autouse=True)
def _off():
    yield
    disable()


# ---------------------------------------------------------------------------------------
# namedtuple
# ---------------------------------------------------------------------------------------


def scenario_namedtuple():
    log = []
    P = collections.namedtuple("P", "x y z", defaults=[0])
    Q = collections.namedtuple("Q", ["a"])
    R = collections.namedtuple("R", "a b", rename=True, module="mod")
    TN = typing.NamedTuple("TN", [("u", int), ("v", str)])

    class Sub(P):
        __slots__ = ()

        def total(self):
            return self.x + self.y

    class SubDict(P):
        pass

    for cls, args, kw in [
        (P, (1, 2), {}),
        (P, (1, 2, 3), {}),
        (P, (1,), {"y": 2}),
        (P, (), {"x": 1, "y": 2, "z": 3}),
        (P, (1, 2, 3, 4), {}),
        (P, (1,), {}),
        (P, (1, 2), {"x": 1}),
        (P, (1, 2), {"w": 1}),
        (Q, (1,), {}),
        (Q, (), {}),
        (R, (1, 2), {}),
        (TN, (1, "s"), {}),
        (Sub, (1, 2), {}),
        (SubDict, (1, 2), {}),
    ]:
        log.append(outcome(lambda cls=cls, args=args, kw=kw: cls(*args, **kw)))
    p = P(1, 2, 3)
    for op in [
        lambda: P._make([4, 5, 6]),
        lambda: P._make((4, 5, 6)),
        lambda: P._make([4, 5]),
        lambda: P._make(iter([4, 5, 6])),
        lambda: P._make(x for x in (4, 5, 6)),
        lambda: P._make("abc"),
        lambda: Sub._make([1, 2, 3]),
        lambda: type(Sub._make([1, 2, 3])),
        lambda: SubDict._make([1, 2, 3]),
        lambda: p._replace(y=9),
        lambda: p._replace(),
        lambda: p._replace(w=1),
        lambda: p._replace(x=1, z=2),
        lambda: copy.replace(p, z=5),
        lambda: Sub(1, 2)._replace(x=5),
        lambda: SubDict(1, 2)._replace(x=5),
        lambda: tuple.__new__(P, (1, 2, 3)),
        lambda: P.__new__(P, 1, 2),
        lambda: P.__new__(tuple, 1, 2),
        lambda: P.__new__(int, 1, 2),
        lambda: hash(P(1, 2)) == hash((1, 2, 0)),
        lambda: pickle.loads(pickle.dumps(P(1, 2))),
        lambda: P(1, 2)._asdict(),
        lambda: Sub(1, 2).total(),
    ]:
        log.append(outcome(op))
    return log


def test_namedtuple_differential():
    with Counting(NT) as c:
        plain, fast = both(scenario_namedtuple, ["generated"])
    assert plain == fast
    assert c.hits(NT) > 10 or FREE_THREADED


def test_namedtuple_mutated_defaults_and_globals():
    def scenario():
        log = []
        P = collections.namedtuple("P", "x y", defaults=[1])
        log.append(outcome(lambda: P(0)))
        P.__new__.__defaults__ = (5,)
        log.append(outcome(lambda: P(0)))
        P.__new__.__defaults__ = None
        log.append(outcome(lambda: P(0)))
        seen = []
        P.__new__.__globals__["_tuple_new"] = lambda cls, t: seen.append(t) or tuple.__new__(cls, t)
        log.append(outcome(lambda: P(1, 2)))
        log.append(tuple(seen))
        return log

    plain, fast = both(scenario, ["generated"])
    assert plain == fast


@settings(max_examples=50)
@given(
    st.integers(1, 8),
    st.integers(0, 3),
    st.lists(st.tuples(st.integers(0, 10), st.lists(st.sampled_from("abcdefghxyz"), max_size=4)), max_size=8),
)
def test_hypothesis_namedtuple(nfields, ndefaults, calls):
    names = [f"f{i}" for i in range(nfields)]

    def scenario():
        P = collections.namedtuple("P", names, defaults=list(range(min(ndefaults, nfields))))
        log = []
        for npos, kws in calls:
            kw = {f"f{ord(k) % (nfields + 2)}": k for k in kws}
            log.append(outcome(lambda npos=npos, kw=kw: P(*range(npos), **kw)))
            log.append(outcome(lambda npos=npos: P._make(list(range(npos)))))
            p = P(*range(nfields))
            log.append(outcome(lambda p=p, kw=kw: p._replace(**kw)))
        return log

    plain, fast = both(scenario, ["generated"])
    assert plain == fast


# ---------------------------------------------------------------------------------------
# dataclass __init__
# ---------------------------------------------------------------------------------------


def scenario_dataclass():
    log = []
    made = []

    def factory():
        made.append(1)
        return ["user"]

    @dataclasses.dataclass(frozen=True)
    class F:
        x: int
        y: list = dataclasses.field(default_factory=list)
        z: int = 3
        w: dict = dataclasses.field(default_factory=dict, init=False)

    @dataclasses.dataclass(frozen=True, slots=True, kw_only=True)
    class S:
        a: int
        b: str = "s"
        c: set = dataclasses.field(default_factory=set)

    @dataclasses.dataclass(frozen=True)
    class U:
        a: list = dataclasses.field(default_factory=factory)

    @dataclasses.dataclass(frozen=True)
    class Self:
        self: int
        other: int = 0

    @dataclasses.dataclass(frozen=True)
    class Post:
        a: int

        def __post_init__(self):
            log.append(("post", self.a))

    @dataclasses.dataclass(frozen=True)
    class Init:
        a: int
        b: dataclasses.InitVar[int] = 0

    @dataclasses.dataclass(frozen=True)
    class Child(F):
        extra: int = 0

    @dataclasses.dataclass
    class Mutable:
        a: int

    @dataclasses.dataclass(frozen=True)
    class Empty:
        pass

    class Desc:
        def __set_name__(self, owner, name):
            self.name = "_" + name

        def __get__(self, inst, owner):
            return 0 if inst is None else getattr(inst, self.name, 0)

        def __set__(self, inst, value):
            log.append(("desc set", value))
            object.__setattr__(inst, self.name, value)

    @dataclasses.dataclass(frozen=True)
    class WithDesc:
        d: int = Desc()

    for cls, args, kw in [
        (F, (1,), {}),
        (F, (1, [2]), {}),
        (F, (), {"x": 1, "z": 9}),
        (F, (1, 2, 3, 4), {}),
        (F, (), {}),
        (F, (1,), {"x": 2}),
        (F, (1,), {"w": {}}),
        (S, (), {"a": 1}),
        (S, (), {"a": 1, "b": "t", "c": {1}}),
        (S, (1,), {}),
        (S, (), {}),
        (U, (), {}),
        (U, ([1],), {}),
        (Self, (1,), {}),
        (Self, (), {"self": 1, "other": 2}),
        (Post, (1,), {}),
        (Init, (1, 2), {}),
        (Child, (1,), {"extra": 5}),
        (Child, (1, [], 4, 5), {}),
        (Mutable, (1,), {}),
        (Empty, (), {}),
        (WithDesc, (5,), {}),
        (WithDesc, (), {}),
    ]:
        log.append(outcome(lambda cls=cls, args=args, kw=kw: cls(*args, **kw)))
        log.append(outcome(lambda cls=cls, args=args, kw=kw: vars(cls(*args, **kw)) if hasattr(cls, "__dict__") and "__slots__" not in cls.__dict__ else None))
    log.append(len(made))
    f = F(1)
    log.append(outcome(lambda: f.__init__(2)))  # re-init a frozen instance: allowed by object.__setattr__
    log.append(outcome(lambda: f))
    log.append(outcome(lambda: F.__init__(object(), 1)))
    log.append(outcome(lambda: F.__init__(F, 1)))
    log.append(outcome(lambda: dataclasses.replace(f, z=8)))
    log.append(outcome(lambda: pickle.loads(pickle.dumps(S(a=2)))))
    log.append(outcome(lambda: F(1) == F(1)))
    return log


def test_dataclass_differential():
    with Counting(DC) as c:
        plain, fast = both(scenario_dataclass, ["generated"])
    assert plain == fast
    assert c.hits(DC) > 5 or FREE_THREADED


def test_dataclass_class_changes():
    def scenario():
        log = []

        @dataclasses.dataclass(frozen=True)
        class F:
            x: int = 0

        log.append(outcome(lambda: F(x=1)))

        class Shadow:
            def __set__(self, inst, value):
                log.append("shadow set")

            def __get__(self, inst, owner):
                return "shadow"

        F.x = Shadow()  # a data descriptor named like a field: store runs user code
        log.append(outcome(lambda: F(x=2)))
        del F.x
        log.append(outcome(lambda: F(x=3)))
        F.__init__.__defaults__ = (7,)
        log.append(outcome(lambda: F().x))
        F.__init__.__closure__[0].cell_contents = list  # __dataclass_builtins_object__
        log.append(outcome(lambda: F(x=4)))
        return log

    plain, fast = both(scenario, ["generated"])
    assert plain == fast


fields = st.lists(
    st.tuples(
        st.sampled_from(["plain", "default", "list", "dict", "set", "user", "noinit_list", "noinit_default"]),
        st.booleans(),  # kw_only
    ),
    min_size=0,
    max_size=6,
)


@settings(max_examples=60)
@given(fields, st.booleans(), st.booleans(), st.lists(st.tuples(st.integers(0, 7), st.integers(0, 3)), max_size=6))
def test_hypothesis_dataclass(spec, slots, frozen, calls):
    def scenario():
        made = []
        ns = {"__annotations__": {}}
        anns = ns["__annotations__"]
        seen_default = False
        for i, (kind, kw_only) in enumerate(spec):
            name = f"f{i}"
            anns[name] = int
            if kind == "plain":
                if seen_default and not kw_only:
                    kind = "default"
                else:
                    if kw_only:
                        ns[name] = dataclasses.field(kw_only=True)
                    continue
            seen_default = seen_default or not kw_only
            if kind == "default":
                ns[name] = dataclasses.field(default=i, kw_only=kw_only)
            elif kind in ("list", "dict", "set"):
                ns[name] = dataclasses.field(default_factory={"list": list, "dict": dict, "set": set}[kind], kw_only=kw_only)
            elif kind == "user":
                ns[name] = dataclasses.field(default_factory=lambda i=i: made.append(i) or i, kw_only=kw_only)
            elif kind == "noinit_list":
                ns[name] = dataclasses.field(default_factory=list, init=False)
            elif kind == "noinit_default":
                ns[name] = dataclasses.field(default=i, init=False)
        try:
            C = dataclasses.dataclass(frozen=frozen, slots=slots)(type("C", (), ns))
        except Exception as e:  # noqa: BLE001
            return [("class error", type(e).__name__, str(e))]
        log = []
        for npos, nkw in calls:
            kw = {f"f{j}": j * 10 for j in range(nkw)}
            o = outcome(lambda npos=npos, kw=kw: C(*range(npos), **kw))
            log.append(o)
        log.append(tuple(made))
        return log

    plain, fast = both(scenario, ["generated"])
    assert plain == fast


# ---------------------------------------------------------------------------------------
# Mechanisms
# ---------------------------------------------------------------------------------------


@ft_only_skip
def test_disable_restores_generated_functions():
    enable(["generated"])
    P = collections.namedtuple("P", "a b")

    @dataclasses.dataclass(frozen=True)
    class F:
        a: int

    with Counting(NT, DC) as c:
        P(1, 2)
        P._make((1, 2))
        F(a=1)
    assert c.hits(NT) == 2 and c.hits(DC) == 1
    disable()
    with Counting(NT, DC) as c:
        assert P(1, 2) == (1, 2) and P._make((1, 2)) == (1, 2) and F(a=1).a == 1
        # Until accel's on_disable lands, a dynamic site gets its vectorcall back at its
        # first call after disable() (it falls back meanwhile).
        assert P(1, 2)._replace(a=3) == (3, 2)
    assert c.hits(NT) == 0 and c.hits(DC) == 0
    for f in (P.__new__, P._make.__func__, P._replace, F.__init__):
        assert A.vectorcall_is_original(f), f


@ft_only_skip
def test_positional_calls_routed_and_restored():
    enable(["generated"])

    @dataclasses.dataclass(frozen=True)
    class F:
        a: int
        b: int = 0

    class Own(F):
        def __init__(self, a):
            super().__init__(a, 1)

    class Inherit(F):
        pass

    assert not A.datamodel_generic_alloc(F)
    assert A.datamodel_generic_alloc(Own) and A.datamodel_generic_alloc(Inherit)
    with Counting(DC) as c:
        for _ in range(3):
            assert F(1, 2) == F(a=1, b=2)
            assert Own(5).b == 1
    assert c.hits(DC) == 9
    seen = []
    old = sys.getprofile()
    sys.setprofile(lambda *a: seen.append(1))
    try:
        F(1)
    finally:
        sys.setprofile(old)
    assert A.datamodel_generic_alloc(F)  # a tool: the specializer inlines again
    disable()
    assert A.datamodel_generic_alloc(F)

    enable(["generated"])

    @dataclasses.dataclass(frozen=True)
    class G:
        a: int

    assert not A.datamodel_generic_alloc(G)
    disable()  # on_disable restores eagerly
    assert A.datamodel_generic_alloc(G)
    assert G(1).a == 1


def test_adoption_of_classes_created_before_enable():
    if not getattr(sys, "_is_gil_enabled", lambda: True)():
        pytest.skip("adoption is GIL-builds only this wave")
    import tokenize

    disable()
    Before = collections.namedtuple("Before", "a b")
    enable(["generated"])
    with Counting(NT) as c:
        Before._make([1, 2])
        tokenize.TokenInfo._make((1, "x", (1, 0), (1, 1), "x"))
        Before(1, 2)._replace(a=3)
    assert c.hits(NT) == 3


@ft_only_skip
def test_tools_see_generated_frames():
    enable(["generated"])

    @dataclasses.dataclass(frozen=True)
    class F:
        a: int

    P = collections.namedtuple("P", "a b")
    codes = {F.__init__.__code__, P.__new__.__code__}
    seen = []

    def tracer(frame, event, arg):
        if event == "call" and frame.f_code in codes:
            seen.append(frame.f_code.co_name)

    with Counting(NT, DC) as c:
        sys.settrace(tracer)
        try:
            F(a=1)
            P(1, 2)
        finally:
            sys.settrace(None)
    assert sorted(seen) == ["<lambda>", "__init__"] or sorted(seen) == ["__init__", "__new__"]
    assert c.hits(NT) == 0 and c.hits(DC) == 0
    with Counting(NT, DC) as c:
        F(a=1)
        P(1, 2)
    assert c.hits(NT) == 1 and c.hits(DC) == 1


def test_patched_generator_dependency_installs_nothing():
    enable(["generated"])
    with mock.patch("builtins.eval", eval):
        pass  # same object back: pins re-arm
    for _ in range(2000):
        pass
    calls = []
    real_exec = exec

    def spy_exec(*a, **k):
        calls.append(1)
        return real_exec(*a, **k)

    with mock.patch.object(dataclasses, "exec", spy_exec, create=True):

        @dataclasses.dataclass(frozen=True)
        class F:
            a: int

        with Counting(DC) as c:
            F(a=1)
        assert c.hits(DC) == 0
    assert calls


def test_introspection_of_generated_functions():
    enable(["generated"])
    P = collections.namedtuple("P", "a b")

    @dataclasses.dataclass(frozen=True)
    class F:
        a: int

    import inspect

    snap = lambda: (
        str(inspect.signature(P)),
        str(inspect.signature(F)),
        P.__new__.__qualname__,
        F.__init__.__qualname__,
        F.__init__.__code__.co_qualname,
        F.__init__.__defaults__,
    )
    a = snap()
    disable()
    assert snap() == a


def test_threads_stress():
    enable(["generated"])
    P = collections.namedtuple("P", "a b c")

    @dataclasses.dataclass(frozen=True, slots=True)
    class F:
        a: int
        b: list = dataclasses.field(default_factory=list)

    def work():
        r = None
        for i in range(300):
            r = (P(i, 2, 3)._replace(a=1), P._make([1, 2, 3]), F(a=1), F(a=2, b=[1]))
        return r

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
        for _ in range(20):
            # Classes created concurrently with calls on others.
            Q = collections.namedtuple("Q", "x y")
            Q(1, 2)
            dataclasses.make_dataclass("M", ["x"], frozen=True)(x=1)
    finally:
        stop.set()
        for t in ts:
            t.join()
    assert not errors, errors[:3]
