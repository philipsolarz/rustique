"""Core behaviour of untyped `rustique.List`: builtin-`list` parity, storage strategies,
live iterators, buffer exports, identity/NaN semantics and thread safety."""

import array
import copy
import gc
import math
import pickle
import random
import sys
import threading

import pytest

from rustique import List
from rustique._rustique import arange

NAN = float("nan")

DATASETS = {
    "ints": [3, 1, 4, 1, 5, 9, 2, 6, -7, 0],
    "floats": [3.5, -1.0, 2.25, 0.0, -0.0, 1e300, 2.25],
    "strs": ["pear", "apple", "fig", "", "ünïcode", "😀", "apple"],
    "bools": [True, False, True, True],
    "mixed": [1, "a", 2.5, None, (1, 2), True, 1],
    "bigints": [2**70, -(2**65), 5, 2**70],
    "empty": [],
}


def same(a, b):
    """Element-wise equality including element types (NaN equals NaN)."""
    a, b = list(a), list(b)
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        if type(x) is not type(y):
            return False
        if isinstance(x, float) and math.isnan(x) and math.isnan(y):
            continue
        if x != y:
            return False
    return True


def run(op, target):
    try:
        return op(target), None
    except Exception as e:  # noqa: BLE001 - parity of exception types is the point
        return None, e


def plain(value):
    return value.to_list() if isinstance(value, List) else value


def check_parity(op, data):
    """Apply `op` to a builtin list and a List built from `data`; results, exception
    types and final contents must agree."""
    builtin, ours = list(data), List(data)
    expected, expected_error = run(op, builtin)
    actual, actual_error = run(op, ours)
    assert type(actual_error) is type(expected_error), (expected_error, actual_error)
    if expected_error is None:
        if expected is builtin:
            assert actual is ours
        elif isinstance(expected, list):
            assert isinstance(actual, (List, list)), actual
            assert same(plain(actual), expected), (actual, expected)
        else:
            assert same([plain(actual)], [expected]), (actual, expected)
    assert same(ours.to_list(), builtin), (ours, builtin)


SLICE_BOUNDS = [None, 0, 1, 3, -1, -3, 100, -100]
SLICE_STEPS = [None, 1, 2, 3, -1, -2, -3]
SLICES = [slice(a, b, c) for a in SLICE_BOUNDS for b in SLICE_BOUNDS for c in SLICE_STEPS]

READ_OPS = [
    len,
    bool,
    lambda l: l[0],
    lambda l: l[-1],
    lambda l: l[2],
    lambda l: l[-100],
    lambda l: l[True],
    lambda l: l[1.0],
    lambda l: l["a"],
    lambda l: l[2**100],
    lambda l: l[::0],
    lambda l: list(l),
    lambda l: list(reversed(l)),
    lambda l: 1 in l,
    lambda l: 1.0 in l,
    lambda l: True in l,
    lambda l: "apple" in l,
    lambda l: None in l,
    lambda l: 2**70 in l,
    lambda l: (1, 2) in l,
    lambda l: l.count(1),
    lambda l: l.count("apple"),
    lambda l: l.count(2.25),
    lambda l: l.count(True),
    lambda l: l.index(1),
    lambda l: l.index(1, 2),
    lambda l: l.index(1, -3, -1),
    lambda l: l.index("apple", 2),
    lambda l: l.index(2.25),
    lambda l: l.index(12345),
    lambda l: l.index(1, "x"),
    lambda l: l.copy(),
    lambda l: l * 2,
    lambda l: 3 * l,
    lambda l: l * 0,
    lambda l: l * -1,
    lambda l: l * True,
    lambda l: l * 1.5,
    lambda l: l * 2**100,
    lambda l: l + [7, "x"],
    lambda l: l + (1,),
    lambda l: l == [3, 1, 4, 1, 5, 9, 2, 6, -7, 0],
    lambda l: l != [],
    lambda l: l < [3, 2],
    lambda l: l <= ["pear"],
    lambda l: l > [True],
    lambda l: l >= [],
    lambda l: l == "abc",
]

