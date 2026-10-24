"""Typed containers: schema enforcement and coercion (ARCHITECTURE §3.4) and the
`List[int]`-style alias objects (§3.5)."""

from __future__ import annotations

import copy
import re
import struct
import typing
from typing import Annotated, Any, Optional, Union

import pytest
from _support import assert_same_items, import_rustique_module_level

rs = import_rustique_module_level()
List, Dict, Set = rs.List, rs.Dict, rs.Set

type Num = int
type MaybeStr = str | None


class Point:
    def __init__(self, x: int = 0) -> None:
        self.x = x

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Point) and other.x == self.x

    def __hash__(self) -> int:
        return hash(self.x)


class Point3(Point):
    pass


def msg(container: str, expected: str, got: str, index: int | None = None) -> str:
    """Regex for the documented validation message (index optional when not given)."""
    base = f"{re.escape(container)} expects {re.escape(expected)}, got {re.escape(got)}"
    if index is None:
        return f"^{base}( at index \\d+)?$"
    return f"^{base} at index {index}$"


# ======================================================================================
# Alias objects (§3.5)
# ======================================================================================


class TestAliases:
    def test_identity_stable(self) -> None:
        assert List[int] is List[int]
        assert Dict[str, int] is Dict[str, int]
        assert Set[int] is Set[int]
        assert List[int | None] is List[int | None]
        assert List[List[int]] is List[List[int]]
        assert List[rs.u8] is List[rs.u8]

    def test_equivalent_expressions_share_the_alias(self) -> None:
        assert List[Optional[int]] is List[int | None]
        assert List[Union[int, str]] is List[int | str]
        assert List[list[int]] is List[List[int]]
        assert List[dict[str, int]] is List[Dict[str, int]]
        assert List[set[int]] is List[Set[int]]

    def test_distinct_schemas_distinct_aliases(self) -> None:
        assert List[int] is not List[float]
        assert List[int] is not List[bool]
        assert Dict[str, int] is not Dict[int, str]
        assert List[rs.i32] is not List[rs.i64]

    def test_repr(self) -> None:
        assert repr(List[int]) == "List[int]"
        assert repr(List[float]) == "List[float]"
        assert repr(List[bool]) == "List[bool]"
        assert repr(List[str]) == "List[str]"
        assert repr(Dict[str, int]) == "Dict[str, int]"
        assert repr(Set[int]) == "Set[int]"
        assert repr(List[int | None]) == "List[int | None]"
        assert repr(List[rs.u8]) == "List[u8]"
        assert repr(List[rs.f32]) == "List[f32]"
        assert repr(List[List[int]]) == "List[List[int]]"
        assert repr(Dict[str, List[float]]) == "Dict[str, List[float]]"
        assert repr(List[Point]) == "List[Point]"
        assert repr(List) == "<class 'rustique.List'>"

    def test_origin_args_and_schema(self) -> None:
        assert List[int].__origin__ is List
        assert List[int].__args__ == (int,)
        assert Dict[str, int].__origin__ is Dict
        assert Dict[str, int].__args__ == (str, int)
        assert Set[float].__origin__ is Set
        assert Set[float].__args__ == (float,)
        for alias in (List[int], Dict[str, float], Set[str], List[int | None]):
            assert isinstance(alias.schema, str) and alias.schema
        assert "int" in List[int].schema

    def test_callable_constructs_instances_of_the_single_class(self) -> None:
        lst = List[int]([1, 2])
        assert type(lst) is List
        assert lst == [1, 2]
        assert type(List[int]()) is List
        assert len(List[int]()) == 0
        assert type(Dict[str, int]({"a": 1})) is Dict
        assert type(Set[int]([1])) is Set
        with pytest.raises(TypeError):
            List[int]([1], [2])

    def test_schema_type(self) -> None:
        assert List[int]([1]).schema_type is List[int]
        assert List([1]).schema_type is List
        assert List().schema_type is List
        assert List[int | None]().schema_type is List[int | None]
        assert Dict[str, int]().schema_type is Dict[str, int]
        assert Dict().schema_type is Dict
        assert Set[int]().schema_type is Set[int]
        assert Set().schema_type is Set

    def test_isinstance(self) -> None:
        typed = List[int]([1])
        assert isinstance(typed, List[int])
        assert isinstance(typed, List)
        assert not isinstance(typed, List[float])
        assert not isinstance(typed, List[int | None])
        assert not isinstance(List([1]), List[int])
        assert not isinstance([1], List[int])
        assert not isinstance(Set[int]([1]), List[int])
        assert isinstance(Dict[str, int](), Dict[str, int])
        assert not isinstance(Dict[str, int](), Dict[str, float])
        assert not isinstance({"a": 1}, Dict[str, int])
        assert isinstance(Set[int](), Set[int])
        assert not isinstance(Set[int](), Set[str])
        assert isinstance(List[List[int]](), List[List[int]])

    def test_any_schema_spellings(self) -> None:
        for spelling in (object, typing.Any):
            lst = List[spelling]([1, "a", None])
            assert lst == [1, "a", None]
            assert lst.schema_type is List  # the Any schema is the bare class

    def test_annotated_and_type_aliases(self) -> None:
        assert repr(List[Annotated[int, "meta"]]) == "List[int]"
        assert repr(List[Num]) == "List[int]"
        assert repr(List[MaybeStr]) == "List[str | None]"
        lst = List[Num]([1, 2])
        assert lst.schema_type is List[int]
        with pytest.raises(TypeError):
            List[Annotated[int, "meta"]](["x"])

    def test_invalid_type_expressions(self) -> None:
        with pytest.raises(TypeError):
            List[1]  # type: ignore[valid-type]
        with pytest.raises(TypeError):
            Dict[int]  # type: ignore[misc]
        with pytest.raises(TypeError):
            Dict[int, str, float]  # type: ignore[misc]


