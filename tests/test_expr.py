"""Vectorised expressions (`X`): semantics must equal plain Python evaluation.

The oracle evaluates the same expression tree with Python's own operators, element by
element; `List.map` / `List.filter` (native engine, fallbacks, object storage) and
`Expr.__call__` must agree with it exactly — values, result types and exceptions.
"""

import copy
import math
import operator
import struct

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import rustique as rs
from rustique import List
from rustique._rustique import Expr, X

# ---------------------------------------------------------------------------------------
# The oracle: plain Python evaluation of an expression tree.
#
# Trees: ("X",) | ("const", value) | ("u", op, tree) | ("b", op, lhs, rhs) | ("c", type, tree)


def py_is_nan(v):
    if isinstance(v, str):
        raise TypeError("must be real number, not str")
    return False if isinstance(v, int) else math.isnan(v)


UNARY = {
    "neg": operator.neg,
    "pos": operator.pos,
    "abs": abs,
    # `~` is logical on bool (documented deviation from Python's deprecated `~True == -2`).
    "inv": lambda v: (not v) if type(v) is bool else ~v,
    "is_nan": py_is_nan,
}
BINARY = {
    "add": operator.add,
    "sub": operator.sub,
    "mul": operator.mul,
    "truediv": operator.truediv,
    "floordiv": operator.floordiv,
    "mod": operator.mod,
    "pow": operator.pow,
    "lshift": operator.lshift,
    "rshift": operator.rshift,
    "and": operator.and_,
    "or": operator.or_,
    "xor": operator.xor,
    "eq": operator.eq,
    "ne": operator.ne,
    "lt": operator.lt,
    "le": operator.le,
    "gt": operator.gt,
    "ge": operator.ge,
}
REFLECTED_COMPARISON = {"eq": "eq", "ne": "ne", "lt": "gt", "le": "ge", "gt": "lt", "ge": "le"}
CASTS = {"int": int, "float": float, "bool": bool}


def python_eval(tree, x):
    kind = tree[0]
    if kind == "X":
        return x
    if kind == "const":
        return tree[1]
    if kind == "u":
        return UNARY[tree[1]](python_eval(tree[2], x))
    if kind == "b":
        a = python_eval(tree[2], x)
        b = python_eval(tree[3], x)
        return BINARY[tree[1]](a, b)
    return CASTS[tree[1]](python_eval(tree[2], x))


def build(tree):
    """Build the rustique expression for `tree`.

    Returns `(expr_or_constant, normalized_tree)`: Python folds constant subtrees itself and
    reflects `constant < expr` into `expr > constant`, so the oracle evaluates the tree in
    the form the `Expr` actually has.
    """
    kind = tree[0]
    if kind == "X":
        return X, tree
    if kind == "const":
        return tree[1], tree
    if kind == "u":
        a, at = build(tree[2])
        if not isinstance(a, Expr):
            v = UNARY[tree[1]](a)
            return v, ("const", v)
        e = {"neg": lambda: -a, "pos": lambda: +a, "abs": lambda: abs(a), "inv": lambda: ~a, "is_nan": a.is_nan}[tree[1]]()
        return e, ("u", tree[1], at)
    if kind == "c":
        a, at = build(tree[2])
        if not isinstance(a, Expr):
            v = CASTS[tree[1]](a)
            return v, ("const", v)
        return a.cast(CASTS[tree[1]]), ("c", tree[1], at)
    op = tree[1]
    a, at = build(tree[2])
    b, bt = build(tree[3])
    if not isinstance(a, Expr) and not isinstance(b, Expr):
        v = BINARY[op](a, b)
        return v, ("const", v)
    if op in REFLECTED_COMPARISON and not isinstance(a, Expr):
        return BINARY[op](a, b), ("b", REFLECTED_COMPARISON[op], bt, at)
    return BINARY[op](a, b), ("b", op, at, bt)


def outcome(f):
    try:
        return ("ok", f())
    except Exception as e:  # noqa: BLE001 - every exception is part of the semantics
        return ("raise", type(e), str(e))