MUTATE_OPS = [
    lambda l: l.append(42),
    lambda l: l.append("s"),
    lambda l: l.append(1.5),
    lambda l: l.append(None),
    lambda l: l.extend([1, 2]),
    lambda l: l.extend(range(3)),
    lambda l: l.extend("ab"),
    lambda l: l.extend((x for x in [9.5, 8.5])),
    lambda l: l.extend(l),
    lambda l: l.extend(5),
    lambda l: l.insert(0, 99),
    lambda l: l.insert(-1, "z"),
    lambda l: l.insert(100, 2.5),
    lambda l: l.insert(-100, True),
    lambda l: l.insert(2**100, 1),
    lambda l: l.pop(),
    lambda l: l.pop(0),
    lambda l: l.pop(-2),
    lambda l: l.pop(100),
    lambda l: l.pop("a"),
    lambda l: l.remove(1),
    lambda l: l.remove("apple"),
    lambda l: l.remove(2.25),
    lambda l: l.remove(12345),
    lambda l: l.clear(),
    lambda l: l.reverse(),
    lambda l: l.__setitem__(0, "first"),
    lambda l: l.__setitem__(-1, 7),
    lambda l: l.__setitem__(100, 7),
    lambda l: l.__setitem__(slice(1, 3), [10, 20, 30]),
    lambda l: l.__setitem__(slice(1, 3), []),
    lambda l: l.__setitem__(slice(None), l),
    lambda l: l.__setitem__(slice(3, 1), ["x"]),
    lambda l: l.__setitem__(slice(None, None, 2), range(len(l[::2]))),
    lambda l: l.__setitem__(slice(None, None, -1), list(l)),
    lambda l: l.__setitem__(slice(None, None, 2), [1]),
    lambda l: l.__setitem__(slice(0, 1), 5),
    lambda l: l.__delitem__(0),
    lambda l: l.__delitem__(-1),
    lambda l: l.__delitem__(100),
    lambda l: l.__delitem__(slice(1, None, 2)),
    lambda l: l.__delitem__(slice(None, None, -3)),
    lambda l: l.__delitem__(slice(3, 1)),
    lambda l: l.__iadd__([1, 2]),
    lambda l: l.__iadd__(5),
    lambda l: l.__imul__(2),
    lambda l: l.__imul__(0),
    lambda l: l.sort(),
    lambda l: l.sort(reverse=True),
    lambda l: l.sort(key=str),
    lambda l: l.sort(key=lambda x: -hash(x) % 7),
    lambda l: l.sort(key=repr, reverse=True),
]


@pytest.mark.parametrize("data", DATASETS.values(), ids=DATASETS.keys())
@pytest.mark.parametrize("op", READ_OPS)
def test_read_parity(op, data):
    check_parity(op, data)


@pytest.mark.parametrize("data", DATASETS.values(), ids=DATASETS.keys())
@pytest.mark.parametrize("op", MUTATE_OPS)
def test_mutation_parity(op, data):
    check_parity(op, data)


@pytest.mark.parametrize("data", DATASETS.values(), ids=DATASETS.keys())
def test_slicing_parity(data):
    builtin, ours = list(data), List(data)
    for s in SLICES:
        result = ours[s]
        assert isinstance(result, List)
        assert same(result.to_list(), builtin[s]), s


@pytest.mark.parametrize("data", [DATASETS["ints"], DATASETS["strs"], DATASETS["mixed"]])
def test_slice_assignment_and_deletion_parity(data):
    values = [[], [100], [100, 200, 300], ["v"]]
    for s in SLICES:
        for v in values:
            check_parity(lambda l: l.__setitem__(s, v), data)
        check_parity(lambda l: l.__delitem__(s), data)


@pytest.mark.parametrize("seed", range(20))
def test_random_operation_sequences(seed):
    rng = random.Random(seed)
    pool = [0, 1, -1, 2**40, 2**70, 1.5, NAN, -0.0, True, False, "a", "ü", "", None, (1,), "a"]
    builtin = [rng.choice(pool[:4]) for _ in range(rng.randrange(8))]
    ours = List(builtin)
    for _ in range(60):
        kind = rng.randrange(9)
        value = rng.choice(pool)
        i = rng.randrange(-12, 12)
        s = slice(rng.choice(SLICE_BOUNDS), rng.choice(SLICE_BOUNDS), rng.choice([None, 1, 2, -1, -2]))
        ops = [
            lambda l: l.append(value),
            lambda l: l.insert(i, value),
            lambda l: l.pop(i),
            lambda l: l.__setitem__(i, value),
            lambda l: l.__delitem__(s),
            lambda l: l.__setitem__(s, [value] * len(l[s]) if (s.step or 1) != 1 else [value, value]),
            lambda l: l.extend([value, value]),
            lambda l: l.remove(value),
            lambda l: l.reverse(),
        ]
        expected = run(ops[kind], builtin)
        actual = run(ops[kind], ours)
        assert type(expected[1]) is type(actual[1])
        assert same(ours.to_list(), builtin), (seed, kind, value, i, s)
        assert len(ours) == len(builtin)


