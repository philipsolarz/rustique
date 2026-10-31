"""`List.sort` on object storage goes through `objsort` (CPython's own `list.sort` on a
temporary builtin list): results, stability, key calls, exceptions (the permutation left
behind) and mutation during the sort must be exactly the builtin list's."""

from __future__ import annotations

import random
from functools import cmp_to_key
from typing import Any

import pytest
from _support import import_rustique_module_level
from hypothesis import given
from hypothesis import strategies as st

rs = import_rustique_module_level()


def sort_both(data: list[Any], **kwargs: Any) -> tuple[Any, Any, list[Any], list[Any]]:
    """Sort a builtin list and an (object-storage) List of the same objects."""
    ref = list(data)
    lst = rs.List(data)
    try:
        ref.sort(**kwargs)
        ref_err = None
    except Exception as e:  # noqa: BLE001
        ref_err = (type(e), str(e))
    try:
        lst.sort(**kwargs)
        lst_err = None
    except Exception as e:  # noqa: BLE001
        lst_err = (type(e), str(e))
    return ref_err, lst_err, ref, list(lst)


def assert_same_sort(data: list[Any], **kwargs: Any) -> None:
    ref_err, lst_err, ref, got = sort_both(data, **kwargs)
    assert lst_err == ref_err
    assert len(got) == len(ref)
    if rs.List(data).strategy == "object":
        assert all(a is b for a, b in zip(got, ref)), (got, ref)  # same objects, same order
    else:  # native storage: equal values (re-boxed)
        assert got == ref and [type(a) for a in got] == [type(b) for b in ref]


objects = st.one_of(
    st.tuples(st.integers(-5, 5), st.text(max_size=2)),
    st.tuples(st.integers(-5, 5)),
    st.frozensets(st.integers(0, 3), max_size=2),
)


@given(st.lists(objects, max_size=60), st.booleans())
def test_matches_builtin_sort(data: list[Any], reverse: bool) -> None:
    assert rs.List(data).strategy in ("object", "empty")
    assert_same_sort(data, reverse=reverse)
    assert_same_sort(data, key=lambda t: len(t) if not isinstance(t, tuple) else t[0], reverse=reverse)


@given(st.lists(st.one_of(st.integers(), st.floats(allow_nan=False), st.text(max_size=2), st.none()), max_size=40))
def test_incomparable_leaves_cpythons_permutation(data: list[Any]) -> None:
    assert_same_sort(data)
    assert_same_sort(data, reverse=True)


def test_stability_with_equal_keys() -> None:
    data = [(i % 3, str(i)) for i in range(50)]
    random.Random(1).shuffle(data)
    for reverse in (False, True):
        assert_same_sort(data, key=lambda t: t[0], reverse=reverse)


def test_key_called_once_per_element_in_order() -> None:
    data = [(3,), (1,), (2,)]
    calls: dict[str, list[Any]] = {"ref": [], "rs": []}

    def key(which: str) -> Any:
        def k(v: Any) -> Any:
            calls[which].append(v)
            return v

        return k

    list(data).sort(key=key("ref"))
    rs.List(data).sort(key=key("rs"))
    assert calls["rs"] == calls["ref"] == data


def test_single_element_with_key_calls_key() -> None:
    seen = []
    lst = rs.List([(1,)])
    lst.sort(key=lambda v: seen.append(v) or 0)
    assert seen == [(1,)]
    with pytest.raises(ZeroDivisionError):
        rs.List([(1,)]).sort(key=lambda v: 1 // 0)


def test_comparison_error_midway() -> None:
    class Bad:
        def __init__(self, v: int) -> None:
            self.v = v

        def __lt__(self, other: Any) -> bool:
            if self.v == 7 or other.v == 7:
                raise ValueError("bad compare")
            return self.v < other.v

    data = [Bad(v) for v in [5, 3, 9, 7, 1, 8, 2]]
    assert_same_sort(data)
    assert_same_sort(data, reverse=True)


def test_cmp_to_key() -> None:
    data = [(v,) for v in [5, 3, 9, 7, 1, 8, 2]]
    assert_same_sort(data, key=cmp_to_key(lambda a, b: (a > b) - (a < b)))


def test_mutation_during_sort_is_reported_and_discarded() -> None:
    lst = rs.List([(3,), (1,), (2,)])

    def key(v: Any) -> Any:
        lst.append((0,))
        return v

    with pytest.raises(ValueError, match="list modified during sort"):
        lst.sort(key=key)
    assert list(lst) == [(1,), (2,), (3,)]


def test_list_is_empty_while_sorted() -> None:
    lst = rs.List([(3,), (1,), (2,)])
    seen = []

    def key(v: Any) -> Any:
        seen.append((len(lst), list(lst)))
        return v

    lst.sort(key=key)
    assert seen == [(0, [])] * 3
    assert list(lst) == [(1,), (2,), (3,)]


def test_refcounts_are_preserved() -> None:
    import sys

    objs = [(i, "x" * i) for i in range(20)]
    random.Random(2).shuffle(objs)
    before = [sys.getrefcount(o) for o in objs]
    lst = rs.List(objs)
    lst.sort()
    lst.sort(reverse=True, key=lambda t: t[0])
    del lst
    assert [sys.getrefcount(o) for o in objs] == before


def test_large_object_sort() -> None:
    rng = random.Random(3)
    data = [(rng.randrange(1000), f"s{rng.randrange(10**6)}") for _ in range(50_000)]
    assert_same_sort(data)
    assert_same_sort(data, reverse=True)
    assert_same_sort(data, key=lambda t: t[1])
