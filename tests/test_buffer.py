"""Buffer protocol (ARCHITECTURE §4.1 "Buffer protocol", §1.6): zero-copy, read-only
exports of numeric/bool storage; every mutation raises `BufferError` while an export is
alive; bulk ingest from buffer-protocol objects (`array.array`, NumPy, bytes)."""

from __future__ import annotations

import array
import gc
import struct
from collections.abc import Callable
from typing import Any

import pytest
from _support import assert_same_items, import_rustique_module_level

rs = import_rustique_module_level()
np = pytest.importorskip("numpy")
List = rs.List

BUFFER_MSG = "cannot modify List while a buffer export exists"

DTYPES = [
    # (scalar name, format, itemsize, sample values, numpy dtype)
    ("i8", "b", 1, [-128, 0, 127], np.int8),
    ("i16", "h", 2, [-(2**15), 7, 2**15 - 1], np.int16),
    ("i32", "i", 4, [-(2**31), 7, 2**31 - 1], np.int32),
    ("i64", "q", 8, [-(2**63), 7, 2**63 - 1], np.int64),
    ("u8", "B", 1, [0, 7, 255], np.uint8),
    ("u16", "H", 2, [0, 7, 2**16 - 1], np.uint16),
    ("u32", "I", 4, [0, 7, 2**32 - 1], np.uint32),
    ("u64", "Q", 8, [0, 7, 2**64 - 1], np.uint64),
    ("f32", "f", 4, [0.5, -2.25, 1e30], np.float32),
    ("f64", "d", 8, [0.1, -2.5, 1e300], np.float64),
]


def typed(name: str, values: list[Any]) -> Any:
    return List[getattr(rs, name)](values)


# ======================================================================================
# Export
# ======================================================================================


@pytest.mark.parametrize("name,fmt,size,values,npdt", DTYPES, ids=[d[0] for d in DTYPES])
def test_memoryview_layout(name: str, fmt: str, size: int, values: list[Any], npdt: Any) -> None:
    lst = typed(name, values)
    with memoryview(lst) as mv:
        assert mv.format == fmt
        assert mv.itemsize == size
        assert mv.ndim == 1
        assert mv.shape == (len(values),)
        assert mv.strides == (size,)
        assert mv.readonly
        assert mv.nbytes == size * len(values)
        assert mv.c_contiguous
        expected = values if fmt != "f" else [struct.unpack("f", struct.pack("f", v))[0] for v in values]
        assert mv.tolist() == expected
        assert mv.tobytes() == struct.pack(f"={len(values)}{fmt}", *expected)


def test_untyped_native_strategies_export() -> None:
    for values, fmt in (([1, 2, 3], "q"), ([0.5, 1.5], "d"), ([True, False], "?")):
        lst = List(values)
        with memoryview(lst) as mv:
            assert mv.format == fmt
            assert mv.tolist() == values
    with memoryview(List[bool]([True, False, True])) as mv:
        assert mv.format == "?" and mv.itemsize == 1
        assert mv.tolist() == [True, False, True]


def test_empty_typed_list_exports_empty_buffer() -> None:
    with memoryview(List[int]()) as mv:
        assert mv.shape == (0,)
        assert mv.tolist() == []
        assert mv.format == "q"


@pytest.mark.parametrize("data", [["a", "b"], [1, "a"], [None], [2**70], [[1]]], ids=repr)
def test_non_numeric_storage_does_not_export(data: list[Any]) -> None:
    lst = List(data)
    with pytest.raises((TypeError, BufferError)):
        memoryview(lst)
    lst.append(data[0])  # a failed export must not leave the list frozen
    assert len(lst) == len(data) + 1


def test_export_is_read_only() -> None:
    lst = List[int]([1, 2, 3])
    with memoryview(lst) as mv:
        with pytest.raises(TypeError):
            mv[0] = 5  # writing into a read-only memoryview is a TypeError on 3.15
        with pytest.raises(TypeError):
            mv[0:1] = b"\x00" * 8
    arr = np.asarray(lst)
    assert not arr.flags.writeable
    with pytest.raises(ValueError):
        arr[0] = 5
    del arr
    gc.collect()
    assert lst == [1, 2, 3]


def test_writable_request_is_refused() -> None:
    lst = List[int]([1, 2, 3])
    arr = np.frombuffer(lst, dtype=np.int64)
    with pytest.raises(ValueError):
        arr.setflags(write=True)
    del arr
    ctypes = pytest.importorskip("ctypes")
    with pytest.raises((BufferError, TypeError)):
        (ctypes.c_int64 * 3).from_buffer(lst)  # requests a writable buffer
    lst.append(4)


