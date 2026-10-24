"""Type expressions → schemas, and the `List[T]` / `Dict[K, V]` / `Set[T]` aliases.

ARCHITECTURE §3.4–3.5. Schemas are observed through `rustique._rustique._schema_repr`,
which compiles a type expression and renders the resulting schema.
"""

import collections.abc
import copy
import pickle
import threading
import typing
from typing import Annotated, Any, NewType, Optional, TypeVar, Union

import pytest

from rustique import Dict, List, Set
from rustique import _rustique as native
from rustique._rustique import DictType, ListType, SetType
from rustique._rustique import _schema_repr as schema_of


class Point:
    pass


class SubInt(int):
    pass


class Gen[X]:
    pass


type IntList = list[int]
type IntOrStr = int | str
type MaybeInt = int | None
type Pair[K, V] = dict[K, V]
type WithDefault[A, B = float] = dict[A, B]
type Box[E] = list[E]
type Tree = list[Tree]
type Unresolved = NameThatIsNeverDefined  # noqa: F821
type Variadic[*Ts] = tuple[*Ts]

UserId = NewType("UserId", int)
AdminId = NewType("AdminId", UserId)
PointId = NewType("PointId", Point)
T = TypeVar("T")
Bounded = TypeVar("Bounded", bound=int)
Constrained = TypeVar("Constrained", int, str)


# ---------------------------------------------------------------------------------------
# compile: supported forms
# ---------------------------------------------------------------------------------------

SUPPORTED = [
    # scalars and Any
    (int, "int"),
    (float, "float"),
    (bool, "bool"),
    (str, "str"),
    (object, "Any"),
    (Any, "Any"),
    # None and classes
    (None, "None"),
    (type(None), "None"),
    (Point, "Point"),
    (SubInt, "SubInt"),
    (bytes, "bytes"),
    (frozenset, "frozenset"),
    (collections.abc.Sequence, "Sequence"),
    (typing.Sequence, "Sequence"),
    (Gen, "Gen"),
    # optionals
    (int | None, "int | None"),
    (Optional[int], "int | None"),
    (None | int, "int | None"),
    (Union[None, int], "int | None"),
    (Optional[None], "None"),
    # unions keep declaration order
    (int | str, "int | str"),
    (str | int, "str | int"),
    (Union[int, str], "int | str"),
    (Union[str, int], "str | int"),
    (int | str | None, "int | str | None"),
    (None | int | str, "int | str | None"),
    (Optional[Union[int, str]], "int | str | None"),
    (bool | int | float, "bool | int | float"),
    (Point | None, "Point | None"),
    # Any absorbs a union
    (Any | int, "Any"),
    (Optional[Any], "Any"),
    (int | object, "Any"),
    # containers
    (list[int], "List[int]"),
    (typing.List[int], "List[int]"),
    (list, "List"),
    (typing.List, "List"),
    (List, "List"),
    (list[Any], "List"),
    (list[list], "List[List]"),
    (list[int | None], "List[int | None]"),
    (dict[str, int], "Dict[str, int]"),
    (typing.Dict[str, float], "Dict[str, float]"),
    (dict, "Dict"),
    (typing.Dict, "Dict"),
    (Dict, "Dict"),
    (dict[str, Any], "Dict[str, Any]"),
    (dict[Any, int], "Dict[Any, int]"),
    (dict[str, list[float]], "Dict[str, List[float]]"),
    (set[int], "Set[int]"),
    (typing.Set[str], "Set[str]"),
    (set, "Set"),
    (Set, "Set"),
    (list[dict[str, set[Point | None]]], "List[Dict[str, Set[Point | None]]]"),
    (list[int] | None, "List[int] | None"),
    # Annotated is transparent
    (Annotated[int, "meta"], "int"),
    (Annotated[list[int], "meta", 5], "List[int]"),
    (Annotated[int, []], "int"),  # unhashable metadata
    (list[Annotated[str, "x"]], "List[str]"),
    # PEP 695 aliases
    (IntList, "List[int]"),
    (list[IntList], "List[List[int]]"),
    (Pair[int, str], "Dict[int, str]"),
    (Pair, "Dict"),
    (WithDefault[int], "Dict[int, float]"),
    (WithDefault[int, bool], "Dict[int, bool]"),
    (Box[Box[int]], "List[List[int]]"),
    (Box[int | None], "List[int | None]"),
    # NewType resolves to its supertype
    (UserId, "int"),
    (AdminId, "int"),
    (PointId, "Point"),
    # type variables are erased
    (T, "Any"),
    (Bounded, "Any"),
    (Constrained, "Any"),
    (list[T], "List"),
    (dict[str, T], "Dict[str, Any]"),
]


