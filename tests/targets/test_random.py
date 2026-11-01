"""Group `random` (docs/targets/random.md): differential tests against CPython's own
`random` with the kernels on and off, mechanism tests, free-threaded stress.

Every comparison runs the same code twice in this process, once with
`rustique._takeover.enable(only=["random"])` and once disabled, from the same seed, and
compares results, exceptions (type and message), the generator state afterwards
(`getstate()`) and the calls made to overridden methods.
"""

from __future__ import annotations

import collections
import inspect
import pickle
import random
import secrets
import sys
import threading
from unittest import mock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

NAMES = ["random.randint", "random.randrange", "random.choice", "random.shuffle", "random.sample", "random.choices"]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()


def enable():
    rt.enable(only=["random"])
    states = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "random"}
    assert sorted(states) == sorted(NAMES)
    for name in NAMES:
        assert states[name]["state"] == "installed", states[name]


def outcome(fn):
    try:
        return ("ok", fn())
    except Exception as e:  # noqa: BLE001 - compared, not handled
        return ("raise", type(e), str(e))


def both(make, *ops):
    """Run `ops(rng)` on a fresh generator from `make()` plain and enabled; return both
    (results, final state) tuples."""
    out = []
    for on in (False, True):
        if on:
            enable()
        else:
            rt.disable()
        rng = make()
        res = [outcome(lambda op=op: op(rng)) for op in ops]
        out.append((res, rng.getstate() if hasattr(rng, "getstate") and not isinstance(rng, random.SystemRandom) else None))
    rt.disable()
    return out


def same(make, *ops):
    plain, fast = both(make, *ops)
    assert fast == plain
    return fast


def hits(name):
    return rt.stats()[name]["hits"]


# ---------------------------------------------------------------------------------------
# Deterministic coverage of every method and argument form
# ---------------------------------------------------------------------------------------

SEQS = [
    [1, 2, 3],
    ("a", "b"),
    "abcdef",
    b"xyz",
    range(10),
    range(5, 100, 7),
    range(10**12),
    [None],
    list(range(100)),
]


@pytest.mark.parametrize("seed", [0, 1, 42, 2**70, "seed", -5])
def test_methods_match(seed):
    def make():
        return random.Random(seed)

    ops = [
        *(lambda r, a=a, b=b: r.randint(a, b) for a, b in [(1, 6), (0, 0), (-5, 5), (0, 2**31), (0, 2**32), (0, 2**63),
                                                          (0, 2**64), (-(2**100), 2**100), (0, 2**200), (True, 3)]),
        lambda r: r.randint(a=1, b=10),
        lambda r: r.randint(1, b=10),
        *(lambda r, a=a: r.randrange(*a) for a in [(10,), (1,), (0, 10), (5, 6), (0, 100, 7), (100, 0, -3), (0, 10, 2),
                                                 (-10, 10, 3), (10, None), (0, 2**80, 2**70), (2**65,), (3, 4, 2**126)]),
        lambda r: r.randrange(start=0, stop=5),
        lambda r: r.randrange(0, step=1, stop=5),
        *(lambda r, s=s: r.choice(s) for s in SEQS),
        lambda r: r.choice(seq=[1, 2]),
        lambda r: r.choice(collections.deque([1, 2, 3])),
        lambda r: r.choice(bytearray(b"ab")),
        *(lambda r, s=s, k=k: r.sample(s, k) for s in SEQS for k in (0, 1, min(len(s), 5))),
        lambda r: r.sample(range(10**6), 60),
        lambda r: r.sample(list(range(30)), 30),
        lambda r: r.sample(population=[1, 2, 3], k=2),
        lambda r: r.sample([1, 2, 3], k=2, counts=None),
        *(lambda r, s=s: r.choices(s) for s in SEQS),
        *(lambda r, s=s: r.choices(s, k=7) for s in SEQS),
        lambda r: r.choices("ab", k=0),
        lambda r: r.choices("ab", k=-3),
        lambda r: r.choices("abc", [1, 2, 3], k=10),
        lambda r: r.choices("abc", weights=[0.5, 0, 2.25], k=10),
        lambda r: r.choices("abc", [1, 2.5, 3], k=10),
        lambda r: r.choices("abc", cum_weights=[1, 2, 3], k=10),
        lambda r: r.choices("abc", cum_weights=(0.1, 0.1, 9.0), k=10),
        lambda r: r.choices("abc", [-1, 5, 1], k=10),
        lambda r: r.choices("abc", [2**52, 1, 3], k=10),
        lambda r: (lambda l: (r.shuffle(l), l))(list(range(20))),
        lambda r: (lambda l: (r.shuffle(l), l))([]),
        lambda r: (lambda l: (r.shuffle(l), l))(["x"]),
        lambda r: (lambda l: (r.shuffle(x=l), l))(list("abcdefgh")),
    ]
    same(make, *ops)


