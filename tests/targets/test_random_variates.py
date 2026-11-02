"""Group `random`, the continuous distributions (docs/targets/random.md §7):
`normalvariate`, `lognormvariate`, `expovariate`, `paretovariate`, `gauss`.

Differential tests against CPython's own code (kernels off) from the same seed: results
compared bit for bit (`float.hex`), exceptions (type and message), the generator state
afterwards (`getstate()`, which includes `gauss_next`) and the calls made to overrides.
"""

from __future__ import annotations

import collections
import decimal
import fractions
import inspect
import math
import random
import sys
import threading
from unittest import mock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

VARIATES = ["random.normalvariate", "random.lognormvariate", "random.expovariate", "random.paretovariate", "random.gauss"]
METHODS = [n.split(".")[1] for n in VARIATES]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()


def enable():
    rt.enable(only=["random"])
    states = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "random"}
    for name in VARIATES:
        assert states[name]["state"] == "installed", states[name]


def norm(x):
    """Bit-exact comparison key (NaN payloads and signed zeros included)."""
    if isinstance(x, float):
        return ("float", x.hex())
    if isinstance(x, (list, tuple)):
        return type(x).__name__, [norm(v) for v in x]
    return x


def outcome(fn):
    try:
        return ("ok", norm(fn()))
    except Exception as e:  # noqa: BLE001 - compared, not handled
        return ("raise", type(e), str(e))


def run(make, ops, on):
    if on:
        enable()
    else:
        rt.disable()
    try:
        rng = make()
        res = [outcome(lambda op=op: op(rng)) for op in ops]
        state = None if isinstance(rng, random.SystemRandom) else norm(rng.getstate())
        return res, state, norm(getattr(rng, "log", None))
    finally:
        rt.disable()


def same(make, *ops):
    plain = run(make, ops, False)
    fast = run(make, ops, True)
    assert fast == plain
    return fast


PARAMS = [0.0, 1.0, -1.0, 0.5, 2, -3, 10**6, 2**62, 1e-300, 1e300, -0.0, math.inf, -math.inf, math.nan, 0.0625,
          -0.0625, 0.06, 1 / 16 - 1e-17, True, 2**63, -(2**63), 2**64, 7.25]


def ops_for_params():
    ops = []
    for p in PARAMS:
        ops += [
            lambda r, p=p: [r.expovariate(p) for _ in range(3)],
            lambda r, p=p: [r.paretovariate(p) for _ in range(3)],
            lambda r, p=p: [r.normalvariate(p, 1.0) for _ in range(3)],
            lambda r, p=p: [r.normalvariate(0.0, p) for _ in range(3)],
            lambda r, p=p: [r.lognormvariate(p, 0.5) for _ in range(3)],
            lambda r, p=p: [r.lognormvariate(0.0, p) for _ in range(3)],
            lambda r, p=p: [r.gauss(p, 2.0) for _ in range(3)],
            lambda r, p=p: [r.gauss(1.0, p) for _ in range(3)],
        ]
    return ops


@pytest.mark.parametrize("seed", [0, 1, 42, 2**70, "seed", -5])
def test_variates_match(seed):
    ops = [
        lambda r: [r.normalvariate() for _ in range(20)],
        lambda r: [r.normalvariate(10, 2) for _ in range(5)],
        lambda r: [r.normalvariate(mu=3.5, sigma=0.25) for _ in range(5)],
        lambda r: [r.normalvariate(1.0, sigma=4) for _ in range(5)],
        lambda r: [r.lognormvariate(0.0, 0.5) for _ in range(20)],
        lambda r: [r.lognormvariate(mu=1, sigma=2) for _ in range(5)],
        lambda r: [r.expovariate() for _ in range(20)],
        lambda r: [r.expovariate(lambd=3) for _ in range(5)],
        lambda r: [r.expovariate(0.001) for _ in range(5)],
        lambda r: [r.paretovariate(2.0) for _ in range(20)],
        lambda r: [r.paretovariate(alpha=1) for _ in range(5)],
        lambda r: [r.gauss() for _ in range(21)],
        lambda r: [r.gauss(90, 60) for _ in range(7)],
        lambda r: [r.gauss(mu=1.0, sigma=0.5) for _ in range(7)],
        lambda r: r.getstate()[2],
        *ops_for_params(),
    ]
    same(lambda: random.Random(seed), *ops)


