"""Raw-slot hot paths of `List` (Workstream 2): behaviour must match the builtin `list`.

The slots (`x[i]`, `x[i] = v`, `del x[i]`, `len`, `bool`, `in`, iteration, comparisons,
`append`/`pop`/`insert`/`extend`, `__init__`, construction) bypass PyO3, so these tests pin
their semantics against the builtin: error types and messages, reference counting
(displaced objects are released immediately), subclassing, live iteration, pickling of
iterators, and thread interplay with detached kernels.
"""

from __future__ import annotations

import copy
import gc
import pickle
import re
import sys
import threading
import weakref

import pytest

import rustique as rs
from rustique import List

STORAGES = [
    [1, 2, 3, 4],
    [1.5, 2.5, -0.0, 4.25],
    ["a", "bb", "ccc", ""],
    [True, False, True, True],
    [(1,), None, "x", 2.5],
]


class Obj:
    pass


@pytest.mark.parametrize("data", STORAGES)
def test_getitem_matches_list(data: list[object]) -> None:
    x = List(data)
    for i in range(-len(data) - 2, len(data) + 2):
        try:
            expected = data[i]
        except IndexError as e:
            with pytest.raises(IndexError, match=str(e)):
                x[i]
            with pytest.raises(IndexError):
                x.__getitem__(i)
        else:
            assert x[i] == expected and type(x[i]) is type(expected)
            assert x.__getitem__(i) == expected
    with pytest.raises(TypeError, match="list indices must be integers or slices, not str"):
        x["a"]
    with pytest.raises(IndexError, match="cannot fit 'int' into an index-sized integer"):
        x[2**100]
    assert x[True] == data[1]
    assert list(x[1:3]) == data[1:3]


@pytest.mark.parametrize("data", STORAGES)
def test_setitem_delitem_match_list(data: list[object]) -> None:
    for value in (7, 2.5, "s", (1, 2), None, 2**80):
        b, x = list(data), List(data)
        for i in (0, -1, 2):
            b[i] = value
            x[i] = value
            assert list(x) == b
        for i in (len(data), -len(data) - 1):
            with pytest.raises(IndexError, match="list assignment index out of range"):
                x[i] = value
        del b[1]
        del x[1]
        assert list(x) == b
        with pytest.raises(IndexError, match="list assignment index out of range"):
            del x[10]
    x = List(data)
    x.__setitem__(0, "z")
    x.__delitem__(-1)
    assert list(x) == ["z", *data[1:-1]]


def test_typed_setitem_and_append_validate() -> None:
    x = List[int]([1, 2])
    x[0] = 5
    x.append(7)
    x.insert(0, -1)
    assert list(x) == [-1, 5, 2, 7]
    with pytest.raises(TypeError, match=r"List\[int\] expects int, got str"):
        x[0] = "a"
    with pytest.raises(TypeError, match=r"List\[int\] expects int, got str"):
        x.append("a")
    with pytest.raises(TypeError, match=r"List\[int\] expects int, got str"):
        x.insert(0, "a")
    assert list(x) == [-1, 5, 2, 7]
    y = List[List[int]]([[1]])
    y.append([2, 3])
    assert type(y[1]) is List and list(y[1]) == [2, 3]


def _released(make_list, mutate) -> bool:
    """Whether the object displaced by `mutate` is released immediately."""
    obj = Obj()
    ref = weakref.ref(obj)
    x = make_list(obj)
    del obj
    mutate(x)
    return ref() is None


@pytest.mark.parametrize(
    "mutate",
    [
        lambda x: x.__setitem__(0, 1),
        lambda x: x.__delitem__(0),
        lambda x: x.pop(),
        lambda x: x.pop(0),
        lambda x: x.clear(),
        lambda x: x.__init__(),
        lambda x: x.__setitem__(slice(0, 1), []),
    ],
)
def test_displaced_objects_are_released_immediately(mutate) -> None:
    gc.disable()
    try:
        assert _released(lambda o: List([o]), lambda x: (mutate(x), None)[1])
    finally:
        gc.enable()


