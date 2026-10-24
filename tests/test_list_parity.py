"""`List` parity with the builtin `list` (ARCHITECTURE §4.1).

Part 1 ports the historical 1292-line parity suite (`engine3:rustique/tests/test_list.py`)
to the new API. Part 2 covers what that suite never did: sort, `+`/`+=`/`*=`, ordering,
repr, `reversed`, live iterators, NaN/identity, numeric cross-type equality, the native
kernels against their builtin equivalents, and exception-type parity.

Every test is parametrized over an implementation fixture (see `_support.py`):

* ``impl``  -- builtin ``list``, untyped ``List`` and typed ``List[T]`` (``T`` inferred
  from homogeneous data). Used where the data stays homogeneous.
* ``uimpl`` -- builtin ``list`` and untyped ``List``. Used for heterogeneous data.

Running with ``RUSTIQUE_IMPLS=builtin`` proves every expectation against CPython itself.
"""

from __future__ import annotations

import copy
import math
import operator
import random
from typing import Any

import pytest
from _support import ListImpl, assert_same_items, multiset, same

NAN = float("nan")


class CustomObj:
    def __init__(self, value: int) -> None:
        self.value = value

    def __eq__(self, other: object) -> bool:
        return isinstance(other, CustomObj) and self.value == other.value

    __hash__ = None  # type: ignore[assignment]

    def __repr__(self) -> str:
        return f"CustomObj({self.value})"


# ======================================================================================
# Part 1 -- the historical suite, ported
# ======================================================================================


def test_constructor_empty(impl: ListImpl) -> None:
    lst = impl.new([])
    assert len(lst) == 0
    assert lst == []
    impl.check(lst, [])
    # No-argument constructor.
    empty = impl.cls() if impl.kind != "typed" else impl.rs.List[int]()
    assert len(empty) == 0 and empty == []


def test_constructor_with_list(impl: ListImpl) -> None:
    data = [1, 2, 3]
    lst = impl.new(data)
    assert len(lst) == len(data)
    assert lst == data
    assert isinstance(lst, impl.cls)
    impl.check(lst, data)

    data_large = list(range(1000))
    lst_large = impl.new(data_large)
    assert len(lst_large) == len(data_large)
    assert lst_large[:5] == data_large[:5]
    impl.check(lst_large, data_large)

    impl.check(impl.new(iter([1, 2, 3])), [1, 2, 3])
    impl.check(impl.new((1, 2, 3)), [1, 2, 3])
    impl.check(impl.new(b"abc"), list(b"abc"))
    impl.check(impl.new(range(5, 50, 7)), list(range(5, 50, 7)))
    impl.check(impl.new(range(10, -10, -3)), list(range(10, -10, -3)))
    impl.check(impl.new(range(0)), [])
    impl.check(impl.new(x * x for x in range(6)), [0, 1, 4, 9, 16, 25])
    impl.check(impl.new("héllo"), list("héllo"))
    impl.check(impl.new({"a": 1, "b": 2}), ["a", "b"])
    impl.check(impl.new([1.5, -0.0, 2.25]), [1.5, -0.0, 2.25])
    impl.check(impl.new([True, False, True]), [True, False, True])

    lst_set = impl.new({1, 2, 3})
    assert sorted(lst_set) == [1, 2, 3]

    # From an existing container of the same implementation (copy, independent).
    src = impl.new([4, 5, 6])
    dup = impl.cls(src) if impl.kind != "typed" else impl.rs.List[int](src)
    impl.check(dup, [4, 5, 6])
    dup.append(7)
    impl.check(src, [4, 5, 6])


def test_constructor_heterogeneous(uimpl: ListImpl) -> None:
    impl = uimpl
    data_nested = [[1, 2], [3, 4]]
    lst_nested = impl.new(data_nested)
    assert len(lst_nested) == len(data_nested)
    assert lst_nested == data_nested
    impl.check(lst_nested, data_nested)
    # Object storage keeps the very same objects.
    assert lst_nested[0] is data_nested[0]

    data_mixed = ["a", 42, None, 3.14]
    lst_mixed = impl.new(data_mixed)
    assert len(lst_mixed) == len(data_mixed)
    impl.check(lst_mixed, data_mixed)

    impl.check(impl.new([1, 2.0, True, "x", None, b"y", (1,)]), [1, 2.0, True, "x", None, b"y", (1,)])
    impl.check(impl.new([2**63, -(2**63) - 1, 2**200]), [2**63, -(2**63) - 1, 2**200])
    impl.check(impl.new([1, 2**64]), [1, 2**64])


def test_constructor_iterator_exhaustion(impl: ListImpl) -> None:
    data = iter([1, 2, 3])
    lst = impl.new(data)
    assert lst == [1, 2, 3]
    assert list(data) == []


def test_constructor_rejects_bad_arguments(uimpl: ListImpl) -> None:
    cls = uimpl.cls
    with pytest.raises(TypeError):
        cls(1)
    with pytest.raises(TypeError):
        cls(None)
    with pytest.raises(TypeError):
        cls([1], [2])
    with pytest.raises(TypeError):
        cls(iterable=[1])

    def boom() -> Any:
        yield 1
        raise KeyError("boom")

    with pytest.raises(KeyError):
        cls(boom())


def test_len(impl: ListImpl) -> None:
    assert len(impl.new([])) == 0
    assert len(impl.new([1])) == 1
    assert len(impl.new(list(range(1000)))) == 1000
    assert len(impl.new(["", "a"])) == 2


def test_len_heterogeneous(uimpl: ListImpl) -> None:
    assert len(uimpl.new([None, "", [], {}])) == 4
    assert len(uimpl.new([1, 2, [3, 4], 5])) == 4


def test_bool(impl: ListImpl) -> None:
    assert not impl.new([])
    assert impl.new([0])
    assert bool(impl.new([False])) is True
    assert bool(impl.new([])) is False


def test_getitem_index(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50]
    lst = impl.new(data)
    for i in range(-len(data), len(data)):
        assert same(lst[i], data[i])
    with pytest.raises(IndexError):
        _ = lst[len(data)]
    with pytest.raises(IndexError):
        _ = lst[-(len(data) + 1)]
    with pytest.raises(IndexError):
        _ = lst[2**100]
    with pytest.raises(IndexError):
        _ = lst[-(2**100)]

    lst_single = impl.new([100])
    assert lst_single[0] == 100
    assert lst_single[-1] == 100

    with pytest.raises(IndexError):
        _ = impl.new([])[0]

    # bool and __index__ objects are valid indices.
    assert lst[True] == 20
    assert lst[Idx(3)] == 40


def test_getitem_index_nested(uimpl: ListImpl) -> None:
    nested_data = [1, [2, 3], [4, [5, 6]]]
    lst_nested = uimpl.new(nested_data)
    assert lst_nested[1] == nested_data[1]
    assert lst_nested[2] == nested_data[2]
    assert lst_nested[2][1] == nested_data[2][1]
    assert lst_nested[2][1][1] == nested_data[2][1][1]
    assert lst_nested[1] is nested_data[1]


def test_getitem_bad_index_types(impl: ListImpl) -> None:
    lst = impl.new([1, 2, 3])
    for bad in ("a", 1.0, None, (1,), [1]):
        with pytest.raises(TypeError):
            _ = lst[bad]  # type: ignore[index]


class Idx:
    """An object usable as an index via ``__index__``."""

    def __init__(self, n: int) -> None:
        self.n = n

    def __index__(self) -> int:
        return self.n


SLICES = [
    slice(None),
    slice(2, 5),
    slice(None, 4),
    slice(3, None),
    slice(1, 6, 2),
    slice(None, None, -1),
    slice(None, None, 2),
    slice(1, None, 2),
    slice(-6, -2),
    slice(-1, -6, -2),
    slice(None, 100),
    slice(-100, None),
    slice(100, 200),
    slice(5, 1),
    slice(1, 5, -1),
    slice(-100, 100, 3),
    slice(100, -100, -3),
    slice(2**100, -(2**100), -1),
    slice(-(2**100), 2**100, 2**100),
    slice(Idx(1), Idx(7), Idx(3)),
    slice(True, None, True),
]


@pytest.mark.parametrize("s", SLICES, ids=repr)
def test_getitem_slice(impl: ListImpl, s: slice) -> None:
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = impl.new(data)
    result = lst[s]
    assert type(result) is impl.cls
    impl.check(result, data[s])
    if not impl.is_builtin:
        assert result.schema_type is lst.schema_type
    # Slicing copies: the source is unchanged and the result independent.
    result.append(99)
    impl.check(lst, data)