# ======================================================================================
# Coercion per schema (§3.4)
# ======================================================================================


class TestIntSchema:
    def test_accepts_ints_and_bools(self) -> None:
        lst = List[int]([1, -5, 0, True, False, 2**63 - 1, -(2**63)])
        assert_same_items(lst, [1, -5, 0, 1, 0, 2**63 - 1, -(2**63)])
        assert lst.strategy == "i64"

    def test_bool_is_stored_as_int(self) -> None:
        lst = List[int]([True])
        assert type(lst[0]) is int and lst[0] == 1
        lst.append(False)
        lst[0] = True
        lst.insert(0, True)
        lst.extend([False])
        assert_same_items(lst, [1, 1, 0, 0])

    def test_big_ints_demote_but_stay_validated(self) -> None:
        lst = List[int]([1, 2])
        lst.append(2**64)
        assert lst.strategy == "object"
        assert_same_items(lst, [1, 2, 2**64])
        with pytest.raises(TypeError):
            lst.append("x")
        with pytest.raises(TypeError):
            lst.append(1.5)
        lst.append(True)
        assert_same_items(lst, [1, 2, 2**64, 1])
        assert lst.schema_type is List[int]
        big = List[int]([-(2**100)])
        assert big.strategy == "object" and big[0] == -(2**100)

    @pytest.mark.parametrize("bad", [1.0, 1.5, "1", None, b"1", [1], (1,), 1j])
    def test_rejects(self, bad: Any) -> None:
        with pytest.raises(TypeError, match=msg("List[int]", "int", type(bad).__name__, 1)):
            List[int]([1, bad])

    def test_message_format(self) -> None:
        with pytest.raises(TypeError, match=msg("List[int]", "int", "str", 3)):
            List[int]([0, 1, 2, "x"])
        lst = List[int]([1, 2, 3])
        with pytest.raises(TypeError, match=msg("List[int]", "int", "str")):
            lst.append("x")
        with pytest.raises(TypeError, match=msg("List[int]", "int", "float")):
            lst[0] = 1.5
        with pytest.raises(TypeError, match=msg("List[int]", "int", "NoneType")):
            lst.insert(0, None)
        assert_same_items(lst, [1, 2, 3])


