"""`Set` parity with builtin `set` (ARCHITECTURE §4.3), typed behaviour and strategies."""

import copy
import gc
import operator
import pickle
import threading
from decimal import Decimal
from enum import IntEnum
from fractions import Fraction

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import Dict, Set

NAN = float("nan")


def typed_elements(s):
    return sorted(((type(e).__name__, repr(e)) for e in s))


def check_same(s, ref):
    assert len(s) == len(ref)
    assert s == ref
    assert typed_elements(s) == typed_elements(ref)


# --- construction ---------------------------------------------------------------------


def test_construction_forms():
    assert Set() == set()
    for source in ([1, 2, 2], (3, 4), {5, 6}, frozenset({7}), {"k": 1}, "abca", range(5)):
        check_same(Set(source), set(source))
    check_same(Set(x for x in "xy"), set("xy"))
    check_same(Set(Set([1, "a"])), {1, "a"})
    check_same(Set(Dict({1: 2})), {1})
    with pytest.raises(TypeError):
        Set(5)
    with pytest.raises(TypeError):
        Set([], [])


def test_numeric_equivalence_keeps_first_element():
    check_same(Set([1, 1.0, True]), {1, 1.0, True})
    check_same(Set([True, 1, 1.0]), {True, 1, 1.0})
    check_same(Set([1.0, 1]), {1.0, 1})
    s = Set([1, 2])
    assert 1.0 in s and True in s and Fraction(2) in s and Decimal(2) in s and 2.5 not in s

    class Color(IntEnum):
        RED = 1

    assert Color.RED in s


def test_strategies():
    assert Set()._strategy == "empty"
    assert Set([1, 2])._strategy == "i64"
    assert Set(["a"])._strategy == "str"
    assert Set([1.5])._strategy == "object"
    assert Set([True])._strategy == "object"
    s = Set([1, 2])
    s.add("x")
    assert s._strategy == "object"
    check_same(s, {1, 2, "x"})


def test_unhashable_elements():
    message = r"cannot use 'list' as a set element \(unhashable type: 'list'\)"
    with pytest.raises(TypeError, match=message):
        Set([[1]])
    s = Set([1])
    for op in (lambda: s.add([1]), lambda: [1] in s, lambda: s.discard([1]), lambda: s.remove({}), lambda: Set([(1, [2])])):
        with pytest.raises(TypeError):
            op()
    # Like CPython, an unhashable set is looked up as a frozenset.
    t = Set([frozenset({1, 2})])
    assert {1, 2} in t
    t.discard({1, 2})
    assert len(t) == 0
    with pytest.raises(KeyError):
        t.remove({1})


# --- mutation -------------------------------------------------------------------------


def test_add_discard_remove_pop_clear():
    s = Set()
    s.add(1)
    s.add("a")
    s.add(1)
    assert len(s) == 2
    s.discard("missing")
    s.discard("a")
    assert s == {1}
    with pytest.raises(KeyError) as exc:
        s.remove((1, 2))
    assert exc.value.args == ((1, 2),)
    s.remove(1)
    with pytest.raises(KeyError, match="pop from an empty set"):
        s.pop()
    s.update([3, 4, 5])
    popped = {s.pop() for _ in range(3)}
    assert popped == {3, 4, 5} and len(s) == 0
    s.update("abc")
    s.clear()
    assert s == set() and len(s) == 0


def test_heavy_churn_matches_set():
    s, ref = Set(), set()
    for i in range(5000):
        k = (i * 7919) % 211
        if i % 3 == 0:
            s.discard(k)
            ref.discard(k)
        else:
            s.add(k)
            ref.add(k)
    check_same(s, ref)
    assert list(s) == list(dict.fromkeys(e for e in s))  # no duplicates in iteration


