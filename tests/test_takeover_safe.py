"""Safe tier of the takeover (docs/TAKEOVER.md): exact builtins with Rust kernels behind
`list.sort`/`index`/`count`, `x in list`, and the same for `tuple`.

Every test runs each operation twice, with the kernels installed and with CPython's
originals, on identical inputs, and requires identical observable results: return value
(by identity where CPython returns an element), resulting element order by identity
(stability), exception type and message, and side effects of reentrant `__eq__`.
"""

from __future__ import annotations

import builtins
import math
import random
import sys
import threading

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import rustique as rs
from rustique import _rustique

K = _rustique._takeover
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


@pytest.fixture(autouse=True)
def _restore():
    """Leave the process exactly as we found it."""
    before = K.installed()
    yield
    if before:
        K.install()
    else:
        K.uninstall()


def both(fn):
    """Run `fn()` with the originals and with the kernels: (original, kernel) outcomes."""
    out = []
    for install in (False, True):
        (K.install if install else K.uninstall)()
        try:
            out.append(("ok", fn()))
        except Exception as e:  # noqa: BLE001
            out.append(("raise", type(e), str(e)))
    K.uninstall()
    return out


# --- values -------------------------------------------------------------------------------


class Eq:
    """A user type with a Python `__eq__` (forces the generic comparison)."""

    def __init__(self, v):
        self.v = v

    def __eq__(self, other):
        return isinstance(other, Eq) and self.v == other.v or other == self.v

    __hash__ = None

    def __repr__(self):
        return f"Eq({self.v!r})"


scalars = st.one_of(
    st.integers(-5, 5),
    st.integers(-(2**70), 2**70),
    st.booleans(),
    st.floats(allow_nan=True, allow_infinity=True),
    st.sampled_from([0.0, -0.0, 1.0, float("nan"), 2**31 + 0.0]),
    st.text(max_size=4),
    st.sampled_from(["", "a", "é", "日本", "\U0001f600", "a\x00"]),
    st.none(),
)
values_ = st.one_of(scalars, st.tuples(st.integers(-3, 3)), st.builds(Eq, st.integers(-3, 3)))


def snapshot(seq):
    return [id(v) for v in seq]


def fresh(items):
    """A new list holding the *same objects* (so identity can be compared)."""
    return list(items)


# --- installation -----------------------------------------------------------------------


def test_install_is_idempotent_and_reversible():
    sort_before = list.sort
    names = K.install()
    assert set(names) == {
        "list.sort",
        "list.index",
        "list.count",
        "tuple.index",
        "tuple.count",
        "list.__contains__",
        "tuple.__contains__",
    }
    assert K.install() == names
    assert list.sort is sort_before  # descriptors keep their identity
    K.uninstall()
    K.uninstall()
    assert K.installed() == []


def installed_names(st_):
    return {t["name"] for t in st_["targets"] if t["state"] == "installed"}


def test_enable_disable_status():
    st_ = rs.enable()
    assert st_["process"]["enabled"] and "list.sort" in installed_names(st_)
    assert type([]) is list and type({}) is dict  # objects stay exact builtins
    st_ = rs.disable()
    assert not st_["process"]["enabled"] and not installed_names(st_)


def test_identity_repr_doc_pickle_of_patched_callables():
    import pickle

    before = (list.sort, list.index, [].sort.__doc__, repr(list.sort), repr(min), list.sort.__text_signature__)
    K.install()
    after = (list.sort, list.index, [].sort.__doc__, repr(list.sort), repr(min), list.sort.__text_signature__)
    assert before == after
    assert pickle.loads(pickle.dumps(list.sort)) is list.sort


# --- x in seq, index, count ----------------------------------------------------------------


