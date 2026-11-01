"""Differential tests of the replacement builtins (``rustique._builtins``) against the
originals: same results (type, value, float bits, *identity* of returned elements), same
exceptions and messages, same sequence of Python-level calls under re-entrancy, same
introspection, pickling and interpreter specialisation.

The replacements are obtained with ``_native.replacement(name)`` and called directly, so
most tests do not depend on whether they are installed in ``builtins``; the install tests
check the installation itself and always restore the originals.
"""

from __future__ import annotations

import builtins
import dis
import inspect
import opcode
import math
import pickle
import pydoc
import random
import sys
from collections.abc import Callable, Iterator
from decimal import Decimal
from fractions import Fraction
from typing import Any

import pytest
from _support import import_rustique_module_level
from hypothesis import given
from hypothesis import strategies as st

rs = import_rustique_module_level()

from rustique import _builtins  # noqa: E402
from rustique._rustique import _builtins as _native  # noqa: E402

NAMES = ("sum", "min", "max", "sorted", "any", "all")
ORIG: dict[str, Callable[..., Any]] = {name: getattr(builtins, name) for name in NAMES}
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


@pytest.fixture(scope="module", autouse=True)
def replacements() -> Iterator[dict[str, Callable[..., Any]]]:
    before = {name: getattr(builtins, name) for name in NAMES}
    assert before == ORIG, "another test left builtins replaced"
    _builtins.install()
    repl = {name: _native.replacement(name) for name in NAMES}
    _builtins.uninstall()
    assert {name: getattr(builtins, name) for name in NAMES} == ORIG
    REPL.update(repl)
    yield repl
    _builtins.uninstall()


REPL: dict[str, Callable[..., Any]] = {}

NAN = float("nan")


# --------------------------------------------------------------------------------------
# Comparison helpers
# --------------------------------------------------------------------------------------


def same(a: Any, b: Any) -> bool:
    """Equal with the same types, float bits (sign of zero), NaN == NaN, recursively."""
    if type(a) is not type(b):
        return False
    if isinstance(a, float):
        return (math.isnan(a) and math.isnan(b)) or (a == b and math.copysign(1, a) == math.copysign(1, b))
    if isinstance(a, complex):
        return same(a.real, b.real) and same(a.imag, b.imag)
    if isinstance(a, (list, tuple)):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    return bool(a == b)


def outcome(f: Callable[..., Any], *args: Any, **kwargs: Any) -> tuple[str, Any]:
    try:
        return ("ok", f(*args, **kwargs))
    except Exception as e:  # noqa: BLE001 -- compared below
        return ("err", (type(e), str(e)))


def check(name: str, *args: Any, identity: bool | None = None, **kwargs: Any) -> Any:
    """Call the original and the replacement with the same arguments and compare.

    `identity`: returned elements must be the very objects the original returns (default:
    unless an argument is a Rustique container, whose native storage creates new objects).
    """
    if identity is None:
        identity = not any(isinstance(a, (rs.List, rs.Tuple, rs.Set, rs.FrozenSet, rs.Dict)) for a in args)
    expected = outcome(ORIG[name], *args, **kwargs)
    got = outcome(REPL[name], *args, **kwargs)
    assert got[0] == expected[0], (name, args, kwargs, expected, got)
    if expected[0] == "err":
        assert got[1] == expected[1], (name, args, kwargs, expected, got)
        return None
    assert same(got[1], expected[1]), (name, args, kwargs, expected, got)
    if identity and name in ("min", "max"):
        assert got[1] is expected[1], (name, args, kwargs, "returned a different object")
    if identity and name == "sorted" and isinstance(got[1], list):
        assert all(x is y for x, y in zip(got[1], expected[1])), (name, args, "different element objects")
    return got[1]


def check_fresh(name: str, make: Callable[[], Any], **kwargs: Any) -> None:
    """`check` for one-shot arguments (iterators, generators): built anew for each call."""
    expected = outcome(ORIG[name], make(), **kwargs)
    got = outcome(REPL[name], make(), **kwargs)
    assert got[0] == expected[0] and same(got[1], expected[1]), (name, kwargs, expected, got)


