"""`FrozenSet` parity with builtin `frozenset`: hashing (bit-identical to CPython's),
equality and ordering against every set type, algebra result types and element sources,
copying, pickling, typing, subclassing and free-threaded sharing."""

import collections.abc as cabc
import copy
import operator
import pickle
import sys
import threading
import weakref

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import rustique as rs
from rustique import FrozenSet, List, Set

INTS = st.integers(-(2**70), 2**70)
SMALL_INTS = st.integers(-40, 40)
TEXT = st.text(max_size=6)  # includes non-ASCII, 2-byte and astral code points
FLOATS = st.floats(allow_nan=False)
SCALARS = st.one_of(INTS, SMALL_INTS, TEXT, FLOATS, st.booleans(), st.none(), st.binary(max_size=3))
HASHABLE = st.one_of(SCALARS, st.tuples(SMALL_INTS, TEXT), st.frozensets(SMALL_INTS, max_size=3))

SET_OPS = {"|": operator.or_, "&": operator.and_, "-": operator.sub, "^": operator.xor}
CMPS = (operator.eq, operator.ne, operator.le, operator.lt, operator.ge, operator.gt)


def typed_elements(s):
    return sorted((type(e).__name__, repr(e)) for e in s)


def check_same(got, ref):
    assert len(got) == len(ref)
    assert got == ref
    assert typed_elements(got) == typed_elements(ref)


# --- hashing ---------------------------------------------------------------------------


@settings(max_examples=300)
@given(st.lists(HASHABLE, max_size=12))
def test_hash_is_cpythons(items):
    assert hash(FrozenSet(items)) == hash(frozenset(items))


@settings(max_examples=200)
@given(st.lists(st.one_of(INTS, SMALL_INTS), max_size=20))
def test_hash_of_native_ints(items):
    fs = FrozenSet(items)
    assert fs._strategy in ("empty", "i64", "object")
    assert hash(fs) == hash(frozenset(items))


@settings(max_examples=200)
@given(st.lists(TEXT, max_size=20))
def test_hash_of_native_strs(items):
    fs = FrozenSet(items)
    assert hash(fs) == hash(frozenset(items))
    assert hash(FrozenSet[str](items)) == hash(frozenset(items))


def test_hash_edge_cases():
    for items in ([], [0], [-1], [-2], [2**61 - 1], [2**63 - 1, -(2**63)], [""], ["\x00"], ["\xff", "Ā"],
                  ["\U0001f600", "a"], [1.0, 2.5], [True], [None, (1, "a")], [frozenset({1})], [FrozenSet([1])]):
        assert hash(FrozenSet(items)) == hash(frozenset(items)), items
    # Equal frozen sets of different strategies hash alike.
    assert hash(FrozenSet([1, 2])) == hash(FrozenSet([1.0, 2.0])) == hash(frozenset({1, 2}))
    assert hash(FrozenSet[str](["a"])) == hash(frozenset({"a"}))


def test_hash_is_cached_and_works_as_key():
    fs = FrozenSet("abcd")
    assert hash(fs) == hash(fs)
    d = {fs: 1, frozenset("xy"): 2}
    assert d[frozenset("dcba")] == 1 and d[FrozenSet("yx")] == 2
    assert {frozenset("ab"): 3}[FrozenSet("ba")] == 3
    s = {FrozenSet([1]), frozenset([1]), FrozenSet([2])}
    assert len(s) == 2


def test_hash_after_algebra_with_tombstones():
    a = FrozenSet(range(100))
    b = a - FrozenSet(range(0, 100, 3))
    assert hash(b) == hash(frozenset(range(100)) - frozenset(range(0, 100, 3)))
    c = FrozenSet([(i,) for i in range(50)]) ^ FrozenSet([(i,) for i in range(25, 75)])
    assert hash(c) == hash(frozenset((i,) for i in range(50)) ^ frozenset((i,) for i in range(25, 75)))


# --- construction, identity, copying -----------------------------------------------------


