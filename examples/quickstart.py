"""Rustique in two minutes: builtin-compatible containers with native storage.

Run: python examples/quickstart.py
"""

import rustique as rs
from rustique import Dict, List, Set, X


def main() -> None:
    # A List behaves like a list ...
    nums = List([5, 3, 8, 1])
    nums.append(13)
    nums.sort()
    assert nums == [1, 3, 5, 8, 13]
    assert nums[-1] == 13 and nums[1:3] == [3, 5]
    assert repr(nums) == "List([1, 3, 5, 8, 13])"

    # ... but homogeneous data is stored unboxed in Rust (here a Vec<i64>).
    assert nums.strategy == "i64"
    anything: List[int | str] = List([1, 2])
    anything.append("mixed")  # still fine: the storage demotes to Python objects, once
    assert anything.strategy == "object"
    assert anything == [1, 2, "mixed"]

    # Work that touches every element is one Rust call.
    assert nums.sum() == 30
    assert nums.min() == 1 and nums.max() == 13
    assert nums.map(X * 2 + 1) == [3, 7, 11, 17, 27]
    assert nums.filter(X % 2 == 1) == [1, 3, 5, 13]

    # Typed lists validate once at the boundary and pin their storage.
    prices = List[float]([9.99, 5, 12.5])  # ints convert to float
    assert prices.to_list() == [9.99, 5.0, 12.5]
    try:
        prices.append("free")  # type: ignore[arg-type]  # rejected at runtime, too
    except TypeError as e:
        assert str(e).startswith("List[float] expects float, got str")
    else:
        raise AssertionError("expected TypeError")

    # Ranges are materialised natively, without per-element conversion.
    big = rs.arange(1_000_000)
    assert len(big) == 1_000_000 and big.sum() == 499_999_500_000

    # Dict and Set follow the builtin semantics, including key equivalence.
    counts: Dict[str, int] = Dict()
    for word in "the quick the lazy the end".split():
        counts[word] = counts.get(word, 0) + 1
    assert counts == {"the": 3, "quick": 1, "lazy": 1, "end": 1}
    assert list(counts) == ["the", "quick", "lazy", "end"]  # insertion ordered

    seen = Set([1, 2, 3])
    assert seen | {4} == {1, 2, 3, 4}
    assert seen.issubset(range(10))

    # Interop: convert back to builtins whenever you like.
    assert type(nums.to_list()) is list
    assert counts.to_dict() == {"the": 3, "quick": 1, "lazy": 1, "end": 1}
    print("quickstart: ok")


if __name__ == "__main__":
    main()