def test_iteration_is_insertion_ordered_and_live():
    s = Set([3, 1, 2])
    assert list(s) == [3, 1, 2]
    s.discard(1)
    s.add(1)
    assert list(s) == [3, 2, 1]
    it = iter(s)
    next(it)
    s.add(9)
    with pytest.raises(RuntimeError, match="Set changed size during iteration"):
        next(it)
    it = iter(Set([1]))
    assert list(it) == [1]
    with pytest.raises(StopIteration):
        next(it)


# --- algebra --------------------------------------------------------------------------

OPERANDS = [
    lambda xs: Set(xs),
    lambda xs: set(xs),
    lambda xs: frozenset(xs),
]


@pytest.mark.parametrize("make", OPERANDS)
@pytest.mark.parametrize(
    ("a", "b"),
    [
        ([1, 2, 3], [2, 3, 4]),
        (["a", "b"], ["b", "c"]),
        ([1, "a", (1, 2)], [(1, 2), 2.5]),
        ([1, 2], [1.0, True, "x"]),
        ([], [1]),
        ([1, 2], []),
    ],
)
def test_operators_match_set(make, a, b):
    left, ref = Set(a), set(a)
    right = make(b)
    for op in (operator.or_, operator.and_, operator.sub, operator.xor):
        got, want = op(left, right), op(ref, set(b))
        assert type(got) is Set
        assert got == want, op
    for op in (operator.eq, operator.ne, operator.le, operator.lt, operator.ge, operator.gt):
        assert op(left, right) == op(ref, set(b)), op
        assert op(right, left) == op(set(b), ref), op
    assert left.isdisjoint(right) == ref.isdisjoint(b)
    assert left.issubset(right) == ref.issubset(b)
    assert left.issuperset(right) == ref.issuperset(b)


def test_reflected_operators_with_builtin_sets():
    s = Set([1, 2, 3])
    for op in (operator.or_, operator.and_, operator.sub, operator.xor):
        got = op({2, 9}, s)
        assert type(got) is Set
        assert got == op({2, 9}, {1, 2, 3})
        got = op(frozenset({2, 9}), s)
        assert got == op(frozenset({2, 9}), {1, 2, 3})


def test_operators_reject_non_sets():
    s = Set([1])
    for op in (operator.or_, operator.and_, operator.sub, operator.xor, operator.lt, operator.le):
        with pytest.raises(TypeError):
            op(s, [1])
        with pytest.raises(TypeError):
            op([1], s)
    for op in (operator.ior, operator.iand, operator.isub, operator.ixor):
        t = Set([1])
        with pytest.raises(TypeError):
            op(t, [1])
    assert (s == [1]) is False and (s != [1]) is True


def test_method_forms_accept_iterables_and_many_arguments():
    s, ref = Set([1, 2, 3, 4]), {1, 2, 3, 4}
    assert s.union([5], (6,), "a") == ref.union([5], (6,), "a")
    assert s.intersection([1, 2, 3], range(2, 9)) == ref.intersection([1, 2, 3], range(2, 9))
    assert s.difference([1], {2}) == ref.difference([1], {2})
    assert s.symmetric_difference([4, 5, 5]) == ref.symmetric_difference([4, 5, 5])
    assert s.union() == ref and s.union() is not s
    assert s.intersection() == ref
    assert s.issubset(range(10)) and s.issuperset(iter([1, 2])) and s.isdisjoint("xyz")


def test_in_place_variants():
    for method, args in [
        ("update", ([5, 6], "ab")),
        ("intersection_update", ([1, 2, 3, 9], range(2, 5))),
        ("difference_update", ([1], {2, 99})),
        ("symmetric_difference_update", ([3, 4, 7, 7],)),
    ]:
        s, ref = Set([1, 2, 3, 4]), {1, 2, 3, 4}
        assert getattr(s, method)(*args) is None
        getattr(ref, method)(*args)
        check_same(s, ref)
    s = Set([1, 2, 3])
    alias = s
    s |= {4}
    s &= frozenset({1, 2, 4})
    s -= Set([1])
    s ^= {2, 5}
    assert s is alias
    check_same(s, {4, 5})


