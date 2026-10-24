"""Fixed-width scalars `rs.i8 … rs.u64`, `rs.f32`, `rs.f64` (ARCHITECTURE §4.4).

Every operator and every Rust-named method family is checked against a pure-Python
reference model (unbounded ints with explicit range checks, `% 2**bits` for wrapping,
Python floats rounded to binary32 for f32) with Hypothesis.
"""

import copy
import math
import operator
import pickle
import struct
import sys

import pytest
from hypothesis import assume, example, given, settings
from hypothesis import strategies as st

import rustique
from rustique import _rustique

INT_NAMES = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"]
FLOAT_NAMES = ["f32", "f64"]

# The scalars must be reachable as `rustique.<name>` for pickling; re-export them here in
# case the package `__init__` does not (yet).
for _name in INT_NAMES + FLOAT_NAMES:
    if not hasattr(rustique, _name):
        setattr(rustique, _name, getattr(_rustique, _name))
rs = rustique

try:
    import numpy as np
except ImportError:  # pragma: no cover - numpy is a dev dependency
    np = None


class IntSpec:
    def __init__(self, name):
        self.name = name
        self.cls = getattr(rs, name)
        self.signed = name.startswith("i")
        self.bits = int(name[1:])
        self.lo = -(2 ** (self.bits - 1)) if self.signed else 0
        self.hi = 2 ** (self.bits - 1) - 1 if self.signed else 2**self.bits - 1

    def __repr__(self):
        return self.name

    def fits(self, v):
        return self.lo <= v <= self.hi

    def wrap(self, v):
        v %= 2**self.bits
        if self.signed and v > self.hi:
            v -= 2**self.bits
        return v

    def clamp(self, v):
        return min(max(v, self.lo), self.hi)

    def values(self):
        edges = sorted(v for v in {self.lo, self.lo + 1, -2, -1, 0, 1, 2, 3, 7, self.hi - 1, self.hi} if self.fits(v))
        return st.one_of(st.sampled_from(edges), st.integers(self.lo, self.hi), st.integers(-130, 130).filter(self.fits))


INTS = [IntSpec(n) for n in INT_NAMES]
SIGNED = [s for s in INTS if s.signed]


def trunc_div(a, b):
    q = abs(a) // abs(b)
    return q if (a >= 0) == (b >= 0) else -q


def trunc_rem(a, b):
    return a - b * trunc_div(a, b)


def euclid_rem(a, b):
    r = trunc_rem(a, b)
    return r + abs(b) if r < 0 else r


def euclid_div(a, b):
    return (a - euclid_rem(a, b)) // b


def check_int(spec, thunk, expected):
    """`expected` is a Python int (checked against the range) or an exception type."""
    if isinstance(expected, type) and issubclass(expected, BaseException):
        with pytest.raises(expected):
            thunk()
        return
    if not spec.fits(expected):
        with pytest.raises(OverflowError):
            thunk()
        return
    result = thunk()
    assert type(result) is spec.cls
    assert result == expected and int(result) == expected


# --------------------------------------------------------------------------------------
# Construction, class attributes, conversions
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize("spec", INTS, ids=repr)
def test_int_class_metadata(spec):
    cls = spec.cls
    assert cls.__name__ == spec.name and cls.__module__ == "rustique"
    assert cls.BITS == spec.bits
    assert type(cls.MIN) is cls and cls.MIN == spec.lo
    assert type(cls.MAX) is cls and cls.MAX == spec.hi
    assert cls.__match_args__ == ("value",)
    with pytest.raises((TypeError, AttributeError)):
        cls.MAX = cls(0)
    with pytest.raises(TypeError):

        class Sub(cls):  # noqa: F841 - scalar classes are final
            pass


@pytest.mark.parametrize("spec", INTS, ids=repr)
def test_int_construction(spec):
    cls = spec.cls
    for v in (spec.lo, spec.hi, 0, 1, True, False):
        x = cls(v)
        assert type(x) is cls and x == v and x.value == v and type(x.value) is int
    assert cls() == 0
    for v in (spec.lo - 1, spec.hi + 1, 2**200, -(2**200)):
        with pytest.raises(OverflowError):
            cls(v)
    for bad in ("5", b"5", 5.0, 1.5, None, [1], rs.f64(1.0), rs.f32(1.0), 1j):
        with pytest.raises(TypeError):
            cls(bad)
    with pytest.raises(TypeError):
        cls(1, 2)
    with pytest.raises(TypeError):
        cls(value=1)

    class Index:
        def __index__(self):
            return 7

    assert cls(Index()) == 7
    assert cls(rs.u8(5)) == 5 and cls(rs.i64(7)) == 7
    assert cls(cls(3)) == 3