@pytest.mark.parametrize(("expr", "expected"), SUPPORTED, ids=[repr(e) for e, _ in SUPPORTED])
def test_compile(expr, expected):
    assert schema_of(expr) == expected
    assert schema_of(expr) == expected  # second call goes through the caches


def test_union_flattening_through_aliases():
    assert schema_of(IntOrStr) == "int | str"
    assert schema_of(IntOrStr | float) == "int | str | float"
    assert schema_of(float | IntOrStr) == "float | int | str"
    assert schema_of(IntOrStr | int) == "int | str"
    assert schema_of(MaybeInt | str) == "int | str | None"
    assert schema_of(Optional[MaybeInt]) == "int | None"
    assert schema_of(UserId | int) == "int"
    assert schema_of(Annotated[int, "x"] | int) == "int"


def test_union_order_is_not_confused_by_caching():
    # Python considers these pairs equal (and hashes them equally); schemas differ.
    for first, second in [(int | float, float | int), (list[int | str], list[str | int])]:
        assert first == second
        a, b = schema_of(first), schema_of(second)
        assert a != b
        assert schema_of(first) == a
        assert schema_of(second) == b


def test_cache_ignores_classes_with_custom_equality():
    class Meta(type):
        def __eq__(cls, other):
            return isinstance(other, Meta)

        def __hash__(cls):
            return 42

    class A(metaclass=Meta):
        pass

    class B(metaclass=Meta):
        pass

    assert A == B and hash(A) == hash(B)
    a, b = A.__qualname__, B.__qualname__
    assert schema_of(list[A]) == f"List[{a}]"
    assert schema_of(list[B]) == f"List[{b}]"
    assert schema_of(A) == a
    assert schema_of(B) == b


def test_abc_metaclass_classes_compile():
    assert schema_of(collections.abc.Mapping) == "Mapping"
    assert schema_of(list[collections.abc.Hashable]) == "List[Hashable]"


@pytest.mark.skipif(not hasattr(native, "u8"), reason="fixed-width scalars not available")
def test_scalar_element_types():
    assert schema_of(native.u8) == "u8"
    assert schema_of(native.f32 | None) == "f32 | None"
    assert schema_of(list[native.i16]) == "List[i16]"
    assert List[native.i64] is List[native.i64]
    assert List[native.i64] is not List[int]
    assert repr(Dict[str, native.f64]) == "Dict[str, f64]"
    assert List[native.u8].__args__ == (native.u8,)
    assert pickle.loads(pickle.dumps(List[native.u64])) is List[native.u64]


# ---------------------------------------------------------------------------------------
# compile: unsupported forms
# ---------------------------------------------------------------------------------------