# --------------------------------------------------------------------------------------
# Construction
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "source",
    [
        [1, 2, 3],
        (1.5, 2.5),
        range(5),
        range(10, -10, -3),
        range(0),
        range(2**62, 2**62 + 3),
        range(2**63 - 2, 2**63 + 2),
        "héllo",
        {"a": 1, "b": 2},
        {3},
        b"bytes",
        bytearray(b"\x00\xff"),
        (x * x for x in range(4)),
        iter([1, "a"]),
        [True, False],
        [2**64, 1],
        ["a", "b\udc80"],
    ],
)
def test_construction_matches_list(source):
    if hasattr(source, "__next__"):
        items = list(source)
        assert same(List(iter(items)).to_list(), items)
    else:
        assert same(List(source).to_list(), list(source))


def test_construction_errors():
    with pytest.raises(TypeError):
        List(None)
    with pytest.raises(TypeError):
        List(1)
    with pytest.raises(TypeError):
        List([1], [2])
    with pytest.raises(TypeError):
        List(iterable=[1])


def test_construction_from_buffers():
    assert List(array.array("d", [1.0, 2.5])).to_list() == [1.0, 2.5]
    assert List(array.array("d", [1.0])).strategy == "f64"
    assert List(array.array("i", [-1, 7])).strategy == "i64"
    assert List(array.array("f", [0.5])).strategy == "f64"
    assert List(array.array("Q", [2**64 - 1])).to_list() == [2**64 - 1]
    assert List(memoryview(b"\x00\x02").cast("?")).to_list() == [False, True]
    assert List(memoryview(array.array("q", [5, 6]))[::-1]).to_list() == [6, 5]
    assert same(List(array.array("w", "ab")).to_list(), ["a", "b"])


def test_construction_from_numpy():
    np = pytest.importorskip("numpy")
    ints = List(np.arange(5, dtype=np.int32))
    assert ints.to_list() == [0, 1, 2, 3, 4] and ints.strategy == "i64"
    assert all(type(x) is int for x in ints)
    floats = List(np.linspace(0, 1, 3))
    assert floats.to_list() == [0.0, 0.5, 1.0] and floats.strategy == "f64"
    assert List(np.array([True, False])).strategy == "bool"
    grid = List(np.zeros((2, 2)))  # 2-D: iterates rows like `list()`
    assert len(grid) == 2 and grid.strategy == "object"


def test_construction_from_list_shares_strategy():
    source = List([1.5, 2.5])
    clone = List(source)
    assert clone.strategy == "f64" and clone == source
    source.append(3.5)
    assert len(clone) == 2


# --------------------------------------------------------------------------------------
# Strategies
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "data, strategy",
    [
        ([], "empty"),
        ([1, 2], "i64"),
        ([1.0], "f64"),
        (["x"], "str"),
        ([True, False], "bool"),
        ([1, 2.0], "object"),
        ([True, 1], "object"),
        ([2**63], "object"),
        (["ok", "\udc80"], "object"),
        ([None], "object"),
        ([1, "a"], "object"),
    ],
)
def test_strategy_selection(data, strategy):
    lst = List(data)
    assert lst.strategy == strategy
    assert same(lst.to_list(), data)


def test_first_value_decides_and_demotion_is_sticky():
    lst = List()
    assert lst.strategy == "empty"
    lst.append(1)
    assert lst.strategy == "i64"
    lst.append(2)
    lst.append(2.5)
    assert lst.strategy == "object"
    assert same(lst.to_list(), [1, 2, 2.5])
    lst.pop()
    assert lst.strategy == "object"  # no automatic re-promotion
    assert type(lst[0]) is int