class TestFloatSchema:
    def test_accepts_floats_and_converts_ints(self) -> None:
        lst = List[float]([1.5, 2, -3, 0, 4])
        assert_same_items(lst, [1.5, 2.0, -3.0, 0.0, 4.0])
        assert lst.strategy == "f64"
        lst.append(7)
        lst[0] = 9
        lst.insert(0, 10)
        lst.extend([11])
        lst[1:2] = [12]
        assert all(type(x) is float for x in lst)
        assert lst.to_list() == [10.0, 12.0, 2.0, -3.0, 0.0, 4.0, 7.0, 11.0]

    def test_special_values(self) -> None:
        lst = List[float]([float("inf"), float("-inf"), -0.0, float("nan")])
        got = lst.to_list()
        assert got[0] == float("inf") and got[1] == float("-inf")
        assert str(got[2]) == "-0.0"
        assert got[3] != got[3]

    def test_huge_int_overflows(self) -> None:
        with pytest.raises(OverflowError):
            List[float]([2**1024])
        lst = List[float]()
        with pytest.raises(OverflowError):
            lst.append(-(2**2000))
        assert len(lst) == 0

    @pytest.mark.parametrize("bad", ["1.0", None, b"x", [1.0], 1j])
    def test_rejects(self, bad: Any) -> None:
        with pytest.raises(TypeError, match=msg("List[float]", "float", type(bad).__name__, 0)):
            List[float]([bad])


class TestBoolStrSchemas:
    def test_bool_only_bools(self) -> None:
        lst = List[bool]([True, False])
        assert lst.strategy == "bool"
        assert_same_items(lst, [True, False])
        for bad in (1, 0, 1.0, "True", None):
            with pytest.raises(TypeError, match=msg("List[bool]", "bool", type(bad).__name__)):
                lst.append(bad)
        assert_same_items(lst, [True, False])

    def test_str_only_strs(self) -> None:
        lst = List[str](["a", "", "é", "😀"])
        assert lst.strategy == "str"
        assert_same_items(lst, ["a", "", "é", "😀"])
        for bad in (b"a", 1, None, ["a"]):
            with pytest.raises(TypeError, match=msg("List[str]", "str", type(bad).__name__)):
                lst.append(bad)

    def test_str_with_lone_surrogates_round_trips(self) -> None:
        """Lone surrogates are valid Python `str` values; a typed `List[str]` must accept
        and return them unchanged (they cannot live in native UTF-8 storage)."""
        lst = List[str](["a", "\ud800", "b\udfff"])
        assert_same_items(lst, ["a", "\ud800", "b\udfff"])
        lst.append("\udc00")
        assert lst[-1] == "\udc00"


FIXED = [
    ("i8", -(2**7), 2**7 - 1),
    ("i16", -(2**15), 2**15 - 1),
    ("i32", -(2**31), 2**31 - 1),
    ("i64", -(2**63), 2**63 - 1),
    ("u8", 0, 2**8 - 1),
    ("u16", 0, 2**16 - 1),
    ("u32", 0, 2**32 - 1),
    ("u64", 0, 2**64 - 1),
]