def test_getitem_slice_edge_cases(impl: ListImpl) -> None:
    for data in ([], [100]):
        lst = impl.new(data, elem=int)
        for s in (slice(None), slice(None, 5), slice(-5, None), slice(None, None, -1), slice(None, 1)):
            impl.check(lst[s], data[s])
    lst = impl.new([1, 2, 3])
    with pytest.raises(ValueError):
        _ = lst[::0]
    with pytest.raises(TypeError):
        _ = lst["a":]  # type: ignore[misc]
    with pytest.raises(TypeError):
        _ = lst[:1.5]  # type: ignore[misc]
    assert lst[:] is not lst


def test_getitem_slice_nested(uimpl: ListImpl) -> None:
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    uimpl.check(lst_nested[1:], nested_data[1:])
    uimpl.check(lst_nested[:2], nested_data[:2])
    uimpl.check(lst_nested[::2], nested_data[::2])
    # Slices are shallow: the inner objects are shared.
    assert lst_nested[1:][0] is nested_data[1]


def test_setitem_index(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50]
    lst = impl.new(data)
    for i, v in ((0, 100), (2, 300), (-1, 500), (-3, 250)):
        lst[i] = v
        data[i] = v
        impl.check(lst, data)
    with pytest.raises(IndexError):
        lst[5] = 600
    with pytest.raises(IndexError):
        lst[-6] = 700
    with pytest.raises(IndexError):
        lst[2**100] = 1
    with pytest.raises(TypeError):
        lst["a"] = 1  # type: ignore[index]
    for i in range(len(data)):
        lst[i] = i * 10
        data[i] = i * 10
    impl.check(lst, data)
    lst[True] = 7
    data[True] = 7
    impl.check(lst, data)

    lst_single = impl.new([100])
    lst_single[0] = 200
    impl.check(lst_single, [200])

    lst_empty = impl.new([], elem=int)
    with pytest.raises(IndexError):
        lst_empty[0] = 1


def test_setitem_index_heterogeneous(uimpl: ListImpl) -> None:
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    lst_nested[1] = [7, 8]
    nested_data[1] = [7, 8]
    assert lst_nested == nested_data
    lst_nested[2][1] = 9
    assert lst_nested[2] == [5, 9]
    assert lst_nested == nested_data

    # Writing a value of another type demotes native storage without losing information.
    data = [1, 2, 3]
    lst = uimpl.new(data)
    for i, v in ((0, "x"), (1, None), (2, 2.5), (0, 2**70), (1, True)):
        lst[i] = v
        data[i] = v
        uimpl.check(lst, data)