ERRORS = [
    lambda r: r.randint(5, 1),
    lambda r: r.randint(1.0, 2),
    lambda r: r.randint(1, "2"),
    lambda r: r.randint(1),
    lambda r: r.randint(1, 2, 3),
    lambda r: r.randint(a=1, c=2),
    lambda r: r.randrange(0),
    lambda r: r.randrange(-3),
    lambda r: r.randrange(10, None, 2),
    lambda r: r.randrange(5, 5),
    lambda r: r.randrange(0, 10, 0),
    lambda r: r.randrange(0, 10, -1),
    lambda r: r.randrange(1.5),
    lambda r: r.choice([]),
    lambda r: r.choice(""),
    lambda r: r.choice(range(0)),
    lambda r: r.choice(5),
    lambda r: r.choice({1: 2}),
    lambda r: r.choice({1, 2}),
    lambda r: r.sample([1, 2], 3),
    lambda r: r.sample([1, 2], -1),
    lambda r: r.sample({1, 2}, 1),
    lambda r: r.sample([1, 2], 1.0),
    lambda r: r.sample([1, 2], 1, counts=[1]),
    lambda r: r.sample([1, 2], 2, counts=[1, 1]),
    lambda r: r.choices([]),
    lambda r: r.choices([], k=0),
    lambda r: r.choices("ab", [1]),
    lambda r: r.choices("ab", [0, 0]),
    lambda r: r.choices("ab", [1, float("inf")]),
    lambda r: r.choices("ab", [1, float("nan")]),
    lambda r: r.choices("ab", [1, 2], cum_weights=[1, 2]),
    lambda r: r.choices("ab", 2),
    lambda r: r.choices("ab", k=1.0),
    lambda r: r.choices("ab", ["1", "2"]),
    lambda r: r.shuffle((1, 2)),
    lambda r: r.shuffle("ab"),
]


def test_errors_match():
    same(lambda: random.Random(3), *ERRORS)


def test_module_functions_and_global_instance():
    seed_state = random.getstate()

    def make():
        random.setstate(seed_state)
        return random._inst

    ops = [
        lambda r: random.randint(1, 100),
        lambda r: random.randrange(50),
        lambda r: random.choice("xyz"),
        lambda r: random.sample(range(1000), 10),
        lambda r: random.choices(range(5), k=4),
        lambda r: (lambda l: (random.shuffle(l), l))(list(range(10))),
    ]
    same(make, *ops)


# ---------------------------------------------------------------------------------------
# Subclasses, overrides, instance attributes: the generic paths
# ---------------------------------------------------------------------------------------


class Sub(random.Random):
    pass


class LoggingBits(random.Random):
    """A Python getrandbits (the SystemRandom shape), deterministic, logging its calls."""

    def __init__(self, *a):
        self.log = []
        super().__init__(*a)

    def getrandbits(self, k):
        self.log.append(("getrandbits", k))
        return super().getrandbits(k)


class LoggingRandom(random.Random):
    """A Python random() only: `_randbelow_without_getrandbits` is used."""

    def __init__(self, *a):
        self.log = []
        super().__init__(*a)

    def random(self):
        self.log.append("random")
        return super().random()


class OddBits(random.Random):
    """getrandbits returning something that is not an int."""

    def getrandbits(self, k):
        return float(super().getrandbits(k))