UNSUPPORTED = [
    ("int", "forward reference"),
    (typing.ForwardRef("int"), "forward reference"),
    (Optional["Point"], "forward reference"),
    (typing.Literal[1, 2], "Literal"),
    (typing.Callable[[int], int], "cannot be checked at runtime"),
    (tuple[int, ...], "use the bare class tuple"),
    (Gen[int], "use the bare class"),
    (collections.abc.Sequence[int], "cannot be checked at runtime"),
    (5, "not a supported element type"),
    (typing.Never, "not a supported element type"),
    (typing.Self, "not a supported element type"),
    (list[int, str], "expects 1 type argument, got 2"),
    (dict[int], "expects 2 type arguments, got 1"),
    (typing.ParamSpec("P"), "ParamSpec"),
    (typing.TypeVarTuple("Ts"), "TypeVarTuple"),
    (Tree, "recursive type alias Tree"),
    (Pair[int, str, float], "type alias Pair expects 2 type arguments, got 3"),
    (Variadic[int], "ParamSpec or TypeVarTuple"),
    (list[Variadic[int]], "ParamSpec or TypeVarTuple"),
    (int | typing.Literal["x"], "Literal"),
]


@pytest.mark.parametrize(("expr", "message"), UNSUPPORTED, ids=[repr(e) for e, _ in UNSUPPORTED])
def test_unsupported_raises_type_error(expr, message):
    with pytest.raises(TypeError, match=message):
        schema_of(expr)


def test_unresolvable_alias_reports_the_alias():
    with pytest.raises(TypeError, match="could not resolve type alias Unresolved") as info:
        schema_of(Unresolved)
    assert isinstance(info.value.__cause__, NameError)


def test_errors_are_not_cached():
    for _ in range(2):
        with pytest.raises(TypeError):
            schema_of(list["int"])


def test_deep_nesting_raises_recursion_error():
    expr = int
    for _ in range(500):
        expr = list[expr]
    with pytest.raises(RecursionError):
        schema_of(expr)
    shallow = int
    for _ in range(50):
        shallow = list[shallow]
    assert schema_of(shallow) == "List[" * 50 + "int" + "]" * 50


# ---------------------------------------------------------------------------------------
# Aliases: identity, repr, introspection
# ---------------------------------------------------------------------------------------


def test_alias_types():
    assert type(List[int]) is ListType
    assert type(Dict[str, int]) is DictType
    assert type(Set[int]) is SetType
    assert ListType.__module__ == "rustique"
    with pytest.raises(TypeError):
        ListType()


def test_alias_identity():
    assert List[int] is List[int]
    assert Dict[str, int] is Dict[str, int]
    assert Set[str] is Set[str]
    assert List[int | None] is List[Optional[int]]
    assert List[Annotated[int, "x"]] is List[int]
    assert List[typing.List[int]] is List[list[int]]
    assert List[IntList] is List[list[int]]
    assert List[UserId] is List[int]
    assert List[(int,)] is List[int]
    assert List[int | str] is List[int | str]
    assert List[int | str] is not List[str | int]
    assert Dict[str, List[int]] is Dict[str, List[int]]
    assert Dict[str, list[int]] is Dict[str, List[int]]
    assert List[List[int]] is List[list[int]]
    assert List[int] is not List[float]


def test_union_aliases_respect_member_order_across_repeated_lookups():
    for _ in range(3):
        assert repr(List[int | float]) == "List[int | float]"
        assert repr(List[float | int]) == "List[float | int]"
        assert repr(List[Union[float, int]]) == "List[float | int]"
        assert repr(Set[str | bytes | None]) == "Set[str | bytes | None]"
        assert repr(Set[bytes | str | None]) == "Set[bytes | str | None]"
        assert repr(Dict[str, int | float]) == "Dict[str, int | float]"
        assert repr(Dict[str, float | int]) == "Dict[str, float | int]"
        assert repr(Dict[int | str, int]) == "Dict[int | str, int]"
        assert repr(Dict[str | int, int]) == "Dict[str | int, int]"
        assert repr(List[list[int | str]]) == "List[List[int | str]]"
        assert repr(List[list[str | int]]) == "List[List[str | int]]"
        assert List[int | float] is List[int | float]
        assert List[int | float] is not List[float | int]


