"""Zero-copy interop with NumPy and the buffer protocol (ARCHITECTURE §1.6, §4.1).

Run: python examples/numpy_interop.py   (requires numpy)
"""

import array

import numpy as np

import rustique as rs
from rustique import List


def main() -> None:
    # Rust -> Python without per-element conversion, then -> NumPy without a copy.
    lst = rs.arange(1_000_000)
    arr = np.asarray(lst)
    assert arr.dtype == np.int64 and arr.shape == (1_000_000,)
    assert np.asarray(lst).ctypes.data == arr.ctypes.data  # same memory, no copy
    assert int(arr.sum()) == lst.sum()

    # Exports are read-only, and the List is frozen while any export is alive.
    assert not arr.flags.writeable
    try:
        lst.append(1)
    except BufferError as e:
        assert "buffer export" in str(e)
    else:
        raise AssertionError("mutating an exported List must raise BufferError")
    del arr
    lst.append(1)  # the export is gone: mutation works again

    # memoryview works the same way (and is released deterministically with `with`).
    floats = List[float]([0.5, 1.5, 2.5])
    with memoryview(floats) as mv:
        assert mv.format == "d" and mv.readonly and mv.tolist() == [0.5, 1.5, 2.5]
    floats.append(3.5)

    # Python -> Rust: buffer-protocol inputs are ingested with a memcpy.
    from_np = List(np.linspace(0.0, 1.0, 5))
    assert from_np.strategy == "f64" and from_np[-1] == 1.0
    from_array = List[rs.i32](array.array("i", [1, 2, 3]))
    assert from_array.strategy == "i32" and from_array == [1, 2, 3]
    pixels = List[rs.u8](np.arange(4, dtype=np.uint8))
    assert bytes(pixels) == b"\x00\x01\x02\x03"

    # Every numeric element type maps onto the matching NumPy dtype.
    assert np.asarray(List[rs.i8]([1, 2])).dtype == np.int8
    assert np.asarray(List[rs.u16]([1, 2])).dtype == np.uint16
    assert np.asarray(List[rs.f32]([1, 2])).dtype == np.float32
    assert np.asarray(List[rs.u64]([2**64 - 1])).dtype == np.uint64
    assert np.asarray(List([True, False])).dtype == np.bool_
    print("numpy_interop: ok")


if __name__ == "__main__":
    main()