def test_setitem_slice(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = impl.new(data)

    def both(s: slice, values: Any) -> None:
        if isinstance(values, (list, tuple)):
            lst[s] = values
            data[s] = values
        else:  # a one-shot iterator: feed `lst` the iterator, `data` its items
            items = list(values)
            lst[s] = iter(items)
            data[s] = items
        impl.check(lst, data)

    both(slice(2, 5), [300, 400, 500])
    both(slice(None, 3), [100, 200, 300])
    both(slice(4, None), [600, 700, 800])
    both(slice(None, None, 2), [1, 2, 3, 4])

    data[:] = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = impl.new(data)
    both(slice(1, None, 2), [9, 8, 7, 6])
    both(slice(-3, None), [900, 1000, 1100])
    both(slice(None, -5), [400, 500, 600])
    both(slice(1, 3), [55])
    both(slice(1, 2), [11, 22, 33])
    both(slice(None), [1, 2, 3, 4, 5])
    both(slice(100, None), [999])
    both(slice(-100, None), [777, 888])
    both(slice(2, 4), [])
    both(slice(1, 1), [12, 13])
    both(slice(None, None, -1), list(range(len(data))))
    both(slice(0, 0), (x for x in [41, 42]))
    both(slice(1, 3), (7, 8, 9))
    both(slice(5, 1), [3])  # empty slice with stop < start inserts at start
    both(slice(Idx(0), Idx(1)), [0])

    lst_empty = impl.new([], elem=int)
    lst_empty[:] = [1, 2, 3]
    impl.check(lst_empty, [1, 2, 3])

    with pytest.raises(ValueError):
        lst[1:4:2] = [100, 200, 300]
    with pytest.raises(ValueError):
        lst[::2] = []
    with pytest.raises(TypeError):
        lst[0:2] = 5
    with pytest.raises(TypeError):
        lst[::2] = 5
    with pytest.raises(ValueError):
        lst[::0] = []
    impl.check(lst, data)


def test_setitem_slice_from_self(impl: ListImpl) -> None:
    for s in (slice(None), slice(1, 2), slice(1, 1), slice(None, None, -1), slice(0, 0)):
        data = [1, 2, 3]
        lst = impl.new(data)
        lst[s] = lst
        data[s] = data
        impl.check(lst, data)


def test_setitem_slice_extended(impl: ListImpl) -> None:
    for s in (slice(None, None, 2), slice(1, None, 3), slice(None, None, -1), slice(-1, 0, -2), slice(7, 0, -3)):
        data = list(range(10))
        lst = impl.new(data)
        n = len(data[s])
        values = [100 + i for i in range(n)]
        lst[s] = iter(values)
        data[s] = values
        impl.check(lst, data)
        with pytest.raises(ValueError):
            lst[s] = values + [1]
        impl.check(lst, data)


def test_setitem_slice_heterogeneous(uimpl: ListImpl) -> None:
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    lst_nested[:2] = [[7, 8], [9, 10]]
    nested_data[:2] = [[7, 8], [9, 10]]
    uimpl.check(lst_nested, nested_data)

    data = [1, 2, 3, 4]
    lst = uimpl.new(data)
    lst[1:3] = "ab"
    data[1:3] = "ab"
    uimpl.check(lst, data)
    lst[::2] = [None, 2**80]
    data[::2] = [None, 2**80]
    uimpl.check(lst, data)


def test_delitem_index(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50]
    lst = impl.new(data)
    for i in (0, 2, -1, -1):
        del lst[i]
        del data[i]
        impl.check(lst, data)
    with pytest.raises(IndexError):
        del lst[10]
    with pytest.raises(IndexError):
        del lst[-10]
    with pytest.raises(IndexError):
        del lst[2**100]
    with pytest.raises(TypeError):
        del lst["a"]  # type: ignore[arg-type]

    lst = impl.new([10])
    del lst[0]
    impl.check(lst, [])
    assert len(lst) == 0
    with pytest.raises(IndexError):
        del lst[0]


def test_delitem_index_nested(uimpl: ListImpl) -> None:
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    del lst_nested[1]
    del nested_data[1]
    assert lst_nested == nested_data
    del lst_nested[1][0]
    assert lst_nested == [[1, 2], [6]]


DEL_SLICES = [
    slice(2, 5),
    slice(None, 2),
    slice(-2, None),
    slice(None, None, 2),
    slice(1, 2),
    slice(None, 0),
    slice(None),
    slice(None, None, -2),
    slice(1, None, 2),
    slice(-1, 0, -3),
    slice(100, 200),
    slice(2**100, None, -1),
    slice(None, None, 2**100),
]


@pytest.mark.parametrize("s", DEL_SLICES, ids=repr)
def test_delitem_slice(impl: ListImpl, s: slice) -> None:
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = impl.new(data)
    del lst[s]
    del data[s]
    impl.check(lst, data)


def test_delitem_slice_sequence(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = impl.new(data)
    for s in (slice(2, 5), slice(None, 2), slice(-2, None), slice(None, None, 2), slice(1, 2), slice(None, 0), slice(None)):
        del lst[s]
        del data[s]
        impl.check(lst, data)
    assert len(lst) == 0
    del lst[:]
    impl.check(lst, [])
    with pytest.raises(ValueError):
        del lst[::0]


def test_append(impl: ListImpl) -> None:
    data = [10, 20, 30]
    lst = impl.new(data)
    for v in (40, 40, -5, 0):
        assert lst.append(v) is None
        data.append(v)
        impl.check(lst, data)
    assert lst[-1] == data[-1]
    assert lst[-2] == data[-2]

    lst_empty = impl.new([], elem=int)
    lst_empty.append(100)
    impl.check(lst_empty, [100])

    for v in (70, 80, 90):
        lst.append(v)
    assert len(lst) == len(data) + 3


def test_append_heterogeneous(uimpl: ListImpl) -> None:
    data = [10, 20, 30]
    lst = uimpl.new(data)
    for v in (40, "hello", None, [50, 60], {"key": "value"}, 2.5, True, 2**64, b"x"):
        lst.append(v)
        data.append(v)
        uimpl.check(lst, data)

    empty_data: list[Any] = []
    lst_empty = uimpl.new(empty_data)
    for v in (100, [200, 300]):
        lst_empty.append(v)
        empty_data.append(v)
    uimpl.check(lst_empty, empty_data)


@pytest.mark.parametrize(
    "first,second",
    [
        ([1, 2], [3.5]),
        ([1, 2], [True]),
        ([True], [1]),
        ([1.5], [2]),
        (["a"], [1]),
        ([1], ["a"]),
        ([1], [2**63]),
        ([1], [-(2**63) - 1]),
        ([2**63 - 1], [-(2**63)]),
        ([1.0], [None]),
        (["a"], ["\ud800"]),
        ([], [None]),
        ([], ["\udfff"]),
    ],
)
def test_append_demotes_without_loss(uimpl: ListImpl, first: list[Any], second: list[Any]) -> None:
    lst = uimpl.new(first)
    for v in second:
        lst.append(v)
    uimpl.check(lst, first + second)
    if not uimpl.is_builtin:
        assert lst.strategy in ("object", "i64", "f64", "bool", "str")


def test_extend(impl: ListImpl) -> None:
    data = [10, 20, 30]
    lst = impl.new(data)

    def gen() -> Any:
        yield 110
        yield 120

    for it in ([40, 50, 60], (70, 80), gen(), [], range(5), range(10, 0, -3), iter([1, 2]), {90: "a"}.keys()):
        values = list(it) if not isinstance(it, (list, tuple, range)) else it
        assert lst.extend(values) is None
        data.extend(values)
        impl.check(lst, data)

    s = {90, 100}
    lst.extend(s)
    data.extend(s)
    assert sorted(lst) == sorted(data)

    original = [1, 2, 3]
    copy_ = original[:]
    lst.extend(original)
    assert original == copy_

    lst_empty = impl.new([], elem=int)
    lst_empty.extend([1, 2, 3])
    impl.check(lst_empty, [1, 2, 3])

    with pytest.raises(TypeError):
        lst.extend(5)
    with pytest.raises(TypeError):
        lst.extend(None)


def test_extend_from_same_implementation(impl: ListImpl) -> None:
    lst = impl.new([1, 2])
    lst.extend(impl.new([3, 4]))
    impl.check(lst, [1, 2, 3, 4])
    lst.extend(lst)
    impl.check(lst, [1, 2, 3, 4, 1, 2, 3, 4])


def test_extend_heterogeneous(uimpl: ListImpl) -> None:
    data = [10, 20, 30]
    lst = uimpl.new(data)
    for it in ("abc", [None, 3.14, {"key": "value"}], (2**70, True), b"\x00\xff"):
        lst.extend(it)
        data.extend(it)
        uimpl.check(lst, data)
    assert len(lst) == len(data)


def test_extend_with_failing_iterable(uimpl: ListImpl) -> None:
    """The exception propagates. Builtin `list` keeps the items consumed before the
    failure; a three-phase implementation may leave the list unchanged. Either way the
    list stays consistent and usable."""
    lst = uimpl.new([1, 2])

    def failing() -> Any:
        yield 3
        yield 4
        raise RuntimeError("boom")

    with pytest.raises(RuntimeError, match="boom"):
        lst.extend(failing())
    got = list(lst)
    assert got in ([1, 2], [1, 2, 3, 4])
    lst.append(5)
    assert list(lst) == got + [5]


def test_insert(impl: ListImpl) -> None:
    data = [10, 20, 30, 40]
    lst = impl.new(data)
    for i, v in ((0, 5), (2, 15), (len(data), 50), (-1, 45), (100, 60), (-100, 1), (1, 20), (True, 7), (Idx(2), 8)):
        i_ = i if not isinstance(i, Idx) else i.__index__()
        assert lst.insert(i, v) is None
        data.insert(i_, v)
        impl.check(lst, data)

    lst_empty = impl.new([], elem=int)
    lst_empty.insert(0, 100)
    impl.check(lst_empty, [100])
    lst_empty = impl.new([], elem=int)
    lst_empty.insert(-5, 100)
    impl.check(lst_empty, [100])

    with pytest.raises(TypeError):
        lst.insert("a", 1)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        lst.insert(1.0, 1)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        lst.insert(1)  # type: ignore[call-arg]
    impl.check(lst, data)


def test_insert_heterogeneous(uimpl: ListImpl) -> None:
    data = [10, 20, 30, 40]
    lst = uimpl.new(data)
    for i, v in ((3, "hello"), (5, None), (7, {"key": "value"}), (2, [1, 2, 3]), (0, 2**65), (1, 0.5)):
        lst.insert(i, v)
        data.insert(i, v)
        uimpl.check(lst, data)


def test_remove(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50, 20]
    lst = impl.new(data)
    for v in (20, 20, 40, 10, 50):
        assert lst.remove(v) is None
        data.remove(v)
        impl.check(lst, data)
    with pytest.raises(ValueError):
        lst.remove(100)
    with pytest.raises(ValueError):
        impl.new([], elem=int).remove(1)
    lst_single = impl.new([99])
    lst_single.remove(99)
    impl.check(lst_single, [])


def test_remove_heterogeneous(uimpl: ListImpl) -> None:
    mixed_data = [1, "hello", 3.14, None, [1, 2], "hello"]
    lst_mixed = uimpl.new(mixed_data)
    for v in ("hello", [1, 2], None):
        lst_mixed.remove(v)
        mixed_data.remove(v)
        uimpl.check(lst_mixed, mixed_data)
    with pytest.raises(ValueError):
        lst_mixed.remove("nope")
    # Cross-type equality: removes the first element equal to the value.
    lst = uimpl.new([1, 1.0, True])
    lst.remove(True)
    uimpl.check(lst, [1.0, True])


def test_remove_types_never_match(impl: ListImpl) -> None:
    lst = impl.new([1, 2, 3])
    for v in ("1", None, b"1", (1,), 2**70, 1.5):
        with pytest.raises(ValueError):
            lst.remove(v)
    impl.check(lst, [1, 2, 3])


def test_pop(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50]
    lst = impl.new(data)
    for args in ((), (1,), (0,), (-1,), (0,)):
        assert same(lst.pop(*args), data.pop(*args))
        impl.check(lst, data)
    assert len(lst) == 0
    with pytest.raises(IndexError):
        lst.pop()
    with pytest.raises(IndexError):
        lst.pop(0)

    lst = impl.new([1, 2, 3])
    with pytest.raises(IndexError):
        lst.pop(10)
    with pytest.raises(IndexError):
        lst.pop(-4)
    with pytest.raises(TypeError):
        lst.pop("a")  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        lst.pop(1.0)  # type: ignore[arg-type]
    assert lst.pop(True) == 2

    data = [1, 2, 3, 4, 5]
    lst = impl.new(data)
    while lst:
        assert same(lst.pop(), data.pop())
        impl.check(lst, data)


def test_pop_returns_same_object(uimpl: ListImpl) -> None:
    obj = object()
    big = 2**100
    lst = uimpl.new([obj, big, "x"])
    assert lst.pop(0) is obj
    assert lst.pop(0) is big


def test_clear(impl: ListImpl) -> None:
    lst = impl.new([10, 20, 30, 40, 50])
    assert lst.clear() is None
    impl.check(lst, [])
    lst.clear()
    impl.check(lst, [])
    lst.append(1)
    impl.check(lst, [1])


def test_clear_heterogeneous(uimpl: ListImpl) -> None:
    for data in ([1, "hello", None, 3.14, [1, 2]], [[1, 2], [3, 4]]):
        lst = uimpl.new(data)
        lst.clear()
        uimpl.check(lst, [])
        lst.append(data[0])
        uimpl.check(lst, [data[0]])


def test_index(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50, 20, 60]
    lst = impl.new(data)
    for v in (10, 30, 60, 20):
        assert lst.index(v) == data.index(v)
    for args in ((20, 2), (20, 0, 5), (20, -2), (20, -100), (20, 1, -1), (60, -1), (20, 2, 100)):
        assert lst.index(*args) == data.index(*args)
    for args in ((100,), (40, 0, 2), (20, 6), (20, 2, 5), (10, 1), (60, -100, -1), (10, 5, 2)):
        with pytest.raises(ValueError):
            lst.index(*args)
        with pytest.raises(ValueError):
            data.index(*args)
    # Huge bounds are clamped, never OverflowError.
    assert lst.index(10, -(2**100)) == 0
    assert lst.index(60, 0, 2**100) == 6
    with pytest.raises(ValueError):
        lst.index(10, 2**100)
    assert lst.index(20, Idx(2)) == 5
    with pytest.raises(TypeError):
        lst.index(20, "a")  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        lst.index(20, 1.0)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        lst.index()  # type: ignore[call-arg]

    with pytest.raises(ValueError):
        impl.new([], elem=int).index(1)
    assert impl.new([99]).index(99) == 0


def test_index_heterogeneous(uimpl: ListImpl) -> None:
    mixed_data = [1, "hello", 3.14, None, [1, 2], "hello"]
    lst_mixed = uimpl.new(mixed_data)
    for v in ("hello", None, [1, 2], 3.14, 1):
        assert lst_mixed.index(v) == mixed_data.index(v)
    with pytest.raises(ValueError):
        lst_mixed.index("not in list")
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    assert lst_nested.index([3, 4]) == 1
    assert lst_nested.index([5, 6]) == 2


def test_count(impl: ListImpl) -> None:
    data = [10, 20, 30, 20, 40, 50, 20, 60, 20]
    lst = impl.new(data)
    for v in (20, 30, 100, "20", None, 20.0, 2**80):
        assert lst.count(v) == data.count(v)
    assert impl.new([], elem=int).count(10) == 0
    lst_single = impl.new([99])
    assert lst_single.count(99) == 1
    assert lst_single.count(100) == 0
    dup = [1, 1, 1, 2, 3, 1, 4, 1]
    lst_dup = impl.new(dup)
    for v in (1, 2, 5, True, 1.0):
        assert lst_dup.count(v) == dup.count(v)


def test_count_heterogeneous(uimpl: ListImpl) -> None:
    mixed_data = [1, "hello", 3.14, None, "hello", 3.14, 3.14, True, 1.0]
    lst_mixed = uimpl.new(mixed_data)
    for v in ("hello", 3.14, None, 1, True, 1.0, "absent"):
        assert lst_mixed.count(v) == mixed_data.count(v)
    nested_data = [[1, 2], [3, 4], [1, 2], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    for v in ([1, 2], [5, 6], [7, 8]):
        assert lst_nested.count(v) == nested_data.count(v)


def test_reverse(impl: ListImpl) -> None:
    for data in ([10, 20, 30, 40, 50], [99], [], [1, 2, 3, 4], [1, 2, 2, 3, 3, 3], ["a", "b"], [1.5, -0.0], [True, False, False]):
        lst = impl.new(data, elem=int if not data else None)
        assert lst.reverse() is None
        data.reverse()
        impl.check(lst, data)
        lst.reverse()
        data.reverse()
        impl.check(lst, data)


def test_reverse_heterogeneous(uimpl: ListImpl) -> None:
    for data in ([1, "hello", 3.14, None, [5, 6]], [[1, 2], [3, 4], [5, 6]]):
        lst = uimpl.new(data)
        lst.reverse()
        data.reverse()
        uimpl.check(lst, data)


def test_copy(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50]
    lst = impl.new(data)
    lst_copy = lst.copy()
    assert type(lst_copy) is impl.cls
    impl.check(lst_copy, data)
    assert lst_copy is not lst
    if not impl.is_builtin:
        assert lst_copy.schema_type is lst.schema_type
    lst_copy.append(60)
    impl.check(lst, data)
    assert lst != lst_copy

    for d in ([], [99], [1, 2, 2, 3, 3, 3]):
        src = impl.new(d, elem=int)
        c = src.copy()
        impl.check(c, d)
        assert c is not src


def test_copy_is_shallow(uimpl: ListImpl) -> None:
    mixed_data = [1, "hello", 3.14, None, [5, 6]]
    lst_mixed = uimpl.new(mixed_data)
    lst_mixed_copy = lst_mixed.copy()
    uimpl.check(lst_mixed_copy, mixed_data)
    lst_mixed_copy[4].append(7)
    assert lst_mixed[4] == [5, 6, 7]
    assert lst_mixed[4] is lst_mixed_copy[4]


def test_contains(impl: ListImpl) -> None:
    data = [10, 20, 30, 40, 50]
    lst = impl.new(data)
    for v in (10, 30, 50, 100, 0, 10.0, True, "10", None, 2**70, 10.5):
        assert (v in lst) == (v in data)
        assert (v not in lst) == (v not in data)
    lst_empty = impl.new([], elem=int)
    assert 10 not in lst_empty
    assert None not in lst_empty
    dup = impl.new([1, 2, 2, 3, 3, 3])
    assert 2 in dup and 3 in dup and 4 not in dup


def test_contains_heterogeneous(uimpl: ListImpl) -> None:
    mixed_data = [1, "hello", 3.14, None, [1, 2]]
    lst_mixed = uimpl.new(mixed_data)
    for v in ("hello", 3.14, None, [1, 2], "world", 1.0, True, 2):
        assert (v in lst_mixed) == (v in mixed_data)
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = uimpl.new(nested_data)
    assert [1, 2] in lst_nested
    assert [7, 8] not in lst_nested

    obj1 = CustomObj(1)
    obj2 = CustomObj(2)
    obj_list = uimpl.new([obj1, obj2])
    assert obj1 in obj_list
    assert CustomObj(1) in obj_list
    assert CustomObj(3) not in obj_list


def test_eq(impl: ListImpl) -> None:
    data1 = [10, 20, 30, 40, 50]
    lst1 = impl.new(data1)
    lst2 = impl.new([10, 20, 30, 40, 50])
    assert lst1 == lst2
    assert lst1 == data1
    assert data1 == lst1
    assert not (lst1 != lst2)
    for other in ([10, 20, 30, 40, 60], [10, 20, 30], [10, 20, 30, 40, 50, 60], []):
        o = impl.new(other, elem=int)
        assert (lst1 == o) == (data1 == other)
        assert (lst1 != o) == (data1 != other)
        assert (lst1 == other) == (data1 == other)
        assert (other == lst1) == (other == data1)
    for other in ("not a list", None, 123, (10, 20, 30, 40, 50), {10, 20, 30, 40, 50}):
        assert (lst1 == other) is False
        assert (lst1 != other) is True
        assert (other == lst1) is False
    # The rich comparison returns NotImplemented for foreign types.
    assert lst1.__eq__((10, 20, 30, 40, 50)) is NotImplemented
    assert lst1.__ne__(None) is NotImplemented
    # Numeric cross-type equality.
    assert impl.new([1, 2]) == [1.0, 2.0]
    assert impl.new([1, 0]) == [True, False]


def test_eq_heterogeneous(uimpl: ListImpl) -> None:
    nested1 = [[1, 2], [3, 4], [5, 6]]
    assert uimpl.new(nested1) == uimpl.new([[1, 2], [3, 4], [5, 6]])
    assert uimpl.new(nested1) != uimpl.new([[1, 2], [3, 4], [7, 8]])

    m1 = uimpl.new([[1, 2], [3, 4]])
    m2 = uimpl.new([[1, 2], [3, 4]])
    m1[1][1] = 99
    assert m1 != m2

    mixed1 = [1, "hello", 3.14, None, [5, 6]]
    assert uimpl.new(mixed1) == uimpl.new([1, "hello", 3.14, None, [5, 6]])
    assert uimpl.new(mixed1) == mixed1

    obj1, obj2, obj3 = CustomObj(1), CustomObj(1), CustomObj(2)
    assert uimpl.new([obj1, obj2]) == uimpl.new([obj1, obj2])
    assert uimpl.new([obj1, obj2]) != uimpl.new([obj1, obj3])
    assert uimpl.new([obj1]) == [obj2]


def test_mul(impl: ListImpl) -> None:
    data = [1, 2, 3]
    lst = impl.new(data)
    for n in (3, 1, 0, -1, -100, True, False, Idx(2)):
        r = lst * n
        r2 = n * lst
        nn = n.__index__() if isinstance(n, Idx) else n
        assert type(r) is impl.cls and type(r2) is impl.cls
        impl.check(r, data * nn)
        impl.check(r2, nn * data)
        assert r is not lst
    impl.check(lst, data)

    lst_empty = impl.new([], elem=int)
    impl.check(lst_empty * 3, [])
    impl.check(3 * lst_empty, [])
    impl.check(impl.new([99]) * 4, [99] * 4)
    large = list(range(10))
    impl.check(impl.new(large) * 5, large * 5)
    impl.check(5 * impl.new(large), 5 * large)

    for bad in (2.0, "a", None, [2]):
        with pytest.raises(TypeError):
            _ = lst * bad
        with pytest.raises(TypeError):
            _ = bad * lst
    with pytest.raises((OverflowError, MemoryError)):
        _ = lst * 2**100


def test_mul_heterogeneous(uimpl: ListImpl) -> None:
    mixed = [1, "hello", 3.14, None]
    uimpl.check(uimpl.new(mixed) * 2, mixed * 2)
    uimpl.check(2 * uimpl.new(mixed), 2 * mixed)
    nested = [[1, 2], [3, 4]]
    r = uimpl.new(nested) * 2
    uimpl.check(r, nested * 2)
    # Repetition is shallow: the same inner objects repeat.
    assert r[0] is r[2]


# ======================================================================================
# Part 2 -- new coverage
# ======================================================================================

# ---- sort ---------------------------------------------------------------------------


SORT_DATA = [
    [],
    [1],
    [3, 1, 2],
    [5, -3, 8, 6, 2, 9, -3, 0, 2**62, -(2**62)],
    list(range(100, 0, -1)),
    [2.5, -1.0, 0.0, 1e300, -1e-300, float("inf"), float("-inf"), 3.0],
    ["b", "a", "", "ab", "B", "é", "z", "😀", "￿", "aa"],
    [True, False, True, False],
]


@pytest.mark.parametrize("data", SORT_DATA, ids=lambda d: repr(d)[:40])
@pytest.mark.parametrize("reverse", [False, True])
def test_sort(impl: ListImpl, data: list[Any], reverse: bool) -> None:
    lst = impl.new(data, elem=int if not data else None)
    assert lst.sort(reverse=reverse) is None
    impl.check(lst, sorted(data, reverse=reverse))


def test_sort_random_large(impl: ListImpl) -> None:
    rng = random.Random(1234)
    for data in (
        [rng.randrange(-1000, 1000) for _ in range(5000)],
        [rng.uniform(-1e6, 1e6) for _ in range(5000)],
        ["".join(rng.choice("abcxyzé😀") for _ in range(rng.randrange(6))) for _ in range(2000)],
        [rng.random() < 0.5 for _ in range(1000)],
    ):
        lst = impl.new(data)
        lst.sort()
        impl.check(lst, sorted(data))
        lst.sort(reverse=True)
        impl.check(lst, sorted(data, reverse=True))


def test_sort_key(impl: ListImpl) -> None:
    data = [5, 3, 8, 6, 2, 9, 4, 7, 1, 0, 11, 14]
    for key in (lambda x: x % 3, lambda x: -x, abs, str, lambda x: 0):
        for reverse in (False, True):
            lst = impl.new(data)
            lst.sort(key=key, reverse=reverse)
            impl.check(lst, sorted(data, key=key, reverse=reverse))
    words = ["banana", "Apple", "cherry", "apple", "Banana", "date"]
    for key in (str.lower, len, None):
        for reverse in (False, True):
            lst = impl.new(words)
            lst.sort(key=key, reverse=reverse)
            impl.check(lst, sorted(words, key=key, reverse=reverse))


def test_sort_is_stable(uimpl: ListImpl) -> None:
    records = [(k, i) for i, k in enumerate([3, 1, 2, 3, 1, 2, 3, 1, 2, 0])]
    for reverse in (False, True):
        lst = uimpl.new(records)
        lst.sort(key=operator.itemgetter(0), reverse=reverse)
        expected = sorted(records, key=operator.itemgetter(0), reverse=reverse)
        uimpl.check(lst, expected)
        # Equal keys keep their original relative order even when reversed.
        for k in range(4):
            idx = [i for (kk, i) in lst if kk == k]
            assert idx == sorted(idx)


def test_sort_stable_on_equal_values_without_key(impl: ListImpl) -> None:
    """Native sorts must be stable *by Python equality*: 0.0 == -0.0 keep their order,
    and so do 1 / 1.0 / True in object storage."""
    data = [0.0, -0.0, 1.0, -0.0, 0.0, -1.0]
    lst = impl.new(data)
    lst.sort()
    impl.check(lst, sorted(data))
    lst = impl.new(data)
    lst.sort(reverse=True)
    impl.check(lst, sorted(data, reverse=True))


def test_sort_mixed_numeric_stable(uimpl: ListImpl) -> None:
    data = [1, True, 1.0, 0, False, 0.0, 2, -1.5]
    lst = uimpl.new(data)
    lst.sort()
    uimpl.check(lst, sorted(data))
    lst = uimpl.new(data)
    lst.sort(reverse=True)
    uimpl.check(lst, sorted(data, reverse=True))


def test_sort_big_ints(impl: ListImpl) -> None:
    data = [2**70, -(2**70), 5, 2**63, -(2**63) - 1, 0, 2**63 - 1]
    lst = impl.new(data)
    lst.sort()
    impl.check(lst, sorted(data))


def test_sort_with_nan_does_not_crash(impl: ListImpl) -> None:
    data = [3.0, NAN, 1.0, 2.0, NAN, -1.0]
    lst = impl.new(data)
    lst.sort()
    got = list(lst)
    assert len(got) == len(data)
    assert sum(math.isnan(x) for x in got) == 2
    assert sorted(x for x in got if not math.isnan(x)) == [-1.0, 1.0, 2.0, 3.0]
    lst.sort(reverse=True)
    assert len(lst) == 6


def test_sort_incomparable_raises(uimpl: ListImpl) -> None:
    data = [3, "a", 1, None, 2.5]
    lst = uimpl.new(data)
    with pytest.raises(TypeError):
        lst.sort()
    # The list is left as some permutation of its items.
    assert multiset(lst) == multiset(data)


def test_sort_key_raises_leaves_list_intact(impl: ListImpl) -> None:
    data = [3, 1, 2]
    lst = impl.new(data)

    def bad_key(x: int) -> int:
        if x == 2:
            raise ZeroDivisionError("key")
        return x

    with pytest.raises(ZeroDivisionError):
        lst.sort(key=bad_key)
    impl.check(lst, data)


def test_sort_detects_mutation_during_sort(impl: ListImpl) -> None:
    lst = impl.new([3, 2, 1])

    def key(x: int) -> int:
        lst.append(0)
        return x

    with pytest.raises(ValueError):
        lst.sort(key=key)
    assert len(lst) >= 0  # still usable


def test_sort_signature(impl: ListImpl) -> None:
    lst = impl.new([3, 1, 2])
    with pytest.raises(TypeError):
        lst.sort(None)  # type: ignore[misc]
    with pytest.raises(TypeError):
        lst.sort(lambda x: x)  # type: ignore[misc]
    with pytest.raises(TypeError):
        lst.sort(cmp=None)  # type: ignore[call-arg]
    with pytest.raises(TypeError):
        lst.sort(key=5)  # not callable
    lst.sort(key=None, reverse=False)
    impl.check(lst, [1, 2, 3])


# ---- + and += -------------------------------------------------------------------------


def test_add(impl: ListImpl) -> None:
    a_data, b_data = [1, 2], [3, 4, 5]
    a, b = impl.new(a_data), impl.new(b_data)
    for left, right in ((a, b), (a, b_data), (a_data, b)):
        r = left + right
        assert type(r) is impl.cls
        impl.check(r, a_data + b_data)
        assert r is not left and r is not right
    impl.check(a, a_data)
    impl.check(b, b_data)
    impl.check(a + impl.new([], elem=int), a_data)
    impl.check(impl.new([], elem=int) + [], [])
    for bad in ((3,), None, 3, "ab", {3}, range(2)):
        with pytest.raises(TypeError):
            _ = a + bad
        with pytest.raises(TypeError):
            _ = bad + a


def test_add_heterogeneous(uimpl: ListImpl) -> None:
    r = uimpl.new([1, "a"]) + [None, 2.5]
    uimpl.check(r, [1, "a", None, 2.5])
    r = [None] + uimpl.new([1])
    uimpl.check(r, [None, 1])
    r = uimpl.new([1, 2]) + ["x"]
    uimpl.check(r, [1, 2, "x"])


def test_iadd(impl: ListImpl) -> None:
    data = [1, 2]
    lst = impl.new(data)
    ident = id(lst)

    def gen() -> Any:
        yield 9
        yield 10

    for make_other in (lambda: [3], lambda: (4, 5), lambda: range(6, 8), gen, lambda: [], lambda: impl.new([11])):
        lst += make_other()
        data += list(make_other())
        assert id(lst) == ident
        impl.check(lst, data)
    lst += lst
    data += data
    impl.check(lst, data)
    with pytest.raises(TypeError):
        lst += 5  # type: ignore[operator]
    with pytest.raises(TypeError):
        lst += None  # type: ignore[operator]
    impl.check(lst, data)


def test_iadd_heterogeneous(uimpl: ListImpl) -> None:
    data = [1, 2]
    lst = uimpl.new(data)
    for other in ("ab", [None], (2**80,)):
        lst += other
        data += other
        uimpl.check(lst, data)


def test_imul(impl: ListImpl) -> None:
    for n in (3, 1, 0, -2, True, False, Idx(2)):
        data = [1, 2, 3]
        lst = impl.new(data)
        ident = id(lst)
        lst *= n
        data *= n.__index__() if isinstance(n, Idx) else n
        assert id(lst) == ident
        impl.check(lst, data)
    lst = impl.new([1])
    for bad in (2.0, "a", None):
        with pytest.raises(TypeError):
            lst *= bad  # type: ignore[operator]
    with pytest.raises((OverflowError, MemoryError)):
        lst *= 2**100
    impl.check(lst, [1])


# ---- ordering ---------------------------------------------------------------------------

ORDER_PAIRS = [
    ([1, 2], [1, 3]),
    ([1, 2], [1, 2]),
    ([1, 2], [1, 2, 0]),
    ([], []),
    ([], [0]),
    ([2], [1, 9]),
    ([1.5, 2.0], [1.5, 1.0]),
    (["a", "b"], ["a", "c"]),
    (["b"], ["a", "z"]),
    ([True, False], [True, True]),
    ([2**70], [2**70 + 1]),
    ([NAN], [NAN]),
    ([1.0, NAN], [1.0, 0.5]),
]

OPS = [operator.lt, operator.le, operator.gt, operator.ge, operator.eq, operator.ne]


@pytest.mark.parametrize("a,b", ORDER_PAIRS, ids=repr)
@pytest.mark.parametrize("op", OPS, ids=lambda o: o.__name__)
def test_ordering(impl: ListImpl, a: list[Any], b: list[Any], op: Any) -> None:
    elem = None
    if not a and not b:
        elem = int
    la = impl.new(a, elem=elem or (type(a[0]) if a else type(b[0])))
    lb = impl.new(b, elem=elem or (type(b[0]) if b else type(a[0])))
    # NaN pairs share one NaN object, so builtin identity semantics and the native
    # "NaN is identical to NaN" rule agree.
    expected = op(a, b)
    assert op(la, lb) == expected
    assert op(la, b) == expected
    assert op(a, lb) == expected


def test_ordering_cross_type(uimpl: ListImpl) -> None:
    assert uimpl.new([1, 2]) < [1, 2.5]
    assert uimpl.new([1.0]) <= [True]
    assert uimpl.new([2**70]) > [1e20]
    assert uimpl.new([1, "a"]) < [1, "b"]
    assert uimpl.new([1, "a"]) < [2, None]  # decided before reaching incomparable items
    with pytest.raises(TypeError):
        _ = uimpl.new([1]) < ["a"]
    with pytest.raises(TypeError):
        _ = uimpl.new([None]) < [1]
    assert not (uimpl.new([None]) < [None])  # equal items are skipped via ==, never ordered


def test_ordering_against_foreign_types(impl: ListImpl) -> None:
    lst = impl.new([1, 2])
    for other in ((1, 2), 1, None, "ab", {1, 2}):
        for op in (operator.lt, operator.le, operator.gt, operator.ge):
            with pytest.raises(TypeError):
                op(lst, other)
            with pytest.raises(TypeError):
                op(other, lst)
    assert lst.__lt__((1, 2)) is NotImplemented


def test_ordering_mixed_implementations(rs: Any) -> None:
    a = rs.List[int]([1, 2])
    b = rs.List([1, 2.5])
    c = rs.List[float]([1.0, 2.0])
    assert a < b and b > a and a <= c and a >= c and a == c
    assert not (a != c)


# ---- repr ---------------------------------------------------------------------------------

REPR_DATA = [
    [],
    [1, 2, 3],
    [-(2**63), 2**63 - 1],
    [0.1, 1e16, 1e-7, -0.0, float("inf"), float("-inf"), 123456789.123, 2.0**70],
    ["a", "a'b", 'a"b', "x\ny", "é", "😀", "\x00", "\\"],
    [True, False],
    [NAN],
]


@pytest.mark.parametrize("data", REPR_DATA, ids=lambda d: repr(d)[:30])
def test_repr(impl: ListImpl, data: list[Any]) -> None:
    lst = impl.new(data, elem=int if not data else None)
    assert repr(lst) == impl.expected_repr(lst, data)
    assert str(lst) == repr(lst)


def test_repr_heterogeneous(uimpl: ListImpl) -> None:
    data = [1, "a", None, 2.5, [1, [2]], (3,), {"k": 1}, 2**100, CustomObj(4), True]
    lst = uimpl.new(data)
    assert repr(lst) == uimpl.expected_repr(lst, data)


def test_repr_recursive(uimpl: ListImpl) -> None:
    lst = uimpl.new([1])
    lst.append(lst)
    r = repr(lst)
    assert "..." in r
    assert r.startswith(repr(uimpl.cls([1]))[:-2])
    assert lst[1] is lst


def test_repr_typed(rs: Any) -> None:
    assert repr(rs.List[int]([1, 2, 3])) == "List[int]([1, 2, 3])"
    assert repr(rs.List[int]()) == "List[int]([])"
    assert repr(rs.List([1, "a"])) == "List([1, 'a'])"
    assert repr(rs.List()) == "List([])"
    assert repr(rs.List[str](["a"])) == "List[str](['a'])"
    assert repr(rs.List[float]([1])) == "List[float]([1.0])"


# ---- reversed and iterators -----------------------------------------------------------------


def test_reversed(impl: ListImpl) -> None:
    for data in ([], [1], [1, 2, 3], list(range(50))):
        lst = impl.new(data, elem=int)
        r = reversed(lst)
        assert iter(r) is r
        assert_same_items(r, data[::-1])
        assert list(r) == []


def test_reversed_is_live(impl: ListImpl) -> None:
    data = [1, 2, 3]
    for mutate in (lambda l: l.append(9), lambda l: l.clear(), lambda l: l.pop(), lambda l: l.insert(0, 7), lambda l: l.__setitem__(0, 5)):
        lst = impl.new(data)
        ref = list(data)
        r_lst, r_ref = reversed(lst), reversed(ref)
        assert next(r_lst) == next(r_ref)
        mutate(lst)
        mutate(ref)
        assert list(r_lst) == list(r_ref)

    # An index past the end after shrinking ends the reverse iteration (CPython semantics).
    lst = impl.new([1, 2, 3])
    r = reversed(lst)
    lst.pop()
    assert list(r) == []


def test_iter_basics(impl: ListImpl) -> None:
    lst = impl.new([1, 2, 3])
    it = iter(lst)
    assert iter(it) is it
    assert next(it) == 1
    assert list(it) == [2, 3]
    with pytest.raises(StopIteration):
        next(it)
    assert list(iter(impl.new([], elem=int))) == []


def test_iter_is_live(impl: ListImpl) -> None:
    lst = impl.new([1, 2, 3])
    it = iter(lst)
    assert next(it) == 1
    lst.append(4)
    lst[1] = 20
    assert list(it) == [20, 3, 4]


def test_iter_exhausted_stays_exhausted(impl: ListImpl) -> None:
    lst = impl.new([1, 2])
    it = iter(lst)
    assert list(it) == [1, 2]
    lst.append(3)
    with pytest.raises(StopIteration):
        next(it)


def test_iter_mutation_during_for_loop(impl: ListImpl) -> None:
    data = [1, 2, 3, 4, 5, 6]
    lst = impl.new(data)
    ref = list(data)
    seen, seen_ref = [], []
    for x in lst:
        seen.append(x)
        lst.remove(x)
    for x in ref:
        seen_ref.append(x)
        ref.remove(x)
    assert seen == seen_ref
    impl.check(lst, ref)

    lst = impl.new([1, 2, 3])
    ref = [1, 2, 3]
    out, out_ref = [], []
    for x in lst:
        out.append(x)
        if len(lst) < 6:
            lst.append(x * 10)
    for x in ref:
        out_ref.append(x)
        if len(ref) < 6:
            ref.append(x * 10)
    assert out == out_ref
    impl.check(lst, ref)


def test_iter_shrink_and_regrow(impl: ListImpl) -> None:
    lst = impl.new([1, 2, 3, 4])
    ref = [1, 2, 3, 4]
    it, it_ref = iter(lst), iter(ref)
    assert next(it) == next(it_ref)
    assert next(it) == next(it_ref)
    lst.clear()
    ref.clear()
    lst.extend([7, 8, 9, 10])
    ref.extend([7, 8, 9, 10])
    assert list(it) == list(it_ref)


def test_iter_over_demotion(uimpl: ListImpl) -> None:
    """A live iterator survives a strategy change (native -> object) mid-iteration."""
    lst = uimpl.new([1, 2, 3])
    it = iter(lst)
    assert next(it) == 1
    lst.append("x")
    lst[1] = None
    assert_same_items(it, [None, 3, "x"])


# ---- NaN and identity -------------------------------------------------------------------


def test_nan_same_object(impl: ListImpl) -> None:
    """With the very same NaN object, builtin identity semantics and the native
    "NaN is identical to NaN" rule agree."""
    nan = NAN
    lst = impl.new([1.0, nan, 2.0])
    assert nan in lst
    assert lst.index(nan) == 1
    assert lst.count(nan) == 1
    assert lst == [1.0, nan, 2.0]
    lst.remove(nan)
    impl.check(lst, [1.0, 2.0])


def test_nan_distinct_objects(impl: ListImpl) -> None:
    """Builtin lists compare distinct NaN objects as unequal; native float storage treats
    every NaN as identical (ARCHITECTURE §4.1, "Equality semantics note")."""
    a, b = float("nan"), float("nan")
    assert a is not b
    lst = impl.new([1.0, a])
    native = not impl.is_builtin and lst.strategy == "f64"
    assert (b in lst) is native
    assert lst.count(b) == (1 if native else 0)
    assert (lst == [1.0, b]) is native
    if native:
        assert lst.index(b) == 1
    else:
        with pytest.raises(ValueError):
            lst.index(b)


def test_nan_in_object_storage_uses_identity(uimpl: ListImpl) -> None:
    a, b = float("nan"), float("nan")
    lst = uimpl.new([a, "x"])  # heterogeneous -> object storage keeps the objects
    assert lst[0] is a
    assert a in lst
    assert b not in lst
    assert lst.count(b) == 0
    assert lst == [a, "x"]
    assert lst != [b, "x"]


def test_nan_values_round_trip(impl: ListImpl) -> None:
    lst = impl.new([NAN, float("inf"), -0.0])
    got = list(lst)
    assert math.isnan(got[0])
    assert got[1] == float("inf")
    assert got[2] == 0.0 and math.copysign(1.0, got[2]) == -1.0


def test_identity_preserved_in_object_storage(uimpl: ListImpl) -> None:
    objs = [object(), [1], 2**100, "x" * 50, CustomObj(1)]
    lst = uimpl.new(objs)
    for i, o in enumerate(objs):
        assert lst[i] is o
    assert [x for x in lst][0] is objs[0]
    assert lst[1:3][0] is objs[1]
    assert lst.copy()[4] is objs[4]


def test_custom_eq_is_called(uimpl: ListImpl) -> None:
    calls: list[object] = []

    class Spy:
        def __eq__(self, other: object) -> bool:
            calls.append(other)
            return other == 42

        __hash__ = None  # type: ignore[assignment]

    lst = uimpl.new([1, 42, 3])
    assert Spy() in lst
    assert calls  # Python-level __eq__ ran (outside any lock)
    assert lst.count(Spy()) == 1
    assert lst.index(Spy()) == 1


def test_eq_raising_propagates(uimpl: ListImpl) -> None:
    class Bad:
        def __eq__(self, other: object) -> bool:
            raise ArithmeticError("no eq")

        __hash__ = None  # type: ignore[assignment]

    lst = uimpl.new([Bad()])
    with pytest.raises(ArithmeticError):
        _ = 1 in lst
    with pytest.raises(ArithmeticError):
        lst.count(1)
    with pytest.raises(ArithmeticError):
        lst.index(1)
    with pytest.raises(ArithmeticError):
        lst.remove(1)
    assert len(lst) == 1


def test_eq_mutating_list_is_safe(uimpl: ListImpl) -> None:
    """`__eq__` that mutates the list being searched must not crash. CPython semantics:
    the search continues by index over the *current* contents (no RuntimeError)."""
    lst = uimpl.new([])

    class Mutator:
        def __eq__(self, other: object) -> bool:
            lst.clear()
            return False

        __hash__ = None  # type: ignore[assignment]

    lst.extend([Mutator(), Mutator(), Mutator()])
    assert 1 not in lst
    assert len(lst) == 0
    lst.extend([Mutator(), 1])
    with pytest.raises(ValueError):
        lst.index(1)


# ---- numeric cross-type comparisons on native storage --------------------------------------


@pytest.mark.parametrize(
    "data,value",
    [
        ([2**63 - 1], float(2**63)),
        ([9007199254740992.0], 9007199254740993),
        ([9007199254740993], 9007199254740992.0),
        ([1, 2, 3], 2.0),
        ([1.0, 2.0], 2),
        ([1, 0], True),
        ([True, False], 1),
        ([1.5], True),
        ([-(2**63)], float(-(2**63))),
        ([2**53 + 1], float(2**53 + 1)),
        ([0.0], -0.0),
        ([1], float("inf")),
        ([1.0], 2**1000),
        (["1"], 1),
        ([1], "1"),
    ],
    ids=repr,
)
def test_cross_type_membership(impl: ListImpl, data: list[Any], value: Any) -> None:
    lst = impl.new(data)
    assert (value in lst) == (value in data)
    assert lst.count(value) == data.count(value)
    if value in data:
        assert lst.index(value) == data.index(value)
    else:
        with pytest.raises(ValueError):
            lst.index(value)
    assert (lst == [value]) == (data == [value])


# ---- native kernels vs builtins --------------------------------------------------------------

SUM_DATA = [
    [],
    [1, 2, 3],
    [2**62, 2**62, 2**62],
    [-(2**63), -1],
    [2**63 - 1, 1],
    [0.1] * 10,
    [1e100, 1.0, -1e100],
    [1.5, float("inf")],
    [float("inf"), float("-inf")],
    [True, True, False],
    [2**80, -(2**80), 3],
]


@pytest.mark.parametrize("data", SUM_DATA, ids=lambda d: repr(d)[:30])
def test_sum(impl: ListImpl, data: list[Any]) -> None:
    lst = impl.new(data, elem=int if not data else None)
    assert same(impl.sum(lst), sum(data))


def test_sum_heterogeneous(uimpl: ListImpl) -> None:
    for data in ([1, 2.5], [True, 2], [1, 2**70, 0.5], [0.1, 0.2, 3]):
        assert same(uimpl.sum(uimpl.new(data)), sum(data))
    with pytest.raises(TypeError):
        uimpl.sum(uimpl.new([1, "a"]))


def test_sum_strings_raises(impl: ListImpl) -> None:
    with pytest.raises(TypeError):
        impl.sum(impl.new(["a", "b"]))


MINMAX_DATA = [
    [3, 1, 2],
    [2**63 - 1, -(2**63)],
    [2.5, -1.0, 7.0],
    [NAN, 1.0, 2.0],
    [1.0, NAN, 0.5],
    [1.0, NAN],
    [0.0, -0.0],
    [-0.0, 0.0],
    ["b", "a", "é", "B"],
    [True, False],
    [1, 1, 1],
    [2**70, 5, -(2**70)],
]


@pytest.mark.parametrize("data", MINMAX_DATA, ids=repr)
def test_min_max(impl: ListImpl, data: list[Any]) -> None:
    lst = impl.new(data)
    assert same(impl.min(lst), min(data))
    assert same(impl.max(lst), max(data))


def test_min_max_empty(impl: ListImpl) -> None:
    lst = impl.new([], elem=int)
    with pytest.raises(ValueError):
        impl.min(lst)
    with pytest.raises(ValueError):
        impl.max(lst)


def test_min_max_heterogeneous(uimpl: ListImpl) -> None:
    for data in ([1, 2.5, True], [False, 0, 0.0], [0, False]):
        lst = uimpl.new(data)
        assert same(uimpl.min(lst), min(data))
        assert same(uimpl.max(lst), max(data))
    with pytest.raises(TypeError):
        uimpl.min(uimpl.new([1, "a"]))


def test_map_filter_with_callables(impl: ListImpl) -> None:
    data = [1, 2, 3, 4, 5, 6]
    lst = impl.new(data)
    r = impl.map(lst, lambda x: x * 10)
    assert type(r) is impl.cls
    assert_same_items(r, [x * 10 for x in data])
    r = impl.filter(lst, lambda x: x % 2 == 0)
    assert type(r) is impl.cls
    assert_same_items(r, [2, 4, 6])
    impl.check(lst, data)
    r = impl.map(lst, str)
    assert_same_items(r, [str(x) for x in data])
    with pytest.raises(ZeroDivisionError):
        impl.map(lst, lambda x: 1 // (x - 3))


# ---- misc protocol -------------------------------------------------------------------------


def test_unhashable(impl: ListImpl) -> None:
    lst = impl.new([1])
    with pytest.raises(TypeError):
        hash(lst)
    assert type(lst).__hash__ is None
    with pytest.raises(TypeError):
        {lst}  # noqa: B018


def test_copy_module(impl: ListImpl) -> None:
    data = [1, 2, 3]
    lst = impl.new(data)
    c = copy.copy(lst)
    d = copy.deepcopy(lst)
    for x in (c, d):
        assert type(x) is impl.cls
        impl.check(x, data)
        assert x is not lst
    c.append(4)
    impl.check(lst, data)


def test_deepcopy_heterogeneous(uimpl: ListImpl) -> None:
    inner = [1, [2]]
    lst = uimpl.new([inner, "x", inner])
    d = copy.deepcopy(lst)
    assert d == lst
    assert d[0] is not inner
    assert d[0] is d[2]  # memo preserves sharing
    d[0][1].append(3)
    assert inner == [1, [2]]
    s = copy.copy(lst)
    assert s[0] is inner


def test_slice_independent_after_mutation(impl: ListImpl) -> None:
    lst = impl.new([1, 2, 3, 4])
    s = lst[1:3]
    lst[1] = 9
    impl.check(s, [2, 3])


def test_operations_preserve_types_after_roundtrip(uimpl: ListImpl) -> None:
    data = [1, 2.0, True, "s", None, 2**70, -0.0]
    lst = uimpl.new([])
    for x in data:
        lst.append(x)
    uimpl.check(lst, data)
    lst.reverse()
    uimpl.check(lst, data[::-1])
    uimpl.check(lst[::-1], data)
    uimpl.check(lst * 2, data[::-1] * 2)


# ---- exception-type parity, table driven ----------------------------------------------------

ERROR_CASES: list[tuple[str, Any]] = [
    ("getitem-oob", lambda l: l[10]),
    ("getitem-neg-oob", lambda l: l[-10]),
    ("getitem-huge", lambda l: l[2**100]),
    ("getitem-str", lambda l: l["a"]),
    ("getitem-float", lambda l: l[1.0]),
    ("getitem-none", lambda l: l[None]),
    ("getitem-step0", lambda l: l[::0]),
    ("getitem-slice-str", lambda l: l["a":]),
    ("setitem-oob", lambda l: l.__setitem__(10, 1)),
    ("setitem-str", lambda l: l.__setitem__("a", 1)),
    ("setitem-ext-len", lambda l: l.__setitem__(slice(None, None, 2), [1])),
    ("setitem-slice-noniter", lambda l: l.__setitem__(slice(0, 2), 5)),
    ("setitem-ext-noniter", lambda l: l.__setitem__(slice(None, None, 2), 5)),
    ("setitem-step0", lambda l: l.__setitem__(slice(None, None, 0), [])),
    ("delitem-oob", lambda l: l.__delitem__(10)),
    ("delitem-str", lambda l: l.__delitem__("a")),
    ("delitem-step0", lambda l: l.__delitem__(slice(None, None, 0))),
    ("pop-oob", lambda l: l.pop(10)),
    ("pop-huge", lambda l: l.pop(2**100)),
    ("pop-str", lambda l: l.pop("a")),
    ("pop-too-many", lambda l: l.pop(1, 2)),
    ("insert-huge", lambda l: l.insert(2**100, 1)),
    ("insert-str", lambda l: l.insert("a", 1)),
    ("insert-missing", lambda l: l.insert(1)),
    ("remove-missing", lambda l: l.remove(99)),
    ("remove-noarg", lambda l: l.remove()),
    ("index-missing", lambda l: l.index(99)),
    ("index-bad-start", lambda l: l.index(1, "a")),
    ("index-none-start", lambda l: l.index(1, None)),
    ("index-noarg", lambda l: l.index()),
    ("count-noarg", lambda l: l.count()),
    ("append-noarg", lambda l: l.append()),
    ("append-two", lambda l: l.append(1, 2)),
    ("extend-int", lambda l: l.extend(5)),
    ("extend-none", lambda l: l.extend(None)),
    ("reverse-arg", lambda l: l.reverse(1)),
    ("clear-arg", lambda l: l.clear(1)),
    ("copy-arg", lambda l: l.copy(1)),
    ("sort-positional", lambda l: l.sort(None)),
    ("sort-unknown-kw", lambda l: l.sort(foo=1)),
    ("sort-key-noncallable", lambda l: l.sort(key=5)),
    ("mul-float", lambda l: l * 2.0),
    ("mul-str", lambda l: l * "a"),
    ("rmul-float", lambda l: 2.0 * l),
    ("mul-huge", lambda l: l * 2**100),
    ("add-tuple", lambda l: l + (1,)),
    ("add-int", lambda l: l + 1),
    ("radd-tuple", lambda l: (1,) + l),
    ("lt-int", lambda l: l < 1),
    ("ge-none", lambda l: l >= None),
    ("hash", lambda l: hash(l)),
    ("call", lambda l: l()),
    ("getattr", lambda l: l.no_such_attribute),
    ("pop-empty", lambda l: (l.clear(), l.pop())),
    ("index-empty", lambda l: (l.clear(), l.index(1))),
]


@pytest.mark.parametrize("name,op", ERROR_CASES, ids=[c[0] for c in ERROR_CASES])
def test_exception_type_parity(impl: ListImpl, name: str, op: Any) -> None:
    expected_exc: type[BaseException] | None = None
    try:
        op([1, 2, 3])
    except Exception as e:
        expected_exc = type(e)
    assert expected_exc is not None, f"builtin list did not raise for {name}"
    lst = impl.new([1, 2, 3])
    with pytest.raises(expected_exc):
        op(lst)


RESULT_CASES: list[tuple[str, Any]] = [
    ("index-start-stop", lambda l: l.index(2, -5, 10)),
    ("index-neg-stop", lambda l: l.index(1, 0, -1)),
    ("count-float", lambda l: l.count(2.0)),
    ("count-bool", lambda l: l.count(True)),
    ("contains-float", lambda l: 3.0 in l),
    ("contains-str", lambda l: "1" in l),
    ("slice-neg-step", lambda l: list(l[::-2])),
    ("slice-huge", lambda l: list(l[-(2**100) : 2**100])),
    ("pop-neg", lambda l: l.pop(-2)),
    ("pop-bool", lambda l: l.pop(True)),
    ("len-after-imul0", lambda l: (l.__imul__(0), len(l))[1]),
    ("insert-neg-huge-ok", lambda l: (l.insert(-(10**6), 0), list(l))[1]),
    ("bool", lambda l: bool(l)),
    ("eq-tuple", lambda l: l == (1, 2, 3)),
    ("ne-tuple", lambda l: l != (1, 2, 3)),
    ("eq-list", lambda l: l == [1, 2, 3]),
    ("lt-list", lambda l: l < [1, 2, 4]),
    ("ge-prefix", lambda l: l >= [1, 2]),
]


@pytest.mark.parametrize("name,op", RESULT_CASES, ids=[c[0] for c in RESULT_CASES])
def test_result_parity(impl: ListImpl, name: str, op: Any) -> None:
    expected = op([1, 2, 3])
    got = op(impl.new([1, 2, 3]))
    assert same(got, expected), (got, expected)