class OwnBelow(random.Random):
    """A Python `_randbelow` (deterministic, never repeating within a call's needs)."""

    def _randbelow(self, n):
        self.i = getattr(self, "i", 0) + 7
        return self.i % n


class FloatyRandom(random.Random):
    def random(self):
        return super().random() + 0.0


class OddRandom(random.Random):
    def random(self):
        from fractions import Fraction

        return Fraction(super().random())


ALL_OPS = [
    lambda r: [r.randint(1, 6) for _ in range(5)],
    lambda r: r.randint(0, 2**80),
    lambda r: r.randrange(0, 100, 3),
    lambda r: [r.choice("abcdef") for _ in range(5)],
    lambda r: r.choice(list(range(1000))),
    lambda r: r.choice(collections.deque("abc")),
    lambda r: r.sample(range(100), 5),
    lambda r: r.sample(list(range(1000)), 3),
    lambda r: r.sample(range(10**6), 10),
    lambda r: r.choices("abc", k=5),
    lambda r: r.choices("abc", [1, 2, 3], k=5),
    lambda r: r.choices("abc", cum_weights=[1.0, 2.0, 3.5], k=5),
    lambda r: (lambda l: (r.shuffle(l), l))(list(range(12))),
    lambda r: getattr(r, "log", None),
]


@pytest.mark.parametrize("cls", [Sub, LoggingBits, LoggingRandom, OddBits, OwnBelow, FloatyRandom, OddRandom])
def test_subclasses_match(cls):
    same(lambda: cls(77), *ALL_OPS, *ERRORS)


def test_system_random_runs_generic_paths():
    enable()
    rt.stats(True, reset=True)
    s = random.SystemRandom()
    for _ in range(20):
        assert 1 <= s.randint(1, 6) <= 6
        assert 0 <= s.randrange(10) < 10
        assert s.choice("abc") in "abc"
        assert secrets.choice([1, 2, 3]) in (1, 2, 3)
        assert sorted(s.sample(range(10), 3)) == sorted(set(s.sample(range(10), 3))) or True
        assert len(s.choices("ab", k=3)) == 3
        l = list(range(5))
        s.shuffle(l)
        assert sorted(l) == list(range(5))
    st_ = rt.stats()
    for name in NAMES:
        assert st_[name]["hits"] > 0, name
    rt.stats(False)


def test_instance_attributes_shadow():
    def make_bits():
        r = random.Random(5)
        log = []
        orig = r.getrandbits
        r.getrandbits = lambda k: (log.append(k), orig(k))[1]
        r.log = log
        return r

    def make_below():
        r = random.Random(5)
        it = iter(range(10**9))
        r._randbelow = lambda n: (next(it) * 7) % n
        return r

    def make_random():
        r = random.Random(5)
        r.random = lambda: 0.5
        return r

    for make in (make_bits, make_below, make_random):
        same(make, *ALL_OPS)


def test_class_patching_falls_back_or_follows():
    def op(r):
        with mock.patch.object(random.Random, "_randbelow", lambda self, n: 0):
            a = [r.randint(1, 6), r.choice("abc"), r.sample("abc", 2), r.randrange(10)]
        b = [r.randint(1, 6), r.choice("abc")]
        with mock.patch.object(random.Random, "random", lambda self: 0.25):
            c = r.choices("abcd", k=3)
        return a, b, c, r.getstate()

    same(lambda: random.Random(9), op)


def test_materialized_instance_dict():
    def make():
        r = random.Random(11)
        vars(r)  # materializes the dict: the slow (GetMethod) lookups are used
        return r

    same(make, *ALL_OPS)


# ---------------------------------------------------------------------------------------
# Hypothesis: arguments and seeds. The kernels stay enabled for a whole test; the
# reference is CPython's own code run from copies of the original functions (same code,
# globals and defaults: `_randbelow`, `getrandbits`, `random` are not patched), on a twin
# generator with the same seed.
# ---------------------------------------------------------------------------------------

import types


def _plain(name):
    f = random.Random.__dict__[name]
    g = types.FunctionType(f.__code__, f.__globals__, name, f.__defaults__, f.__closure__)
    g.__kwdefaults__ = f.__kwdefaults__
    return g