@pytest.mark.parametrize(
    "start, value",
    [([1, 2], "x"), ([1.0], 1), (["a"], 1), ([True], 1), ([1], True), ([1], 2**64), (["a"], "\udc80")],
)
def test_demotion_preserves_values_and_types(start, value):
    for mutate in (
        lambda l: l.append(value),
        lambda l: l.insert(0, value),
        lambda l: l.__setitem__(0, value),
        lambda l: l.extend([value]),
        lambda l: l.__setitem__(slice(0, 0), [value]),
    ):
        builtin, ours = list(start), List(start)
        mutate(builtin)
        mutate(ours)
        assert ours.strategy == "object"
        assert same(ours.to_list(), builtin)


def test_strategy_survives_same_type_operations():
    lst = List(range(10))
    lst[2:5] = [7, 8]
    lst.extend(range(3))
    lst += [1]
    lst *= 2
    del lst[::3]
    lst.sort()
    lst.reverse()
    assert lst.strategy == "i64"
    assert List(["b", "a"]).copy().strategy == "str"
    assert (List([1.5]) + List([2.5])).strategy == "f64"
    assert (List([1]) + [2.5]).strategy == "object"


def test_unicode_round_trip():
    words = ["", "ascii", "latin-1 é", "bmp ✓", "astral 😀𝄞", "mixed é✓😀", "nul\x00inside"]
    lst = List(words)
    assert lst.strategy == "str"
    assert lst.to_list() == words
    assert [type(w) for w in lst] == [str] * len(words)


# --------------------------------------------------------------------------------------
# Comparisons, repr, hashing, copying
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize("a", [[], [1], [1, 2], [1, 2.0], [2], ["a"], [1, "a"], [True], [2**70]])
@pytest.mark.parametrize("b", [[], [1], [1, 2], [1.0, 2.0], [0, 9], ["b"], [1, "b"], [1, 2, 3]])
def test_ordering_parity(a, b):
    import operator

    for op in (operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge):
        expected, expected_error = run(lambda _: op(a, b), None)
        for left, right in ((List(a), List(b)), (List(a), list(b)), (list(a), List(b))):
            actual, actual_error = run(lambda _: op(left, right), None)
            assert type(actual_error) is type(expected_error), (op, a, b)
            assert actual == expected, (op, left, right)


def test_comparison_with_foreign_types():
    lst = List([1])
    assert lst.__eq__("x") is NotImplemented
    assert lst.__lt__((1,)) is NotImplemented
    assert (lst == (1,)) is False
    assert lst != (1,)
    with pytest.raises(TypeError):
        lst < (1,)  # noqa: B015


def test_identity_and_nan_semantics():
    obj = object()
    nan = NAN
    assert nan in List([nan]) and List([nan]) == [nan]
    assert List([nan]).count(nan) == 1 and List([nan]).index(nan) == 0
    assert nan in List([nan, "mixed"])  # object storage: identity
    # Documented List semantics: NaN equals NaN whatever the strategy.
    assert float("nan") in List([float("nan")]) and float("nan") in List([1, "mixed", float("nan")])
    assert List([1, float("nan")]).count(float("nan")) == 1
    assert List([float("nan"), "x"]) == [float("nan"), "x"]
    assert obj in List([1, obj]) and List([obj]).index(obj) == 0
    lst = List([nan, 1.0])
    assert lst == lst and not (lst != lst)


def test_object_storage_keeps_identity():
    marker = 1.5
    lst = List([1, "x"])
    lst.append(marker)
    lst.insert(0, marker)
    lst[1] = marker
    lst.extend([marker])
    lst[5:5] = [marker]
    assert all(x is marker for i, x in enumerate(lst) if i in (0, 1, 3, 4, 5))
    big = 2**70
    assert List(iter([big, "s"]))[0] is big


def test_python_equality_fallback():
    class AlwaysEqual:
        def __init__(self):
            self.calls = 0

        def __eq__(self, other):
            self.calls += 1
            return True

        __hash__ = None

    probe = AlwaysEqual()
    assert probe in List([5, 6])
    assert List(["a", "b"]).count(probe) == 2
    assert List([1.5]).index(probe) == 0
    lst = List([3, 4])
    lst.remove(probe)
    assert lst.to_list() == [4]

    class Boom:
        def __eq__(self, other):
            raise RuntimeError("boom")

    with pytest.raises(RuntimeError):
        Boom() in List([1, "x"])  # noqa: B015