def containers(values: list[Any]) -> list[Any]:
    """The same values in every container kind that can hold them."""
    out: list[Any] = [list(values), tuple(values), rs.List(values), rs.Tuple(values)]
    try:
        hashed = set(values)
    except TypeError:
        return out
    if len(hashed) == len(values):  # duplicates would change sums
        out += [hashed, frozenset(values), dict.fromkeys(values), rs.Set(values), rs.FrozenSet(values)]
        out += [rs.Dict(dict.fromkeys(values))]
    return out


def check_all_functions(values: list[Any]) -> None:
    for c in containers(values):
        check("sum", c)
        check("sum", c, 0)
        check("sum", c, 10)
        check("sum", c, 0.5)
        check("sum", c, start=-0.0)
        check("sum", c, start=2**70)
        check("any", c)
        check("all", c)
        check("min", c)
        check("max", c)
        check("min", c, default="d")
        check("max", c, key=None, default=None)
        check("sorted", c)
        check("sorted", c, reverse=True)


# --------------------------------------------------------------------------------------
# Values
# --------------------------------------------------------------------------------------

INT_CASES = [
    [],
    [0],
    [1, 2, 3],
    [-5, 3, -1, 0, 7],
    [True, False, True],
    [1, True, 1.0],
    [True, 1],
    [2**62, 2**62, 2**62],  # overflows i64 in the middle
    [2**63 - 1, 1],
    [-(2**63), -1],
    [2**100, -(2**100), 5],
    [5, 2**100, 3],
    [2**30 - 1, 2**30, -(2**30)],
    list(range(-50, 50)),
    [3] * 20,
]

FLOAT_CASES = [
    [0.1] * 10,
    [1e16, 1.0, -1e16],
    [1e308, 1e308, -1e308],
    [-0.0],
    [-0.0, -0.0],
    [0.0, -0.0],
    [-0.0, 0.0],
    [NAN, 1.0, 2.0],
    [1.0, NAN, 0.5],
    [math.inf, -math.inf],
    [math.inf, 1.0],
    [1.5, 2, 3.25, -7],
    [1, 2.5, 2**60, 0.1],
    [2**53 + 1, float(2**53)],
    [float(2**53), 2**53 + 1],
    [2**63, 1.5],
    [1.5, 2**1000],
    [1.0, 10**400],  # OverflowError: int too large to convert to float
    [1e300, 2**1023 * 2],
]

STR_CASES = [
    ["b", "a", "c"],
    ["", "a", ""],
    ["é", "e", "z"],
    ["€", "\U0001f600", "a", "\xff"],
    ["ab", "a", "abc", "b"],
    ["x" * 100, "x" * 99 + "y"],
]

MIXED_CASES = [
    [1, "a"],
    ["a", 1],
    [None, 1],
    [1, None],
    [(1, "a"), (0, "b"), (1, "a")],
    [[1], [0]],
    [1, 2j],
    [1.5, 2j, 3],
    [2j, 1, 1.5],
    [Fraction(1, 3), 1, 2.5],
    [1, Fraction(1, 3), 2],
    [Decimal("1.1"), 2],
    [1, 2, Decimal("1.1")],
    [b"", b"x"],
    [(), (1,), [], [0]],
    [0, 0.0, "", None, False],
]


@pytest.mark.parametrize("values", INT_CASES + FLOAT_CASES + STR_CASES + MIXED_CASES, ids=repr)
def test_all_functions_match(values: list[Any]) -> None:
    check_all_functions(values)