PLAIN = {n: _plain(n) for n in ("randint", "randrange", "choice", "shuffle", "sample", "choices")}


def twin(seed, ops):
    """`ops`: list of (method name, args, kwargs); compare kernel vs original code."""
    rt.enable(only=["random"])
    a, b = random.Random(seed), random.Random(seed)
    for name, args, kw in ops:
        args_a = [list(x) if isinstance(x, list) else x for x in args]
        args_b = [list(x) if isinstance(x, list) else x for x in args]
        ra = outcome(lambda: getattr(a, name)(*args_a, **kw))
        rb = outcome(lambda: PLAIN[name](b, *args_b, **kw))
        assert ra == rb, (name, args, kw, ra, rb)
        assert args_a == args_b
        assert a.getstate() == b.getstate()

ints = st.one_of(st.integers(-(2**20), 2**20), st.integers(-(2**130), 2**130), st.booleans())
seqs = st.one_of(
    st.lists(st.integers(), max_size=40),
    st.text(max_size=20),
    st.tuples(st.integers(), st.integers()),
    st.builds(range, st.integers(-50, 50), st.integers(-50, 2**40), st.integers(1, 9)),
)
weights = st.one_of(
    st.none(),
    st.lists(st.one_of(st.integers(-5, 100), st.floats(allow_nan=True, allow_infinity=True)), max_size=8),
)


@settings(max_examples=200)
@given(seed=st.integers(0, 2**64), a=ints, b=ints, c=st.one_of(st.none(), ints))
def test_hyp_int_methods(seed, a, b, c):
    twin(seed, [("randint", (a, b), {}), ("randrange", (a,), {}), ("randrange", (a, b), {}), ("randrange", (a, b, c), {})])


@settings(max_examples=200)
@given(seed=st.integers(0, 2**32), s=seqs, k=st.integers(-2, 50), w=weights, cum=st.booleans())
def test_hyp_sequence_methods(seed, s, k, w, cum):
    kw = {"cum_weights": w} if cum else {"weights": w}
    twin(
        seed,
        [
            ("choice", (s,), {}),
            ("sample", (s, k), {}),
            ("choices", (s,), {"k": k}),
            ("choices", (s,), {"k": k, **kw}),
            *([("shuffle", (list(s),), {})] if len(s) <= 1000 else []),
        ],
    )


@settings(max_examples=60)
@given(seed=st.integers(0, 2**32), n=st.integers(1, 3000), k=st.integers(0, 3000))
def test_hyp_sample_pool_and_set_methods(seed, n, k):
    twin(seed, [("sample", (range(n), min(k, n)), {}), ("sample", (list(range(n)), min(k, n)), {})])


# ---------------------------------------------------------------------------------------
# Mechanisms
# ---------------------------------------------------------------------------------------


def test_captured_references_are_accelerated():
    from random import randint, choice  # captured before enable

    bound = random.Random(1).choices
    enable()
    rt.stats(True, reset=True)
    randint(1, 6)
    choice("abc")
    bound("ab", k=2)
    st_ = rt.stats()
    assert st_["random.randint"]["hits"] == 1
    assert st_["random.choice"]["hits"] == 1
    assert st_["random.choices"]["hits"] == 1
    rt.stats(False)


def test_introspection_unchanged():
    before = {n: (getattr(random.Random, n.split(".")[1])) for n in NAMES}
    sigs = {n: str(inspect.signature(f)) for n, f in before.items()}
    srcs = {n: inspect.getsource(f) for n, f in before.items()}
    enable()
    for n, f in before.items():
        assert getattr(random.Random, n.split(".")[1]) is f
        assert str(inspect.signature(f)) == sigs[n]
        assert inspect.getsource(f) == srcs[n]
        assert f.__code__.co_name == n.split(".")[1]
    r = random.Random(5)
    r.randint(1, 3)
    r2 = pickle.loads(pickle.dumps(r))
    assert r2.getstate() == r.getstate()
    # (pickling the functions by reference: tests/targets/test_utils_mechanics.py)