def test_numeric_cross_type_search():
    ints = List([1, 2, 3])
    assert 2.0 in ints and True in ints and ints.index(3.0) == 2
    assert 2**64 not in ints and 1.5 not in ints and "1" not in ints
    floats = List([0.5, 2.0**64])
    assert 2**64 in floats and floats.index(2**64) == 1
    assert 2**64 + 1 not in floats
    bools = List([False, True])
    assert 1 in bools and 0.0 in bools and bools.count(1) == 1


def test_repr():
    assert repr(List()) == "List([])"
    assert repr(List([1, 2])) == "List([1, 2])"
    assert repr(List([1.0, 1e16, 0.1, -0.0, NAN])) == "List([1.0, 1e+16, 0.1, -0.0, nan])"
    assert repr(List(["a'b", 'c"d', "\n"])) == "List(" + repr(["a'b", 'c"d', "\n"]) + ")"
    assert repr(List([True, None, (1,)])) == "List([True, None, (1,)])"
    assert repr(List([List([1])])) == "List([List([1])])"
    assert str(List([1])) == "List([1])"


def test_self_referencing_repr_and_eq():
    lst = List([1])
    lst.append(lst)
    assert repr(lst) == "List([1, [...]])"
    nested = [lst]
    assert repr(nested) == "[List([1, [...]])]"
    assert lst == lst


def test_unhashable():
    with pytest.raises(TypeError, match="unhashable type: 'List'"):
        hash(List())
    assert List.__hash__ is None
    with pytest.raises(TypeError):
        {List(): 1}


def test_copy_deepcopy_pickle():
    inner = [1, 2]
    lst = List([inner, "x"])
    shallow = copy.copy(lst)
    deep = copy.deepcopy(lst)
    assert shallow[0] is inner and deep[0] is not inner and deep == lst
    native = List([1.5, 2.5])
    assert copy.deepcopy(native) == native and copy.copy(native).strategy == "f64"
    cyclic = List()
    cyclic.append(cyclic)
    clone = copy.deepcopy(cyclic)
    assert clone[0] is clone
    for data in DATASETS.values():
        restored = pickle.loads(pickle.dumps(List(data)))
        assert isinstance(restored, List) and same(restored.to_list(), data)


def test_concatenation_and_repetition():
    assert (List([1]) + List(["a"])).to_list() == [1, "a"]
    assert ([0] + List([1])).to_list() == [0, 1] and isinstance([0] + List([1]), List)
    with pytest.raises(TypeError):
        List([1]) + (2,)
    with pytest.raises(TypeError):
        (2,) + List([1])
    with pytest.raises(OverflowError):
        List([1]) * 2**100
    with pytest.raises(MemoryError):
        List([1, 2]) * 2**62
    with pytest.raises(MemoryError):
        List([1, "x"]) * 2**62
    lst = List([1, 2])
    alias = lst
    lst += [3]
    lst *= 2
    assert alias is lst and lst.to_list() == [1, 2, 3, 1, 2, 3]
    with pytest.raises(TypeError):
        lst *= 1.5
    lst.extend(lst)
    assert len(lst) == 12


# --------------------------------------------------------------------------------------
# Native extras
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize("data", DATASETS.values(), ids=DATASETS.keys())
def test_sum_min_max_parity(data):
    lst = List(data)
    for ours, builtin in ((lst.sum, lambda: sum(data)), (lst.min, lambda: min(data)), (lst.max, lambda: max(data))):
        expected, expected_error = run(lambda _: builtin(), None)
        actual, actual_error = run(lambda _: ours(), None)
        assert type(actual_error) is type(expected_error)
        if expected_error is None:
            assert same([actual], [expected])


def test_sum_exactness():
    big = List([2**62, 2**62, 2**62])
    assert big.sum() == 3 * 2**62 and type(big.sum()) is int
    assert List([True, True]).sum() == 2
    assert List([]).sum() == 0
    assert List([0.1] * 10).sum() == sum([0.1] * 10)
    with pytest.raises(ValueError, match="min\\(\\) iterable argument is empty"):
        List().min()