@pytest.mark.parametrize("values", INT_CASES + FLOAT_CASES + STR_CASES, ids=repr)
def test_typed_lists(values: list[Any]) -> None:
    for t in (int, float, str, bool, rs.u8, rs.i16, rs.f32):
        try:
            lst = rs.List[t](values)
        except (TypeError, OverflowError):
            continue
        for start in ((), (0,), (3,), (0.25,), (2**70,)):
            check("sum", lst, *start)
        for name in ("min", "max"):
            check(name, lst, identity=False)
            check(name, lst, default=0, identity=False)
        check("any", lst)
        check("all", lst)
        check("sorted", lst, identity=False)
        check("sorted", lst, reverse=True, identity=False)
        check("sorted", lst, key=lambda v: -v if not isinstance(v, str) else v, identity=False)


def test_min_max_positional_forms() -> None:
    for a, b in [(1, 2), (2, 1), (1, 1.0), (1.0, 1), (NAN, 1), (1, NAN), ("a", "b"), ("a", 1), (-0.0, 0.0),
                 (0.0, -0.0), (2**80, 1), (1, 2**80), (True, 1), ((1,), (0,)), (None, None)]:
        for name in ("min", "max"):
            check(name, a, b)
            check(name, a, b, a)
            check(name, a, b, key=None)
            check(name, a, b, key=lambda v: 0)
            check(name, a, b, default=1)
    for name in ("min", "max"):
        check(name)
        check(name, 1)
        check(name, [], foo=1)
        check(name, [1], key=1)
        check(name, [3, 1, 2], key=lambda v: -v)
        check(name, [], default=None)
        check_fresh(name, lambda: iter([3, 1, 2]))
        check_fresh(name, lambda: (x for x in [3, 1, 2]))
        check(name, range(5))
        check(name, "hello")
        check(name, {3: 1, 1: 2})
        check(name, [], default=0, key=None)


def test_tuple_comparisons() -> None:
    nan = NAN
    other_nan = float("nan")
    cases = [
        [(1, "a"), (1, "b"), (0, "z")],
        [(1, 2), (1,), (1, 2, 3), ()],
        [(nan, 1), (nan, 0)],  # the same NaN object: equal by identity, then 1 vs 0
        [(nan, 1), (other_nan, 0)],  # different NaNs: unequal, then nan < nan is False
        [(1, (2, 3)), (1, (2, 2)), (1, (2,))],
        [((((1,),),),), ((((0,),),),)],
        [(1, Decimal(2)), (1, Decimal(1))],  # impure element compared
        [(0, Decimal(2)), (1, Decimal(1))],  # impure element never compared
        [(1, "a"), (1, 2)],  # TypeError at the second element
        [(1, None), (1, None)],  # identical None: equal, then lengths
        [(1, None), (1, 0)],  # None vs 0: TypeError
        [(2**70, 1), (2**70, 0), (2**70 + 1, 0)],
        [(1.0, "b"), (1, "a"), (True, "c")],
        [(1, 2), [1, 2]],  # tuple vs list: TypeError
        [None, None],
        [(1,), 1],
        [("a",), ("a", 1), ("a", 0)],
        [tuple(range(i, i + 3)) for i in (5, 3, 9, 3)],
    ]
    deep: Any = 0
    for _ in range(40):
        deep = (deep,)
    cases.append([deep, deep[0]])
    for values in cases:
        for c in (values, tuple(values), rs.List(values)):
            for name in ("min", "max"):
                check(name, c)
                check(name, c, default=None)
            check("sorted", c)
        if len(values) > 1:
            check("min", *values)
            check("max", *values)


def test_sum_argument_forms() -> None:
    check("sum")
    check("sum", [1], 2, 3)
    check("sum", [1], start=2)
    check("sum", [1], 2, start=3)
    check("sum", [1], begin=2)
    check("sum", 5)
    check("sum", ["a", "b"])
    check("sum", ["a"], "")
    check("sum", [b"a"], b"")
    check("sum", [], bytearray())
    check("sum", [[1], [2]], [])
    check("sum", [1, 2], True)
    check("sum", [1, 2], 1j)
    check("sum", [1.5], Fraction(1, 2))
    check_fresh("sum", lambda: (x for x in [1, 2, 3]))
    check_fresh("sum", lambda: (x for x in [1, 2, 3]), start=10)
    check("sum", range(10))
    check_fresh("sum", lambda: iter([1.5, 2]))
    check("sum", [1, 2], start=1.5)
    check("sum", [1.5, 2], start=2**70)
    check("sum", [2**62, 2**62, 0.5])


