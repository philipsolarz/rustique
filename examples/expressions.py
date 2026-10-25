"""Vectorised expressions over the element placeholder `X` (ARCHITECTURE §4.5).

`lst.map(expr)` / `lst.filter(expr)` evaluate column-at-a-time in Rust. Any Python
callable works too, but runs per element (the documented slow path).

Run: python examples/expressions.py
"""

import math

import rustique as rs
from rustique import List, X


def main() -> None:
    xs = List[int](range(10))

    # Arithmetic with Python semantics: floor division and modulo, true division -> float.
    assert xs.map(X * 2 + 1) == [2 * x + 1 for x in range(10)]
    assert xs.map(X // 3) == [x // 3 for x in range(10)]
    assert List([-7, 7]).map(X % 3) == [-7 % 3, 7 % 3]
    halves = xs.map(X / 2)
    assert halves == [x / 2 for x in range(10)] and halves.strategy == "f64"
    assert xs.map(-X + abs(X - 5)) == [-x + abs(x - 5) for x in range(10)]
    assert xs.map(2**X) == [2**x for x in range(10)]

    # Comparisons produce booleans; combine them with & | ~ (not `and`/`or`).
    assert xs.filter((X > 2) & (X % 2 == 0)) == [4, 6, 8]
    assert xs.filter((X < 2) | (X >= 8)) == [0, 1, 8, 9]
    assert xs.filter(~(X == 0)) == list(range(1, 10))
    assert xs.map(X > 4).strategy == "bool"

    # Expressions are values: build them once, reuse, inspect.
    affine = X * 3 - 1
    assert repr(affine) == "((X * 3) - 1)"
    assert xs.map(affine).sum() == sum(3 * x - 1 for x in range(10))

    # `bool(expr)` is an error, so `if X > 1:` mistakes fail loudly.
    try:
        bool(X > 1)
    except TypeError:
        pass
    else:
        raise AssertionError("bool(Expr) must raise")

    # Floats: NaN checks and casts.
    data = List([1.5, math.nan, -2.25])
    assert data.filter(~X.is_nan()) == [1.5, -2.25]
    assert data.filter(~X.is_nan()).map(X.cast(int)) == [1, -2]
    small = List[int]([1, 2, 300]).map(X.cast(rs.i16))
    assert small.strategy == "i16" and small == [1, 2, 300]

    # Division by zero raises like Python.
    try:
        _ = xs.map(1 // X)
    except ZeroDivisionError:
        pass
    else:
        raise AssertionError("expected ZeroDivisionError")

    # Python ints never overflow: i64 overflow falls back to exact arithmetic.
    big = List[int]([2**62, 2**62]).map(X * 4)
    assert big == [2**64, 2**64]

    # Any callable works (slow path), and strings use it too.
    words = List(["apple", "kiwi", "banana"])
    assert words.map(str.upper) == ["APPLE", "KIWI", "BANANA"]
    assert words.filter(lambda w: len(w) > 4) == ["apple", "banana"]
    print("expressions: ok")


if __name__ == "__main__":
    main()