@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_conversions(spec, data):
    v = data.draw(spec.values())
    x = spec.cls(v)
    assert int(x) == v and type(int(x)) is int
    assert x.__index__() == v and operator.index(x) == v
    assert float(x) == float(v)
    assert bool(x) is bool(v)
    assert repr(x) == f"{spec.name}({v})"
    assert str(x) == str(v)
    assert hash(x) == hash(v)
    assert x == v and v == x and not (x != v)
    for fmt in ("", "d", "x", "08b", "+,", "_o", "^12", "e", ".3%"):
        assert format(x, fmt) == format(v, fmt)
    assert f"{x}" == str(v)
    assert pickle.loads(pickle.dumps(x)) == x
    assert type(pickle.loads(pickle.dumps(x, protocol=0))) is spec.cls
    assert copy.copy(x) is x and copy.deepcopy(x) is x
    assert math.trunc(x) == v and math.floor(x) == v and math.ceil(x) == v
    assert round(x) == v
    match x:
        case spec.cls(value=matched):
            assert matched == v
    assert list(range(10))[x % 10] == v % 10


@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_round_with_ndigits(spec, data):
    v = data.draw(spec.values())
    n = data.draw(st.integers(-25, 3))
    check_int(spec, lambda: round(spec.cls(v), n), round(v, n))


def test_hash_and_dict_interop():
    d = {rs.i64(5): "a", 2**63 - 1: "b"}
    assert d[5] == "a" and d[rs.i64(2**63 - 1)] == "b" and d[5.0] == "a"
    assert rs.u64(5) not in d  # different scalar types never compare equal, as in Rust
    assert rs.i8(1) in {1, 2} and 1.0 in {rs.f64(1.0)} and rs.f32(0.5) in {0.5}
    assert hash(rs.i8(-1)) == hash(-1) == -2
    for v in (0, 2**61 - 1, 2**61, 2**63 - 1, 2**64 - 1):
        assert hash(rs.u64(v)) == hash(v)


# --------------------------------------------------------------------------------------
# Integer operators (Python meaning, checked overflow)
# --------------------------------------------------------------------------------------

BINOPS = {
    "+": operator.add,
    "-": operator.sub,
    "*": operator.mul,
    "//": operator.floordiv,
    "%": operator.mod,
    "&": operator.and_,
    "|": operator.or_,
    "^": operator.xor,
}


def int_ref(op, a, b):
    if op in ("//", "%") and b == 0:
        return ZeroDivisionError
    return BINOPS[op](a, b)


@settings(max_examples=60)
@pytest.mark.parametrize("op", list(BINOPS))
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_binops_match_python(spec, op, data):
    a, b = data.draw(spec.values()), data.draw(spec.values())
    expected = int_ref(op, a, b)
    T, f = spec.cls, BINOPS[op]
    check_int(spec, lambda: f(T(a), T(b)), expected)
    check_int(spec, lambda: f(T(a), b), expected)
    check_int(spec, lambda: f(a, T(b)), expected)
    check_int(spec, lambda: f(T(a), bool(b) if b in (0, 1) else T(b)), expected)


@settings(max_examples=60)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_divmod_and_truediv(spec, data):
    a, b = data.draw(spec.values()), data.draw(spec.values())
    T = spec.cls
    for x, y in ((T(a), T(b)), (T(a), b), (a, T(b))):
        if b == 0:
            for f in (operator.truediv, divmod):
                with pytest.raises(ZeroDivisionError):
                    f(x, y)
            continue
        q, r = divmod(a, b)
        if spec.fits(q):
            got = divmod(x, y)
            assert type(got[0]) is T and type(got[1]) is T and got == (q, r)
        else:
            with pytest.raises(OverflowError):
                divmod(x, y)
        result = x / y
        assert type(result) is float and result == a / b  # correctly rounded, like int / int


def test_int_true_division_is_correctly_rounded_for_large_values():
    for a, b in ((2**64 - 1, 3), (2**63 - 1, 2**53 + 1), (2**64 - 1, 2**64 - 3)):
        assert rs.u64(a) / rs.u64(b) == a / b
    assert rs.i64(-(2**63)) / rs.i64(7) == -(2**63) / 7


@pytest.mark.parametrize("spec", INTS, ids=repr)
def test_int_operand_must_fit_the_type(spec):
    T = spec.cls
    big, small = spec.hi + 1, spec.lo - 1
    for f in BINOPS.values():
        with pytest.raises(OverflowError):
            f(T(0), big)
        with pytest.raises(OverflowError):
            f(small, T(1))
    if spec.signed:
        with pytest.raises(OverflowError):
            T(-1) + big  # the result would fit, but the operand does not
    with pytest.raises(OverflowError):
        T(0) + 2**200


@pytest.mark.parametrize("spec", INTS, ids=repr)
def test_int_rejects_other_operand_types(spec):
    T = spec.cls
    other_int = rs.i16 if T is not rs.i16 else rs.i32
    for other in (1.5, 1.0, "1", None, [1], other_int(1), rs.f64(1.0), 1j):
        for f in [*BINOPS.values(), operator.truediv, operator.pow, operator.lshift, operator.rshift, divmod]:
            if f is operator.mul and isinstance(other, (str, list)):
                continue  # sequence repetition via __index__, like int
            with pytest.raises(TypeError):
                f(T(1), other)
            with pytest.raises(TypeError):
                f(other, T(1))
    assert T(2) * [1] == [1, 1] and "ab" * T(2) == "abab"
    assert T(1) != other_int(1)  # different types never compare equal (NotImplemented)
    with pytest.raises(TypeError):
        T(1) < other_int(2)