ERRORS = [
    lambda r: r.expovariate(0),
    lambda r: r.expovariate(0.0),
    lambda r: r.expovariate(-0.0),
    lambda r: r.expovariate(False),
    lambda r: r.paretovariate(0),
    lambda r: r.paretovariate(0.0),
    lambda r: [r.paretovariate(0.01) for _ in range(20)],  # pow range errors, input-dependent
    lambda r: [r.paretovariate(-0.01) for _ in range(20)],
    lambda r: r.lognormvariate(1000.0, 1.0),
    lambda r: [r.lognormvariate(705.0, 1.0) for _ in range(30)],  # overflow on some draws
    lambda r: r.lognormvariate(0.0),
    lambda r: r.paretovariate(),
    lambda r: r.expovariate(1.0, 2.0),
    lambda r: r.expovariate(lambd=1.0, other=2),
    lambda r: r.normalvariate(0.0, 1.0, 2.0),
    lambda r: r.normalvariate(0.0, mu=1.0),
    lambda r: r.gauss(1, 2, 3),
    lambda r: r.expovariate("2"),
    lambda r: r.normalvariate("a", 1.0),
    lambda r: r.normalvariate(1.0, "a"),
    lambda r: r.gauss(None, 1.0),
    lambda r: r.lognormvariate(10**400, 1.0),
    lambda r: r.expovariate(10**400),
    lambda r: r.paretovariate(10**400),
    lambda r: r.expovariate(decimal.Decimal(2)),
    lambda r: r.expovariate(fractions.Fraction(2, 3)),
    lambda r: r.normalvariate(decimal.Decimal(1), 2),
    lambda r: r.gauss(fractions.Fraction(1, 3), 2.0),
    lambda r: r.paretovariate(fractions.Fraction(5, 2)),
    lambda r: r.getstate()[2],
]


def test_errors_and_other_types_match():
    same(lambda: random.Random(3), *ERRORS)


def test_module_functions_and_global_instance():
    seed_state = random.getstate()

    def make():
        random.setstate(seed_state)
        return random._inst

    ops = [
        lambda r: [random.normalvariate(1, 2) for _ in range(5)],
        lambda r: [random.lognormvariate(0, 1) for _ in range(5)],
        lambda r: [random.expovariate(1.5) for _ in range(5)],
        lambda r: [random.paretovariate(3) for _ in range(5)],
        lambda r: [random.gauss(0, 1) for _ in range(5)],
    ]
    same(make, *ops)


# ---------------------------------------------------------------------------------------
# Subclasses, overrides, instance attributes
# ---------------------------------------------------------------------------------------


class Sub(random.Random):
    pass


class LoggingRandom(random.Random):
    def __init__(self, *a):
        self.log = []
        super().__init__(*a)

    def random(self):
        self.log.append("random")
        return super().random()


class OwnNormal(random.Random):
    def __init__(self, *a):
        self.log = []
        super().__init__(*a)

    def normalvariate(self, mu=0.0, sigma=1.0):
        self.log.append(("normalvariate", mu, sigma))
        return super().normalvariate(mu, sigma) * 2


class OddRandom(random.Random):
    def random(self):
        return fractions.Fraction(super().random())


class Slotted(random.Random):
    __slots__ = ("gauss_next",)


class SetattrLogger(random.Random):
    def __init__(self, *a):
        object.__setattr__(self, "log", [])
        super().__init__(*a)

    def __setattr__(self, name, value):
        self.log.append((name, norm(value)))
        super().__setattr__(name, value)


class ClassNext(random.Random):
    gauss_next = None


class PropNext(random.Random):
    @property
    def gauss_next(self):
        self.__dict__.setdefault("pl", []).append("get")
        return self.__dict__.get("_gn")

    @gauss_next.setter
    def gauss_next(self, v):
        self.__dict__["_gn"] = v


