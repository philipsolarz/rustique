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