def test_construction_forms():
    assert FrozenSet() == frozenset()
    for source in ([1, 2, 2], (3, 4), {5, 6}, frozenset({7}), {"k": 1}, "abca", range(5), List([1, 1, 2]), Set("ab")):
        check_same(FrozenSet(source), frozenset(source))
    check_same(FrozenSet(x for x in "xy"), frozenset("xy"))
    check_same(FrozenSet([1, 1.0, True]), frozenset([1, 1.0, True]))
    check_same(FrozenSet([True, 1]), frozenset([True, 1]))
    with pytest.raises(TypeError):
        FrozenSet(5)
    with pytest.raises(TypeError):
        FrozenSet([], [])
    with pytest.raises(TypeError):
        FrozenSet(x=1)
    with pytest.raises(TypeError):
        FrozenSet([[1]])


def test_identity_and_copy():
    fs = FrozenSet([1, 2])
    assert FrozenSet(fs) is fs
    assert fs.copy() is fs and copy.copy(fs) is fs
    assert FrozenSet.__new__(FrozenSet, fs) is fs
    typed = FrozenSet[int]([1, 2])
    assert FrozenSet[int](typed) is typed
    untyped = FrozenSet(typed)
    assert untyped is not typed and untyped == typed and untyped.schema_type is FrozenSet
    class Tracer:
        def __init__(self, v):
            self.v = v

        def __hash__(self):
            return self.v

        def __deepcopy__(self, memo):
            return Tracer(self.v + 1)

    dup = copy.deepcopy(FrozenSet([Tracer(10)]))
    assert type(dup) is FrozenSet and [t.v for t in dup] == [11]
    fs.__init__([9])  # object.__init__: a no-op, like frozenset
    assert fs == {1, 2}


def test_immutable():
    fs = FrozenSet([1])
    for name in ("add", "discard", "remove", "pop", "clear", "update", "__ior__"):
        assert not hasattr(fs, name), name
    fs2 = FrozenSet([1])
    alias = fs2
    fs2 |= {2}
    assert alias == {1} and fs2 == {1, 2} and type(fs2) is FrozenSet


def test_repr():
    assert repr(FrozenSet()) == "FrozenSet()"
    assert repr(FrozenSet([1, 2])) == "FrozenSet({1, 2})"
    assert repr(FrozenSet[int]([3])) == "FrozenSet[int]({3})"


def test_weakref_and_abcs():
    fs = FrozenSet([1])
    r = weakref.ref(fs)
    assert r() is fs
    assert isinstance(fs, cabc.Set) and isinstance(fs, cabc.Hashable) and not isinstance(fs, cabc.MutableSet)


# --- equality, ordering, algebra ----------------------------------------------------------


@settings(max_examples=300)
@given(st.lists(SMALL_INTS, max_size=8), st.lists(SMALL_INTS, max_size=8), st.sampled_from(["FrozenSet", "Set", "set", "frozenset"]))
def test_comparisons_match_frozenset(a, b, kind):
    make = {"FrozenSet": FrozenSet, "Set": Set, "set": set, "frozenset": frozenset}[kind]
    left, ref, right = FrozenSet(a), frozenset(a), make(b)
    for op in CMPS:
        assert op(left, right) == op(ref, set(b)), op
        assert op(right, left) == op(set(b), ref), op
    assert left.isdisjoint(right) == ref.isdisjoint(b)
    assert left.issubset(right) == ref.issubset(b)
    assert left.issuperset(right) == ref.issuperset(b)
    assert left.issubset(b) == ref.issubset(b) and left.isdisjoint(iter(b)) == ref.isdisjoint(b)


ELEMENTS = st.one_of(SMALL_INTS, st.sampled_from([1.0, 2.0, True, False, 0.5, "a", "b", (1,), None]))


