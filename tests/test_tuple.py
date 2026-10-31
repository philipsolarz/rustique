"""`rs.Tuple`: an immutable sequence with native storage and builtin `tuple` semantics.

Differential tests against the builtin `tuple`, including bit-identical hashing (so equal
`Tuple`s and `tuple`s are interchangeable dict/set keys).
"""

from __future__ import annotations

import copy
import gc
import math
import pickle
import sys
import threading

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import rustique as rs
from rustique import Tuple

scalars = st.one_of(
    st.integers(),
    st.floats(allow_nan=True),
    st.text(max_size=5),
    st.booleans(),
    st.none(),
)
elements = st.recursive(scalars, lambda inner: st.tuples(inner, inner) | st.frozensets(inner, max_size=3), max_leaves=6)

SAMPLES = [
    (),
    (0,),
    (1, 2, 3),
    (-1, 2**62, -(2**63)),
    (2**70, 1),
    (1.5, -0.0, float("inf")),
    (float("nan"),),
    ("a", "bb", "", "é", "\U0001f600"),
    (True, False),
    (True, 1, 1.0),
    (None, (1, 2), "x", 2.5),
    ((1, 2), (3, (4,))),
]


@pytest.mark.parametrize("t", SAMPLES, ids=repr)
def test_basic_parity(t: tuple) -> None:
    x = Tuple(t)
    assert len(x) == len(t) and bool(x) == bool(t)
    assert list(x) == list(t)
    assert x == t and t == x and not (x != t)
    assert hash(x) == hash(t)
    for i in range(-len(t) - 1, len(t) + 1):
        try:
            expected = t[i]
        except IndexError:
            with pytest.raises(IndexError, match="tuple index out of range"):
                x[i]
        else:
            got = x[i]
            assert got == expected or (got != got and expected != expected)
            assert type(got) is type(expected)
    for s in (slice(None), slice(1, None), slice(None, None, -1), slice(0, 10, 2), slice(-2, None)):
        assert type(x[s]) is Tuple and tuple(x[s]) == t[s]
    for v in (*t, 99, "zz", None):
        assert (v in x) == (v in t)
        assert x.count(v) == t.count(v)
        if v in t:
            assert x.index(v) == t.index(v)
    assert tuple(reversed(x)) == tuple(reversed(t))
    assert repr(x) == f"Tuple({t!r})"


def test_exact_hash_values() -> None:
    # CPython's own expected values (test_tuple.test_hash_exact), 64-bit.
    if sys.hash_info.width != 64:
        pytest.skip("64-bit hash constants")
    assert hash(Tuple(())) == 5740354900026072187
    assert hash(Tuple((0,))) == -8753497827991233192
    assert hash(Tuple((0, 0))) == -8458139203682520985
    assert hash(Tuple((0.5,))) == -408149959306781352
    assert hash(Tuple((0.5, (), (-2, 3, (4, 6))))) == -1845940830829704396


@settings(max_examples=300, deadline=None)
@given(st.lists(elements, max_size=8))
def test_hash_and_equality_match_tuple(items: list) -> None:
    t = tuple(items)
    x = Tuple(items)
    assert x == t
    try:
        expected = hash(t)
    except TypeError:
        with pytest.raises(TypeError):
            hash(x)
        return
    assert hash(x) == expected
    assert {t: 1}[x] == 1 and x in {t} and t in {x}


@settings(max_examples=200, deadline=None)
@given(st.lists(st.one_of(st.integers(-5, 5), st.floats(-3, 3)), max_size=6), st.lists(st.one_of(st.integers(-5, 5), st.floats(-3, 3)), max_size=6))
def test_ordering_matches_tuple(a: list, b: list) -> None:
    ta, tb = tuple(a), tuple(b)
    for x, y in ((Tuple(a), Tuple(b)), (Tuple(a), tb), (ta, Tuple(b))):
        assert (x < y) == (ta < tb)
        assert (x <= y) == (ta <= tb)
        assert (x > y) == (ta > tb)
        assert (x >= y) == (ta >= tb)
        assert (x == y) == (ta == tb)
        assert (x != y) == (ta != tb)