def test_pop_and_insert_arguments_like_list() -> None:
    for c in (list, List):
        x = c([1, 2, 3])
        with pytest.raises(TypeError, match="pop expected at most 1 argument, got 2"):
            x.pop(1, 2)
        with pytest.raises(TypeError, match="insert expected 2 arguments, got 1"):
            x.insert(1)
        with pytest.raises(TypeError, match="takes no keyword arguments"):
            x.pop(index=1)
        with pytest.raises(TypeError, match="'str' object cannot be interpreted as an integer"):
            x.pop("a")
        with pytest.raises(OverflowError):
            x.pop(2**100)
        with pytest.raises(IndexError, match="pop index out of range"):
            x.pop(5)
        with pytest.raises(OverflowError):
            x.insert(2**100, 9)
        x.insert(10**9, 9)
        x.insert(-(10**9), 0)
        assert list(x) == [0, 1, 2, 3, 9]
        assert x.pop(True) == 1
        x.clear()
        with pytest.raises(IndexError, match="pop from empty list"):
            x.pop()

    class I:
        def __index__(self) -> int:
            return 1

    x = List([1, 2, 3])
    assert x.pop(I()) == 2
    x.insert(I(), 7)
    assert list(x) == [1, 7, 3]


def test_append_demotes_and_keeps_identity() -> None:
    x = List([1, 2])
    o = Obj()
    x.append(o)
    assert x.strategy == "object" and x[2] is o
    y = List([1.5])
    y.append(1)
    assert y.strategy == "object" and type(y[1]) is int
    z = List([1])
    z.append(2**70)
    assert z[1] == 2**70 and z.strategy == "object"


@pytest.mark.parametrize("data", STORAGES)
def test_contains_len_bool(data: list[object]) -> None:
    x = List(data)
    assert len(x) == len(data) and bool(x) and not List()
    for v in [*data, 99, "zz", (1,), float("nan")]:
        assert (v in x) == (v in data)
    assert x.__len__() == len(data) and x.__contains__(data[0])


def test_contains_follows_the_live_list() -> None:
    # Like CPython's list_contains: each step reads the current list.
    x = List([Obj(), Obj(), 3])

    class Shrink:
        def __eq__(self, other: object) -> bool:
            if len(x) > 1:
                del x[-1]
            return False

    assert Shrink() not in x
    assert len(x) == 1


def test_virtual_iteration_is_live() -> None:
    for c in (list, List):
        x = c([1, 2, 3])
        seen = []
        for v in x:
            seen.append(v)
            if v == 1:
                x.append(4)
            if v == 2:
                del x[0]
        assert seen == [1, 2, 4], c
    x = List(range(5))
    assert [v for v in x] == [0, 1, 2, 3, 4]
    assert sum(1 for _ in List()) == 0


@pytest.mark.parametrize("data", STORAGES)
def test_iterators(data: list[object]) -> None:
    x = List(data)
    assert list(iter(x)) == data
    assert list(reversed(x)) == data[::-1]
    it = iter(x)
    assert it.__length_hint__() == len(data)
    assert iter(it) is it
    next(it)
    assert it.__length_hint__() == len(data) - 1
    r = reversed(x)
    next(r)
    assert r.__length_hint__() == len(data) - 1
    assert type(iter(x)).__name__ == "ListIterator"
    assert type(reversed(x)).__name__ == "ListReverseIterator"
    with pytest.raises(TypeError):
        type(iter(x))()
    a, *b = x
    assert [a, *b] == data
    assert list(zip(x, x)) == list(zip(data, data))
    # Many short-lived iterators (the free list) stay independent.
    its = [iter(x) for _ in range(40)]
    assert all(list(i) == data for i in its)


def test_reverse_iterator_stops_when_list_shrinks() -> None:
    for c in (list, List):
        x = c([1, 2, 3, 4])
        r = reversed(x)
        assert next(r) == 4
        del x[1:]
        assert list(r) == []
        x.extend([5, 6, 7])
        assert list(r) == []


@pytest.mark.parametrize("proto", range(pickle.HIGHEST_PROTOCOL + 1))
def test_iterator_pickling(proto: int) -> None:
    orig = List([4, 5, 6, 7])
    data = [10, 11, 12, 13, 14, 15]
    for make, expect in ((iter, lambda k: data[k:]), (reversed, lambda k: data[len(orig) - 1 - k :: -1])):
        it = make(orig)
        for k in range(len(orig)):
            d = pickle.dumps((it, orig), proto)
            it2, a = pickle.loads(d)
            a[:] = data
            assert type(it2) is type(it)
            assert list(it2) == expect(k)
            next(it)
        assert list(it) == []
        it2, a = pickle.loads(pickle.dumps((it, orig), proto))
        assert list(it2) == []


def test_init_reinitialises() -> None:
    a = List([1, 2, 3])
    a.__init__()
    assert list(a) == []
    a.__init__([4, 5, 6])
    assert list(a) == [4, 5, 6]
    a.__init__(a)  # like list: cleared first, then extended with itself
    assert list(a) == []
    t = List[int]([1])
    t.__init__([2, 3])
    assert list(t) == [2, 3] and t.schema_type is List[int]
    with pytest.raises(TypeError):
        t.__init__(["x"])
    with pytest.raises(TypeError, match="takes no keyword arguments"):
        List(a=1)
    with pytest.raises(TypeError, match="expected at most 1 argument"):
        List([1], [2])
    with pytest.raises(TypeError, match="takes no keyword arguments"):
        a.__init__(x=1)
    assert list(List.__new__(List, [1, 2])) == []