@settings(max_examples=400)
@given(st.lists(ELEMENTS, max_size=8), st.lists(ELEMENTS, max_size=8), st.sampled_from(["FrozenSet", "Set", "set", "frozenset"]))
def test_algebra_matches_frozenset(a, b, kind):
    make = {"FrozenSet": FrozenSet, "Set": Set, "set": set, "frozenset": frozenset}[kind]
    left, ref = FrozenSet(a), frozenset(a)
    for name, op in SET_OPS.items():
        got, want = op(left, make(b)), op(ref, frozenset(b))
        assert type(got) is FrozenSet, name
        check_same(got, want)
        got = op(make(b), left)  # left operand's kind decides the result type
        want = op(frozenset(b) if kind in ("FrozenSet", "frozenset") else set(b), ref)
        assert type(got) is (FrozenSet if kind in ("FrozenSet", "frozenset") else Set), name
        check_same(got, want)
    check_same(left.union(b, a), ref.union(b, a))
    check_same(left.intersection(b, b), ref.intersection(b, b))
    check_same(left.difference(b, [1]), ref.difference(b, [1]))
    check_same(left.symmetric_difference(b), ref.symmetric_difference(b))


def test_element_sources_follow_cpython():
    # Intersection keeps the elements of the iterated (smaller; right on ties) operand.
    check_same(FrozenSet([1]) & FrozenSet([1.0]), frozenset([1]) & frozenset([1.0]))
    check_same(FrozenSet([1, 2]) & FrozenSet([1.0]), frozenset([1, 2]) & frozenset([1.0]))
    check_same(FrozenSet([1]) & FrozenSet([1.0, 2.0]), frozenset([1]) & frozenset([1.0, 2.0]))
    check_same(FrozenSet([1]).intersection([1.0, True]), frozenset([1]).intersection([1.0, True]))
    check_same(FrozenSet([1, "a"]) | FrozenSet([1.0, "b"]), frozenset([1, "a"]) | frozenset([1.0, "b"]))
    check_same(FrozenSet([1, 2]) ^ FrozenSet([True, 3.0]), frozenset([1, 2]) ^ frozenset([True, 3.0]))


def test_operators_reject_non_sets():
    fs = FrozenSet([1])
    for op in (*SET_OPS.values(), operator.lt, operator.le):
        with pytest.raises(TypeError):
            op(fs, [1])
        with pytest.raises(TypeError):
            op([1], fs)
    assert (fs == [1]) is False and (fs != [1]) is True


def test_typed_results():
    a, b = FrozenSet[int]([1, 2, 3]), FrozenSet[int]([2, 3, 4])
    for op in SET_OPS.values():
        assert op(a, b).schema_type is FrozenSet[int]
        assert op(a, Set[int]([2])).schema_type is FrozenSet[int]
        assert op(a, {2}).schema_type is FrozenSet
    assert (Set[int]([1]) | a).schema_type is Set[int]


def test_set_of_sets():
    s = FrozenSet([frozenset("ab"), FrozenSet("cd")])
    assert frozenset("ba") in s and FrozenSet("dc") in s
    assert Set("ab") in s and set("dc") in s and Set("xy") not in s
    with pytest.raises(TypeError):
        [1] in s


# --- pickling and iteration ------------------------------------------------------------------


def test_pickle_round_trips():
    for obj in (FrozenSet(), FrozenSet([1, "a", (2, 3)]), FrozenSet[str](["x", "y"]), FrozenSet[int](range(5))):
        for proto in range(pickle.HIGHEST_PROTOCOL + 1):
            back = pickle.loads(pickle.dumps(obj, proto))
            assert back == obj and type(back) is FrozenSet and back.schema_type is obj.schema_type
            assert hash(back) == hash(obj)


def test_iteration_and_iterator_pickling():
    fs = FrozenSet([3, 1, 2])
    assert list(fs) == [3, 1, 2]  # insertion order
    it = iter(fs)
    assert it.__length_hint__() == 3
    assert next(it) == 3
    assert it.__length_hint__() == 2
    for proto in range(pickle.HIGHEST_PROTOCOL + 1):
        assert list(pickle.loads(pickle.dumps(it, proto))) == [1, 2]
    assert list(it) == [1, 2] and list(it) == [] and it.__length_hint__() == 0
    assert list(iter(FrozenSet(range(100)) - FrozenSet(range(0, 100, 2)))) == list(range(1, 100, 2))