ALL_OPS = [
    lambda r: [r.normalvariate() for _ in range(5)],
    lambda r: [r.normalvariate(2, 3) for _ in range(3)],
    lambda r: [r.lognormvariate(0.0, 0.5) for _ in range(5)],
    lambda r: [r.expovariate(2.0) for _ in range(5)],
    lambda r: [r.paretovariate(2.0) for _ in range(5)],
    lambda r: [r.gauss(1.0, 2.0) for _ in range(5)],
    lambda r: [r.gauss() for _ in range(4)],
    lambda r: getattr(r, "log", None),
    lambda r: r.__dict__.get("pl") if hasattr(r, "__dict__") else None,
]


@pytest.mark.parametrize("cls", [Sub, LoggingRandom, OwnNormal, OddRandom, Slotted, SetattrLogger, ClassNext, PropNext])
def test_subclasses_match(cls):
    same(lambda: cls(77), *ALL_OPS)


def test_overrides_decided_at_entry():
    enable()
    for cls, native in ((Sub, set(VARIATES)), (LoggingRandom, set()),
                        (OwnNormal, set(VARIATES) - {"random.lognormvariate"}),
                        (Slotted, set(VARIATES) - {"random.gauss"}), (SetattrLogger, set(VARIATES) - {"random.gauss"}),
                        (ClassNext, set(VARIATES)), (PropNext, set(VARIATES) - {"random.gauss"})):
        rt.stats(True, reset=True)
        r = cls(5)
        r.normalvariate()
        r.lognormvariate(0.0, 1.0)
        r.expovariate(1.0)
        r.paretovariate(1.0)
        r.gauss()
        st_ = rt.stats()
        rt.stats(False)
        for name in VARIATES:
            want = int(name in native)
            if cls is OwnNormal and name == "random.normalvariate":
                want = 2  # the override's super() call, also from lognormvariate's fallback
            assert st_[name]["hits"] == want, (cls, name, st_[name])


def test_instance_attributes_shadow():
    def make_random():
        r = random.Random(5)
        r.random = lambda: 0.5
        return r

    def make_normal():
        r = random.Random(5)
        r.normalvariate = lambda mu, sigma: 1.25
        return r

    def make_next():
        r = random.Random(5)
        r.gauss_next = 0.75
        return r

    def make_next_int():
        r = random.Random(5)
        r.gauss_next = 3
        return r

    def make_materialized():
        r = random.Random(11)
        vars(r)
        r.extra = 1
        return r

    for make in (make_random, make_normal, make_next, make_next_int, make_materialized):
        same(make, *ALL_OPS)


def test_gauss_next_state_and_setstate():
    def op(r):
        out = [r.gauss(), r.getstate()[2]]
        st_ = r.getstate()
        out.append(r.gauss())
        r.setstate(st_)
        out.append(r.gauss())
        r.seed(9)
        out += [r.gauss(), r.gauss(), r.gauss(), r.getstate()[2]]
        del r.gauss_next
        out.append(outcome(r.gauss))
        r.gauss_next = 0.5
        out += [r.gauss(3.0, 2.0), r.gauss(3.0, 2.0)]
        return out

    same(lambda: random.Random(1), op)


def test_class_patching_falls_back_or_follows():
    def op(r):
        with mock.patch.object(random.Random, "normalvariate", lambda self, mu, sigma: 7.0):
            a = [r.lognormvariate(0.0, 1.0), r.normalvariate(0.0, 1.0)]
        b = [r.lognormvariate(0.0, 1.0)]
        with mock.patch.object(random.Random, "random", lambda self: 0.25):
            c = [r.expovariate(1.0), r.gauss(), r.paretovariate(3.0), r.normalvariate()]
        return a, b, c, r.getstate()

    same(lambda: random.Random(9), op)


def test_system_random_falls_back():
    enable()
    rt.stats(True, reset=True)
    s = random.SystemRandom()
    for _ in range(10):
        assert s.expovariate(1.0) >= 0
        assert s.paretovariate(2.0) >= 1
        assert isinstance(s.normalvariate(), float)
        assert s.lognormvariate(0, 1) > 0
        assert isinstance(s.gauss(), float)
    st_ = rt.stats()
    rt.stats(False)
    for name in VARIATES:
        assert st_[name]["hits"] == 0, name


class RaisingRandom(random.Random):
    def random(self):
        raise KeyError("random")