def test_subclassing() -> None:
    class Sub(List):
        def extra(self) -> int:
            return len(self) * 2

    s = Sub([1, 2, 3])
    assert type(s) is Sub and isinstance(s, List)
    assert s.extra() == 6 and s[0] == 1 and list(s) == [1, 2, 3]
    s.append(4)
    s[0] = 9
    assert list(s) == [9, 2, 3, 4] and 4 in s
    assert type(s * 1) is List and s * 1 == s and s * 1 is not s
    s.attr = 1  # subclasses get a __dict__

    class Over(List):
        def __getitem__(self, key: object) -> str:
            return str(key) + "!!!"

    o = Over((1, 2))
    assert o[0] == "0!!!"
    assert next(iter(o)) == 1  # iteration reads the storage, like list

    class Init(List):
        def __init__(self, n: int) -> None:
            super().__init__(range(n))

    assert list(Init(3)) == [0, 1, 2]

    freed = []

    class Tracked(List):
        def __del__(self) -> None:
            freed.append(True)

    it = iter(Tracked([1, 2]))
    assert list(it) == [1, 2]
    gc.collect()
    assert freed == [True]


def test_type_is_immutable() -> None:
    with pytest.raises(TypeError):
        List.append = lambda self, v: None  # type: ignore[method-assign]
    assert List.append.__qualname__.endswith("append")


@pytest.mark.parametrize("a", STORAGES)
@pytest.mark.parametrize("b", STORAGES)
def test_comparisons_match_list(a: list[object], b: list[object]) -> None:
    for x, y in ((List(a), List(b)), (List(a), b)):
        for op in ("__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"):
            try:
                expected = getattr(a, op)(b)
            except TypeError:
                with pytest.raises(TypeError):
                    getattr(x, op)(y)
                continue
            assert getattr(x, op)(y) == expected, (a, b, op)
    assert List(a) == List(a) and not (List(a) != List(a))


def test_compare_follows_the_live_lists() -> None:
    x = List([Obj(), Obj(), Obj()])

    class Grow:
        def __eq__(self, other: object) -> bool:
            x.append(Obj())
            return True

    y = List([Grow(), Grow(), Grow()])
    assert (x == y) is False  # lengths compared up front (equal), then items, then lengths


def test_construction_vectorcall() -> None:
    assert list(List()) == []
    assert list(List((1, 2))) == [1, 2]
    assert List(x for x in range(3)) == [0, 1, 2]
    assert List([1, "a"]).strategy == "object"
    assert copy.copy(List([1])) == [1]


def test_refcounts_balanced() -> None:
    o = Obj()
    base = sys.getrefcount(o)
    x = List([o, o])
    for _ in range(100):
        x[0]
        x[0] = o
        _ = o in x
        list(x)
        for v in x:
            pass
        a, b = x
        x.append(o)
        x.pop()
        x.insert(0, o)
        del x[0]
    del a, b, v
    assert sys.getrefcount(o) == base + 2
    del x
    assert sys.getrefcount(o) == base


def test_detached_sort_and_concurrent_mutation() -> None:
    # A big native sort runs detached (the GIL released) holding the storage; other
    # threads that read or mutate the list wait for it, never see a half-sorted column.
    x = List[int](range(400_000, 0, -1))
    errors: list[BaseException] = []
    seen: list[int] = []

    def mutate() -> None:
        try:
            for _ in range(50):
                x.append(0)
                seen.append(x[0])
                x.remove(0)  # wherever a concurrent sort moved it
        except BaseException as e:  # pragma: no cover - reported below
            errors.append(e)

    t = threading.Thread(target=mutate)
    t.start()
    for _ in range(3):
        x.sort()
        x.reverse()
    t.join()
    assert not errors
    assert len(x) == 400_000 and sorted(x) == list(range(1, 400_001))
    assert all(type(v) is int and 0 <= v <= 400_000 for v in seen)


def test_panic_free_errors_from_raw_slots() -> None:
    x = List([1, 2])
    mv = memoryview(x)
    with pytest.raises(BufferError):
        x[0] = 5
    with pytest.raises(BufferError):
        x.append(3)
    with pytest.raises(BufferError):
        x.pop()
    with pytest.raises(BufferError):
        del x[0]
    mv.release()
    x.append(3)
    assert list(x) == [1, 2, 3]