@settings(max_examples=300, deadline=None)
@given(st.lists(values_, max_size=24), values_)
def test_contains_index_count_differential(items, needle):
    pool = items + [needle]
    for container in (list, tuple):
        res = both(lambda: needle in container(pool[:-1]))
        assert res[0] == res[1]
        res = both(lambda: container(pool[:-1]).count(needle))
        assert res[0] == res[1]
        res = both(lambda: container(pool[:-1]).index(needle))
        assert res[0] == res[1]


@settings(max_examples=150, deadline=None)
@given(st.lists(values_, max_size=40), st.integers(0, 40))
def test_membership_of_present_elements(items, k):
    if not items:
        return
    needle = items[k % len(items)]
    for container in (list, tuple):
        res = both(lambda: (needle in container(items), container(items).index(needle), container(items).count(needle)))
        assert res[0] == res[1]


def test_contains_semantics_spot_checks():
    K.install()
    nan = float("nan")
    assert nan in [nan] and float("nan") not in [nan]  # identity first, then ==
    assert 1 in [1.0] and 1.0 in [True] and True in [1]
    assert 2**40 in [2**40 + 0] and 2**40 not in [2**40 + 1] and 2**40 in [float(2**40)]
    assert (2**53 + 1) not in [float(2**53)]  # exact int/float comparison
    assert "é" in ["e", "é"] and "日本" not in ["日", "本"]
    assert None in [0, None] and None not in [0, "", 0.0]
    assert Eq(1) in [0, 1]  # generic comparison, item on the left
    assert [1, 2].index(2) == 1 and (1, 2, 1).count(1) == 2
    with pytest.raises(ValueError, match=r"^list\.index\(x\): x not in list$"):
        [1].index(2)
    with pytest.raises(ValueError, match=r"^tuple\.index\(x\): x not in tuple$"):
        (1,).index(2)
    assert [1, 2, 3, 2].index(2, 2) == 3  # start/stop: the original's parsing


def test_contains_reentrant_mutation_matches_cpython():
    def run():
        log = []

        class Mut:
            def __eq__(self, other):
                log.append(len(lst))
                lst.clear() if len(lst) > 3 else lst.append(7)
                return False

            __hash__ = None

        lst = [1, 2, 3, Mut(), 7, 8]
        hit = 7 in lst
        return hit, log, len(lst)

    res = both(run)
    assert res[0] == res[1]


def test_eq_exception_propagates():
    class Boom:
        def __eq__(self, other):
            raise KeyError("boom")

        __hash__ = None

    for container in (list, tuple):
        res = both(lambda: 5 in container([1, Boom(), 5]))
        assert res[0] == res[1] and res[0][0] == "raise"
        res = both(lambda: container([1, 2, Boom()]).count(1))
        assert res[0] == res[1]


def test_generic_elements_after_a_long_fast_prefix():
    # Past the delegation window the kernel compares foreign elements itself.
    items = list(range(40)) + [Eq(3), Eq(99)]
    res = both(lambda: (Eq(99) in items, 99 in items, items.count(3), items.index(Eq(99))))
    assert res[0] == res[1]


def test_list_subclass_inherits_kernel_semantics():
    class L(list):
        pass

    res = both(lambda: (3 in L([1, 2, 3]), L([1, 2]).index(2), L([1, 1]).count(1)))
    assert res[0] == res[1]


# --- sort / sorted --------------------------------------------------------------------------


orderable = st.one_of(
    st.lists(st.one_of(st.integers(-10, 10), st.integers(-(2**64), 2**64), st.booleans()), max_size=60),
    st.lists(st.floats(allow_nan=False), max_size=60),
    st.lists(st.sampled_from([0.0, -0.0, 1.5, -1.5, math.inf, -math.inf]), max_size=60),
    st.lists(st.floats(allow_nan=True), max_size=20),
    st.lists(st.text(max_size=6), max_size=60),
    st.lists(st.sampled_from(["a", "a\x00", "", "é", "é", "日本", "\U0001f600", "ab", "b"]), max_size=60),
    st.lists(st.tuples(st.integers(0, 3), st.integers(0, 3)), max_size=30),
    st.lists(st.one_of(st.integers(-5, 5), st.floats(-5, 5)), max_size=30),
)