def test_zero_copy() -> None:
    lst = List[int](range(1000))
    a = np.asarray(lst)
    b = np.asarray(lst)
    c = np.frombuffer(lst, dtype=np.int64)
    assert a.dtype == np.int64
    assert a.ctypes.data == b.ctypes.data == c.ctypes.data
    assert np.shares_memory(a, b)
    assert a.tolist() == list(range(1000))
    with memoryview(lst) as mv1, memoryview(lst) as mv2:
        assert np.asarray(mv1).ctypes.data == np.asarray(mv2).ctypes.data == a.ctypes.data


def test_numpy_dtypes() -> None:
    for name, _fmt, _size, values, npdt in DTYPES:
        arr = np.asarray(typed(name, values))
        assert arr.dtype == npdt
        assert arr.tolist() == np.array(values, dtype=npdt).tolist()
    assert np.asarray(List([True, False])).dtype == np.bool_
    assert np.asarray(List([0.5])).dtype == np.float64


# ======================================================================================
# Mutation while exported
# ======================================================================================

MUTATIONS: list[tuple[str, Callable[[Any], Any]]] = [
    ("append", lambda l: l.append(9)),
    ("extend", lambda l: l.extend([9])),
    ("insert", lambda l: l.insert(0, 9)),
    ("pop", lambda l: l.pop()),
    ("pop0", lambda l: l.pop(0)),
    ("remove", lambda l: l.remove(1)),
    ("clear", lambda l: l.clear()),
    ("setitem", lambda l: l.__setitem__(0, 9)),
    ("setitem-same-value", lambda l: l.__setitem__(0, 1)),
    ("setslice", lambda l: l.__setitem__(slice(0, 1), [9, 9])),
    ("setslice-ext", lambda l: l.__setitem__(slice(None, None, 2), [9, 9])),
    ("delitem", lambda l: l.__delitem__(0)),
    ("delslice", lambda l: l.__delitem__(slice(0, 2))),
    ("sort", lambda l: l.sort()),
    ("sort-reverse", lambda l: l.sort(reverse=True)),
    ("reverse", lambda l: l.reverse()),
    ("iadd", lambda l: l.__iadd__([9])),
    ("imul", lambda l: l.__imul__(2)),
    ("imul0", lambda l: l.__imul__(0)),
]


@pytest.mark.parametrize("name,op", MUTATIONS, ids=[m[0] for m in MUTATIONS])
@pytest.mark.parametrize("exporter", ["memoryview", "numpy"])
def test_mutation_while_exported_raises(name: str, op: Callable[[Any], Any], exporter: str) -> None:
    lst = List[int]([1, 2, 3])
    export: Any = memoryview(lst) if exporter == "memoryview" else np.asarray(lst)
    with pytest.raises(BufferError, match=BUFFER_MSG):
        op(lst)
    assert_same_items(lst, [1, 2, 3])
    assert export.tolist() == [1, 2, 3]
    if exporter == "memoryview":
        export.release()
    else:
        del export
        gc.collect()
    op(lst)  # works again once the export is gone


def test_reads_allowed_while_exported() -> None:
    lst = List[int]([3, 1, 2])
    with memoryview(lst):
        assert len(lst) == 3
        assert lst[0] == 3 and lst[-1] == 2
        assert list(lst) == [3, 1, 2]
        assert 1 in lst
        assert lst.index(2) == 2 and lst.count(1) == 1
        assert lst.sum() == 6 and lst.min() == 1 and lst.max() == 3
        assert lst[1:] == [1, 2]
        c = lst.copy()
        c.append(4)  # the copy is not exported
        assert lst == [3, 1, 2]
        assert repr(lst) == "List[int]([3, 1, 2])"
        assert sorted(lst) == [1, 2, 3]
        assert lst + [4] == [3, 1, 2, 4]
        assert lst * 2 == [3, 1, 2, 3, 1, 2]
        assert List(lst) == lst
        assert np.asarray(lst).sum() == 6


def test_export_counting() -> None:
    lst = List[int]([1, 2, 3, 4])
    mv1 = memoryview(lst)
    mv2 = memoryview(lst)
    mv1.release()
    with pytest.raises(BufferError):
        lst.append(5)
    mv2.release()
    lst.append(5)

    mv = memoryview(lst)
    sub = mv[::2]  # shares the parent's export
    mv.release()
    with pytest.raises(BufferError):
        lst.append(6)
    sub.release()
    lst.append(6)
    assert lst == [1, 2, 3, 4, 5, 6]


def test_export_released_by_garbage_collection() -> None:
    lst = List[float]([1.0, 2.0])
    arr = np.asarray(lst)
    view = arr[1:]
    del arr
    gc.collect()
    with pytest.raises(BufferError):
        lst.append(3.0)
    del view
    gc.collect()
    lst.append(3.0)
    assert lst == [1.0, 2.0, 3.0]


def test_failed_mutation_does_not_demote() -> None:
    lst = List([1, 2, 3])
    with memoryview(lst):
        with pytest.raises(BufferError):
            lst.append("x")
    assert lst.strategy == "i64"