def test_sorted_argument_forms() -> None:
    for c in ([3, 1, 2], rs.List([3, 1, 2]), rs.List([3.0, NAN, 1.0]), rs.Set([3, 1, 2]), rs.List([(1,), (0,)])):
        check("sorted", c, identity=False)
        check("sorted", c, key=None, reverse=False, identity=False)
        check("sorted", c, key=lambda v: v, identity=False)
        check("sorted", c, reverse=1, identity=False)
        check("sorted", c, reverse=[], identity=False)
        check("sorted", c, cmp=None)
        check("sorted", c, 1)
        check("sorted", c, key=1)
    check("sorted")


def test_any_all_forms() -> None:
    for name in ("any", "all"):
        check(name, 1)
        check_fresh(name, lambda: iter([0, 1]))
        check_fresh(name, lambda: (x for x in [0, 0]))
        check(name, "")
        check(name, "ab")
        check(name, b"\x00")
        check(name, [[], (), "", b"", 0, 0.0, None, False])
        check(name, [[0], (0,), " ", b"0", 1, -0.5, True])
        check(name, [-0.0, 0.0, NAN])
        check(name, [0, 2**100])
        check(name, [Decimal(0), 1])
        check(name, [1, Decimal(0)])
        check(name, {0: 1, 1: 0})


# --------------------------------------------------------------------------------------
# Subclasses and Python-level dunders: the fast paths must not apply
# --------------------------------------------------------------------------------------


class MyInt(int):
    def __add__(self, other: Any) -> Any:
        return "MyInt.__add__"

    __radd__ = __add__

    def __lt__(self, other: Any) -> bool:
        return True

    def __gt__(self, other: Any) -> bool:
        return True

    def __bool__(self) -> bool:
        return False


class MyFloat(float):
    def __radd__(self, other: Any) -> Any:
        return 42.0


class MyStr(str):
    def __lt__(self, other: Any) -> bool:
        return False


class MyList(list):  # type: ignore[type-arg]
    def __iter__(self) -> Iterator[Any]:
        return iter([100, 200])


class MyTuple(tuple):  # type: ignore[type-arg]
    def __iter__(self) -> Iterator[Any]:
        return iter([7])


def test_subclasses_take_the_slow_path() -> None:
    for values in ([1, MyInt(5), 2], [MyInt(0)], [1.5, MyFloat(2.0)], [MyFloat(2.0), 1], ["b", MyStr("a"), "c"],
                   [MyStr("b"), "a"]):
        check_all_functions(values)
    for c in (MyList([1, 2, 3]), MyTuple((1, 2, 3))):
        for name in NAMES:
            check(name, c)


# --------------------------------------------------------------------------------------
# Re-entrancy: Python code run during the computation sees and mutates the live list
# --------------------------------------------------------------------------------------


class Logger:
    """Records the order of Python-level calls; optionally mutates a target list."""

    def __init__(self) -> None:
        self.log: list[str] = []
        self.target: list[Any] | None = None