@settings(max_examples=300, deadline=None)
@given(orderable, st.booleans())
def test_sort_differential_by_identity(items, reverse):
    # Copies of equal values are distinct objects, so identity order checks stability.
    pool = [v for v in items]

    def run():
        lst = fresh(pool)
        lst.sort(reverse=reverse)
        return snapshot(lst)

    res = both(run)
    assert res[0] == res[1]
    res = both(lambda: snapshot(sorted(pool, reverse=reverse)))
    assert res[0] == res[1]


def test_sort_stability_with_equal_distinct_objects():
    big = [10**20 + 0 for _ in range(5)]  # equal, distinct objects
    mixed = [1, True, 1, True, 0, False]
    strs = ["".join(["a", "b"]) for _ in range(5)]
    zeros = [0.0, -0.0, 0.0, -0.0]
    for pool in (big, mixed, strs, zeros):
        for reverse in (False, True):
            res = both(lambda: snapshot(sorted(pool, reverse=reverse)))
            assert res[0] == res[1]


def test_sort_argument_handling_matches():
    cases = [
        lambda: [3, 1].sort(key=None),
        lambda: [3, 1].sort(reverse=1),
        lambda: [3, 1].sort(reverse=2**100),
        lambda: [3, 1].sort(reverse="yes"),
        lambda: [3, 1].sort(key=lambda v: -v),
        lambda: [3, 1].sort(1),
        lambda: [3, 1].sort(nope=1),
        lambda: [3, "a"].sort(),
        lambda: sorted([3, 1], key=None, reverse=True),
        lambda: sorted(),
        lambda: sorted([2, 1], cmp=None),
    ]
    for case in cases:
        res = both(case)
        assert res[0] == res[1], case


def test_sort_with_nan_matches_cpython_order():
    rng = random.Random(5)
    for _ in range(50):
        pool = [rng.choice([float("nan"), 1.0, 2.0, -1.0, 0.0]) for _ in range(12)]
        res = both(lambda: snapshot(sorted(pool)))
        assert res[0] == res[1]


def test_large_sorts_match():
    rng = random.Random(1)
    ints = [rng.randrange(-(10**9), 10**9) for _ in range(20000)]
    floats = [rng.random() for _ in range(20000)]
    strs = [str(rng.random()) for _ in range(20000)]
    for pool in (ints, floats, strs):
        res = both(lambda: snapshot(sorted(pool)))
        assert res[0] == res[1]


# --- free threading -------------------------------------------------------------------------


@pytest.mark.skipif(not FREE_THREADED, reason="needs the free-threaded build")
def test_concurrent_mutation_during_kernels_is_safe():
    K.install()
    shared = list(range(2000))
    stop = threading.Event()
    errors = []

    def mutate():
        r = random.Random(2)
        while not stop.is_set():
            op = r.random()
            if op < 0.4:
                shared.append(r.randrange(2000))
            elif op < 0.8 and shared:
                try:
                    shared.pop()
                except IndexError:
                    pass
            else:
                shared[:] = list(range(r.randrange(10, 3000)))

    def read():
        try:
            for i in range(300):
                _ = (i in shared, shared.count(i))
                try:
                    shared.index(i)
                except ValueError:
                    pass
                c = shared.copy()
                c.sort()
                assert c == builtins.sorted(c)
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    def sorter():
        try:
            for _ in range(100):
                shared.sort()
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=mutate) for _ in range(2)]
    threads += [threading.Thread(target=read) for _ in range(3)] + [threading.Thread(target=sorter)]
    for t in threads:
        t.start()
    for t in threads[2:]:
        t.join()
    stop.set()
    for t in threads[:2]:
        t.join()
    assert not errors