def same_value(a, b):
    if type(a) is not type(b):
        return False
    if type(a) is float:
        return struct.pack("<d", a) == struct.pack("<d", b) or (math.isnan(a) and math.isnan(b))
    if type(a) is complex:
        return same_value(a.real, b.real) and same_value(a.imag, b.imag)
    return a == b


def same_values(xs, ys):
    return len(xs) == len(ys) and all(same_value(x, y) for x, y in zip(xs, ys))


def same_outcome(got, expected):
    if got[0] != expected[0]:
        return False
    if got[0] == "raise":
        return got[1:] == expected[1:]
    return same_values(got[1], expected[1])


# ---------------------------------------------------------------------------------------
# Strategies. Operands are bounded so the oracle never builds astronomically large ints or
# strings: `**` and `<<` only take small constant right operands, and integers are either
# small or near the 64-bit limits (repeating a string that often fails fast).

SMALL_INTS = st.integers(-100, 100)
EDGE_INTS = st.sampled_from([2**63 - 1, -(2**63), 2**62, -(2**62) - 1, 2**53 + 1])
INTS = st.one_of(SMALL_INTS, SMALL_INTS, EDGE_INTS, st.integers(2**60, 2**63 - 1))
BIG_INTS = st.sampled_from([2**63, -(2**63) - 1, 2**64 + 5, -(2**70)])
FLOATS = st.one_of(
    st.integers(-40, 40).map(lambda i: i / 4),
    st.floats(allow_nan=True, allow_infinity=True),
    st.sampled_from([0.0, -0.0, math.nan, math.inf, -math.inf, 1e308, 5e-324]),
)
STRS = st.sampled_from(["", "a", "ab", "ba", "12", " -3 ", "1_000", "2.5", "inf", "x'y", "é", "１２"])
CONSTANTS = st.one_of(
    st.integers(-6, 6),
    EDGE_INTS,
    st.sampled_from([0.0, -0.0, 0.5, 1.5, -2.5, 0.1, 3.0, 1e300, math.inf, -math.inf, math.nan]),
    st.booleans(),
)
TEXT_CONSTANTS = st.one_of(CONSTANTS, STRS)
EXPONENTS = st.one_of(st.integers(-3, 6), st.sampled_from([0.5, -1.5, 2.0]), st.booleans())
SHIFTS = st.one_of(st.integers(-2, 70), st.booleans())
SAFE_BINARY = [op for op in BINARY if op not in ("pow", "lshift")]


def trees(constants):
    leaf = st.one_of(st.just(("X",)), st.just(("X",)), constants.map(lambda v: ("const", v)))

    def extend(children):
        return st.one_of(
            st.tuples(st.just("u"), st.sampled_from(list(UNARY)), children),
            st.tuples(st.just("b"), st.sampled_from(SAFE_BINARY), children, children),
            st.tuples(st.just("b"), st.just("pow"), children, EXPONENTS.map(lambda v: ("const", v))),
            st.tuples(st.just("b"), st.just("lshift"), children, SHIFTS.map(lambda v: ("const", v))),
            st.tuples(st.just("c"), st.sampled_from(list(CASTS)), children),
        )

    return st.recursive(leaf, extend, max_leaves=6)


def expressions(constants=CONSTANTS):
    """(Expr, normalized tree) pairs whose tree mentions X."""

    def make(tree):
        try:
            expr, normalized = build(tree)
        except Exception:  # noqa: BLE001 - a constant subtree raised while building
            return None
        return (expr, normalized) if isinstance(expr, Expr) else None

    return trees(constants).map(make).filter(lambda pair: pair is not None)


DATA = st.one_of(
    st.lists(INTS, max_size=40),
    st.lists(FLOATS, max_size=40),
    st.lists(st.booleans(), max_size=40),
    st.lists(STRS, max_size=40),
    # Mixed (object storage) and big ints (demoted untyped storage).
    st.lists(st.one_of(INTS, FLOATS, st.booleans(), STRS, BIG_INTS), max_size=20),
)

SETTINGS = settings(max_examples=400, deadline=None)