@settings(max_examples=60)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_unary_ops(spec, data):
    a = data.draw(spec.values())
    T = spec.cls
    check_int(spec, lambda: -T(a), -a)
    check_int(spec, lambda: abs(T(a)), abs(a))
    assert +T(a) == a and type(+T(a)) is T
    inv = ~T(a)
    assert type(inv) is T and inv == spec.wrap(~a)  # bitwise complement within the width


@settings(max_examples=60)
@pytest.mark.parametrize("left", [True, False], ids=["shl", "shr"])
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_shift_operators(spec, left, data):
    a = data.draw(spec.values())
    n = data.draw(st.one_of(st.integers(-3, spec.bits + 3), st.sampled_from([2**40, -(2**70), 2**70])))
    T = spec.cls
    f = operator.lshift if left else operator.rshift
    if n < 0:
        expected = ValueError
    elif n >= spec.bits:
        expected = OverflowError
    else:
        # Rust semantics: bits shifted out are dropped; `>>` is arithmetic for signed types.
        expected = spec.wrap(a << n) if left else a >> n
    check_int(spec, lambda: f(T(a), n), expected)
    if spec.fits(n):
        check_int(spec, lambda: f(T(a), T(n)), expected)
        if n >= 0:
            check_int(spec, lambda: f(a, T(n)), expected)


@settings(max_examples=80)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
@example(data=None)
def test_int_pow(spec, data):
    T = spec.cls
    if data is None:
        cases = [(0, 0), (2, spec.bits), (-1, 2**70), (-1, 2**70 + 1), (1, 2**64), (0, 2**64), (3, 2**40), (0, -1), (2, -2)]
        cases = [(a, e) for a, e in cases if spec.fits(a)]
    else:
        cases = [(data.draw(spec.values()), data.draw(st.integers(-4, 70)))]
    for a, e in cases:
        if e < 0:
            if a == 0:
                with pytest.raises(ZeroDivisionError):
                    T(a) ** e
            else:
                result = T(a) ** e
                assert type(result) is float and result == a**e
            continue
        expected = a**e if abs(a) <= 1 or e <= 200 else 2**1000
        check_int(spec, lambda: T(a) ** e, expected)
        if spec.fits(e):
            check_int(spec, lambda: T(a) ** T(e), expected)
            check_int(spec, lambda: a ** T(e), expected)


@settings(max_examples=60)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_modular_pow(spec, data):
    T = spec.cls
    a, m = data.draw(spec.values()), data.draw(spec.values())
    e = data.draw(st.integers(-3, 300))
    try:
        expected = pow(a, e, m)
    except (ValueError, ZeroDivisionError) as exc:
        expected = type(exc)
    check_int(spec, lambda: pow(T(a), e, m), expected)
    check_int(spec, lambda: pow(T(a), e, T(m)), expected)
    if spec.fits(e):
        check_int(spec, lambda: pow(a, T(e), m), expected)


@settings(max_examples=60)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_int_comparisons_are_exact(spec, data):
    a = data.draw(spec.values())
    other = data.draw(
        st.one_of(
            spec.values(),
            st.integers(),
            st.integers(spec.lo - 3, spec.hi + 3),
            st.floats(),
            st.sampled_from([float(spec.hi), float(spec.lo), 2.0**63, 2.0**64, -(2.0**63), math.inf, -math.inf, math.nan, 0.5, -0.5]),
            st.booleans(),
        )
    )
    x = spec.cls(a)
    for f in (operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge):
        assert f(x, other) is f(a, other)
        assert f(other, x) is f(other, a)
        if isinstance(other, int) and spec.fits(other):
            assert f(x, spec.cls(other)) is f(a, other)


# --------------------------------------------------------------------------------------
# Rust-named families
# --------------------------------------------------------------------------------------

# name -> (kind, Python model on unbounded ints). kinds: "bin" (rhs of the type), "div"
# (rhs of the type, divisor), "count" (u32 count), "unary".
FAMILY_OPS = {
    "add": ("bin", operator.add),
    "sub": ("bin", operator.sub),
    "mul": ("bin", operator.mul),
    "div": ("div", trunc_div),
    "rem": ("div", trunc_rem),
    "div_euclid": ("div", euclid_div),
    "rem_euclid": ("div", euclid_rem),
    "neg": ("unary", operator.neg),
    "shl": ("count", None),
    "shr": ("count", None),
    "pow": ("count", None),
}

RUST_HAS = {
    "wrapping": {"add", "sub", "mul", "div", "rem", "div_euclid", "rem_euclid", "neg", "shl", "shr", "pow"},
    "checked": {"add", "sub", "mul", "div", "rem", "div_euclid", "rem_euclid", "neg", "shl", "shr", "pow"},
    "overflowing": {"add", "sub", "mul", "div", "rem", "div_euclid", "rem_euclid", "neg", "shl", "shr", "pow"},
    "saturating": {"add", "sub", "mul", "div", "pow"},
}


def family_cases():
    for family, ops in RUST_HAS.items():
        for op in sorted(ops):
            yield family, op


def rem_overflows(spec, op, a, b):
    """Rust reports overflow for `MIN % -1` although the remainder (0) fits."""
    return op in ("rem", "rem_euclid") and spec.signed and a == spec.lo and b == -1