def test_untyped_aliases_are_the_classes():
    assert List[Any] is List
    assert List[object] is List
    assert List[T] is List
    assert Dict[Any, Any] is Dict
    assert Dict[object, Any] is Dict
    assert Dict[str, Any] is not Dict
    assert Set[Any] is Set


@pytest.mark.parametrize(
    ("alias", "expected"),
    [
        (lambda: List[int], "List[int]"),
        (lambda: List[int | None], "List[int | None]"),
        (lambda: List[str | int], "List[str | int]"),
        (lambda: List[Point], "List[Point]"),
        (lambda: List[None], "List[None]"),
        (lambda: List[list], "List[List]"),
        (lambda: Dict[str, List[float]], "Dict[str, List[float]]"),
        (lambda: Dict[str, Any], "Dict[str, Any]"),
        (lambda: Dict[Any, int], "Dict[Any, int]"),
        (lambda: Set[str], "Set[str]"),
        (lambda: List[Dict[str, Set[int]] | None], "List[Dict[str, Set[int]] | None]"),
    ],
)
def test_alias_repr(alias, expected):
    assert repr(alias()) == expected


def test_alias_schema_property():
    assert List[int | None].schema == "int | None"
    assert Dict[str, List[float]].schema == "str, List[float]"
    assert Set[str].schema == "str"


def test_alias_origin_and_args():
    assert List[int].__origin__ is List
    assert Dict[str, int].__origin__ is Dict
    assert Set[int].__origin__ is Set
    assert List[int].__args__ == (int,)
    assert Dict[str, float].__args__ == (str, float)
    assert Set[bytes].__args__ == (bytes,)
    assert List[Annotated[int, "x"]].__args__ == (int,)
    assert List[int | None].__args__ == (int | None,)
    assert List[str | int].__args__[0].__args__ == (str, int)
    assert Dict[str, Any].__args__ == (str, Any)
    assert List[list].__args__ == (List,)
    (inner,) = List[list[int]].__args__
    assert inner is List[int]
    assert List[int].__args__ is List[int].__args__
    assert List[int].__parameters__ == ()
    assert List[int].__name__ == "List"
    assert Dict[str, int].__qualname__ == "Dict"


def test_args_round_trip():
    for alias in [List[int | None], Dict[str, List[float]], Set[Point], List[str | int | None]]:
        subscript = alias.__args__ if len(alias.__args__) > 1 else alias.__args__[0]
        assert alias.__origin__[subscript] is alias


def test_alias_equality_and_hash():
    assert List[int] == List[int]
    assert List[int] != List[str]
    assert List[int] != Set[int]
    assert Dict[str, int] != Dict[int, str]
    assert List[int] != list[int]
    assert hash(List[int]) == hash(List[int])
    assert {List[int]: 1, Set[int]: 2}[List[int]] == 1


def test_alias_arity_errors():
    with pytest.raises(TypeError, match=r"Dict\[\.\.\.\] expects 2 type arguments \(key, value\), got 1"):
        Dict[int]
    with pytest.raises(TypeError, match="got 3"):
        Dict[int, str, float]
    with pytest.raises(TypeError, match=r"List\[\.\.\.\] expects 1 type argument, got 2"):
        List[int, str]
    with pytest.raises(TypeError, match="got 0"):
        Set[()]


def test_alias_element_errors_propagate():
    with pytest.raises(TypeError, match="forward reference"):
        List["int"]
    with pytest.raises(TypeError, match="not a supported element type"):
        Dict[str, 5]


def test_alias_cannot_be_subscripted_again():
    with pytest.raises(TypeError, match=r"List\[int\] is already parameterised"):
        List[int][str]
    with pytest.raises(TypeError, match="already parameterised"):
        Dict[str, int][int]


def test_alias_cannot_be_subclassed():
    with pytest.raises(TypeError, match=r"cannot subclass List\[int\]"):

        class Bad(List[int]):
            pass

    with pytest.raises(TypeError, match=r"cannot subclass Dict\[str, int\]"):

        class AlsoBad(Dict[str, int]):
            pass