def make_mutator(logger: Logger, action: str, value: int) -> Any:
    class Item:
        def __init__(self, v: int) -> None:
            self.v = v

        def _act(self, what: str) -> None:
            logger.log.append(f"{what}:{self.v}")
            t = logger.target
            if t is None:
                return
            if action == "append" and len(t) < 20:
                t.append(self.v + 100)
            elif action == "clear":
                t.clear()
            elif action == "pop" and t:
                t.pop()
            elif action == "insert" and len(t) < 20:
                t.insert(0, -1)

        def __radd__(self, other: Any) -> Any:
            self._act("radd")
            return other + self.v

        def __add__(self, other: Any) -> Any:
            self._act("add")
            return self.v + (other.v if isinstance(other, Item) else other)

        def __lt__(self, other: Any) -> bool:
            self._act("lt")
            return self.v < (other.v if isinstance(other, Item) else other)

        def __gt__(self, other: Any) -> bool:
            self._act("gt")
            return self.v > (other.v if isinstance(other, Item) else other)

        def __bool__(self) -> bool:
            self._act("bool")
            return bool(self.v % 2)

        def __repr__(self) -> str:
            return f"Item({self.v})"

    return Item(value)


def run_logged(f: Callable[..., Any], build: Callable[[Logger], list[Any]], *args: Any) -> tuple[Any, list[str], Any]:
    logger = Logger()
    target = build(logger)
    logger.target = target
    result = outcome(f, target, *args)
    if result[0] == "ok":
        res = result[1]
        res = res.v if hasattr(res, "v") else res
        result = ("ok", res)
    snapshot = [getattr(x, "v", x) for x in target]
    return result, logger.log, snapshot


@pytest.mark.parametrize("action", ["none", "append", "clear", "pop", "insert"])
@pytest.mark.parametrize("name", ["sum", "min", "max", "any", "all"])
@pytest.mark.parametrize("position", [0, 1, 3, 5])
def test_reentrant_mutation_matches(action: str, name: str, position: int) -> None:
    def build(logger: Logger) -> list[Any]:
        values: list[Any] = [2, 4, 6, 8, 10, 12]
        if name in ("any", "all"):
            values = [0, 0, 0, 0, 0, 0] if name == "any" else [1, 1, 1, 1, 1, 1]
        values[position] = make_mutator(logger, action, 3)
        return values

    expected = run_logged(ORIG[name], build)
    got = run_logged(REPL[name], build)
    assert got == expected


def test_reentrant_mutation_sorted_rustique() -> None:
    lst = rs.List([3, 1, 2])

    def key(v: int) -> int:
        lst.append(0)
        return v

    assert REPL["sorted"](lst, key=key) == [1, 2, 3]
    assert len(lst) == 6


# --------------------------------------------------------------------------------------
# Property-based
# --------------------------------------------------------------------------------------

scalars = st.one_of(
    st.integers(min_value=-(2**65), max_value=2**65),
    st.integers(min_value=-5, max_value=5),
    st.booleans(),
    st.floats(allow_nan=True, allow_infinity=True),
    st.sampled_from([0.0, -0.0, 0.5, -1.5, 1e308]),
)
orderables = st.one_of(
    st.lists(st.one_of(st.integers(min_value=-(2**65), max_value=2**65), st.booleans(), st.floats()), max_size=40),
    st.lists(st.text(max_size=4), max_size=40),
)


@given(st.lists(scalars, max_size=60))
def test_numeric_property(values: list[Any]) -> None:
    for c in (values, tuple(values), rs.List(values)):
        for start in ((), (0,), (1.25,), (-7,)):
            check("sum", c, *start)
        check("any", c)
        check("all", c)


@given(orderables)
def test_order_property(values: list[Any]) -> None:
    for c in (values, tuple(values), rs.List(values)):
        ident = not isinstance(c, rs.List)
        check("min", c, identity=ident)
        check("max", c, identity=ident)
        check("min", c, default=None, identity=ident)
        check("sorted", c, identity=ident)
        check("sorted", c, reverse=True, identity=ident)
    if values and len(set(values)) == len(values):
        for c in (set(values), frozenset(values), dict.fromkeys(values), rs.Set(values)):
            check("min", c)
            check("max", c)


@given(st.lists(st.one_of(scalars, st.text(max_size=3), st.none(), st.tuples(st.integers())), max_size=30))
def test_mixed_property(values: list[Any]) -> None:
    for c in (values, tuple(values), rs.List(values)):
        check("any", c)
        check("all", c)
        check("sum", c)
        check("min", c)
        check("max", c)


