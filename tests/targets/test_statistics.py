"""Group `statistics` (docs/targets/statistics.md): differential tests against CPython's
own `statistics` with the kernels on and off (and check mode for Hypothesis)."""

from __future__ import annotations

import math
import statistics as S
import sys
from decimal import Decimal
from fractions import Fraction

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

NAMES = ["statistics.mean", "statistics.variance", "statistics.pvariance", "statistics.stdev", "statistics.pstdev"]
FUNCS = [S.mean, S.variance, S.pvariance, S.stdev, S.pstdev]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()
    rt.check(None)


def enable():
    rt.enable(only=["statistics"])
    states = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "statistics"}
    for name in NAMES:
        assert states[name]["state"] == "installed", states[name]


def outcome(fn):
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001
        return ("raise", type(e), str(e))
    if isinstance(r, float):
        return ("ok", float, r.hex())
    return ("ok", type(r), r)


def same(calls):
    out = []
    for on in (False, True):
        enable() if on else rt.disable()
        out.append([outcome(f) for f in calls])
    rt.disable()
    for i, (p, q) in enumerate(zip(*out)):
        assert q == p, (i, p, q)


DATA = [
    [],
    [1],
    [1.5],
    [1, 2],
    [1, 2, 3, 4],
    [1, 2, 3, 4, 4],
    [True, False, True],
    [2.75, 1.75, 1.25, 0.25, 0.5, 1.25, 3.5],
    [1e308, 1e308, -1e308],
    [1e308, 1e308],
    [5e-324, 1e-300, 3],
    [1e50, 1, -1e50] * 100,
    [0.1] * 10,
    list(range(1000)),
    [x / 7 for x in range(1000)],
    (1, 2.5, 3),
    [2**62, -(2**62), 2**62 - 1],
    [2**70, 1],
    [float("nan"), 1.0],
    [float("inf"), 1.0],
    [Decimal("0.5"), Decimal("0.75")],
    [Fraction(1, 3), Fraction(2, 3)],
    [1, Fraction(1, 2)],
    [1.0, Decimal("1")],
    ["a", "b"],
    [-0.0, 0.0],
    [-0.0],
    [3, 3, 3, 3],
    [1.0, 1.0, 1.0],
]


def test_functions_match():
    calls = []
    for d in DATA:
        for f in FUNCS:
            calls.append(lambda f=f, d=d: f(d))
        calls.append(lambda d=d: S.variance(d, None))
        calls.append(lambda d=d: S.variance(d, 1))
        calls.append(lambda d=d: S.pstdev(d, mu=0))
    calls.append(lambda: S.mean(iter([1, 2, 3])))
    calls.append(lambda: S.stdev(x for x in [1.0, 2.0, 4.0]))
    calls.append(lambda: S.mean(range(10)))
    calls.append(lambda: S.mean({1: 2, 3: 4}))
    calls.append(lambda: S.mean(5))
    same(calls)


finite = st.floats(allow_nan=False, allow_infinity=False)
values = st.one_of(st.integers(-(2**63) + 1, 2**63 - 1), finite, st.booleans())


@settings(max_examples=500)
@given(data=st.lists(values, max_size=40), tup=st.booleans())
def test_hyp_check_mode(data, tup):
    rt.enable(only=["statistics"])
    rt.check("raise")
    d = tuple(data) if tup else data
    for f in FUNCS:
        try:
            f(d)
        except Exception as e:  # noqa: BLE001
            assert type(e).__name__ != "RustiqueCheckError", e


@settings(max_examples=200)
@given(data=st.lists(st.floats(min_value=-1e10, max_value=1e10, allow_nan=False), min_size=2, max_size=200))
def test_hyp_floats_check_mode(data):
    rt.enable(only=["statistics"])
    rt.check("raise")
    for f in FUNCS:
        f(data)


def test_mechanics():
    from _mechanics import assert_disable_restores, assert_introspection_unchanged, assert_tools_see_original

    for name, f in zip(NAMES, FUNCS):
        call = lambda f=f: f([1.0, 2.5, 4.0])  # noqa: E731
        assert_introspection_unchanged(f, name)
        assert_disable_restores(name, f, call)
        assert_tools_see_original(name, f, call)