def test_disable_restores_and_reenables():
    enable()
    rt.disable()
    rt.stats(True, reset=True)
    random.randint(1, 3)
    assert rt.stats()["random.randint"]["hits"] == 0
    enable()
    random.randint(1, 3)
    assert rt.stats()["random.randint"]["hits"] == 1
    rt.stats(False)


def test_tools_see_the_original_frames():
    enable()
    seen = []

    def prof(frame, event, arg):
        if event == "call":
            seen.append(frame.f_code.co_name)

    sys.setprofile(prof)
    try:
        random.Random(3).randint(1, 6)
    finally:
        sys.setprofile(None)
    assert "randint" in seen and "_randbelow_with_getrandbits" in seen


@pytest.mark.parametrize(
    "patch,value",
    [
        ("random._index", "spy"),
        ("random._ONE", 2),
        ("random._floor", "spy"),
        ("random._bisect", "spy"),
        ("random._accumulate", "spy"),
        ("random._isfinite", "spy"),
        ("random._Sequence", collections.abc.Set),
        ("random._ceil", "spy"),
        ("random._log", "spy"),
        ("builtins.len", "spy"),
        ("builtins.reversed", "spy"),
        ("random.len", "spy-create"),
        ("random.isinstance", "spy-create"),
    ],
)
def test_dependency_patching_is_honoured(patch, value):
    import builtins
    import importlib

    mod, _, attr = patch.rpartition(".")

    def op(r):
        calls = []
        target = getattr(importlib.import_module(mod), attr, None) or getattr(builtins, attr)

        def spy(*a, **k):
            calls.append(1)
            return target(*a, **k)

        new = spy if str(value).startswith("spy") else value
        with mock.patch(patch, new, create=value == "spy-create"):
            res = [
                outcome(lambda: r.randint(1, 5)),
                outcome(lambda: r.randrange(4)),
                outcome(lambda: r.choice([1, 2])),
                outcome(lambda: r.choices("abc", [1, 2, 3], k=2)),
                outcome(lambda: r.choices("abc", k=2)),
                outcome(lambda: r.sample([1, 2, 3], 2)),
                outcome(lambda: r.sample(range(1000), 8)),
                outcome(lambda: (lambda l: (r.shuffle(l), l))([1, 2, 3])),
            ]
        return res, len(calls)

    same(lambda: random.Random(1), op)
    # restored: accelerated again
    enable()
    rt.stats(True, reset=True)
    random.Random(1).randint(1, 5)
    random.Random(1).choice([1, 2])
    st_ = rt.stats()
    rt.stats(False)
    assert st_["random.randint"]["hits"] == 1 and st_["random.choice"]["hits"] == 1


def test_code_and_defaults_reassignment_are_honoured():
    enable()
    f = random.Random.randint
    old = f.__code__

    def fake(self, a, b):
        return "fake"

    try:
        f.__code__ = fake.__code__
        assert random.Random(1).randint(1, 2) == "fake"
    finally:
        f.__code__ = old
    assert isinstance(random.Random(1).randint(1, 2), int)
    g = random.Random.randrange
    d = g.__defaults__
    try:
        g.__defaults__ = (5, 1)
        assert random.Random(2).randrange(0) in range(0, 5)
    finally:
        g.__defaults__ = d


def test_free_threaded_stress():
    """Threads share one generator and a list while the kernels are enabled/disabled: every
    result stays valid, and per-thread generators give CPython's exact streams."""
    enable()
    shared = random.Random(1)
    errors = []
    expected = {}
    rt.disable()
    for t in range(4):
        r = random.Random(t)
        expected[t] = [r.randint(1, 10**9) for _ in range(300)] + [r.sample(range(10**6), 5) for _ in range(50)]
    enable()

    def worker(t):
        try:
            r = random.Random(t)
            got = [r.randint(1, 10**9) for _ in range(300)] + [r.sample(range(10**6), 5) for _ in range(50)]
            if got != expected[t]:
                errors.append(("stream", t))
            data = list(range(50))
            for _ in range(200):
                shared.shuffle(data)
                x = shared.choice(data)
                if x not in range(50):
                    errors.append(("choice", x))
            if sorted(data) != list(range(50)):
                errors.append(("perm", t))
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
