"""Interoperability: `arange` (the Rust -> Python `List::from_column` path, ARCHITECTURE
§5), pickling, `copy`/`deepcopy`, ABC registration, the public module surface, and use of
Rustique containers with stdlib functions that expect sequences/mappings."""

from __future__ import annotations

import bisect
import collections
import collections.abc as cabc
import copy
import gc
import pickle
import random
import sys
import weakref
from typing import Any

import pytest
from _support import assert_same_items, import_rustique_module_level

rs = import_rustique_module_level()
List, Dict, Set = rs.List, rs.Dict, rs.Set

SCALARS = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64"]


# ======================================================================================
# Module surface
# ======================================================================================


def test_public_api() -> None:
    expected = {"List", "Dict", "Set", "ListType", "DictType", "SetType", "X", "Expr", "arange", "__version__", *SCALARS}
    assert set(rs.__all__) == expected
    for name in expected:
        assert hasattr(rs, name), name
    assert isinstance(rs.__version__, str) and rs.__version__.count(".") >= 2
    assert isinstance(rs.X, rs.Expr)
    for cls in (List, Dict, Set, rs.Expr):
        assert cls.__module__ == "rustique"
    for name in SCALARS:
        assert getattr(rs, name).__module__ == "rustique"
        assert getattr(rs, name).__name__ == name
    assert type(List[int]) is rs.ListType
    assert type(Dict[str, int]) is rs.DictType
    assert type(Set[int]) is rs.SetType


def test_abc_registration() -> None:
    lst, d, s = List([1]), Dict({"a": 1}), Set([1])
    for abc in (cabc.MutableSequence, cabc.Sequence, cabc.Reversible, cabc.Collection, cabc.Iterable, cabc.Sized, cabc.Container):
        assert isinstance(lst, abc), abc
    for abc in (cabc.MutableMapping, cabc.Mapping, cabc.Collection):
        assert isinstance(d, abc), abc
    for abc in (cabc.MutableSet, cabc.Set, cabc.Collection):
        assert isinstance(s, abc), abc
    for obj in (lst, d, s):
        assert not isinstance(obj, cabc.Hashable)
    assert not isinstance(lst, cabc.Mapping)
    assert issubclass(List, cabc.MutableSequence)


def test_not_subclassable_as_builtins() -> None:
    assert not isinstance(List(), list)
    assert not isinstance(Dict(), dict)
    assert not isinstance(Set(), set)


# ======================================================================================
# arange: the Rust -> Python zero-conversion path
# ======================================================================================


@pytest.mark.parametrize(
    "args",
    [(5,), (0,), (-3,), (2, 10), (2, 10, 3), (10, 2, -3), (10, 2), (-5, 5, 2), (0, 10**6, 7), (5, 5), (2**62, 2**62 + 3)],
    ids=repr,
)
def test_arange_matches_range(args: tuple[int, ...]) -> None:
    lst = rs.arange(*args)
    expected = list(range(*args))
    assert type(lst) is List
    assert len(lst) == len(expected)
    assert lst == expected
    assert lst.to_list() == expected
    assert lst.schema_type is List[int]
    assert lst.strategy == "i64"


def test_arange_keywords_and_errors() -> None:
    assert rs.arange(0, 6, step=2) == [0, 2, 4]
    assert rs.arange(3, stop=6) == [3, 4, 5]
    assert rs.arange(start=1, stop=3) == [1, 2]
    with pytest.raises(ValueError):
        rs.arange(0, 10, 0)
    with pytest.raises(TypeError):
        rs.arange("a")
    with pytest.raises(TypeError):
        rs.arange(1.5)


def test_arange_dtypes() -> None:
    f = rs.arange(4, dtype=float)
    assert f.schema_type is List[float] and f.strategy == "f64"
    assert_same_items(f, [0.0, 1.0, 2.0, 3.0])
    for name in ("i8", "i32", "u8", "u64", "f32"):
        t = getattr(rs, name)
        lst = rs.arange(5, dtype=t)
        assert lst.strategy == name
        assert lst.schema_type is List[t]
        assert lst == [0, 1, 2, 3, 4]
    with pytest.raises(OverflowError):
        rs.arange(300, dtype=rs.u8)
    with pytest.raises(OverflowError):
        rs.arange(-1, 2, dtype=rs.u32)
    with pytest.raises(TypeError):
        rs.arange(3, dtype=str)