def test_to_list_returns_builtins():
    for data in DATASETS.values():
        out = List(data).to_list()
        assert type(out) is list and same(out, data)


def test_sort_semantics():
    words = List(["b", "A", "c", "a"])
    words.sort(key=str.lower)
    assert words.to_list() == ["A", "a", "b", "c"]
    pairs = [(1, "x"), (0, "y"), (1, "a"), (0, "b")]
    lst = List(pairs)
    lst.sort(key=lambda p: p[0], reverse=True)
    assert lst.to_list() == sorted(pairs, key=lambda p: p[0], reverse=True)
    floats = List([2.5, -1.0, 0.0])
    floats.sort(key=None)
    assert floats.to_list() == [-1.0, 0.0, 2.5]
    with pytest.raises(TypeError):
        List([1, "a"]).sort()
    with pytest.raises(TypeError):
        List([1]).sort(lambda x: x)
    nan_list = List([3.0, NAN, 1.0])
    nan_list.sort()  # must not panic; NaN placement is unspecified for builtins too
    assert sorted(x for x in nan_list if not math.isnan(x)) == [1.0, 3.0]


def test_sort_detects_modification():
    lst = List([3, 1, 2])

    def key(x):
        lst.append(0)
        return x

    with pytest.raises(ValueError, match="list modified during sort"):
        lst.sort(key=key)


def test_sort_large_python_keys_matches_builtin():
    rng = random.Random(1)
    data = [(rng.randrange(50), rng.random()) for _ in range(3000)]
    lst = List(data)
    lst.sort(key=lambda t: t[0])
    assert lst.to_list() == sorted(data, key=lambda t: t[0])
    lst.sort(reverse=True)
    assert lst.to_list() == sorted(data, reverse=True)


def test_extend_keeps_items_before_iterator_error():
    def gen():
        yield 1
        yield 2
        raise KeyError("stop")

    builtin, ours = [0], List([0])
    with pytest.raises(KeyError):
        builtin.extend(gen())
    with pytest.raises(KeyError):
        ours.extend(gen())
    assert ours.to_list() == builtin


def test_arange():
    assert arange(5).to_list() == list(range(5))
    assert arange(2, 11, 3).to_list() == list(range(2, 11, 3))
    assert arange(10, 0, -4).to_list() == [10, 6, 2]
    assert arange(3, dtype=float).to_list() == [0.0, 1.0, 2.0]
    assert arange(0, 1, 0.25, dtype=float).to_list() == [0.0, 0.25, 0.5, 0.75]
    assert arange(5, 5).to_list() == []
    with pytest.raises(ValueError):
        arange(0, 10, 0)
    with pytest.raises(TypeError):
        arange(0.5)
    with pytest.raises(TypeError):
        arange(3, dtype="i8")


def test_sizeof_grows():
    small, large = List([1]), List(range(10000))
    assert sys.getsizeof(large) > sys.getsizeof(small) + 8 * 9000


# --------------------------------------------------------------------------------------
# Iterators
# --------------------------------------------------------------------------------------


def test_live_iterator_sees_appends_and_shrinks():
    for data in ([1, 2, 3], ["a", "b", "c"], [1, "x", None]):
        builtin, ours = list(data), List(data)
        seen_b, seen_o = [], []
        for x in builtin:
            seen_b.append(x)
            if len(builtin) < 6:
                builtin.append(x)
        for x in ours:
            seen_o.append(x)
            if len(ours) < 6:
                ours.append(x)
        assert seen_o == seen_b
        builtin, ours = list(data) * 2, List(data * 2)
        it_b, it_o = iter(builtin), iter(ours)
        assert next(it_b) == next(it_o)
        del builtin[1:], ours[1:]
        assert list(it_b) == list(it_o) == []


def test_exhausted_iterator_stays_exhausted():
    lst = List([1])
    it = iter(lst)
    assert list(it) == [1]
    lst.append(2)
    assert list(it) == []
    assert it.__length_hint__() == 0


def test_length_hints():
    lst = List([1, 2, 3])
    it = iter(lst)
    assert it.__length_hint__() == 3
    next(it)
    assert it.__length_hint__() == 2
    rev = reversed(lst)
    assert rev.__length_hint__() == 3
    next(rev)
    assert rev.__length_hint__() == 2
    assert iter(it) is it