class TestFixedSchemas:
    @pytest.mark.parametrize("name,lo,hi", FIXED)
    def test_range(self, name: str, lo: int, hi: int) -> None:
        t = getattr(rs, name)
        lst = List[t]([lo, 0, hi])
        assert lst.strategy == name
        assert_same_items(lst, [lo, 0, hi])
        assert all(type(x) is int for x in lst)
        assert repr(lst) == f"List[{name}]([{lo}, 0, {hi}])"
        for bad in (lo - 1, hi + 1):
            with pytest.raises(OverflowError):
                List[t]([bad])
            with pytest.raises(OverflowError):
                lst.append(bad)
            with pytest.raises(OverflowError):
                lst[0] = bad
        assert_same_items(lst, [lo, 0, hi])
        for bad in ("1", None, 1.0):
            with pytest.raises(TypeError):
                lst.append(bad)

    @pytest.mark.parametrize("name,lo,hi", FIXED)
    def test_accepts_matching_scalar(self, name: str, lo: int, hi: int) -> None:
        t = getattr(rs, name)
        lst = List[t]([t(hi), t(lo)])
        assert_same_items(lst, [hi, lo])

    def test_f32_rounds(self) -> None:
        lst = List[rs.f32]([0.1, 1, 2.5])
        assert lst.strategy == "f32"
        f32_01 = struct.unpack("f", struct.pack("f", 0.1))[0]
        assert_same_items(lst, [f32_01, 1.0, 2.5])
        assert repr(lst) == f"List[f32]([{f32_01!r}, 1.0, 2.5])"

    def test_f64(self) -> None:
        lst = List[rs.f64]([0.1, 1])
        assert lst.strategy == "f64"
        assert_same_items(lst, [0.1, 1.0])
        with pytest.raises(TypeError):
            lst.append("0.1")

    def test_arithmetic_results_are_python_ints(self) -> None:
        lst = List[rs.u8]([200, 100])
        assert lst.sum() == 300  # Python semantics: the sum is an exact Python int
        assert type(lst[0]) is int


class TestOptionalUnion:
    def test_optional(self) -> None:
        for alias in (List[int | None], List[Optional[int]]):
            lst = alias([1, None, True])
            assert lst.strategy == "object"
            assert_same_items(lst, [1, None, 1])
            with pytest.raises(TypeError, match=msg("List[int | None]", "int | None", "str", 0)):
                alias(["x"])
            with pytest.raises(TypeError):
                lst.append(1.5)

    def test_union_accepts_any_member(self) -> None:
        lst = List[int | str]([1, "a", 2])
        assert_same_items(lst, [1, "a", 2])
        for bad in (1.5, None, b"a"):
            with pytest.raises(TypeError):
                lst.append(bad)

    def test_union_first_match_converts(self) -> None:
        assert_same_items(List[float | str]([1, "a"]), [1.0, "a"])
        assert_same_items(List[str | float]([1]), [1.0])
        assert_same_items(List[int | float]([True, 1.5]), [1, 1.5])

    def test_union_with_classes(self) -> None:
        p = Point(1)
        lst = List[Point | None]([p, None, Point3(2)])
        assert lst[0] is p
        with pytest.raises(TypeError):
            lst.append(1)


class TestNested:
    def test_list_of_lists_converts_iterables(self) -> None:
        lst = List[List[int]]([[1, 2], (3,), range(2), List([4])])
        assert all(type(x) is List for x in lst)
        assert all(x.schema_type is List[int] for x in lst)
        assert lst == [[1, 2], [3], [0, 1], [4]]
        assert lst.strategy == "object"

    def test_identical_schema_is_stored_as_is(self) -> None:
        inner = List[int]([1, 2])
        outer = List[List[int]]([inner])
        assert outer[0] is inner
        inner.append(3)
        assert outer[0] == [1, 2, 3]

    def test_untyped_inner_is_converted(self) -> None:
        inner = List([1, 2])
        outer = List[List[int]]([inner])
        assert outer[0] is not inner
        assert outer[0].schema_type is List[int]

    def test_nested_rejections(self) -> None:
        with pytest.raises(TypeError):
            List[List[int]]([["a"]])
        with pytest.raises(TypeError):
            List[List[int]]([1])
        with pytest.raises(TypeError):
            List[List[int]]([List[float]([1.5])])
        outer = List[List[int]]()
        with pytest.raises(TypeError):
            outer.append([1, "x"])
        assert len(outer) == 0

    def test_lowercase_generic_aliases(self) -> None:
        lst = List[list[int]]([[1]])
        assert type(lst[0]) is List and lst[0].schema_type is List[int]

    def test_dict_and_set_elements(self) -> None:
        lst = List[Dict[str, int]]([{"a": 1}, Dict[str, int]({"b": 2})])
        assert all(type(x) is Dict and x.schema_type is Dict[str, int] for x in lst)
        assert lst[0] == {"a": 1}
        with pytest.raises(TypeError):
            lst.append({"a": "x"})
        with pytest.raises(TypeError):
            lst.append({1: 1})
        sets = List[Set[int]]([{1, 2}, [3, 3]])
        assert all(type(x) is Set for x in sets)
        assert sets[1] == {3}

    def test_deep_nesting(self) -> None:
        lst = List[List[List[int]]]([[[1], [2, 3]], []])
        assert lst[0][1].schema_type is List[int]
        assert lst == [[[1], [2, 3]], []]
        with pytest.raises(TypeError):
            lst.append([[1.5]])

    def test_dict_of_lists(self) -> None:
        d = Dict[str, List[float]]({"a": [1, 2.5]})
        assert type(d["a"]) is List and d["a"].schema_type is List[float]
        assert_same_items(d["a"], [1.0, 2.5])