def expected_map(tree, data):
    return outcome(lambda: [python_eval(tree, x) for x in data])


def expected_filter(tree, data):
    return outcome(lambda: [x for x in data if python_eval(tree, x)])


# ---------------------------------------------------------------------------------------
# Differential tests against the oracle.


@SETTINGS
@given(expressions(TEXT_CONSTANTS), st.one_of(INTS, FLOATS, st.booleans(), STRS, BIG_INTS))
def test_call_matches_python(pair, x):
    expr, tree = pair
    assert same_outcome(outcome(lambda: [expr(x)]), outcome(lambda: [python_eval(tree, x)]))


@SETTINGS
@given(expressions(TEXT_CONSTANTS), DATA)
def test_map_matches_python(pair, data):
    expr, tree = pair
    got = outcome(lambda: List(data).map(expr).to_list())
    assert same_outcome(got, expected_map(tree, data)), (expr, data, got)


@SETTINGS
@given(expressions(TEXT_CONSTANTS), DATA)
def test_filter_matches_python(pair, data):
    expr, tree = pair
    got = outcome(lambda: List(data).filter(expr).to_list())
    assert same_outcome(got, expected_filter(tree, data)), (expr, data, got)


@SETTINGS
@given(
    expressions(),
    st.one_of(
        st.tuples(st.just(int), st.lists(INTS, max_size=40)),
        st.tuples(st.just(float), st.lists(FLOATS, max_size=40)),
        st.tuples(st.just(bool), st.lists(st.booleans(), max_size=40)),
        st.tuples(st.just(str), st.lists(STRS, max_size=40)),
    ),
)
def test_typed_map_and_filter_match_python(pair, typed):
    expr, tree = pair
    ty, data = typed
    lst = List[ty](data)
    got = outcome(lambda: lst.map(expr).to_list())
    assert same_outcome(got, expected_map(tree, data)), (expr, data, got)
    kept = outcome(lambda: lst.filter(expr))
    expected = expected_filter(tree, data)
    if kept[0] == "ok":
        assert kept[1].schema_type is List[ty]
        kept = ("ok", kept[1].to_list())
    assert same_outcome(kept, expected)


@settings(max_examples=15, deadline=None)
@given(expressions(), st.sampled_from(["int", "float"]))
def test_large_lists_match_python(pair, kind):
    # Large enough to run detached from the interpreter and across worker threads.
    expr, tree = pair
    n = 140_000
    data = [(i * 7919) % 2001 - 1000 for i in range(n)]
    if kind == "float":
        data = [x / 8 for x in data]
    got = outcome(lambda: List(data).map(expr).to_list())
    assert same_outcome(got, expected_map(tree, data))


# ---------------------------------------------------------------------------------------
# Results: values, schemas and strategies.


def test_public_exports():
    assert rs.X is X
    assert rs.Expr is Expr


def test_map_and_filter_basics():
    lst = List(range(10))
    assert (X * 2 + 1)(5) == 11
    assert lst.map(X * 2 + 1).to_list() == [2 * i + 1 for i in range(10)]
    assert lst.filter(X % 3 == 0).to_list() == [0, 3, 6, 9]
    # filter keeps truthy results, not just True.
    assert lst.filter(X % 3).to_list() == [1, 2, 4, 5, 7, 8]
    assert List[int]([1, 5, 9]).filter((X > 2) & ~(X > 8)).to_list() == [5]
    assert List(["a", "", "bc"]).map(X + "!").to_list() == ["a!", "!", "bc!"]


