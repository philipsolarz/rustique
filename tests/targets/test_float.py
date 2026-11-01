"""Target `float.repr`: mechanics, reachability and differential tests against CPython.

`float.__repr__` (the wrapper descriptor) keeps calling CPython's original `float_repr`
while the kernel is installed, so it is the in-process oracle.
"""

from __future__ import annotations

import math
import struct
import sys
import threading

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")
from rustique import _takeover as _tk  # noqa: E402  (status/stats until the DX layer exports them)
from rustique._rustique import _accel  # noqa: E402

t = _accel._t_float
NAME = "float.repr"


def install():
    rustique.enable(only=["float"])
    st_ = {d["name"]: d for d in _tk.status()["targets"]}[NAME]
    assert st_["state"] == "installed", st_


def uninstall():
    _accel.disable([NAME])


def counters():
    return _tk.stats()[NAME]


def reset_counters():
    _tk.stats(reset=True)

ORIG = float.__repr__

SPECIALS = [
    0.0, -0.0, 1.0, -1.0, 0.1, -0.1, 1.5, 123.456, 1e15, 1e16, -1e16, 1.5e16, 9999999999999998.0,
    1e17, 1e22, 1e-4, 9.999999999999999e-5, 1e-5, -1e-5, 1.2345e-5, 1e-6, 1e-7, 5e-324, -5e-324,
    2.2250738585072014e-308, 1.7976931348623157e308, -1.7976931348623157e308, 2.0**53, 2.0**53 + 2,
    2.0**-25, 0.30000000000000004, math.pi, math.e, math.inf, -math.inf, math.nan, -math.nan,
]


@pytest.fixture(autouse=True)
def installed():
    install()
    _tk.stats(enabled=True, reset=True)
    yield
    _tk.stats(enabled=False)


def from_bits(b: int) -> float:
    return struct.unpack("<d", struct.pack("<Q", b))[0]


def test_status():
    st_ = {d["name"]: d for d in _tk.status()["targets"]}[NAME]
    assert st_["group"] == "float"
    assert st_["state"] == "installed" and st_["reason"] is None
    assert [s.split()[0] for s in st_["sites"]] == ["float.tp_repr", "float.__format__"]
    assert st_["versions"] == ["3.14", "3.14t", "3.15", "3.15t"]


@pytest.mark.parametrize("x", SPECIALS)
def test_specials(x):
    assert repr(x) == ORIG(x)
    assert str(x) == ORIG(x)
    assert f"{x}" == ORIG(x)
    assert f"{x!r}" == ORIG(x)
    assert "%r|%s" % (x, x) == f"{ORIG(x)}|{ORIG(x)}"
    assert format(x, "") == ORIG(x)
    assert "{}".format(x) == ORIG(x)
    assert t.core_repr(x) == ORIG(x)


def test_reachability():
    x = 1.23  # a constant: the compiler may fold nothing here, repr is a call
    reset_counters()
    for expr in (lambda: repr(x), lambda: str(x), lambda: f"{x}", lambda: f"{x!r}", lambda: "%s" % (x,),
                 lambda: "%r" % (x,), lambda: format(x), lambda: "{}".format(x), lambda: [x].__repr__()):
        before = counters()["hits"]
        expr()
        assert counters()["hits"] == before + 1


def test_format_spec_paths():
    class S(float):
        def __str__(self):
            return "S!"

    class Spec(str):
        pass

    for x in SPECIALS:
        assert x.__format__("") == ORIG(x)
        assert float.__format__(x, "") == ORIG(x)
        assert f"{x:.2f}" == "%.2f" % x
        assert f"{x:>12}" == ORIG(x).rjust(12)
        assert f"{x:g}" == "%g" % x
        assert format(x, Spec("")) == ORIG(x)
    assert f"{S(1.5)}" == "S!"
    assert format(S(1.5), "") == "S!"
    with pytest.raises(TypeError):
        float.__format__(1.5, None)
    with pytest.raises(TypeError, match="must be str"):
        (1.5).__format__(b"")