class TestClassSchema:
    def test_isinstance_check(self) -> None:
        p, q = Point(1), Point3(2)
        lst = List[Point]([p, q])
        assert lst[0] is p and lst[1] is q
        assert lst.strategy == "object"
        with pytest.raises(TypeError, match=msg("List[Point]", "Point", "int", 0)):
            List[Point]([1])
        with pytest.raises(TypeError, match=msg("List[Point]", "Point", "NoneType")):
            lst.append(None)

    def test_builtin_class_schema(self) -> None:
        lst = List[bytes]([b"a"])
        assert lst[0] == b"a"
        with pytest.raises(TypeError):
            lst.append("a")
        lst2 = List[tuple]([(1,), ()])
        assert lst2 == [(1,), ()]


# ======================================================================================
# Schema propagation through operations
# ======================================================================================


class TestSchemaPropagation:
    def test_derived_lists_keep_the_schema(self) -> None:
        lst = List[int]([3, 1, 2])
        for derived in (lst[1:], lst[::-1], lst.copy(), copy.copy(lst), copy.deepcopy(lst), lst * 2, 2 * lst, lst.filter(lambda x: x > 1)):
            assert derived.schema_type is List[int]
        lst.sort()
        assert lst.schema_type is List[int]

    def test_add_schemas(self) -> None:
        a = List[int]([1])
        assert (a + List[int]([2])).schema_type is List[int]
        assert (a + List[float]([2.0])).schema_type is List
        assert (a + [2]).schema_type is List
        assert ([2] + a).schema_type is List
        assert (a + List([2])).schema_type is List
        assert (a + List[float]([2.5])) == [1, 2.5]

    def test_rejected_mutations_leave_the_list_unchanged(self) -> None:
        lst = List[int]([1, 2, 3])
        ops = [
            lambda: lst.append("x"),
            lambda: lst.insert(0, "x"),
            lambda: lst.extend(["x"]),
            lambda: lst.__iadd__(["x"]),
            lambda: lst.__setitem__(0, "x"),
            lambda: lst.__setitem__(slice(0, 1), ["x"]),
            lambda: lst.__setitem__(slice(0, 2), [1, "x"]),
            lambda: lst.__setitem__(slice(None, None, 2), ["x", "y"]),
        ]
        for op in ops:
            with pytest.raises(TypeError):
                op()
            assert_same_items(lst, [1, 2, 3])
            assert lst.strategy == "i64"

    def test_failed_typed_extend_keeps_list_consistent(self) -> None:
        lst = List[int]([1])
        with pytest.raises(TypeError):
            lst.extend([2, 3, "x", 4])
        assert lst.to_list() in ([1], [1, 2, 3])
        assert all(type(x) is int for x in lst)

    def test_typed_float_mutations_convert(self) -> None:
        lst = List[float]([1.0])
        lst += [2]
        lst *= 2
        assert_same_items(lst, [1.0, 2.0, 1.0, 2.0])


# ======================================================================================
# Typed Dict and Set
# ======================================================================================