def _frames(fn):
    try:
        fn()
    except KeyError as e:
        tb, names = e.__traceback__, []
        while tb is not None:
            names.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
            tb = tb.tb_next
        return type(e), e.args, names[1:]
    raise AssertionError("no error")


@pytest.mark.parametrize("name", METHODS)
def test_user_code_errors_have_cpythons_frames(name):
    args = {"lognormvariate": (0.0, 1.0), "paretovariate": (2.0,)}.get(name, ())

    def go():
        return getattr(RaisingRandom(1), name)(*args)

    rt.disable()
    plain = _frames(go)
    enable()
    fast = _frames(go)
    assert fast == plain
    assert len(plain[2]) >= 2


# ---------------------------------------------------------------------------------------
# Hypothesis: seeds x parameters, plain run vs kernels
# ---------------------------------------------------------------------------------------

params = st.one_of(
    st.floats(allow_nan=True, allow_infinity=True),
    st.floats(-50, 50),
    st.floats(0.0625, 20),
    st.integers(-(2**64), 2**64),
    st.integers(-20, 20),
)


@settings(max_examples=150, deadline=None)
@given(seed=st.integers(0, 2**64), a=params, b=params, n=st.integers(1, 6))
def test_hyp_variates(seed, a, b, n):
    ops = [
        lambda r: [r.normalvariate(a, b) for _ in range(n)],
        lambda r: [r.lognormvariate(a, b) for _ in range(n)],
        lambda r: [r.expovariate(a) for _ in range(n)],
        lambda r: [r.paretovariate(b) for _ in range(n)],
        lambda r: [r.gauss(a, b) for _ in range(n)],
        lambda r: [r.gauss(b, a) for _ in range(n)],
    ]
    same(lambda: random.Random(seed), *ops)


@settings(max_examples=60, deadline=None)
@given(seed=st.integers(0, 2**32), mu=st.floats(-800, 800), sigma=st.floats(0, 120))
def test_hyp_lognormvariate_near_overflow(seed, mu, sigma):
    same(lambda: random.Random(seed), lambda r: [r.lognormvariate(mu, sigma) for _ in range(8)])


@settings(max_examples=60, deadline=None)
@given(seed=st.integers(0, 2**32), alpha=st.floats(-0.2, 0.2))
def test_hyp_paretovariate_small_alpha(seed, alpha):
    same(lambda: random.Random(seed), lambda r: [r.paretovariate(alpha) for _ in range(8)])


# ---------------------------------------------------------------------------------------
# Mechanisms
# ---------------------------------------------------------------------------------------


def test_hits_and_captured_references():
    from random import expovariate, gauss  # captured before enable

    bound = random.Random(1).normalvariate
    enable()
    rt.stats(True, reset=True)
    expovariate(1.0)
    gauss()
    bound()
    r = random.Random(2)
    r.lognormvariate(0, 1)
    r.paretovariate(3)
    st_ = rt.stats()
    rt.stats(False)
    for name in VARIATES:
        assert st_[name]["hits"] == 1, (name, st_[name])


def test_group_declarations_and_libm_self_test():
    enable()
    r = random.Random(4)
    r.expovariate(1.0)  # first native use runs the generator and C library self-tests
    r.gauss()
    for t in rt.status()["targets"]:
        if t["name"] in VARIATES:
            assert t["state"] == "installed", t
            assert t.get("shared") == "random.variates", t
            assert "degraded" not in t, t


def test_introspection_unchanged():
    before = {m: random.Random.__dict__[m] for m in METHODS}
    sigs = {m: str(inspect.signature(f)) for m, f in before.items()}
    srcs = {m: inspect.getsource(f) for m, f in before.items()}
    enable()
    for m, f in before.items():
        assert random.Random.__dict__[m] is f
        assert str(inspect.signature(f)) == sigs[m]
        assert inspect.getsource(f) == srcs[m]


def test_tools_see_the_original_frames():
    enable()
    seen = []

    def prof(frame, event, arg):
        if event == "call":
            seen.append(frame.f_code.co_name)

    r = random.Random(3)
    sys.setprofile(prof)
    try:
        r.lognormvariate(0.0, 1.0)
        r.gauss()
    finally:
        sys.setprofile(None)
    assert "lognormvariate" in seen and "normalvariate" in seen and "gauss" in seen


