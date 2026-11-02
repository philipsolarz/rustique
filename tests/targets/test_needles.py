"""Class / identity needles and tuple needles in `x in list|tuple`, `index`, `count`.

Every case runs with CPython's originals and with the `containers` group, and compares the
result (by identity for `index`), exceptions, and the log of every `__eq__` a user type
runs (order and arguments): the kernel continues CPython's loop at the first item whose
comparison could run code, so user `__eq__`s run exactly as in CPython.
"""

from __future__ import annotations

import math
import random

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")

LOG: list = []


@pytest.fixture(autouse=True)
def _clean():
    rustique.disable()
    yield
    rustique.disable()


class A:
    pass


class B:
    pass


class Eq:
    def __init__(self, v):
        self.v = v

    def __eq__(self, other):
        LOG.append(("eq", self.v, type(other).__name__))
        return isinstance(other, Eq) and other.v == self.v

    __hash__ = object.__hash__


class Meta(type):
    def __eq__(cls, other):
        LOG.append(("meta-eq", cls.__name__))
        return False

    __hash__ = type.__hash__


class WithMeta(metaclass=Meta):
    pass


class T(tuple):
    def __eq__(self, other):
        LOG.append(("T-eq",))
        return tuple.__eq__(self, other)

    __hash__ = tuple.__hash__


def run(fn):
    LOG.clear()
    try:
        r = ("ok", fn())
    except Exception as e:  # noqa: BLE001
        r = ("raise", type(e), str(e))
    return r, list(LOG)


def both(fn):
    a = run(fn)
    rustique.enable(only=["containers"])
    try:
        b = run(fn)
    finally:
        rustique.disable()
    return a, b


def ops(seq, needle):
    """Every membership operation, for lists and tuples of `seq`."""
    out = []
    for c in (list, tuple):
        s = c(seq)
        out.append(needle in s)
        out.append(s.count(needle))
        try:
            out.append(s.index(needle))
        except ValueError as e:
            out.append(("ValueError", str(e)))
    return out


o1, o2 = A(), A()
nan = float("nan")
CASES = [
    ([A, B, int, str], B),
    ([A, B], int),
    ([A, 1, "x", None, 2.5, True, B], B),
    ([A, 1, "x", None, 2.5, True], WithMeta),
    ([WithMeta, A], A),  # metaclass __eq__ on an item: continuation
    ([o1, o2, A()], o2),
    ([o1, Eq(1), o2], o2),  # item __eq__ after one pure item
    ([Eq(1), Eq(2)], o1),
    ([(1, "a"), (2, "b"), (3, "c")], (2, "b")),
    ([(1, "a"), (2, "b")], (9, "z")),
    ([(1, "a"), (1.0, "a")], (1.0, "a")),
    ([(True, 1), (1, True)], (1, 1)),
    ([(0.0,), (-0.0,)], (-0.0,)),
    ([(nan,), (nan,)], (nan,)),
    ([(float("nan"),)], (float("nan"),)),
    ([((1, 2), 3), ((1, 2), 4)], ((1, 2), 4)),
    ([(1, 2), (1, 2, 3)], (1, 2, 3)),
    ([(1, 2, 3), (1, 2)], (1, 2)),
    ([(1, Eq(5)), (2, "x")], (2, "x")),  # element __eq__: continuation
    ([T((1, 2)), (1, 2)], (1, 2)),  # tuple subclass item: continuation
    ([1, "a", (1, "a")], (1, "a")),
    ([None, A, (None,)], (None,)),
    ([(A, B), (B, A)], (B, A)),
    ([(o1, 1), (o2, 1)], (o2, 1)),
    ([(10**40, "big"), (10**40, "big")], (10**40, "big")),
    ([("é", "€"), ("e", "€")], ("e", "€")),
    ([], A),
    ([], (1,)),
    ([(1,)] * 40 + [Eq(3)], (2,)),  # long pure prefix, then user code
]


@pytest.mark.parametrize("i", range(len(CASES)))
def test_cases(i):
    seq, needle = CASES[i]
    a, b = both(lambda: ops(seq, needle))
    assert a == b, (seq, needle)


def test_index_identity_and_start_argument():
    seq = [A, B, A]
    a, b = both(lambda: (seq.index(A), seq.index(A, 1), tuple(seq).index(B)))
    assert a == b


def test_mutation_during_eq_matches():
    class Mut:
        def __eq__(self, other):
            LOG.append("mut")
            lst.clear()
            return False

        __hash__ = object.__hash__

    def fn():
        global lst
        lst = [A, Mut(), B, A]
        return (B in lst, len(lst))

    a, b = both(fn)
    assert a == b


def test_counters_hit_for_class_and_tuple_needles():
    rustique.enable(only=["containers"])
    rustique.stats(True, reset=True)
    try:
        assert int in (str, bytes, int)
        assert (1, "a") in [(0, "z"), (1, "a")]
        assert [A, B].count(B) == 1
        st_ = rustique.stats()
    finally:
        rustique.stats(False)
        rustique.disable()
    assert st_["tuple.__contains__"]["hits"] >= 1
    assert st_["list.__contains__"]["hits"] >= 1
    assert st_["list.count"]["hits"] >= 1


atoms = st.one_of(
    st.integers(-3, 3),
    st.sampled_from([True, False, None, 0.0, -0.0, 1.0, nan, "a", "b", "é", A, B, o1, o2]),
)
tuples = st.tuples(atoms, atoms) | st.tuples(atoms) | st.tuples(st.tuples(atoms, atoms), atoms)
needles = st.one_of(atoms, tuples, st.sampled_from([Eq(1), WithMeta, A]))
items = st.lists(st.one_of(atoms, tuples, st.sampled_from([Eq(1), Eq(2), WithMeta, T((1, 2))])), max_size=25)


@settings(max_examples=300, deadline=None)
@given(items, needles)
def test_hypothesis_differential(seq, needle):
    a, b = both(lambda: ops(seq, needle))
    assert a == b


def test_large_random():
    rng = random.Random(3)
    pool = [A, B, int, str, (1, 2), (2, 1), ("x", 1.5), 1, "s", None, o1, o2]
    for _ in range(200):
        seq = [rng.choice(pool) for _ in range(rng.randrange(60))]
        needle = rng.choice(pool + [(9, 9), Eq(0)])
        a, b = both(lambda: ops(seq, needle))
        assert a == b
    assert not math.isnan(0.0)
