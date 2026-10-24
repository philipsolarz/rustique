"""Fixed-width scalars with Rust semantics (ARCHITECTURE §4.4).

Run: python examples/scalars.py
"""

import rustique as rs
from rustique import List, i8, u8, u32, f32


def main() -> None:
    # Operators keep the type and check for overflow ...
    a = i8(100)
    b = a + 27
    assert b == 127 and type(b) is i8 and repr(b) == "i8(127)"
    try:
        _ = a + 28
    except OverflowError:
        pass
    else:
        raise AssertionError("i8 overflow must raise")

    # ... with Python's meaning: floor division, modulo with the divisor's sign, / is float.
    assert i8(-7) // 2 == -4 and i8(-7) % 2 == 1
    assert i8(7) / 2 == 3.5 and type(i8(7) / 2) is float

    # Rust's explicit overflow families are available by name.
    assert u8(250).wrapping_add(10) == u8(4)
    assert u8(250).checked_add(10) is None
    assert u8(250).saturating_add(10) == u8.MAX
    assert u8(250).overflowing_add(10) == (u8(4), True)
    assert i8.MIN.wrapping_abs() == i8.MIN
    assert i8(-7).rem_euclid(2) == i8(1)

    # Bits and bytes.
    assert u32(1).rotate_right(1) == u32(2**31)
    assert u8(0b1011_0000).leading_zeros() == 0 and u8(0b1011_0000).count_ones() == 3
    assert u32(0x12345678).to_be_bytes() == b"\x12\x34\x56\x78"
    assert u32.from_le_bytes(b"\x78\x56\x34\x12") == u32(0x12345678)
    assert u8.BITS == 8 and int(u8.MAX) == 255

    # Hashing and equality agree with the equal Python number.
    assert hash(i8(5)) == hash(5) and i8(5) == 5
    assert {u8(1): "one"}[u8(1)] == "one"

    # f32 rounds to single precision; repr shows the shortest round-tripping digits.
    tenth = f32(0.1)
    assert str(tenth) == "0.1" and float(tenth) != 0.1
    assert type(tenth * 2) is f32

    # Scalar classes double as element-type markers for native storage.
    pixels = List[u8]([0, 127, 255])
    assert pixels.strategy == "u8"
    first: int = pixels[0]  # reads are plain ints
    assert first == 0
    assert pixels.sum() == 382  # an exact Python int, no wrap-around
    halfs = List[rs.f32]([0.5, 0.25])
    assert halfs.sum() == 0.75
    print("scalars: ok")


if __name__ == "__main__":
    main()