def test_rs_exports_still_there() -> None:
    assert rs.List is List


@pytest.mark.parametrize("data", STORAGES[:4])
def test_element_cache_stays_coherent(data: list[object]) -> None:
    # Repeated element reads enable the boxed element cache; every mutation must keep it
    # coherent with the native storage.
    x, b = List(data * 3), data * 3
    for _ in range(5):
        assert [x[i] for i in range(len(b))] == b
        assert list(x) == b and [v for v in x] == b
    x[0] = data[1]
    b[0] = data[1]
    assert x[0] == b[0] and list(x) == b
    x.append(data[2])
    b.append(data[2])
    assert x[-1] == b[-1] and [v for v in x] == b
    assert x.pop() == b.pop() and x.pop() == b.pop()
    assert list(x) == b
    x.insert(1, data[3])
    b.insert(1, data[3])
    assert [x[i] for i in range(len(b))] == b
    x.sort()
    b.sort()
    assert [v for v in x] == b
    x.reverse()
    b.reverse()
    assert [x[i] for i in range(len(b))] == b
    del x[2:4]
    del b[2:4]
    assert [v for v in x] == b
    x.append(object())  # demotes to object storage
    assert x.strategy == "object" and list(x)[:-1] == b
    x.clear()
    assert list(x) == []


def test_element_cache_identity_and_refcounts() -> None:
    x = List([1000 + i for i in range(10)])
    for _ in range(3):
        for v in x:
            pass
    assert x[3] is x[3]  # cached box (like a builtin list's element)
    v = x[3]
    base = sys.getrefcount(v)
    for _ in range(100):
        x[3]
        for _ in x:
            pass
    assert sys.getrefcount(v) == base
    big = 10**6 + 7
    x[3] = big
    assert x[3] is big  # the assigned object itself, like a builtin list
    s = List(["a", "b"] * 20)
    for _ in range(3):
        list(s)
    word = "".join(["wo", "rd"])
    s[0] = word
    assert s[0] is word
    assert x.pop() == 1009
    t = rs.Tuple(["a", "b"] * 20)
    for _ in range(3):
        list(t)
    assert t[1] is t[1] and t[1] == "b"


def test_element_cache_under_concurrency() -> None:
    # Readers fill the element cache while writers maintain or clear it (free-threaded
    # builds run these truly in parallel).
    x = List(range(1000, 1200))
    t = rs.Tuple(range(1000, 1200))
    stop = threading.Event()
    errors: list[BaseException] = []

    def reader() -> None:
        try:
            while not stop.is_set():
                for v in x:
                    assert type(v) is int
                n = len(x)
                if n:
                    v = x[n // 2] if n // 2 < len(x) else 0
                    assert type(v) is int
                assert sum(t) == sum(range(1000, 1200))
        except IndexError:
            pass
        except BaseException as e:  # pragma: no cover
            errors.append(e)

    def writer() -> None:
        try:
            for k in range(300):
                x.append(k)
                x[0] = k + 5000
                x.pop()
                if k % 50 == 0:
                    x.sort()
                if k % 70 == 0:
                    x.insert(0, 7)
        except BaseException as e:  # pragma: no cover
            errors.append(e)

    readers = [threading.Thread(target=reader) for _ in range(3)]
    w = threading.Thread(target=writer)
    for r in readers:
        r.start()
    w.start()
    w.join()
    stop.set()
    for r in readers:
        r.join()
    assert not errors
    assert len(x) == 205 and all(type(v) is int for v in x)


@pytest.mark.parametrize("data", [[(3, "c"), (1, "a"), (2, "b"), (1, "z")], [3, "a", 2.5], [None, None], [[2], [1], [3]]])
def test_object_sort_is_cpython_timsort(data: list[object]) -> None:
    for kwargs in ({}, {"reverse": True}, {"key": repr}, {"key": repr, "reverse": True}):
        b, x = list(data), List(data)
        try:
            b.sort(**kwargs)
        except TypeError as e:
            with pytest.raises(TypeError, match=re.escape(str(e))):
                x.sort(**kwargs)
            assert sorted(map(repr, x)) == sorted(map(repr, data))  # nothing lost
            continue
        x.sort(**kwargs)
        assert list(x) == b
        assert all(p is q for p, q in zip(x, b))  # the same objects, stable order


def test_object_sort_detects_modification() -> None:
    x = List([(2,), (1,), (3,)])

    def key(v: tuple) -> tuple:
        x.append((0,))
        return v

    with pytest.raises(ValueError, match="list modified during sort"):
        x.sort(key=key)
    assert sorted(x) == [(1,), (2,), (3,)]
