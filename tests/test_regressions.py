"""Regression tests for bugs catalogued in the historical branches (docs/ARCHAEOLOGY.md §5).

Each test names the historical defect it guards against.
"""

import math
import pickle

import pytest

import rustique as rs
from rustique import Dict, List, Set, X


@pytest.mark.parametrize("value", [128, 255, 256, -1, -128, -129, 2**31, 2**63 - 1, -(2**63), 2**63, -(2**63) - 1, 2**200])
def test_int_round_trip_across_byte_and_sign_boundaries(value):
    # engine3 `Int(128)` raised OverflowError and `Int(-1)` became 255 (sign-bit sizing bug).
    for lst in (List([value]), List[int]([value])):
        assert lst[0] == value
        assert type(lst[0]) is int
        assert lst.to_list() == [value]


def test_fixed_width_floor_division_and_modulo_follow_python():
    # main `i8(-7)//2` gave -3 and `%` gave -1 (truncating semantics leaked through operators).
    assert rs.i8(-7) // 2 == -4
    assert rs.i8(-7) % 2 == 1
    assert rs.i8(-7).checked_div(2) == -3  # Rust semantics stay available by name


def test_fixed_width_arithmetic_keeps_its_type():
    # main: `type(i8(42) + 1)` was `int`, so the Rust type was lost after one operation.
    assert type(rs.i8(42) + 1) is rs.i8
    with pytest.raises(OverflowError):
        rs.i8(127) + 1


def test_scalar_hash_matches_python():
    # engine3 hashed Int via `to_u64_wrapping`; main primitives were unhashable.
    for value in (0, -1, 2**62, 2**63 - 1):
        assert hash(rs.i64(value)) == hash(value)
    assert hash(rs.f64(0.5)) == hash(0.5)
    assert {rs.i64(5): "x"}[5] == "x"


def test_str_elements_interoperate_with_builtin_hashing():
    # engine3 `Str('a') in {'a'}` was False (DefaultHasher).
    lst = List(["a", "b"])
    assert lst[0] in {"a"}
    assert "a" in Set(["a"])
    assert Dict({"a": 1})["a"] == 1


def test_map_does_not_consume_the_source():
    # hashmap/engine `collect()` emptied the list via `mem::take`, invalidating benchmarks.
    lst = List([1, 2, 3])
    assert lst.map(X * 2).to_list() == [2, 4, 6]
    assert lst.map(X * 2).to_list() == [2, 4, 6]
    assert lst.to_list() == [1, 2, 3]


def test_reversed_empty_slices_do_not_panic():
    # engine `L[3:1]` raised PanicException; `__setitem__`/`__delitem__` too.
    lst = List([1, 2, 3, 4, 5])
    assert lst[3:1].to_list() == []
    del lst[3:1]
    assert lst.to_list() == [1, 2, 3, 4, 5]
    lst[3:1] = [7]
    assert lst.to_list() == [1, 2, 3, 7, 4, 5]


def test_negative_index():
    # rapid-development `v[-1]` panicked with index 18446744073709551615.
    lst = List([1, 2, 3])
    assert lst[-1] == 3
    with pytest.raises(IndexError):
        lst[-4]


def test_nan_identity_in_containers():
    # hashmap `List([nan]) == [nan]` was False; `nan in List([nan])` was False.
    nan = float("nan")
    lst = List([nan])
    assert nan in lst
    assert lst == [nan]
    assert lst.count(nan) == 1
    assert lst.index(nan) == 0
    assert math.isnan(lst[0])


def test_dict_delete_preserves_insertion_order():
    # hashmap Dict used `IndexMap::swap_remove`, reordering entries on deletion.
    d = Dict({"a": 1, "b": 2, "c": 3, "d": 4})
    del d["a"]
    d.pop("c")
    assert list(d) == ["b", "d"]


def test_dict_pop_missing_raises_key_error():
    # engine3 dict.rs `pop` raised ValueError.
    with pytest.raises(KeyError):
        Dict({"a": 1}).pop("missing")


def test_unhashable_keys_are_rejected():
    # hashmap `Key::hash` only `eprintln!`-ed failures and accepted unhashable keys.
    with pytest.raises(TypeError):
        Dict()[[1, 2]] = 3
    with pytest.raises(TypeError):
        Set([[1]])


def test_typed_list_rejects_wrong_types_with_context():
    # main's typed List was correct here; keep the informative message.
    with pytest.raises(TypeError, match=r"List\[int\] expects int, got str at index 1"):
        List[int]([1, "x"])


def test_typed_alias_identity_and_isinstance():
    # main created a new class per subscript: `List[int] is List[int]` and isinstance failed.
    assert List[int] is List[int]
    assert isinstance(List[int]([1]), List[int])
    assert not isinstance(List[str](["a"]), List[int])


def test_nested_typing():
    # README TODO on every branch: "Add nested lists and nested typings".
    nested = List[List[int]]([[1, 2], [3]])
    assert isinstance(nested[0], List[int])
    assert nested[0].to_list() == [1, 2]
    with pytest.raises(TypeError):
        List[List[int]]([[1, "x"]])
    optional = List[int | None]([1, None, 3])
    assert optional.to_list() == [1, None, 3]


def test_equality_with_foreign_types_returns_not_implemented():
    # engine3 `Int(1) == 'x'` raised; `List.__eq__` returned False instead of NotImplemented.
    lst = List([1])
    assert (lst == "x") is False
    assert lst.__eq__("x") is NotImplemented


def test_pickle_round_trip():
    lst = List[int]([1, 2, 3])
    clone = pickle.loads(pickle.dumps(lst))
    assert clone == lst
    assert isinstance(clone, List[int])


def test_deeply_composed_aliases_raise_instead_of_crashing():
    # Safety review C2: composing aliases one level at a time bypassed the nesting limit
    # and overflowed the Rust stack (SIGSEGV) in repr/compile.
    alias = int
    with pytest.raises(RecursionError):
        for _ in range(10_000):
            alias = List[alias]
    assert repr(alias).startswith("List[")