def test_arange_result_is_an_ordinary_list() -> None:
    lst = rs.arange(5)
    lst.append(5)
    lst[0] = 10
    lst.insert(0, -1)
    assert lst == [-1, 10, 1, 2, 3, 4, 5]
    with pytest.raises(TypeError):
        lst.append("x")  # typed List[int]
    lst.append(2**70)
    assert lst.strategy == "object" and lst[-1] == 2**70


def test_arange_large_is_native_and_zero_copy() -> None:
    n = 2_000_000
    lst = rs.arange(n)
    assert lst.sum() == n * (n - 1) // 2
    assert lst[n - 1] == n - 1
    with memoryview(lst) as mv:
        assert mv.format == "q" and mv.shape == (n,)
    np = pytest.importorskip("numpy")
    a = np.asarray(lst)
    assert a.dtype == np.int64 and int(a[-1]) == n - 1
    assert np.asarray(lst).ctypes.data == a.ctypes.data


# ======================================================================================
# pickle
# ======================================================================================

PICKLE_CASES: list[Any] = [
    lambda: List(),
    lambda: List([1, 2, 3]),
    lambda: List([0.5, -0.0, float("inf")]),
    lambda: List([True, False]),
    lambda: List(["a", "é", "😀"]),
    lambda: List([1, "a", None, 2.5, [1, 2], (3,), 2**100]),
    lambda: List[int]([1, 2**70]),
    lambda: List[float]([1.5]),
    lambda: List[rs.u8]([1, 255]),
    lambda: List[rs.f32]([0.5]),
    lambda: List[int | None]([1, None]),
    lambda: List[List[int]]([[1], [2, 3]]),
    lambda: List[str](["x"]),
    lambda: rs.arange(1000),
    lambda: Dict(),
    lambda: Dict({"a": 1, "b": [2]}),
    lambda: Dict({1: "a", 2.5: None, (1,): b"x"}),
    lambda: Dict[str, int]({"a": 1}),
    lambda: Dict[int, List[float]]({1: [1.0]}),
    lambda: Set(),
    lambda: Set([1, 2, 3]),
    lambda: Set(["a", None, 2.5]),
    lambda: Set[int]([1, 2]),
    lambda: Set[str](["a"]),
]


@pytest.mark.parametrize("make", PICKLE_CASES, ids=[str(i) for i in range(len(PICKLE_CASES))])
@pytest.mark.parametrize("protocol", sorted({0, 2, pickle.DEFAULT_PROTOCOL, pickle.HIGHEST_PROTOCOL}))
def test_pickle_round_trip(make: Any, protocol: int) -> None:
    obj = make()
    data = pickle.dumps(obj, protocol=protocol)
    back = pickle.loads(data)
    assert type(back) is type(obj)
    assert back is not obj
    assert back == obj
    assert back.schema_type is obj.schema_type
    if type(obj) is List:
        assert_same_items(back, list(obj))
        assert back.strategy == obj.strategy or obj.strategy == "empty"
    elif type(obj) is Dict:
        assert_same_items(list(back.items()), list(obj.items()))
    back_again = pickle.loads(pickle.dumps(back, protocol=protocol))
    assert back_again == obj


@pytest.mark.parametrize(
    "alias",
    [lambda: List[int], lambda: List[rs.u8], lambda: List[int | None], lambda: Dict[str, int], lambda: Set[float], lambda: List[List[str]]],
    ids=["List[int]", "List[u8]", "List[int|None]", "Dict[str,int]", "Set[float]", "List[List[str]]"],
)
def test_pickle_aliases_preserve_identity(alias: Any) -> None:
    a = alias()
    assert pickle.loads(pickle.dumps(a)) is a


@pytest.mark.parametrize("name", SCALARS)
def test_pickle_scalars(name: str) -> None:
    t = getattr(rs, name)
    v = t(7)
    back = pickle.loads(pickle.dumps(v))
    assert type(back) is t
    assert back == v
    assert pickle.loads(pickle.dumps(t)) is t


def test_pickle_shared_and_recursive_references() -> None:
    inner = [1, 2]
    lst = List([inner, inner])
    back = pickle.loads(pickle.dumps(lst))
    assert back[0] is back[1]
    rec = List([1])
    rec.append(rec)
    back = pickle.loads(pickle.dumps(rec))
    assert back[1] is back


# ======================================================================================
# copy / deepcopy
# ======================================================================================