def test_strategies_and_nan_identity() -> None:
    assert Tuple((1, 2)).strategy == "i64"
    assert Tuple((1.5,)).strategy == "f64"
    assert Tuple(("a",)).strategy == "str"
    assert Tuple((True,)).strategy == "bool"
    assert Tuple(()).strategy == "empty"
    assert Tuple((1, "a")).strategy == "object"
    nan = float("nan")
    t = (nan, 1.0)
    x = Tuple(t)
    assert x.strategy == "object" and x[0] is nan  # identity kept, like tuple
    assert x == t and hash(x) == hash(t)
    assert Tuple(v for v in [nan]).strategy == "object"
    assert Tuple([1.0, nan]) != Tuple([1.0, float("nan")])  # distinct NaN objects


def test_constructor_like_tuple() -> None:
    t = Tuple((1, 2))
    assert Tuple(t) is t
    assert Tuple() == () and Tuple([]) == ()
    assert Tuple("spam") == ("s", "p", "a", "m")
    assert Tuple(x for x in range(10) if x % 2) == (1, 3, 5, 7, 9)
    assert Tuple(range(3)) == (0, 1, 2)
    assert Tuple(rs.List([1, 2])) == (1, 2)
    with pytest.raises(TypeError, match="keyword argument"):
        Tuple(sequence=())
    with pytest.raises(TypeError):
        Tuple(1)
    with pytest.raises(TypeError, match="at most 1 argument"):
        Tuple((1,), (2,))
    with pytest.raises(ZeroDivisionError):
        Tuple(1 // 0 for _ in range(1))


def test_operators() -> None:
    x = Tuple((1, 2))
    assert type(x + (3,)) is Tuple and x + (3,) == (1, 2, 3)
    assert type((0,) + x) is Tuple and (0,) + x == (0, 1, 2)
    assert x + Tuple(("a",)) == (1, 2, "a")
    assert type(x * 2) is Tuple and x * 2 == (1, 2, 1, 2) and 2 * x == (1, 2, 1, 2)
    assert x * 1 is x and x * 0 == () and x * -1 == ()
    with pytest.raises(TypeError):
        x + [3]  # type: ignore[operator]
    with pytest.raises(TypeError):
        [3] + x  # type: ignore[operator]
    y = x
    y += (3,)
    assert y is not x and x == (1, 2)
    assert x[:] is x
    assert (x == [1, 2]) is False and x != [1, 2]
    with pytest.raises(TypeError, match="tuple indices must be integers or slices, not str"):
        x["a"]  # type: ignore[index]
    with pytest.raises(ValueError, match=r"tuple.index\(x\): x not in tuple"):
        x.index(5)
    assert Tuple((1, 2, 1)).index(1, 1) == 2
    with pytest.raises(TypeError):
        x[0] = 1  # type: ignore[index]
    with pytest.raises(AttributeError):
        x.append(3)  # type: ignore[attr-defined]


def test_iterators_and_pickle() -> None:
    data = Tuple((4, 5, 6, 7))
    for proto in range(pickle.HIGHEST_PROTOCOL + 1):
        it = iter(data)
        d = pickle.dumps(it, proto)
        it2 = pickle.loads(d)
        assert type(it2) is type(it) and Tuple(it2) == data
        it3 = pickle.loads(d)
        next(it3)
        assert Tuple(pickle.loads(pickle.dumps(it3, proto))) == data[1:]
        r = reversed(data)
        assert Tuple(pickle.loads(pickle.dumps(r, proto))) == (7, 6, 5, 4)
        for t in SAMPLES:
            x = Tuple(t)
            y = pickle.loads(pickle.dumps(x, proto))
            # (A NaN is a new object after unpickling: compare reprs, as for tuples.)
            assert type(y) is Tuple and repr(y) == repr(x) and y.strategy == x.strategy
    it = iter(data)
    assert it.__length_hint__() == 4
    assert list(it) == [4, 5, 6, 7] and list(it) == []
    assert type(iter(data)).__name__ == "TupleIterator"


def test_virtual_iteration_and_unpacking() -> None:
    x = Tuple((1, "a", 2.5))
    assert [v for v in x] == [1, "a", 2.5]
    a, b, c = x
    assert (a, b, c) == (1, "a", 2.5)
    first, *rest = x
    assert first == 1 and rest == ["a", 2.5]

    def f(*args: object) -> tuple:
        return args

    assert f(*x) == (1, "a", 2.5)


def test_copy_and_deepcopy() -> None:
    x = Tuple((1, 2))
    assert copy.copy(x) is x and copy.deepcopy(x) is x
    y = Tuple(([1], 2))
    z = copy.deepcopy(y)
    assert z == y and z is not y and z[0] is not y[0]
    w = Tuple(((1,), "a"))
    assert copy.deepcopy(w) is w


class PickledSub(Tuple):
    def __init__(self, arg: object, newarg: object = None) -> None:
        self.newarg = newarg


def test_subclasses() -> None:
    class Sub(Tuple):
        pass

    u = Sub([1, 2])
    assert type(u) is Sub and list(u) == [1, 2] and u == (1, 2)
    with pytest.raises(TypeError):
        Sub(sequence=())

    class WithInit(Tuple):
        def __init__(self, arg: object, newarg: object = None) -> None:
            self.newarg = newarg

    v = WithInit([1, 2], newarg=3)
    assert type(v) is WithInit and list(v) == [1, 2] and v.newarg == 3

    class WithNew(Tuple):
        def __new__(cls, arg: object, newarg: object = None) -> WithNew:
            self = super().__new__(cls, arg)
            self.newarg = newarg
            return self

    w = WithNew([1, 2], newarg=3)
    assert type(w) is WithNew and w.newarg == 3
    assert type(u[:]) is Tuple and type(u * 1) is Tuple
    p = pickle.loads(pickle.dumps(PickledSub([1, 2], newarg=3)))
    assert type(p) is PickledSub and p == (1, 2) and p.newarg == 3
    c = copy.copy(v)
    assert type(c) is WithInit and c == (1, 2) and c.newarg == 3

    class Lying(Tuple):
        def __getitem__(self, key: object) -> str:
            return "x"

    assert next(iter(Lying((1, 2)))) == 1


def test_gc_tracking() -> None:
    assert not gc.is_tracked(Tuple((1, 2, 3)))
    assert not gc.is_tracked(Tuple(("a", "b")))
    assert gc.is_tracked(Tuple(([],)))

    class Node:
        pass

    n = Node()
    t = Tuple((n,))
    n.t = t  # a cycle through a Tuple is collected
    import weakref

    r = weakref.ref(n)
    del n, t
    gc.collect()
    assert r() is None


def test_generic_alias_and_abc() -> None:
    from collections.abc import Hashable, Sequence

    assert isinstance(Tuple(()), Sequence) and isinstance(Tuple(()), Hashable)
    alias = Tuple[int]
    assert alias.__origin__ is Tuple and alias.__args__ == (int,)


def test_threads_read_concurrently() -> None:
    x = Tuple(range(1000))
    errors: list[BaseException] = []

    def read() -> None:
        try:
            for _ in range(50):
                assert sum(x) == 499500
                assert 999 in x and x[500] == 500 and hash(x) == hash(tuple(range(1000)))
        except BaseException as e:  # pragma: no cover
            errors.append(e)

    threads = [threading.Thread(target=read) for _ in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors


def test_repr_recursion_and_math() -> None:
    assert repr(Tuple(())) == "Tuple(())"
    assert repr(Tuple((1,))) == "Tuple((1,))"
    assert math.fsum(Tuple((0.1, 0.2))) == math.fsum((0.1, 0.2))