def test_not_reached_but_exact():
    reset_counters()
    assert float.__repr__(2.5) == "2.5"  # wrapper descriptor: the original
    assert counters()["hits"] == 0


def test_fallbacks():
    class F(float):
        pass

    class G(float):
        def __repr__(self):
            return "G!"

    reset_counters()
    assert repr(F(1.5)) == "1.5"
    assert repr(G(1.5)) == "G!"
    assert str(G(1.5)) == "G!"
    for v in (math.inf, -math.inf, math.nan):
        assert repr(v) == ORIG(v)
    c = counters()
    assert c["hits"] == 0
    assert c["fallback"]["args"] == 3  # inf, -inf, nan (Python subclasses copied the original)


def test_result_objects():
    x = 0.1
    a, b = repr(x), repr(x)
    assert a == b and a is not b  # fresh objects, like float_repr
    assert type(a) is str
    assert a.isascii()
    assert sys.getsizeof(a) == sys.getsizeof(ORIG(x))
    assert a.encode() == b"0.1"
    # Hashing, interning, slicing behave as for any str.
    assert hash(a) == hash("0.1")
    assert sys.intern(a) == "0.1"


def test_uninstall_restores():
    uninstall()
    try:
        reset_counters()
        assert repr(1.25) == "1.25"
        assert f"{1.25}" == "1.25"
        assert counters()["hits"] == 0
        st_ = {d["name"]: d for d in _tk.status()["targets"]}[NAME]
        assert st_["state"] == "disabled"
    finally:
        install()
    reset_counters()
    repr(1.25)
    assert counters()["hits"] == 1


def test_subclass_created_while_installed_keeps_working_after_uninstall():
    class H(float):
        pass

    uninstall()
    try:
        assert repr(H(0.5)) == "0.5"
    finally:
        install()


@given(st.floats(allow_nan=True, allow_infinity=True))
@settings(max_examples=3000)
def test_hypothesis_floats(x):
    assert repr(x) == ORIG(x)
    assert str(-x) == ORIG(-x)


@given(st.integers(0, 2**64 - 1))
@settings(max_examples=3000)
def test_hypothesis_bits(b):
    x = from_bits(b)
    assert repr(x) == ORIG(x)
    assert f"{x}" == ORIG(x)


@given(st.integers(-(10**17), 10**17), st.integers(-340, 320))
@settings(max_examples=2000)
def test_hypothesis_decimal_strings(m, e):
    x = float(f"{m}e{e}")
    assert repr(x) == ORIG(x)
    assert float(repr(x)) == x or math.isinf(x)


def test_containers_and_formatting():
    xs = [0.1, -2.5e-7, 1e100, 3.0]
    assert repr(xs) == "[" + ", ".join(ORIG(x) for x in xs) + "]"
    assert repr(tuple(xs)) == "(" + ", ".join(ORIG(x) for x in xs) + ")"
    assert repr({"a": 0.5}) == "{'a': 0.5}"
    assert repr(1.5 + 2.25j) == "(1.5+2.25j)"  # complex has its own formatting


def test_selfcheck_in_process():
    for fam in ("bits", "subnormal", "dyadic", "decimal"):
        checked, bad, ex = t._selfcheck(fam, 20000, 12345)
        assert checked > 0 and bad == 0, ex


def test_free_threaded_stress():
    stop = threading.Event()
    errors = []
    values = [from_bits(0x3FB999999999999A + i) for i in range(64)] + SPECIALS

    def worker():
        while not stop.is_set():
            for x in values:
                if repr(x) != ORIG(x):
                    errors.append(x)

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for th in threads:
        th.start()
    try:
        for _ in range(200):
            uninstall()
            install()
    finally:
        stop.set()
        for th in threads:
            th.join()
    assert not errors
