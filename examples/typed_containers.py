"""Typed containers: schemas, validation, coercion and nesting (ARCHITECTURE §3.4-3.5).

Run: python examples/typed_containers.py
"""

from dataclasses import dataclass

import rustique as rs
from rustique import Dict, List, Set


@dataclass(frozen=True)
class Point:
    x: float
    y: float


def expect_error(exc: type[Exception], fn: object, *args: object) -> str:
    assert callable(fn)
    try:
        fn(*args)
    except exc as e:
        return str(e)
    raise AssertionError(f"expected {exc.__name__}")


def main() -> None:
    # Aliases are cached, introspectable and callable.
    assert List[int] is List[int]
    ints = List[int]([1, 2, 3])
    assert ints.schema_type is List[int]
    assert isinstance(ints, List)
    assert repr(ints) == "List[int]([1, 2, 3])"

    # Validation messages say what was expected, what arrived, and where.
    msg = expect_error(TypeError, List[int], [1, 2, "three"])
    assert msg == "List[int] expects int, got str at index 2", msg
    assert ints == [1, 2, 3]

    # bool is an int (stored as int); floats are not.
    ints.append(True)
    assert ints[-1] == 1 and type(ints[-1]) is int
    expect_error(TypeError, ints.append, 1.5)

    # Python ints of any size are accepted; storage demotes beyond 64 bits.
    ints.append(2**100)
    assert ints.strategy == "object" and ints[-1] == 2**100

    # Fixed-width element types store natively and range-check.
    pixels = List[rs.u8]([0, 128, 255])
    assert pixels.strategy == "u8"
    value: int = pixels[1]  # reads are plain Python ints
    assert value == 128
    expect_error(OverflowError, pixels.append, 256)

    # Optional / Union element types.
    maybe = List[int | None]([1, None, 3])
    assert maybe.count(None) == 1
    labels = List[int | str]([1, "two"])
    expect_error(TypeError, labels.append, 3.0)

    # Nested containers convert any iterable into the declared rustique type.
    matrix = List[List[float]]([[1, 2], (3, 4)])
    row = matrix[0]
    assert type(row) is List and row.schema_type is List[float]
    assert matrix == [[1.0, 2.0], [3.0, 4.0]]

    # Arbitrary classes are checked with isinstance.
    path = List[Point]([Point(0, 0), Point(1, 1)])
    expect_error(TypeError, path.append, (2, 2))

    # Typed dicts and sets.
    scores = Dict[str, List[float]]({"ann": List([9.5, 8.0])})  # untyped List -> List[float]
    scores["bob"] = List[float]([7])
    assert scores["ann"] == [9.5, 8.0] and scores["ann"].schema_type is List[float]
    expect_error(TypeError, scores.__setitem__, 1, [1.0])
    assert scores.get(1) is None  # type: ignore[call-overload]  # a foreign key is a miss, never an error

    tags = Set[str](["a", "b"])
    tags.add("c")
    expect_error(TypeError, tags.add, 1)
    assert tags == {"a", "b", "c"}

    # Derived containers keep their schema; mixing schemas gives an untyped List.
    assert ints[1:].schema_type is List[int]
    mixed = List[int]([1]) + List[float]([2.5])
    assert mixed.schema_type is List
    print("typed_containers: ok")


if __name__ == "__main__":
    main()