class TestTypedDict:
    def test_basic_and_repr(self) -> None:
        d = Dict[str, int]({"a": 1}, b=2)
        assert d == {"a": 1, "b": 2}
        assert repr(d) == "Dict[str, int]({'a': 1, 'b': 2})"
        assert repr(Dict({"a": 1})) == "Dict({'a': 1})"

    def test_validation(self) -> None:
        d = Dict[str, int]()
        with pytest.raises(TypeError):
            d[1] = 1
        with pytest.raises(TypeError):
            d["a"] = "x"
        with pytest.raises(TypeError):
            Dict[str, int]({"a": 1.5})
        with pytest.raises(TypeError):
            d.update({"a": None})
        with pytest.raises(TypeError):
            d.setdefault(1, 1)
        assert len(d) == 0

    def test_coercion(self) -> None:
        d = Dict[int, float]({1: 2, True: 3})  # noqa: F601 -- deliberate key collision
        assert list(d.items()) == [(1, 3.0)]
        assert type(next(iter(d))) is int
        assert type(d[1]) is float
        f = Dict[float, int]({1: 1})
        assert type(next(iter(f))) is float

    def test_lookup_with_foreign_key_is_a_miss(self) -> None:
        d = Dict[str, int]({"a": 1})
        with pytest.raises(KeyError):
            d[1]
        assert 1 not in d
        assert None not in d
        assert d.get(1) is None
        assert d.get(1, "x") == "x"
        assert d.pop(1, "x") == "x"
        with pytest.raises(KeyError):
            d.pop(1)
        with pytest.raises(KeyError):
            del d[1]
        assert d == {"a": 1}

    def test_unhashable_key_raises_type_error(self) -> None:
        d = Dict[str, int]({"a": 1})
        with pytest.raises(TypeError):
            d[["a"]]
        with pytest.raises(TypeError):
            ["a"] in d  # noqa: B015
        with pytest.raises(TypeError):
            d.get(["a"])

    def test_numeric_lookup_never_raises(self) -> None:
        """Whether `1.0` finds the int key `1` in a `Dict[int, V]` is not pinned down by the
        spec (numeric equivalence vs. "unconvertible key is a miss"); it must be consistent
        and must never raise."""
        d = Dict[int, str]({1: "a"})
        found = 1.0 in d
        assert d.get(1.0) == ("a" if found else None)
        assert True in d  # bool converts to the int schema

    def test_schema_preserved(self) -> None:
        d = Dict[str, int]({"a": 1})
        assert d.copy().schema_type is Dict[str, int]
        assert copy.deepcopy(d).schema_type is Dict[str, int]
        assert (d | Dict[str, int]({"b": 2})).schema_type is Dict[str, int]
        # Mixing with an untyped operand follows the List `+` rule (result may be untyped).
        assert d | {"b": 2} == {"a": 1, "b": 2}


class TestTypedSet:
    def test_basic_and_repr(self) -> None:
        s = Set[int]([2, 1, 2])
        assert s == {1, 2}
        assert repr(s) in ("Set[int]({1, 2})", "Set[int]({2, 1})")
        assert repr(Set[int]([5])) == "Set[int]({5})"
        assert repr(Set([5])) == "Set({5})"

    def test_validation_and_coercion(self) -> None:
        s = Set[int]()
        with pytest.raises(TypeError):
            s.add("x")
        with pytest.raises(TypeError):
            s.update([1, "x"])
        s.add(True)
        assert type(next(iter(s))) is int
        f = Set[float]([1])
        assert type(next(iter(f))) is float

    def test_lookup_with_foreign_value_is_a_miss(self) -> None:
        s = Set[int]([1])
        assert "x" not in s
        s.discard("x")
        with pytest.raises(KeyError):
            s.remove("x")
        with pytest.raises(TypeError):
            [1] in s  # noqa: B015
        assert s == {1}

    def test_schema_preserved(self) -> None:
        s = Set[int]([1, 2])
        for derived in (s.copy(), copy.copy(s), copy.deepcopy(s), s | Set[int]([3])):
            assert derived.schema_type is Set[int]