def test_in_place_with_itself():
    s = Set([1, 2])
    s |= s
    assert s == {1, 2}
    s &= s
    assert s == {1, 2}
    s ^= s
    assert s == set()
    t = Set([1, 2])
    t -= t
    assert t == set()


def test_element_source_follows_cpython():
    # `{1} & [1.0]`-style results keep the element CPython keeps.
    check_same(Set([1]).intersection([1.0]), {1}.intersection([1.0]))
    check_same(Set([1]) | {1.0, 2.0}, {1} | {1.0, 2.0})
    check_same(Set([1, 2]) - {1.0}, {1, 2} - {1.0})
    check_same(Set([1]) ^ {1.0, True, 3.0}, {1} ^ {1.0, True, 3.0})


# --- protocol details -----------------------------------------------------------------


def test_hash_and_repr():
    with pytest.raises(TypeError):
        hash(Set())
    assert repr(Set()) == "Set()"
    assert repr(Set([1, 2])) == "Set({1, 2})"
    assert repr(Set(["a", None])) == "Set({'a', None})"
    assert repr(Set[int]([1, 2])) == "Set[int]({1, 2})"
    assert repr(Set[int]()) == "Set[int]()"


def test_to_set_copy_pickle():
    s = Set([1, "a", (2, 3)])
    out = s.to_set()
    assert type(out) is set and out == {1, "a", (2, 3)}
    c = s.copy()
    assert c == s and c is not s
    c.add(9)
    assert 9 not in s
    assert copy.copy(s) == s and copy.deepcopy(s) == s
    for obj in (s, Set[str](["x", "y"])):
        for proto in range(pickle.HIGHEST_PROTOCOL + 1):
            back = pickle.loads(pickle.dumps(obj, proto))
            assert back == obj and back.schema_type is obj.schema_type


def test_nan_elements_use_identity():
    s = Set([NAN])
    assert NAN in s and float("nan") not in s
    s.add(float("nan"))
    assert len(s) == 2


def test_mutating_eq_does_not_crash():
    class Evil:
        def __init__(self, target):
            self.target = target

        def __hash__(self):
            return 1

        def __eq__(self, other):
            self.target.clear()
            return False

    s = Set()
    s.add(Evil(s))
    s.add(Evil(Set()))
    assert len(s) <= 2


# --- typed sets -----------------------------------------------------------------------


def test_typed_sets():
    s = Set[int]([1, 2, True])
    assert s == {1, 2} and s._strategy == "i64"
    with pytest.raises(TypeError, match=r"Set\[int\] expects int, got str"):
        s.add("x")
    with pytest.raises(TypeError):
        s.update(["y"])
    with pytest.raises(TypeError):
        s ^= {"z"}
    assert s == {1, 2}
    assert "x" not in s and 1.0 in s
    f = Set[float]([1, 2.5])
    assert typed_elements(f) == typed_elements({1.0, 2.5})
    assert 1 in f
    assert (s | Set[int]([3])).schema_type is Set[int]
    assert (s | {3}).schema_type is Set
    assert s.intersection(Set[int]([1])).schema_type is Set[int]
    assert s.copy().schema_type is Set[int]
    assert Set[int](s).schema_type is Set[int]
    assert Set(s).schema_type is Set
    big = Set[int]([2**70, 1])
    assert big == {2**70, 1}


# --- GC and threads -------------------------------------------------------------------


def test_reference_cycles_are_collected():
    collected = []

    class Node:
        def __hash__(self):
            return 0

        def __del__(self):
            collected.append(True)

    s = Set()
    node = Node()
    node.owner = s
    s.add(node)
    del s, node
    gc.collect()
    assert collected == [True]