# --- subclassing ---------------------------------------------------------------------


class Sub(FrozenSet):
    pass


class WithInit(FrozenSet):
    def __init__(self, it, newarg=1):
        self.newarg = newarg


def test_subclasses():
    s = Sub([1, 2])
    assert type(s) is Sub and s == {1, 2} and repr(s) == "Sub({1, 2})"
    assert Sub(s) is not s and FrozenSet(s) is not s and type(FrozenSet(s)) is FrozenSet
    c = s.copy()
    assert type(c) is FrozenSet and c == s
    assert type(s | {3}) is FrozenSet
    s.attr = [1]
    back = pickle.loads(pickle.dumps(s))
    assert type(back) is Sub and back == s and back.attr == [1]
    w = WithInit([1], newarg=2)
    assert w == {1} and w.newarg == 2
    with pytest.raises(TypeError):
        Sub([1], newarg=2)
    assert hash(Sub([1, 2])) == hash(frozenset({1, 2}))


# --- typing ----------------------------------------------------------------------------


def test_typing():
    T = FrozenSet[int]
    assert T is FrozenSet[int] and repr(T) == "FrozenSet[int]" and T.__origin__ is FrozenSet
    fs = T([1, 2, True])
    assert isinstance(fs, T) and isinstance(fs, FrozenSet) and not isinstance(FrozenSet([1]), T)
    with pytest.raises(TypeError):
        T(["a"])
    nested = List[FrozenSet[int]]([[1, 2], {3}])
    assert all(type(x) is FrozenSet for x in nested) and nested[0].schema_type is T
    keep = T([5])
    assert List[FrozenSet[int]]([keep])[0] is keep
    s = Set[FrozenSet[str]]([["a", "b"]])
    assert FrozenSet(["b", "a"]) in s and frozenset("ab") in s
    assert rs._rustique._schema_repr(FrozenSet[FrozenSet[int]]) == "FrozenSet[FrozenSet[int]]"
    assert pickle.loads(pickle.dumps(T)) is T


# --- free-threaded sharing -----------------------------------------------------------------


def test_shared_iterator_yields_each_element_once():
    fs = FrozenSet(range(20_000))
    it = iter(fs)
    seen = [[] for _ in range(4)]

    def drain(out):
        for x in it:
            out.append(x)

    threads = [threading.Thread(target=drain, args=(out,)) for out in seen]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    got = [x for out in seen for x in out]
    assert sorted(got) == list(range(20_000))


def test_concurrent_hash_and_algebra_with_mutating_sets():
    fs = FrozenSet(range(0, 2000, 2))
    s = Set(range(1000))
    errors = []
    stop = threading.Event()

    def mutate():
        i = 0
        while not stop.is_set():
            s.add(10_000 + i % 50)
            s.discard(10_000 + (i + 25) % 50)
            i += 1

    def compute():
        try:
            for _ in range(200):
                assert hash(fs) == hash(frozenset(range(0, 2000, 2)))
                inter = fs & s
                assert all(x % 2 == 0 and x < 1000 for x in inter)
                assert len(fs - s) >= 500
                assert type(s | fs) is Set and type(fs | s) is FrozenSet
                fs <= s
                fs.isdisjoint(s)
        except Exception as e:  # pragma: no cover - reported below
            errors.append(e)

    m = threading.Thread(target=mutate)
    m.start()
    workers = [threading.Thread(target=compute) for _ in range(3)]
    for w in workers:
        w.start()
    for w in workers:
        w.join()
    stop.set()
    m.join()
    assert not errors, errors


@pytest.mark.skipif(sys.version_info < (3, 14), reason="needs 3.14+")
def test_frozen_set_as_shared_dict_key_under_threads():
    keys = [FrozenSet([i, i + 1]) for i in range(200)]
    d = {k: i for i, k in enumerate(keys)}

    def look():
        for _ in range(20):
            for i in range(200):
                assert d[frozenset([i, i + 1])] == i

    threads = [threading.Thread(target=look) for _ in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