# ======================================================================================
# Ingest
# ======================================================================================

ARRAY_CODES = [("b", [-1, 2]), ("B", [1, 255]), ("h", [-300, 2]), ("H", [65535]), ("i", [-(2**31), 5]), ("I", [2**32 - 1]), ("l", [2**40]), ("L", [2**63]), ("q", [-(2**63), 2**63 - 1]), ("Q", [2**64 - 1]), ("f", [0.5, -1.25]), ("d", [0.1, 1e300])]


@pytest.mark.parametrize("code,values", ARRAY_CODES, ids=[c[0] for c in ARRAY_CODES])
def test_ingest_array_array(code: str, values: list[Any]) -> None:
    src = array.array(code, values)
    lst = List(src)
    assert_same_items(lst, src.tolist())
    assert lst.strategy != "object" or any(v > 2**63 - 1 for v in src.tolist() if isinstance(v, int))


def test_ingest_preserves_dtype_when_typed() -> None:
    assert List[rs.i32](array.array("i", [1, 2])).strategy == "i32"
    assert List[rs.u8](array.array("B", [1, 2])).strategy == "u8"
    assert List[rs.f32](array.array("f", [0.5])).strategy == "f32"
    assert List[float](array.array("d", [0.5])).strategy == "f64"
    assert_same_items(List[float](array.array("i", [1, 2])), [1.0, 2.0])
    assert_same_items(List[int](array.array("h", [-1, 2])), [-1, 2])


def test_ingest_typed_range_checks() -> None:
    with pytest.raises(OverflowError):
        List[rs.u8](np.array([300], dtype=np.int64))
    with pytest.raises(OverflowError):
        List[rs.u8](array.array("b", [-1]))
    with pytest.raises(OverflowError):
        List[rs.i64](np.array([2**64 - 1], dtype=np.uint64))
    with pytest.raises(TypeError):
        List[int](np.array([1.5]))
    with pytest.raises(TypeError):
        List[bool](np.array([1, 0], dtype=np.int8))


@pytest.mark.parametrize(
    "arr",
    [
        np.arange(10),
        np.arange(10, dtype=np.int32),
        np.arange(5, dtype=np.uint8),
        np.array([2**64 - 1, 0], dtype=np.uint64),
        np.linspace(0, 1, 7),
        np.array([0.5, -0.0, np.inf], dtype=np.float32),
        np.array([True, False, True]),
        np.arange(20)[::3],
        np.arange(20)[::-2],
        np.array([1, 2, 3], dtype=">i4"),
        np.array([1.5, 2.5], dtype=">f8"),
        np.asfortranarray(np.arange(6)),
        np.arange(0),
    ],
    ids=lambda a: f"{a.dtype}{'-strided' if not a.flags.c_contiguous else ''}{a.shape}",
)
def test_ingest_numpy(arr: Any) -> None:
    lst = List(arr)
    assert_same_items(lst, arr.tolist())
    if arr.size and arr.dtype.kind in "iub" and arr.dtype.isnative:
        assert lst.strategy != "object" or arr.max() > 2**63 - 1
    # Round trip back to NumPy.
    if arr.size and arr.dtype.isnative:
        back = np.asarray(lst)
        assert back.tolist() == arr.tolist()


def test_ingest_2d_numpy_follows_iteration_semantics() -> None:
    arr = np.zeros((2, 3))
    lst = List(arr)
    assert len(lst) == 2  # rows, exactly like list(arr)
    assert lst.strategy == "object"
    assert all(isinstance(row, np.ndarray) for row in lst)


def test_ingest_bytes_like() -> None:
    assert_same_items(List(b"\x00\x7f\xff"), [0, 127, 255])
    assert_same_items(List(bytearray(b"ab")), [97, 98])
    assert_same_items(List[rs.u8](b"\x01\x02"), [1, 2])
    assert List[rs.u8](b"\x01\x02").strategy == "u8"
    src = List[int]([5, 6])
    assert_same_items(List(memoryview(src)), [5, 6])
    assert_same_items(List[int](memoryview(src)), [5, 6])


def test_ingest_is_a_copy() -> None:
    src = np.arange(5)
    lst = List(src)
    src[0] = 100
    assert lst[0] == 0
    lst.append(5)  # not frozen by the source's buffer
    assert len(lst) == 6


def test_buffer_consumers() -> None:
    lst = List[rs.u8]([1, 0, 255])
    assert bytes(lst) == b"\x01\x00\xff"
    assert int.from_bytes(List[rs.u8]([1, 0]), "little") == 1
    assert bytearray(lst) == bytearray(b"\x01\x00\xff")
    assert array.array("q", List[int]([1, 2])).tolist() == [1, 2]
    lst.append(7)  # all exports above were released