def test_alias_union_operator():
    assert schema_of(List[int] | None) == "List[int] | None"
    assert schema_of(None | List[int]) == "List[int] | None"
    assert schema_of(int | List[str]) == "int | List[str]"
    assert schema_of(Set[int] | Dict[str, int]) == "Set[int] | Dict[str, int]"
    assert List[List[int] | None] is List[Optional[List[int]]]
    assert repr(List[List[int] | None]) == "List[List[int] | None]"


def test_alias_as_element_type():
    assert schema_of(List[int]) == "List[int]"
    assert schema_of(Dict[str, Set[int]]) == "Dict[str, Set[int]]"
    assert schema_of(list[List[int]]) == "List[List[int]]"


def test_alias_pickle_and_copy():
    for alias in [List[int], Dict[str, List[int | None]], Set[str], List[Point]]:
        assert pickle.loads(pickle.dumps(alias)) is alias
        assert copy.copy(alias) is alias
        assert copy.deepcopy(alias) is alias


def test_concurrent_alias_creation_is_identity_stable():
    classes = [type(f"C{i}", (), {}) for i in range(200)]
    results = [[] for _ in range(8)]
    errors = []
    barrier = threading.Barrier(len(results))

    def work(out):
        try:
            barrier.wait()
            out.extend(List[c | None] for c in classes)
            out.extend(Dict[str, c] for c in classes)
            out.extend(schema_of(c | int) for c in classes)
        except BaseException as exc:
            errors.append(exc)

    threads = [threading.Thread(target=work, args=(out,)) for out in results]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors, errors[0]
    aliases = 2 * len(classes)
    assert len(results[0]) == aliases + len(classes)
    for out in results[1:]:
        assert all(a is b for a, b in zip(out[:aliases], results[0][:aliases], strict=True))
        assert out[aliases:] == results[0][aliases:]


# ---------------------------------------------------------------------------------------
# Aliases: construction and isinstance (need the List/Dict/Set implementations)
# ---------------------------------------------------------------------------------------


def test_list_alias_constructs_and_checks_instances():
    xs = List[int]([1, 2, 3])
    assert isinstance(xs, List)
    assert isinstance(xs, List[int])
    assert not isinstance(xs, List[str])
    assert not isinstance(xs, List[int | None])
    assert isinstance(List[int](), List[int])
    assert not isinstance(List([1]), List[int])
    assert not isinstance([1, 2], List[int])
    assert not isinstance(Set[int](), List[int])


def test_dict_alias_constructs_and_checks_instances():
    d = Dict[str, int]({"a": 1}, b=2)
    assert isinstance(d, Dict)
    assert isinstance(d, Dict[str, int])
    assert not isinstance(d, Dict[str, float])
    assert isinstance(Dict[str, int](a=1), Dict[str, int])
    assert not isinstance(Dict(), Dict[str, int])
    assert not isinstance({"a": 1}, Dict[str, int])


def test_set_alias_constructs_and_checks_instances():
    s = Set[str](["a", "b"])
    assert isinstance(s, Set)
    assert isinstance(s, Set[str])
    assert not isinstance(s, Set[int])
    assert not isinstance(Set(), Set[str])


def test_alias_call_argument_errors():
    with pytest.raises(TypeError, match=r"List\[int\] expected at most 1 argument, got 2"):
        List[int]([1], [2])
    with pytest.raises(TypeError, match="takes no keyword arguments"):
        List[int](iterable=[1])
    with pytest.raises(TypeError, match=r"Set\[int\] expected at most 1 argument, got 2"):
        Set[int]([1], [2])
    with pytest.raises(TypeError, match=r"Dict\[str, int\] expected at most 1 argument, got 2"):
        Dict[str, int]({}, {})