@settings(max_examples=30)
@pytest.mark.parametrize(("family", "op"), list(family_cases()), ids=lambda x: str(x))
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_rust_families_match_model(spec, family, op, data):
    kind, model = FAMILY_OPS[op]
    x = spec.cls(data.draw(spec.values()))
    a = int(x)
    method = getattr(x, f"{family}_{op}")
    if kind == "unary":
        args, b = (), None
        exact = model(a)
    elif kind == "count":
        b = data.draw(st.one_of(st.integers(0, spec.bits + 2), st.integers(0, 2**32 - 1)))
        args = (b,)
        if op == "pow":
            b = b % 80 if family != "saturating" else b
            args = (b,)
            exact = a**b if abs(a) <= 1 or b <= 200 else (2**1000 if a > 0 or b % 2 == 0 else -(2**1000))
        else:
            exact = None
    else:
        b = data.draw(spec.values())
        args = (data.draw(st.sampled_from([b, spec.cls(b)])),)
        if kind == "div" and b == 0:
            if family == "checked":
                assert method(*args) is None
            else:
                with pytest.raises(ZeroDivisionError):
                    method(*args)
            return
        exact = model(a, b)

    result = method(*args)
    if op in ("shl", "shr"):
        n = b % spec.bits
        wrapped = spec.wrap(a << n) if op == "shl" else a >> n
        too_far = b >= spec.bits
        expected = {
            "wrapping": wrapped,
            "checked": None if too_far else wrapped,
            "overflowing": (wrapped, too_far),
        }[family]
    else:
        overflow = not spec.fits(exact) or rem_overflows(spec, op, a, b)
        expected = {
            "wrapping": spec.wrap(exact),
            "checked": None if overflow else exact,
            "saturating": spec.clamp(exact),
            "overflowing": (spec.wrap(exact), overflow),
        }[family]
    if expected is None:
        assert result is None
    elif isinstance(expected, tuple):
        assert type(result) is tuple and type(result[0]) is spec.cls and type(result[1]) is bool
        assert result == expected
    else:
        assert type(result) is spec.cls and result == expected


@settings(max_examples=50)
@pytest.mark.parametrize("spec", SIGNED, ids=repr)
@given(data=st.data())
def test_signed_only_families(spec, data):
    a = data.draw(spec.values())
    x = spec.cls(a)
    assert x.wrapping_abs() == spec.wrap(abs(a))
    assert x.checked_abs() == (abs(a) if spec.fits(abs(a)) else None)
    assert x.overflowing_abs() == (spec.wrap(abs(a)), not spec.fits(abs(a)))
    assert x.saturating_abs() == spec.clamp(abs(a))
    assert x.saturating_neg() == spec.clamp(-a)


def test_unsigned_types_lack_signed_only_methods():
    for name in ("saturating_neg", "saturating_abs", "wrapping_abs", "checked_abs", "overflowing_abs"):
        assert not hasattr(rs.u8(1), name)
        assert hasattr(rs.i8(1), name)
    for name in ("saturating_rem", "saturating_shl"):
        assert not hasattr(rs.i8(1), name)  # Rust has no such method either


@settings(max_examples=60)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_panicking_rust_methods(spec, data):
    a, b = data.draw(spec.values()), data.draw(spec.values())
    x = spec.cls(a)
    if b == 0:
        for name in ("div_euclid", "rem_euclid"):
            with pytest.raises(ZeroDivisionError):
                getattr(x, name)(b)
    else:
        check_int(spec, lambda: x.div_euclid(b), euclid_div(a, b))
        expected = OverflowError if rem_overflows(spec, "rem", a, b) else euclid_rem(a, b)
        check_int(spec, lambda: x.rem_euclid(b), expected)
    e = data.draw(st.integers(0, 70))
    check_int(spec, lambda: x.pow(e), a**e)