def test_large_inputs() -> None:
    rng = random.Random(5)
    ints = [rng.randrange(-(10**9), 10**9) for _ in range(100_000)]
    floats = [rng.uniform(-1e6, 1e6) for _ in range(100_000)]
    strs = [f"s{rng.randrange(10**6)}" for _ in range(20_000)]
    for values in (ints, floats, strs):
        for c in (values, tuple(values), rs.List(values)):
            ident = not isinstance(c, rs.List)
            if values is not strs:
                check("sum", c)
            check("min", c, identity=ident)
            check("max", c, identity=ident)
            check("any", c)
            check("all", c)
        check("sorted", rs.List(values), identity=False)
        check("sorted", rs.List(values), reverse=True, identity=False)
    mixed = [0, 0.0, "", None, (), *range(1, 100), "x", (1,)]
    for c in (rs.List(mixed), rs.List(mixed[::-1]), rs.Set(range(100)), rs.Set(range(1, 100)), rs.Set(["", *"abc" * 20])):
        for name in ("any", "all"):
            check(name, c)
    big = ints + [2**100]
    check("sum", big)
    check("max", big)
    obj = ints[:1000] + [Decimal(1)]
    check("sum", obj)
    check("max", obj)


# --------------------------------------------------------------------------------------
# Introspection, pickling, help, specialisation
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize("name", NAMES)
def test_introspection_matches(name: str) -> None:
    orig, repl = ORIG[name], REPL[name]
    assert repl is not orig
    assert type(repl) is type(orig)
    for attr in ("__name__", "__qualname__", "__module__", "__doc__", "__text_signature__"):
        assert getattr(repl, attr) == getattr(orig, attr), attr
    assert repl.__self__ is orig.__self__
    assert repr(repl) == repr(orig)
    try:
        sig = inspect.signature(orig)
    except ValueError:
        with pytest.raises(ValueError):
            inspect.signature(repl)
    else:
        assert inspect.signature(repl) == sig
    assert pydoc.render_doc(repl) == pydoc.render_doc(orig)
    assert repl.__reduce__() == orig.__reduce__()


@pytest.mark.parametrize("name", NAMES)
def test_pickle_by_reference(name: str) -> None:
    _builtins.install()
    try:
        f = getattr(builtins, name)
        assert f is ORIG[name]  # accelerated in place: identity preserved
        for proto in range(pickle.HIGHEST_PROTOCOL + 1):
            assert pickle.loads(pickle.dumps(f, proto)) is f
    finally:
        _builtins.uninstall()
    assert pickle.loads(pickle.dumps(ORIG[name])) is ORIG[name]


def test_install_uninstall() -> None:
    from builtins import sum as captured_before  # a reference taken before installing

    assert _builtins.installed() == ()
    assert _builtins.install() == _builtins.REPLACED
    assert _builtins.install() == _builtins.REPLACED  # idempotent
    try:
        assert _builtins.installed() == _builtins.REPLACED
        for name in NAMES:
            assert getattr(builtins, name) is ORIG[name]
        assert captured_before is builtins.sum
        assert eval("sum([1, 2, 3])") == 6
        assert captured_before([1.5, 2]) == 3.5
    finally:
        _builtins.uninstall()
    assert _builtins.installed() == ()
    for name in NAMES:
        assert getattr(builtins, name) is ORIG[name]
    _builtins.uninstall()  # idempotent
    assert _builtins.installed() == ()