def test_result_schemas():
    ints = List[int]([1, 2, 3])
    assert ints.map(X * 2).schema_type is List[int]
    assert ints.map(X / 2).schema_type is List[float]
    assert ints.map(X > 1).schema_type is List[bool]
    assert ints.map(X.cast(float)).schema_type is List[float]
    assert List[float]([1.5]).map(X // 1).schema_type is List[float]
    assert List[str](["a"]).map(X * 2).schema_type is List[str]
    untyped = List([1, 2, 3])
    assert untyped.map(X / 2).schema_type is List
    assert untyped.map(X / 2).strategy == "f64"
    assert untyped.map(X > 1).strategy == "bool"
    assert List([1.5]).map(X.cast(int)).strategy == "i64"
    assert List([]).map(X + 1).to_list() == []
    assert List[int]([]).map(X / 2).schema_type is List[float]
    assert List[int]([1]).filter(X > 5).schema_type is List[int]


def test_python_int_overflow_falls_back_to_exact_ints():
    big = List([2**62, 3])
    out = big.map(X * 4)
    assert out.to_list() == [2**64, 12]
    assert out.strategy == "object"
    typed = List[int]([2**62, 3]).map(X * 4)
    assert typed.schema_type is List[int]
    assert typed.to_list() == [2**64, 12]
    assert List[int]([2**63 - 1]).map(-(X + 1) - 1).to_list() == [-(2**63) - 1]
    assert List([2**62]).filter(X * 4 > 2**63).to_list() == [2**62]
    # `int ** negative` is a float; with a column exponent results can mix.
    assert List([2, -1]).map(2**X).to_list() == [4, 0.5]
    # Negative fractional powers are complex.
    assert List([-4.0]).map(X**0.5).to_list() == [(-4.0) ** 0.5]


def test_object_storage_uses_python_semantics():
    mixed = List([1, "a", 2.5, None])
    assert mixed.map(X == 1).to_list() == [True, False, False, False]
    with pytest.raises(TypeError, match="unsupported operand"):
        mixed.map(X * 2)
    assert List([1, "a", 2.5]).map(X * 2).to_list() == [2, "aa", 5.0]
    assert List([10**30, 2]).map(X // 3).to_list() == [10**30 // 3, 0]


def test_python_errors():
    with pytest.raises(ZeroDivisionError, match="^division by zero$"):
        List([1, 0]).map(1 // X)
    with pytest.raises(ZeroDivisionError, match="^division by zero$"):
        List([1.0, 0.0]).map(1 / X)
    with pytest.raises(ValueError, match="^negative shift count$"):
        List([3, 0]).map((1 // X) + (1 << (X - 5)))
    # The first failing element decides, as in a comprehension.
    with pytest.raises(ZeroDivisionError):
        List([0, 3]).map((1 // X) + (1 << (X - 5)))
    with pytest.raises(OverflowError) as info:
        List([2.0]).map(X**10000)
    assert info.value.args == _float_pow_overflow_args()
    with pytest.raises(TypeError, match="can only concatenate str"):
        List(["a"]).map(X + 1)
    with pytest.raises(TypeError, match="'<' not supported between instances of 'str' and 'int'"):
        List(["a"]).map(X < 1)
    with pytest.raises(ValueError, match="invalid literal for int"):
        List(["1", "x"]).map(X.cast(int))
    assert List([" 12 ", "1_0"]).map(X.cast(int)).to_list() == [12, 10]
    with pytest.raises(ValueError, match="cannot convert float NaN to integer"):
        List([math.nan]).map(X.cast(int))
    with pytest.raises(TypeError, match="Expr or a callable"):
        List([1]).map(3)
    with pytest.raises(TypeError, match="Expr or a callable"):
        List([1]).filter("x")


def _float_pow_overflow_args():
    try:
        2.0**10000
    except OverflowError as e:
        return e.args
    raise AssertionError("2.0 ** 10000 did not overflow")


def test_callables_are_the_slow_path():
    lst = List([1, 2, 3])
    assert lst.map(lambda x: x * 10).to_list() == [10, 20, 30]
    assert lst.map(str).to_list() == ["1", "2", "3"]
    assert lst.map(str).schema_type is List
    assert lst.filter(lambda x: x != 2).to_list() == [1, 3]
    assert List[int]([1, 2]).map(lambda x: x / 2).schema_type is List
    with pytest.raises(ZeroDivisionError):
        lst.map(lambda x: 1 // (x - 2))


def test_fixed_width_semantics():
    i8 = List[rs.i8]([100, -100])
    assert i8.map(X + 27).to_list() == [127, -73]
    assert i8.map(X + 27).schema_type is List[rs.i8]
    with pytest.raises(OverflowError):
        i8.map(X + 28)
    with pytest.raises(OverflowError):
        i8.map(X + 300)
    assert i8.map(X / 8).to_list() == [12.5, -12.5]
    assert i8.map(X / 8).schema_type is List[float]
    assert i8.map(X // -3).to_list() == [-34, 33]
    assert i8.map(X > 1000).to_list() == [False, False]
    with pytest.raises(TypeError):
        i8.map(X * 1.5)
    with pytest.raises(OverflowError):
        List[rs.u8]([0]).map(X - 1)
    assert List[rs.u8]([5]).map(~X).to_list() == [250]
    # `cast(int)` gives exact Python ints: no fixed-width overflow.
    assert List[rs.i64]([2**62]).map(X.cast(int) * 4).to_list() == [2**64]
    assert List([1, 2]).map(X.cast(rs.u8)).schema_type is List[rs.u8]
    with pytest.raises(OverflowError):
        List([1, 300]).map(X.cast(rs.u8))
    assert (X + 27)(rs.i8(100)) == rs.i8(127)
    assert X.cast(rs.i16)(3.9) == rs.i16(3)


# ---------------------------------------------------------------------------------------
# The Expr object itself.


@pytest.mark.parametrize(
    ("expr", "text"),
    [
        (X, "X"),
        (X * 2 + 1, "((X * 2) + 1)"),
        (2 * X, "(2 * X)"),
        (-(X + 1), "(-(X + 1))"),
        (+X, "(+X)"),
        (abs(X) < 1.5, "(abs(X) < 1.5)"),
        (~(X > 1) & (X != "a"), "((~(X > 1)) & (X != 'a'))"),
        (1 < X, "(X > 1)"),
        (2**X, "(2 ** X)"),
        (X // 3 % 2, "((X // 3) % 2)"),
        (X << 1 >> 2, "((X << 1) >> 2)"),
        (X ^ True | False, "((X ^ True) | False)"),
        ((X / 2).cast(int), "(X / 2).cast(int)"),
        (X.cast(float).is_nan(), "X.cast(float).is_nan()"),
        (X + 0.1, "(X + 0.1)"),
        (X * 1e16, "(X * 1e+16)"),
        (X == math.inf, "(X == inf)"),
        (X + 2**63, "(X + 9223372036854775808)"),
    ],
)
def test_repr(expr, text):
    assert repr(expr) == text


def test_misuse():
    with pytest.raises(TypeError, match="truth value"):
        bool(X > 1)
    with pytest.raises(TypeError, match="truth value"):
        if X > 1 and X < 5:
            pass
    with pytest.raises(TypeError, match="unhashable"):
        hash(X)
    with pytest.raises(TypeError, match="unhashable"):
        {X: 1}
    with pytest.raises(TypeError):
        X + [1]
    with pytest.raises(TypeError):
        [1] * X
    with pytest.raises(TypeError):
        pow(X, 2, 5)
    with pytest.raises(TypeError, match="cast"):
        X.cast(str)
    with pytest.raises(TypeError, match="cast"):
        X.cast(3)
    with pytest.raises(OverflowError, match="64 bits"):
        X + 10**30
    with pytest.raises(TypeError):
        Expr()
    assert (X == [1]) is False
    assert X in [X]
    assert copy.copy(X) is X and copy.deepcopy(X * 2) is not None
    deep = X
    for _ in range(511):
        deep = deep + 1
    with pytest.raises(RecursionError):
        deep + 1


def test_call_semantics():
    assert (X // -2)(7) == -4
    assert (X % -3)(7) == -2
    assert (~(X > 1))(3) is False
    assert (~X)(True) is False
    assert (~X)(5) == -6
    assert X.is_nan()(math.nan) is True
    assert X.is_nan()(5) is False
    assert X.cast(float)(" 2.5 ") == 2.5
    assert X.cast(bool)("") is False
    assert (X**-1)(4) == 0.25
    assert type((X + True)(1)) is int
    with pytest.raises(TypeError, match="must be real number"):
        X.is_nan()("a")