def test_reversed_iterator_semantics():
    builtin, ours = [1, 2, 3, 4], List([1, 2, 3, 4])
    rb, ro = reversed(builtin), reversed(ours)
    assert next(rb) == next(ro) == 4
    builtin.append(5)
    ours.append(5)
    assert next(rb) == next(ro) == 3
    del builtin[:], ours[:]
    assert list(rb) == list(ro) == []


# --------------------------------------------------------------------------------------
# Buffer protocol
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "data, fmt, itemsize",
    [([1, -2], "q", 8), ([0.5], "d", 8), ([True, False], "?", 1)],
)
def test_buffer_export(data, fmt, itemsize):
    lst = List(data)
    view = memoryview(lst)
    assert view.format == fmt and view.itemsize == itemsize and view.readonly
    assert view.shape == (len(data),) and view.strides == (itemsize,)
    assert view.tolist() == data
    view.release()


def test_buffer_export_freezes_every_mutation():
    lst = List([3, 1, 2])
    view = memoryview(lst)
    mutations = [
        lambda: lst.append(1),
        lambda: lst.extend([1]),
        lambda: lst.insert(0, 1),
        lambda: lst.pop(),
        lambda: lst.remove(1),
        lambda: lst.clear(),
        lambda: lst.reverse(),
        lambda: lst.sort(),
        lambda: lst.sort(key=lambda x: x),
        lambda: lst.__setitem__(0, 5),
        lambda: lst.__setitem__(slice(0, 1), [5]),
        lambda: lst.__delitem__(0),
        lambda: lst.__delitem__(slice(None)),
        lambda: lst.__iadd__([1]),
        lambda: lst.__imul__(2),
    ]
    for mutate in mutations:
        with pytest.raises(BufferError):
            mutate()
    assert lst.to_list() == [3, 1, 2]
    assert lst[0] == 3 and len(lst) == 3 and lst.copy() == lst  # reads still work
    view.release()
    lst.append(4)
    assert lst.to_list() == [3, 1, 2, 4]


def test_buffer_requires_fixed_size_native_strategy():
    for data in ([], ["a"], [1, "a"]):
        with pytest.raises(BufferError):
            memoryview(List(data))
    view = memoryview(List([1]))
    with pytest.raises(TypeError):
        view[0] = 2  # read-only export


def test_numpy_view_shares_memory():
    np = pytest.importorskip("numpy")
    lst = List([1, 2, 3])
    arr = np.asarray(lst)
    assert arr.dtype == np.int64 and not arr.flags.writeable
    assert arr.tolist() == [1, 2, 3]
    with pytest.raises(BufferError):
        lst.append(4)
    del arr
    gc.collect()
    lst.append(4)


# --------------------------------------------------------------------------------------
# GC and threads
# --------------------------------------------------------------------------------------


def test_reference_cycles_are_collected():
    collected = []

    class Marker:
        def __del__(self):
            collected.append(True)

    lst = List([Marker()])
    lst.append(lst)
    it = iter(lst)
    lst.append(it)
    del lst, it
    gc.collect()
    assert collected == [True]


def test_del_runs_outside_the_lock():
    lst = List()

    class Reentrant:
        def __del__(self):
            lst.append("finalized")  # would deadlock if dropped under the lock

    lst.append(Reentrant())
    lst.pop()
    lst.append(Reentrant())
    lst[0] = 1
    lst.append(Reentrant())
    lst.clear()
    lst.append(Reentrant())
    del lst[:]
    assert lst.to_list().count("finalized") >= 1


def test_mutation_from_eq_does_not_crash():
    lst = List([1, "a", 2])

    class Mutator:
        def __eq__(self, other):
            lst.clear()
            return False

        __hash__ = None

    assert Mutator() not in lst
    assert lst.to_list() == []
    lst.extend([1, "a"])
    with pytest.raises(ValueError):
        lst.remove(Mutator())


free_threaded = pytest.mark.skipif(
    getattr(sys, "_is_gil_enabled", lambda: True)(), reason="needs a free-threaded build with the GIL disabled"
)