def test_identity_keyed_compiler_paths_still_apply() -> None:
    # CPython 3.14+ compiles `any(<genexpr>)` / `all(<genexpr>)` into an identity check
    # against the original builtin plus an inlined loop: the check must keep passing.
    code = compile("any(v for v in x), all(v for v in x)", "<test>", "eval")
    consts = [i.argval for i in dis.get_instructions(code) if i.opname == "LOAD_COMMON_CONSTANT"]
    # `dis` resolves the common constants on 3.15; 3.14 reports their table indices.
    table = getattr(opcode, "_common_constants", [])
    consts = [table[c] if type(c) is int and 0 <= c < len(table) else c for c in consts]
    _builtins.install()
    try:
        assert builtins.any is ORIG["any"] and builtins.all is ORIG["all"]
        if consts:  # the optimisation exists on this version
            assert ORIG["any"] in consts and ORIG["all"] in consts
        assert eval(code, {"x": [0, 1]}) == (True, False)
    finally:
        _builtins.uninstall()


def test_patched_builtins_dict_is_left_alone() -> None:
    def my_sum(*args: Any) -> str:
        return "mine"

    builtins.sum = my_sum  # type: ignore[assignment]
    try:
        assert "sum" in _builtins.install()  # the original function object is accelerated
        assert builtins.sum is my_sum  # the builtins namespace is not touched
        assert ORIG["sum"]([1, 2]) == 3
        _builtins.uninstall()
        assert builtins.sum is my_sum
    finally:
        builtins.sum = ORIG["sum"]  # type: ignore[assignment]
        _builtins.uninstall()


SPECIALIZING = sys.implementation.name == "cpython" and hasattr(dis, "get_instructions")


def specialized_call(f: Callable[[], Any]) -> set[str]:
    for _ in range(200):
        f()
    return {i.opname for i in dis.get_instructions(f, adaptive=True) if i.opname.startswith("CALL")}


@pytest.mark.skipif(not SPECIALIZING, reason="needs CPython's specialising interpreter")
def test_call_specialisation_still_fires() -> None:
    src = {
        "sum": "lambda: sum(x)",
        "min": "lambda: min(a, b)",
        "max": "lambda: max(x)",
        "sorted": "lambda: sorted(x)",
        "any": "lambda: any(x)",
        "all": "lambda: all(x)",
    }
    env = {"x": [3, 1, 2], "a": 1, "b": 2}
    for name, code in src.items():
        before = specialized_call(eval(code, dict(env)))
        _builtins.install()
        try:
            after = specialized_call(eval(code, dict(env)))
        finally:
            _builtins.uninstall()
        assert after == before, (name, before, after)
        expected = "CALL_BUILTIN_O" if name in ("any", "all") else "CALL_BUILTIN_FAST_WITH_KEYWORDS"
        if expected in before:  # the build specialises at all
            assert expected in after, (name, after)


# --------------------------------------------------------------------------------------
# Free-threading: concurrent mutation while the builtins read a list
# --------------------------------------------------------------------------------------


def test_concurrent_mutation_is_safe() -> None:
    import threading

    shared: list[Any] = [1] * 2000
    stop = threading.Event()
    errors: list[BaseException] = []

    def writer() -> None:
        r = random.Random(1)
        while not stop.is_set():
            op = r.random()
            if op < 0.4:
                shared.append(r.choice([1, 2, 1.5]))
            elif op < 0.8:
                try:
                    shared.pop()
                except IndexError:
                    pass
            elif op < 0.85:
                shared[:] = [1] * 100

    def reader() -> None:
        try:
            for _ in range(300):
                s = REPL["sum"](shared)
                assert isinstance(s, (int, float))
                m = REPL["min"](shared, default=None)
                assert m in (None, 1, 1.5, 2)
                assert REPL["max"](shared, default=0) in (0, 1, 1.5, 2)
                assert REPL["all"](shared) is True
                assert REPL["any"](shared) in (True, False)
        except BaseException as e:  # noqa: BLE001
            errors.append(e)

    old = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    try:
        writers = [threading.Thread(target=writer, daemon=True) for _ in range(2)]
        readers = [threading.Thread(target=reader, daemon=True) for _ in range(4)]
        for t in writers + readers:
            t.start()
        for t in readers:
            t.join(120)
        stop.set()
        for t in writers:
            t.join(120)
    finally:
        sys.setswitchinterval(old)
    assert not errors, errors[0]