@settings(max_examples=60)
@pytest.mark.parametrize("spec", INTS, ids=repr)
@given(data=st.data())
def test_bit_methods(spec, data):
    a = data.draw(spec.values())
    x = spec.cls(a)
    u = a % 2**spec.bits  # two's complement bit pattern
    bits = spec.bits
    assert x.count_ones() == bin(u).count("1")
    assert x.count_zeros() == bits - bin(u).count("1")
    assert x.leading_zeros() == bits - u.bit_length()
    assert x.trailing_zeros() == (bits if u == 0 else (u & -u).bit_length() - 1)
    assert x.leading_ones() == next((i for i in range(bits) if not (u >> (bits - 1 - i)) & 1), bits)
    assert x.trailing_ones() == next((i for i in range(bits) if not (u >> i) & 1), bits)
    n = data.draw(st.integers(0, 3 * bits))
    k = n % bits
    rotl = ((u << k) | (u >> (bits - k))) % 2**bits
    assert x.rotate_left(n) == spec.wrap(rotl)
    assert x.rotate_right(n) == spec.wrap(((u >> k) | (u << (bits - k))) % 2**bits)
    be = u.to_bytes(bits // 8, "big")
    assert x.swap_bytes() == spec.wrap(int.from_bytes(be, "little"))
    assert x.reverse_bits() == spec.wrap(int(format(u, f"0{bits}b")[::-1], 2))
    assert type(x.to_be_bytes()) is bytes
    assert x.to_be_bytes() == be
    assert x.to_le_bytes() == be[::-1]
    assert x.to_ne_bytes() == (be[::-1] if sys.byteorder == "little" else be)
    T = spec.cls
    assert T.from_be_bytes(be) == a and type(T.from_be_bytes(be)) is T
    assert T.from_le_bytes(bytearray(be[::-1])) == a
    assert T.from_ne_bytes(memoryview(x.to_ne_bytes())) == a


@pytest.mark.parametrize("spec", INTS, ids=repr)
def test_method_argument_errors(spec):
    x = spec.cls(1)
    with pytest.raises(OverflowError):
        x.wrapping_add(spec.hi + 1)
    with pytest.raises(TypeError):
        x.wrapping_add(1.0)
    other = rs.u8 if spec.cls is not rs.u8 else rs.i8
    with pytest.raises(TypeError):
        x.checked_mul(other(1))
    for method in (x.wrapping_shl, x.checked_pow, x.rotate_left):
        with pytest.raises(ValueError):
            method(-1)
        with pytest.raises(OverflowError):
            method(2**32)
        with pytest.raises(TypeError):
            method(1.0)
    assert x.wrapping_shl(rs.u8(1)) == 2  # counts accept any integer
    for name in ("from_be_bytes", "from_le_bytes", "from_ne_bytes"):
        with pytest.raises(ValueError):
            getattr(spec.cls, name)(b"\x00" * (spec.bits // 8 + 1))
        with pytest.raises(TypeError):
            getattr(spec.cls, name)("ab")


# --------------------------------------------------------------------------------------
# Float scalars
# --------------------------------------------------------------------------------------


def r32(x):
    """Round a Python float to binary32 (IEEE: overflow goes to infinity)."""
    try:
        return struct.unpack("<f", struct.pack("<f", x))[0]
    except OverflowError:
        return math.copysign(math.inf, x)


def same_float(a, b):
    return (math.isnan(a) and math.isnan(b)) or (a == b and math.copysign(1, a) == math.copysign(1, b))


def int_to_f32(n):
    """The binary32 nearest to the int `n` (ties to even), or None if it overflows."""
    m = abs(n)
    if m.bit_length() > 24:
        shift = m.bit_length() - 24
        q, rem = divmod(m, 2**shift)
        half = 2 ** (shift - 1)
        if rem > half or (rem == half and q % 2):
            q += 1
        m = q << shift
    if m > 2**128 - 2**104:
        return None
    return math.copysign(float(m), n) if n else 0.0


f32_values = st.floats(width=32)
f64_values = st.floats()
float_or_int = st.one_of(st.floats(), st.integers(-(2**70), 2**70), st.sampled_from([0, 1, -1, 2, 0.5, -0.0, math.inf, -math.inf, math.nan]))


def test_float_class_metadata():
    for cls, info in ((rs.f32, (3.4028234663852886e38, 1.1920928955078125e-07, 1.1754943508222875e-38)), (rs.f64, (sys.float_info.max, sys.float_info.epsilon, sys.float_info.min))):
        mx, eps, tiny = info
        assert cls.__module__ == "rustique"
        assert cls.MAX == mx and cls.MIN == -mx and cls.EPSILON == eps and cls.MIN_POSITIVE == tiny
        assert cls.INFINITY == math.inf and cls.NEG_INFINITY == -math.inf and math.isnan(cls.NAN)
        assert all(type(getattr(cls, n)) is cls for n in ("MIN", "MAX", "EPSILON", "INFINITY", "NAN", "MIN_POSITIVE"))
    assert repr(rs.f32.MAX) == "f32(3.4028235e+38)" and repr(rs.f64.NAN) == "f64(nan)"


@given(x=float_or_int)
def test_float_construction(x):
    assert same_float(rs.f64(x).value, float(x))  # ints: correctly rounded, like float(int)
    if isinstance(x, int):
        expected = int_to_f32(x)
        if expected is None:
            with pytest.raises(OverflowError):
                rs.f32(x)
        else:
            assert rs.f32(x).value == expected
        return
    if math.isfinite(x) and math.isinf(r32(x)):
        with pytest.raises(OverflowError):
            rs.f32(x)
    else:
        assert same_float(rs.f32(x).value, r32(x))


def test_float_construction_edge_cases():
    assert rs.f64() == 0.0 and rs.f32() == 0.0
    assert rs.f32(rs.i64(2**63 - 1)) == 2.0**63 and rs.f64(rs.u8(7)) == 7.0
    assert rs.f32(rs.f64(0.1)).value == r32(0.1)
    assert rs.f64(rs.f32(0.1)).value == r32(0.1)
    with pytest.raises(OverflowError):
        rs.f32(rs.f64(1e300))
    with pytest.raises(OverflowError):
        rs.f64(10**400)
    for bad in ("1.5", b"1", None, [1.0], 1j):
        with pytest.raises(TypeError):
            rs.f64(bad)
        with pytest.raises(TypeError):
            rs.f32(bad)
    from decimal import Decimal
    from fractions import Fraction

    assert rs.f64(Fraction(1, 3)) == 1 / 3 and rs.f64(Decimal("0.1")) == 0.1
    assert rs.f32(2**128 - 2**104 - 1) == rs.f32.MAX  # rounds down to MAX
    with pytest.raises(OverflowError):
        rs.f32(2**128 - 2**103)  # the halfway point rounds to infinity


@given(x=f64_values)
@example(x=2.0**-25)
@example(x=1e16)
@example(x=1e-5)
@example(x=-0.0)
def test_f64_str_repr_hash(x):
    y = rs.f64(x)
    assert str(y) == repr(x) and repr(y) == f"f64({x!r})"
    if not math.isnan(x):
        assert hash(y) == hash(x)
        assert y == x and not (y != x)
    assert format(y, "") == format(x, "")
    for spec in (".3f", "e", "+.2g", ">12", ",", "%", ".0f", "_"):
        if spec in ("_", ",") and not math.isfinite(x):
            continue
        assert format(y, spec) == format(x, spec)
    assert pickle.loads(pickle.dumps(y)) == y or math.isnan(x)


@given(x=f32_values)
@example(x=r32(0.1))
@example(x=2.0**-25)
@example(x=16777216.0)
@example(x=3.4028234663852886e38)
def test_f32_str_repr_hash(x):
    y = rs.f32(x)
    s = str(y)
    assert repr(y) == f"f32({s})"
    if math.isnan(x):
        assert s == "nan"
        return
    assert r32(float(s)) == x or (x == 0 and float(s) == 0)  # round-trips as binary32
    assert hash(y) == hash(x)
    assert format(y, "") == s and f"{y}" == s
    assert format(y, ">20") == format(float(s), ">20")
    assert format(y, ".4e") == format(x, ".4e")
    assert pickle.loads(pickle.dumps(y)) == y
    if np is not None:
        from decimal import Decimal

        if math.isfinite(x):
            assert Decimal(s) == Decimal(np.format_float_scientific(np.float32(x), unique=True))


def test_float_str_matches_python_layout():
    cases = {0.1: "0.1", 1e16: "1e+16", 1e15: "1000000000000000.0", 1e-5: "1e-05", 0.0001: "0.0001", -0.0: "-0.0", math.inf: "inf", -math.inf: "-inf", math.nan: "nan"}
    for x, s in cases.items():
        assert str(rs.f64(x)) == s and str(rs.f32(x)) == s
    assert str(rs.f32(1 / 3)) == "0.33333334" and str(rs.f32(2.0**-149)) == "1e-45"


def test_nan_hash_is_identity_based():
    a, b = rs.f64(math.nan), rs.f64(math.nan)
    assert hash(a) == hash(a) and a != a
    assert hash(a) != hash(b)


def float_ref_binop(op, a, b):
    """Python float semantics for one binary operator (a, b Python floats)."""
    try:
        if op == "/":
            return a / b
        if op == "**":
            r = a**b
            return ValueError if isinstance(r, complex) else r
        return BINOPS[op](a, b) if op != "divmod" else divmod(a, b)
    except ZeroDivisionError:
        return ZeroDivisionError
    except OverflowError:
        return OverflowError


FLOAT_OPS = ["+", "-", "*", "/", "//", "%", "**", "divmod"]


def apply(op, x, y):
    if op == "/":
        return x / y
    if op == "**":
        return x**y
    if op == "divmod":
        return divmod(x, y)
    return BINOPS[op](x, y)


def check_float(cls, thunk, expected, rounder):
    if isinstance(expected, type):
        with pytest.raises(expected):
            thunk()
        return
    result = thunk()
    if isinstance(expected, tuple):
        assert type(result) is tuple
        for r, e in zip(result, expected):
            assert type(r) is cls and same_float(r.value, rounder(e)), (r, e)
    else:
        assert type(result) is cls and same_float(result.value, rounder(expected)), (result, expected)


@settings(max_examples=150)
@pytest.mark.parametrize("op", FLOAT_OPS)
@given(a=f64_values, b=st.one_of(f64_values, st.integers(-(2**60), 2**60), st.sampled_from([0, 1, -1, 2, 0.5, 3.0, -0.0, math.inf])))
def test_f64_ops_match_python_float(op, a, b):
    T = rs.f64
    expected = float_ref_binop(op, a, float(b))
    check_float(T, lambda: apply(op, T(a), b), expected, float)
    check_float(T, lambda: apply(op, T(a), T(b)), expected, float)
    expected_r = float_ref_binop(op, float(b), a)
    check_float(T, lambda: apply(op, b, T(a)), expected_r, float)


@settings(max_examples=150)
@pytest.mark.parametrize("op", ["+", "-", "*", "/"])
@given(a=f32_values, b=st.one_of(f32_values, st.integers(-(2**24), 2**24)))
def test_f32_arithmetic_rounds_to_single_precision(op, a, b):
    T = rs.f32
    b32 = r32(float(b))
    expected = float_ref_binop(op, a, b32)  # exact op in double, then one rounding
    check_float(T, lambda: apply(op, T(a), b), expected, r32)
    check_float(T, lambda: apply(op, T(a), T(b32)), expected, r32)


def f32_divmod_model(a, b):
    """CPython's `_float_div_mod`, with every operation rounded to binary32."""
    mod = math.fmod(a, b)
    div = r32(r32(a - mod) / b)
    if mod:
        if (b < 0) != (mod < 0):
            mod = r32(mod + b)
            div = r32(div - 1.0)
    else:
        mod = math.copysign(0.0, b)
    if div:
        floordiv = math.floor(div) if math.isfinite(div) else div
        floordiv = float(floordiv)
        if div - floordiv > 0.5:
            floordiv = r32(floordiv + 1.0)
    else:
        floordiv = math.copysign(0.0, r32(a / b))
    return floordiv, mod


@settings(max_examples=200)
@given(a=f32_values, b=f32_values)
def test_f32_floor_division_and_modulo(a, b):
    T = rs.f32
    if b == 0:
        for f in (operator.floordiv, operator.mod, divmod):
            with pytest.raises(ZeroDivisionError):
                f(T(a), T(b))
        return
    assume(math.isfinite(a) and math.isfinite(b))
    q, m = f32_divmod_model(a, b)
    got = divmod(T(a), T(b))
    assert same_float(got[0].value, q) and same_float(got[1].value, m)
    assert same_float((T(a) // T(b)).value, q) and same_float((T(a) % T(b)).value, m)
    if np is not None:
        with np.errstate(all="ignore"):
            nq, nm = np.divmod(np.float32(a), np.float32(b))
        assert same_float(float(nm), m)
        assert same_float(float(nq), q)


def test_float_pow_semantics():
    for T in (rs.f32, rs.f64):
        assert T(2) ** 10 == 1024 and type(T(2) ** 0.5) is T
        assert T(0) ** 0 == 1 and T(math.nan) ** 0 == 1 and T(1) ** math.nan == 1
        assert T(-2) ** 3 == -8 and T(-2) ** -1 == -0.5
        with pytest.raises(ValueError):
            T(-8) ** 0.5  # complex in Python
        with pytest.raises(ZeroDivisionError):
            T(0) ** -1
        with pytest.raises(OverflowError):
            T(10) ** 400
        with pytest.raises(TypeError):
            pow(T(2), 2, 5)
        assert 2 ** T(3) == 8 and type(2 ** T(3)) is T
    assert rs.f32(2) ** 127 == 2.0**127
    with pytest.raises(OverflowError):
        rs.f32(2) ** 128  # overflows binary32, not binary64


@settings(max_examples=100)
@given(a=f64_values, b=float_or_int)
def test_f64_comparisons_are_exact(a, b):
    x = rs.f64(a)
    for f in (operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge):
        assert f(x, b) is f(a, b)
        assert f(b, x) is f(b, a)
        if isinstance(b, float):
            assert f(x, rs.f64(b)) is f(a, b)


@settings(max_examples=100)
@given(a=f32_values, b=st.one_of(float_or_int, st.integers()))
def test_f32_comparisons_use_the_exact_value(a, b):
    x = rs.f32(a)
    for f in (operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge):
        assert f(x, b) is f(a, b)
        assert f(b, x) is f(b, a)


def test_float_comparison_examples():
    assert rs.f32(0.1) != 0.1 and rs.f32(0.1) == r32(0.1)
    assert rs.f64(2.0**63) == 2**63 and rs.f64(2.0**63) != 2**63 + 1
    assert rs.f64(2.0**63) < 2**63 + 1 and rs.f64(1e308) < 10**400
    assert rs.f64(math.inf) > 10**400 and rs.f64(-math.inf) < -(10**400)
    assert not (rs.f64(math.nan) == 10**400) and rs.f64(math.nan) != 10**400
    assert rs.f64(1.0) == True  # noqa: E712
    with pytest.raises(TypeError):
        rs.f32(1.0) < rs.f64(2.0)
    assert rs.f32(1.0) != rs.f64(1.0)


def test_float_operand_rules():
    for T in (rs.f32, rs.f64):
        assert type(T(1) + 1) is T and type(1.5 + T(1)) is T and type(T(1) * True) is T
        for other in ("1", None, rs.i8(1), [1], 1j):
            for f in (operator.add, operator.truediv, operator.floordiv, operator.pow):
                with pytest.raises(TypeError):
                    f(T(1), other)
                with pytest.raises(TypeError):
                    f(other, T(1))
            with pytest.raises(TypeError):
                T(1) < other
    with pytest.raises(OverflowError):
        rs.f32(1) + 1e300
    assert math.isinf((rs.f32.MAX * 2).value)  # arithmetic overflow is IEEE, not an error
    for f in (operator.lshift, operator.and_, operator.or_, operator.xor, operator.invert):
        with pytest.raises(TypeError):
            f(rs.f64(1), 1) if f is not operator.invert else f(rs.f64(1))


@given(x=st.one_of(f64_values, f32_values))
def test_float_conversions_and_methods(x):
    for T, rnd in ((rs.f64, float), (rs.f32, r32)):
        y = T(x) if not (T is rs.f32 and math.isfinite(x) and math.isinf(r32(x))) else None
        if y is None:
            continue
        v = rnd(x)
        assert y.value == v or math.isnan(v)
        assert float(y) == v or math.isnan(v)
        assert bool(y) is bool(v)
        if math.isnan(v):
            with pytest.raises(ValueError):
                int(y)
        elif math.isinf(v):
            with pytest.raises(OverflowError):
                int(y)
            with pytest.raises(OverflowError):
                round(y)
        else:
            assert int(y) == int(v) and type(int(y)) is int
            assert math.floor(y) == math.floor(v) and math.ceil(y) == math.ceil(v) and math.trunc(y) == math.trunc(v)
            assert round(y) == round(v) and type(round(y)) is int
            assert same_float(round(y, 2).value, rnd(round(v, 2)))
        assert y.is_nan() is math.isnan(v)
        assert y.is_infinite() is math.isinf(v)
        assert y.is_finite() is math.isfinite(v)
        if not math.isnan(v):
            assert y.is_sign_negative() is (math.copysign(1, v) < 0)
        assert same_float(abs(y).value, abs(v)) and same_float((-y).value, -v) and (+y) is y
        if math.isfinite(v) and v >= 0:
            assert y.sqrt().value == rnd(math.sqrt(v))
        if math.isfinite(v):
            assert y.floor().value == math.floor(v) and y.ceil().value == math.ceil(v)
            assert y.trunc().value == math.trunc(v)
        bits = struct.unpack("<Q" if T is rs.f64 else "<I", struct.pack("<d" if T is rs.f64 else "<f", v))[0]
        if not math.isnan(v):
            assert y.to_bits() == bits and T.from_bits(bits) == y
            assert T.from_be_bytes(y.to_be_bytes()) == y and T.from_le_bytes(y.to_le_bytes()) == y
            assert T.from_ne_bytes(y.to_ne_bytes()) == y
        assert y.to_le_bytes() == struct.pack("<d" if T is rs.f64 else "<f", v) or math.isnan(v)


def test_float_methods_compute_in_own_precision():
    assert rs.f32(2).sqrt().value == r32(math.sqrt(2)) and rs.f64(2).sqrt() == math.sqrt(2)
    assert math.isnan(rs.f64(-1).sqrt().value)  # Rust semantics, not math.sqrt's ValueError
    assert rs.f32(1.5).fract() == 0.5 and rs.f64(-1.5).ceil() == -1.0
    assert rs.f64(3).copysign(-0.0) == -3.0 and rs.f32(2).powi(3) == 8 and rs.f32(4).powf(0.5) == 2
    assert rs.f64(1).min(rs.f64(math.nan)) == 1 and rs.f64(1).max(2) == 2
    with pytest.raises(TypeError):
        rs.f64(1).copysign("x")
    assert rs.f32.from_bits(0x3F800000) == 1.0
    with pytest.raises(OverflowError):
        rs.f32.from_bits(2**32)
    with pytest.raises(ValueError):
        rs.f64.from_le_bytes(b"\x00" * 4)


def test_float_round_and_format_details():
    assert round(rs.f64(2.5)) == 2 and round(rs.f64(2.675), 2) == rs.f64(2.67)
    assert type(round(rs.f32(1.25), 1)) is rs.f32
    assert format(rs.f32(0.1), "+") == "+0.1" and format(rs.f32(0.1), "*^9") == "***0.1***"
    assert format(rs.f32(0.1), ".20f") == format(r32(0.1), ".20f")
    assert format(rs.f64(1234.5), ",.1f") == "1,234.5"


def test_concurrent_use_from_threads():
    # Scalars are immutable; on free-threaded builds they must be safe to share.
    import threading

    shared = [rs.i64(3), rs.u8(7), rs.f32(0.5), rs.f64(2.0)]
    errors = []

    def work(seed):
        try:
            acc, model = rs.i64(seed), seed
            for i in range(2000):
                acc = acc.wrapping_mul(shared[0]).wrapping_add(i)
                model = INTS[3].wrap(model * 3 + i)
                assert (shared[1] + 1) == 8 and hash(shared[2]) == hash(0.5)
                assert str(shared[3] * 2) == "4.0" and shared[0] < 4.5
            assert acc == model
        except Exception as exc:  # pragma: no cover - reported below
            errors.append(exc)

    threads = [threading.Thread(target=work, args=(n,)) for n in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors


# --------------------------------------------------------------------------------------
# Regressions from the historical `i8` (ARCHAEOLOGY §5)
# --------------------------------------------------------------------------------------


def test_historical_i8_defects_are_fixed():
    assert type(rs.i8(42) + 1) is rs.i8  # arithmetic used to return plain int
    assert rs.i8(-7) // 2 == -4 and rs.i8(-7) % 2 == 1  # `//` used to truncate
    assert rs.i8(-7).checked_div(2) == -3  # Rust's truncating division stays available by name
    assert hash(rs.i8(5)) == hash(5) and {rs.i8(5): 1}[5] == 1  # used to be unhashable
    with pytest.raises(TypeError):
        rs.i8("5")  # used to parse strings
    with pytest.raises(TypeError):
        rs.i8(5.7)  # used to truncate floats