@free_threaded
def test_threads_smoke():
    lst = List()
    shared = List(range(1000))
    errors = []
    barrier = threading.Barrier(8)

    def worker(k):
        try:
            barrier.wait()
            for i in range(2000):
                lst.append(i)
                if i % 7 == 0:
                    lst.append("s")
                _ = shared[i % 1000]
                _ = 999 in shared
                if i % 100 == 0:
                    shared.sort(reverse=bool(k % 2))
                    _ = shared == shared.copy()
                    _ = list(shared)[:3]
                    _ = repr(lst)[:10]
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=worker, args=(k,)) for k in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors
    assert len(lst) == 8 * (2000 + len(range(0, 2000, 7)))
    assert sorted(shared.to_list()) == list(range(1000))


@free_threaded
def test_threads_iterate_while_mutating():
    lst = List(range(100))
    stop = threading.Event()
    errors = []

    def mutate():
        try:
            while not stop.is_set():
                lst.append(1)
                lst.pop(0)
                lst.insert(5, "x")
                lst.remove("x")
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    def iterate():
        try:
            for _ in range(200):
                for _x in lst:
                    pass
                _ = lst[::2]
                _ = lst.count(1)
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    workers = [threading.Thread(target=mutate) for _ in range(2)] + [threading.Thread(target=iterate) for _ in range(4)]
    for t in workers[2:]:
        t.start()
    for t in workers[:2]:
        t.start()
    for t in workers[2:]:
        t.join()
    stop.set()
    for t in workers[:2]:
        t.join()
    assert not errors
    assert len(lst) == 100


# --------------------------------------------------------------------------------------
# Typed coercion (Schema::coerce and the bulk ingest fast paths)
# --------------------------------------------------------------------------------------


def test_typed_coercion_and_errors():
    assert List[int]([1, True, -3]).to_list() == [1, 1, -3]
    assert all(type(x) is int for x in List[int]([True]))
    big = List[int]([1, 2**70])
    assert big.strategy == "object" and big.to_list() == [1, 2**70]
    assert List[float]([1, 2.5, True]).to_list() == [1.0, 2.5, 1.0]
    assert List[float](range(3)).strategy == "f64"
    assert List[str](("a", "ü")).strategy == "str"
    with pytest.raises(TypeError, match=r"List\[int\] expects int, got float at index 1"):
        List[int]([1, 2.5])
    with pytest.raises(TypeError, match=r"List\[bool\] expects bool, got int at index 0"):
        List[bool]([1])
    with pytest.raises(OverflowError):
        List[float]([2**2000])
    with pytest.raises(ValueError):
        List[str](["\udc80"])
    typed = List[int]([1])
    with pytest.raises(TypeError, match=r"List\[int\] expects int, got str$"):
        typed.append("x")
    with pytest.raises(TypeError):
        typed.extend([2, "y"])
    assert typed.to_list() == [1]


def test_typed_optional_union_nested_and_classes():
    assert List[int | None]([1, None, True]).to_list() == [1, None, 1]
    with pytest.raises(TypeError, match=r"expects int \| None, got str"):
        List[int | None](["x"])
    inner = List[int]([1, 2])
    nested = List[List[int]]([inner, [3], range(2)])
    assert nested[0] is inner and nested[1].to_list() == [3]
    assert nested[2].schema_type is List[int]
    with pytest.raises(TypeError, match=r"expects List\[int\], got int"):
        List[List[int]]([5])

    class Animal:
        pass

    class Dog(Animal):
        pass

    dog = Dog()
    assert List[Animal]([dog])[0] is dog
    with pytest.raises(TypeError, match=r"expects \S*Animal, got int"):
        List[Animal]([1])


def test_typed_conversion_between_lists_and_buffers():
    assert List[float](List([1, 2])).to_list() == [1.0, 2.0]
    assert List[int](List([True, False])).to_list() == [1, 0]
    assert List[float](array.array("i", [1, 2])).to_list() == [1.0, 2.0]
    with pytest.raises(TypeError):
        List[int](array.array("d", [1.0]))
    with pytest.raises(TypeError):
        List[int](List([1, "x"]))
    typed = List[int]([3, 1])
    assert typed[::-1].schema_type is List[int] and typed.copy().schema_type is List[int]
    assert (typed + [1]).schema_type is List and (typed + typed).schema_type is List[int]
    assert repr(typed) == "List[int]([3, 1])"
    assert pickle.loads(pickle.dumps(List[int | None]([1, None]))).schema_type is List[int | None]