def test_local_tool_on_normalvariate_stops_lognormvariate():
    """A breakpoint-like local event on `normalvariate`'s code: `lognormvariate` (which
    runs that code) falls back, so the tool sees the frame."""
    mon = sys.monitoring
    tool = mon.DEBUGGER_ID
    seen = []
    code = random.Random.normalvariate.__code__
    enable()
    mon.use_tool_id(tool, "rq-test")
    try:
        mon.register_callback(tool, mon.events.PY_START, lambda c, off: seen.append(c.co_name))
        mon.set_local_events(tool, code, mon.events.PY_START)
        random.Random(1).lognormvariate(0.0, 1.0)
    finally:
        mon.set_local_events(tool, code, 0)
        mon.register_callback(tool, mon.events.PY_START, None)
        mon.free_tool_id(tool)
    assert seen == ["normalvariate"]


@pytest.mark.parametrize(
    "patch,value",
    [
        ("random._log", "spy"),
        ("random._exp", "spy"),
        ("random._sqrt", "spy"),
        ("random._cos", "spy"),
        ("random._sin", "spy"),
        ("random.TWOPI", 6.0),
        ("random.NV_MAGICCONST", 1.5),
    ],
)
def test_dependency_patching_is_honoured(patch, value):
    import importlib

    mod, _, attr = patch.rpartition(".")

    def op(r):
        calls = []
        target = getattr(importlib.import_module(mod), attr)

        def spy(*a, **k):
            calls.append(1)
            return target(*a, **k)

        new = spy if value == "spy" else value
        with mock.patch(patch, new):
            res = [
                outcome(lambda: [r.normalvariate() for _ in range(3)]),
                outcome(lambda: [r.lognormvariate(0, 1) for _ in range(3)]),
                outcome(lambda: [r.expovariate(2) for _ in range(3)]),
                outcome(lambda: [r.paretovariate(2) for _ in range(3)]),
                outcome(lambda: [r.gauss() for _ in range(3)]),
            ]
        return res, len(calls)

    same(lambda: random.Random(1), op)
    enable()
    rt.stats(True, reset=True)
    random.Random(1).expovariate(1.0)
    random.Random(1).gauss()
    st_ = rt.stats()
    rt.stats(False)
    assert st_["random.expovariate"]["hits"] == 1 and st_["random.gauss"]["hits"] == 1


def test_code_and_defaults_reassignment_are_honoured():
    enable()
    f = random.Random.normalvariate
    old = f.__code__

    def fake(self, mu=0.0, sigma=1.0):
        return 123.0

    try:
        f.__code__ = fake.__code__
        assert random.Random(1).normalvariate() == 123.0
        assert random.Random(1).lognormvariate(0.0, 1.0) == math.exp(123.0)
    finally:
        f.__code__ = old
    g = random.Random.expovariate
    d = g.__defaults__
    try:
        g.__defaults__ = (2.0,)
        a = random.Random(2).expovariate()
        rt.disable()
        assert random.Random(2).expovariate() == a
    finally:
        g.__defaults__ = d


def test_free_threaded_stress():
    """Per-thread generators give CPython's exact streams while another generator is shared
    and the kernels are enabled/disabled."""
    rt.disable()
    expected = {}
    for t in range(4):
        r = random.Random(t)
        expected[t] = [(r.normalvariate(), r.lognormvariate(0, 1), r.expovariate(2), r.paretovariate(2), r.gauss())
                       for _ in range(300)]
    enable()
    shared = random.Random(1)
    errors = []

    def worker(t):
        try:
            r = random.Random(t)
            got = [(r.normalvariate(), r.lognormvariate(0, 1), r.expovariate(2), r.paretovariate(2), r.gauss())
                   for _ in range(300)]
            if got != expected[t]:
                errors.append(("stream", t))
            for _ in range(300):
                x = (shared.gauss(), shared.expovariate(1.0), shared.normalvariate())
                if not all(isinstance(v, float) and math.isfinite(v) for v in x):
                    errors.append(("value", x))
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=worker, args=(t,)) for t in range(4)]
    for th in threads:
        th.start()
    for _ in range(20):
        rt.disable()
        enable()
    for th in threads:
        th.join()
    assert not errors