@pytest.mark.parametrize("make", PICKLE_CASES, ids=[str(i) for i in range(len(PICKLE_CASES))])
def test_copy_and_deepcopy(make: Any) -> None:
    obj = make()
    for c in (copy.copy(obj), copy.deepcopy(obj), obj.copy()):
        assert type(c) is type(obj)
        assert c is not obj
        assert c == obj
        assert c.schema_type is obj.schema_type


def test_shallow_vs_deep() -> None:
    inner = [1, [2]]
    lst = List([inner, "x"])
    shallow = copy.copy(lst)
    deep = copy.deepcopy(lst)
    assert shallow[0] is inner
    assert deep[0] is not inner and deep[0] == inner
    assert deep[0][1] is not inner[1]

    d = Dict({"k": inner})
    assert copy.copy(d)["k"] is inner
    assert copy.deepcopy(d)["k"] is not inner

    nested = List[List[int]]([[1, 2]])
    deep_nested = copy.deepcopy(nested)
    assert deep_nested[0] is not nested[0]
    assert deep_nested[0].schema_type is List[int]
    shallow_nested = copy.copy(nested)
    assert shallow_nested[0] is nested[0]


def test_deepcopy_memo_and_recursion() -> None:
    rec = List([1])
    rec.append(rec)
    d = copy.deepcopy(rec)
    assert d[1] is d
    shared = Dict({"a": 1})
    holder = List([shared, shared])
    dh = copy.deepcopy(holder)
    assert dh[0] is dh[1] and dh[0] is not shared


def test_copies_are_independent() -> None:
    for obj, mutate in (
        (List([1, 2]), lambda x: x.append(3)),
        (Dict({"a": 1}), lambda x: x.__setitem__("b", 2)),
        (Set([1]), lambda x: x.add(2)),
    ):
        before = obj.copy()
        for c in (copy.copy(obj), copy.deepcopy(obj)):
            mutate(c)
            assert obj == before


# ======================================================================================
# stdlib interop
# ======================================================================================


def test_stdlib_consumers_of_sequences() -> None:
    lst = List([3, 1, 2])
    assert sorted(lst) == [1, 2, 3]
    assert sum(lst) == 6 and min(lst) == 1 and max(lst) == 3
    assert list(reversed(lst)) == [2, 1, 3]
    assert list(enumerate(lst)) == [(0, 3), (1, 1), (2, 2)]
    assert collections.Counter(List(["a", "b", "a"])) == {"a": 2, "b": 1}
    assert ",".join(List(["a", "b"])) == "a,b"
    assert tuple(lst) == (3, 1, 2)
    assert set(lst) == {1, 2, 3}

    s = List([1, 3, 5])
    bisect.insort(s, 4)
    assert s == [1, 3, 4, 5]
    assert bisect.bisect_left(s, 3) == 1

    shuffled = List(range(100))
    random.Random(0).shuffle(shuffled)
    assert sorted(shuffled) == list(range(100))
    assert shuffled != list(range(100))
    assert random.Random(0).choice(List(["only"])) == "only"


def test_stdlib_consumers_of_mappings_and_sets() -> None:
    d = Dict({"a": 1, "b": 2})
    assert dict(d) == {"a": 1, "b": 2}
    assert dict(**d) == {"a": 1, "b": 2}
    assert sorted(d.items()) == [("a", 1), ("b", 2)]
    assert collections.ChainMap(d, {"c": 3})["c"] == 3
    assert "{a}-{b}".format_map(d) == "1-2"
    s = Set([1, 2])
    assert frozenset(s) == {1, 2}
    assert s | {3} == {1, 2, 3}
    assert {3} | s == {1, 2, 3}


def test_containers_as_elements_and_keys() -> None:
    inner = List([1])
    outer = List([inner])
    assert outer[0] is inner
    with pytest.raises(TypeError):
        {inner: 1}  # noqa: B018 -- unhashable
    with pytest.raises(TypeError):
        Set([List([1])])
    with pytest.raises(TypeError):
        Dict({List([1]): 1})


def test_garbage_collection_of_cycles() -> None:
    """Containers holding themselves (through object storage) are collected."""

    class Marker:
        pass

    lst = List()
    lst.append(lst)
    m = Marker()
    lst.append(m)
    ref = weakref.ref(m)
    d = Dict()
    d["self"] = d
    s_holder = List([d])
    del lst, m, d, s_holder
    gc.collect()
    assert ref() is None


def test_sys_getsizeof_does_not_crash() -> None:
    for obj in (List(range(100)), Dict({1: 2}), Set([1]), List[rs.u8]([1])):
        assert sys.getsizeof(obj) > 0