def test_concurrent_adds():
    s = Set()

    def work(base):
        for i in range(3000):
            s.add(base + i)
            s.add((base, i))

    threads = [threading.Thread(target=work, args=(t * 100_000,)) for t in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(s) == 4 * 3000 * 2


# --- differential (Hypothesis) --------------------------------------------------------

ELEMENTS = st.one_of(
    st.integers(-4, 4),
    st.sampled_from([1.0, 0.5, True, False, 2**64, None, "a", "", (1, 2), NAN]),
    st.text(max_size=2),
)

OPS = st.one_of(
    st.tuples(st.just("add"), ELEMENTS),
    st.tuples(st.just("discard"), ELEMENTS),
    st.tuples(st.just("remove"), ELEMENTS),
    st.tuples(st.just("in"), ELEMENTS),
    st.tuples(st.just("pop")),
    st.tuples(
        st.sampled_from(["update", "intersection_update", "difference_update", "symmetric_difference_update"]),
        st.lists(ELEMENTS, max_size=5),
    ),
    st.tuples(st.sampled_from(["|", "&", "-", "^"]), st.lists(ELEMENTS, max_size=5)),
)

BINARY = {"|": operator.or_, "&": operator.and_, "-": operator.sub, "^": operator.xor}


@settings(max_examples=300, deadline=None)
@given(st.lists(ELEMENTS, max_size=10), st.lists(OPS, max_size=30))
def test_differential_against_set(initial, ops):
    s, ref = Set(initial), set(initial)
    check_same(s, ref)
    for op in ops:
        name = op[0]
        if name in ("add", "discard"):
            getattr(s, name)(op[1])
            getattr(ref, name)(op[1])
        elif name == "remove":
            outcomes = []
            for target in (s, ref):
                try:
                    target.remove(op[1])
                    outcomes.append(None)
                except KeyError as e:
                    outcomes.append(e.args)
            assert outcomes[0] == outcomes[1]
        elif name == "in":
            assert (op[1] in s) == (op[1] in ref)
        elif name == "pop":
            if ref:
                e = s.pop()
                assert e in ref
                ref.remove(e)
            else:
                with pytest.raises(KeyError):
                    s.pop()
        elif name in BINARY:
            got, want = BINARY[name](s, set(op[1])), BINARY[name](ref, set(op[1]))
            assert got == want
            assert len(got) == len(want)
        else:
            getattr(s, name)(op[1])
            getattr(ref, name)(op[1])
        assert s == ref, op
        assert len(s) == len(ref), op
    assert s.to_set() == ref
    assert sorted(map(repr, s)) == sorted(map(repr, ref))


def test_intersection_update_element_source():
    s, ref = Set([False]), {False}
    s.intersection_update([0])
    ref.intersection_update([0])
    check_same(s, ref)
    t = Set[int]([1, 2])
    t.intersection_update([1.0])
    assert typed_elements(t) == typed_elements({1})


def test_finalizers_never_run_under_the_lock():
    seen = []

    class K:
        def __init__(self, owner):
            self.owner = owner

        def __hash__(self):
            return 1

        def __eq__(self, other):
            return isinstance(other, K)

        def __del__(self):
            seen.append(len(self.owner))

    s = Set()
    s.update(K(s) for _ in range(3))
    s.symmetric_difference_update([K(s), K(s)])
    assert len(s) == 0 and seen


# --- CPython protocol details: subclassing, weakrefs, __init__, iterator pickling -------


class SubSet(Set):
    pass


class SubWithInit(Set):
    def __init__(self, n, *, step=1):
        super().__init__(range(0, n, step))


def test_subclassing():
    s = SubSet([1, 2])
    assert type(s) is SubSet and isinstance(s, Set) and s == {1, 2}
    assert repr(s) == "SubSet({1, 2})"
    assert type(s.copy()) is Set and type(s | {3}) is Set
    s.attr = "x"
    back = pickle.loads(pickle.dumps(s))
    assert type(back) is SubSet and back == s and back.attr == "x"
    assert SubWithInit(6, step=2) == {0, 2, 4}
    with pytest.raises(TypeError):
        SubSet([1], x=1)

    class Hashable(Set):
        def __hash__(self):
            return id(self) & 0xFFFF

    h = Hashable([1])
    f = {h}
    assert h in f


def test_weakref():
    import weakref

    s = Set([1])
    r = weakref.ref(s)
    assert r() is s
    del s
    gc.collect()
    assert r() is None


def test_init_reinitialises_like_set_init():
    s, ref = Set(), set()
    for arg in ("abc", [1, 2], ()):
        s.__init__(arg)
        ref.__init__(arg)
        check_same(s, ref)
    s.__init__()
    assert s == set()
    t, tref = Set([1, 2]), {1, 2}
    t.__init__(t)  # clears first, like CPython: re-initialising from itself empties it
    tref.__init__(tref)
    check_same(t, tref)
    with pytest.raises(TypeError):
        s.__init__([1], 2)
    with pytest.raises(TypeError):
        s.__init__(1)
    with pytest.raises(TypeError):
        s.__init__(a=1)
    # Construction fills the set once: `__init__` is not a `tp_init` slot of `Set`.
    calls = []

    def gen():
        calls.append(1)
        yield 1

    assert Set(gen()) == {1} and calls == [1]


def test_iterator_pickling_and_release():
    s = Set([3, 1, 2])
    it = iter(s)
    assert next(it) == 3
    for proto in range(pickle.HIGHEST_PROTOCOL + 1):
        assert list(pickle.loads(pickle.dumps(it, proto))) == [1, 2]
    assert list(it) == [1, 2]
    assert list(pickle.loads(pickle.dumps(it))) == []
    s.add(9)  # the exhausted iterator released the set
    assert list(it) == [] and it.__length_hint__() == 0


def test_set_probes_are_looked_up_as_frozensets():
    s = Set([frozenset("abc"), 1])
    for probe in (Set("abc"), set("cab"), Set[str]("abc")):
        assert probe in s
    assert Set("ab") not in s and set() not in s
    s.discard(Set("cba"))
    assert s == {1}
    s.add(frozenset("xy"))
    s.remove(set("yx"))
    with pytest.raises(KeyError):
        s.remove(Set("xy"))
    with pytest.raises(TypeError):
        Set([[1]])
    with pytest.raises(TypeError):
        [1] in s


def test_builtin_set_subclass_sources_read_their_table():
    class Weird(set):
        def __iter__(self):
            return iter([99])

    w = Weird({1, 2})
    assert Set(w) == {1, 2} == set(w)
    assert (Set([1, 5]) & w) == {1}
    assert Set([1]).union(w) == {1, 2}


def test_fast_sources():
    from rustique import List

    check_same(Set(range(-3, 10, 4)), set(range(-3, 10, 4)))
    check_same(Set(range(10, 0, -3)), set(range(10, 0, -3)))
    check_same(Set(range(2**70, 2**70 + 3)), set(range(2**70, 2**70 + 3)))
    check_same(Set[int](range(3)), {0, 1, 2})
    check_same(Set[float](range(3)), {0.0, 1.0, 2.0})
    check_same(Set(List([1, 2, 2])), {1, 2})
    check_same(Set(List(["a", "a"])), {"a"})
    check_same(Set(List([1.5, 1.5])), {1.5})
    check_same(Set[str](List(["a"])), {"a"})
    with pytest.raises(TypeError):
        Set[str](List([1]))
    check_same(Set([1, 2, "a", 1.0, 3]), {1, 2, "a", 1.0, 3})  # native prefix, then objects
    check_same(Set((1.0, 1, True)), {1.0, 1, True})
    check_same(Set({1: 0, 1.0: 0, "a": 0}), {1, "a"})
    check_same(Set({1, 2, (3,)}), {1, 2, (3,)})


@pytest.mark.parametrize(
    "a, b",
    [
        ([1.5, 2.5, "x", (1, 2)], [2.5, (1, 2), None]),
        ([1, 2.0, True], [1.0, 2, 3]),
        ([(1, "a"), (2, "b")], [(1, "a")]),
        ([1.0, 2.0], ["1", 2]),
    ],
)
def test_object_tables_algebra_and_comparisons(a, b):
    x, y = Set(a), Set(b)
    ref_x, ref_y = set(a), set(b)
    for op in (operator.or_, operator.and_, operator.sub, operator.xor):
        check_same(op(x, y), op(ref_x, ref_y))
        check_same(op(y, x), op(ref_y, ref_x))
    for op in (operator.eq, operator.ne, operator.le, operator.lt, operator.ge, operator.gt):
        assert op(x, y) == op(ref_x, ref_y) and op(y, x) == op(ref_y, ref_x)
    assert x.isdisjoint(y) == ref_x.isdisjoint(ref_y)


def test_algebra_with_python_equality_falls_back():
    calls = []

    class Eq:
        def __init__(self, v):
            self.v = v

        def __hash__(self):
            return hash(self.v)

        def __eq__(self, other):
            calls.append(1)
            return isinstance(other, Eq) and other.v == self.v

    a, b = Set([Eq(1), Eq(2)]), Set([Eq(2), Eq(3)])
    assert {e.v for e in a & b} == {2} and calls
    assert {e.v for e in a - b} == {1} and {e.v for e in a ^ b} == {1, 3}
    assert not a <= b and not a.isdisjoint(b) and a != b


def test_equality_and_subset_stop_early():
    import time

    big = Set(range(200_000))
    other = Set(range(1, 200_001))
    start = time.perf_counter()
    for _ in range(2000):
        assert big != other and not big <= other and not big.isdisjoint(other)
    assert time.perf_counter() - start < 2, "comparisons copy or scan whole tables"


def test_large_builtin_operands_are_probed():
    from rustique import FrozenSet

    big = set(range(1000)) | {"a", (1, 2)}
    for make in (Set, FrozenSet):
        for small in ([1.0, 2, "a", "zz"], [True, 999], [(1, 2), 5000], []):
            x, ref = make(small), frozenset(small)
            check_same(x & big, ref & big)
            check_same(x.intersection(big), ref.intersection(big))
            check_same(x - big, ref - big)
            assert x.isdisjoint(big) == ref.isdisjoint(big)
            assert x.issubset(big) == ref.issubset(big)
            for op in (operator.eq, operator.ne, operator.le, operator.lt, operator.ge, operator.gt):
                assert op(x, big) == op(ref, big), op
    calls = []

    class Probe:
        def __hash__(self):
            return -7

        def __eq__(self, other):
            calls.append(other)
            return False

    big2 = {Probe() for _ in range(3)} | set(range(100))
    assert not (Set([-7]) <= big2) and calls  # big2's elements compare as CPython would


@pytest.mark.parametrize("make", [lambda i: i, lambda i: i + 0.5, lambda i: (i, "t")])
def test_concurrent_algebra_between_mutating_sets(make):
    from rustique import FrozenSet

    a = Set(make(i) for i in range(300))
    b = Set(make(i) for i in range(150, 450))
    frozen = FrozenSet(make(i) for i in range(100, 200))
    universe = {make(i) for i in range(-50, 500)}
    stop = threading.Event()
    errors = []

    def mutate(s, lo):
        i = 0
        while not stop.is_set():
            s.add(make(lo - i % 40))
            s.discard(make(lo - (i + 20) % 40))
            i += 1

    def compute():
        try:
            for _ in range(150):
                for r in (a | b, a & b, a - b, a ^ b, a & frozen, frozen | a, b - frozen):
                    assert set(r) <= universe
                a <= b, a == b, a.isdisjoint(b), b >= frozen
                c = Set(a)
                c.update(b)
                c.difference_update(frozen)
                assert set(c) <= universe
        except Exception as e:  # pragma: no cover - reported below
            errors.append(e)

    mutators = [threading.Thread(target=mutate, args=(a, -1)), threading.Thread(target=mutate, args=(b, 490))]
    workers = [threading.Thread(target=compute) for _ in range(3)]
    for t in mutators + workers:
        t.start()
    for t in workers:
        t.join()
    stop.set()
    for t in mutators:
        t.join()
    assert not errors, errors
