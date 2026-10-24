"""The C-API capsule `rustique._rustique._C_API` (ARCHITECTURE §5, "Rust interop").

Two layers:
- the raw ABI, called through ctypes (always runs);
- a separate extension module built on `rustique-api` (`examples/rust_extension`), which
  is skipped unless it has been built into the environment.
"""

import ctypes
import math
import sys
import threading

import pytest

import rustique
import rustique._rustique as rs
from rustique import List

CAPSULE_NAME = b"rustique._rustique._C_API"
OBJ = ctypes.py_object
P = ctypes.c_void_p
SIZE = ctypes.c_size_t


class CApi(ctypes.Structure):
    """Version 1 of the table, field for field."""

    _fields_ = [
        ("api_version", ctypes.c_uint32),
        ("abi_size", SIZE),
        ("list_check", ctypes.PYFUNCTYPE(ctypes.c_int, P)),
        ("list_dtype", ctypes.PYFUNCTYPE(ctypes.c_int, P)),
        ("list_from_slice", ctypes.PYFUNCTYPE(P, ctypes.c_uint8, P, SIZE)),
        ("list_from_utf8", ctypes.PYFUNCTYPE(P, P, P, SIZE)),
        (
            "list_borrow",
            ctypes.PYFUNCTYPE(
                ctypes.c_int, P, ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(P), ctypes.POINTER(SIZE), ctypes.POINTER(P)
            ),
        ),
        ("list_release", ctypes.PYFUNCTYPE(None, P)),
        (
            "list_copy_utf8",
            ctypes.PYFUNCTYPE(ctypes.c_int, P, P, SIZE, ctypes.POINTER(SIZE), SIZE, ctypes.POINTER(SIZE), ctypes.POINTER(SIZE)),
        ),
    ]


CODES = {"bool": 0, "i8": 1, "i16": 2, "i32": 3, "i64": 4, "u8": 5, "u16": 6, "u32": 7, "u64": 8, "f32": 9, "f64": 10, "str": 11}
CTYPES = {
    "bool": ctypes.c_bool,
    "i8": ctypes.c_int8,
    "i16": ctypes.c_int16,
    "i32": ctypes.c_int32,
    "i64": ctypes.c_int64,
    "u8": ctypes.c_uint8,
    "u16": ctypes.c_uint16,
    "u32": ctypes.c_uint32,
    "u64": ctypes.c_uint64,
    "f32": ctypes.c_float,
    "f64": ctypes.c_double,
}


@pytest.fixture(scope="module")
def api():
    get = ctypes.pythonapi.PyCapsule_GetPointer
    get.restype = P
    get.argtypes = [OBJ, ctypes.c_char_p]
    return CApi.from_address(get(rs._C_API, CAPSULE_NAME))


def addr(obj):
    return ctypes.c_void_p(id(obj))


def steal(address):
    """Turn a new reference returned through ctypes into a Python object."""
    obj = ctypes.cast(address, OBJ).value
    ctypes.pythonapi.Py_DecRef(ctypes.c_void_p(address))
    return obj


def borrow(api, obj):
    dtype, data, length, token = ctypes.c_uint8(), P(), SIZE(), P()
    rc = api.list_borrow(addr(obj), ctypes.byref(dtype), ctypes.byref(data), ctypes.byref(length), ctypes.byref(token))
    assert rc == 0
    return dtype.value, data.value, length.value, token


# --- raw ABI via ctypes -----------------------------------------------------------------------


def test_capsule_is_published(api):
    capsule = rs._C_API
    assert type(capsule).__name__ == "PyCapsule"
    assert "rustique._rustique._C_API" in repr(capsule)
    assert api.api_version == 1
    assert api.abi_size == ctypes.sizeof(CApi) == 9 * ctypes.sizeof(SIZE)


@pytest.mark.parametrize("name", list(CTYPES))
def test_list_from_slice_every_dtype(api, name):
    values = [True, False, True] if name == "bool" else [0, 1, 2, 100]
    buf = (CTYPES[name] * len(values))(*values)
    out = steal(api.list_from_slice(CODES[name], ctypes.addressof(buf), len(values)))
    assert type(out) is rustique.List
    assert out == values
    assert out.strategy == name
    expected = {"bool": List[bool], "i64": List[int], "f64": List[float]}.get(name) or List[getattr(rs, name)]
    assert out.schema_type is expected
    assert api.list_dtype(addr(out)) == CODES[name]


def test_list_from_slice_empty_and_errors(api):
    assert steal(api.list_from_slice(CODES["f64"], None, 0)) == []
    with pytest.raises(ValueError, match="data is NULL"):
        api.list_from_slice(CODES["f64"], None, 3)
    with pytest.raises(ValueError, match="unknown dtype code 12"):
        api.list_from_slice(12, None, 0)
    with pytest.raises(ValueError, match="list_from_utf8"):
        api.list_from_slice(CODES["str"], None, 0)
    bad = (ctypes.c_uint8 * 2)(1, 7)
    with pytest.raises(ValueError, match="invalid bool byte 0x07 at index 1"):
        api.list_from_slice(CODES["bool"], ctypes.addressof(bad), 2)
    with pytest.raises(MemoryError):
        api.list_from_slice(CODES["f64"], ctypes.addressof(bad), sys.maxsize)


def test_list_from_utf8(api):
    words = [b"alpha", b"", "é\U0001f40d".encode()]
    bufs = [ctypes.create_string_buffer(w, len(w)) for w in words]
    ptrs = (P * 3)(*[ctypes.addressof(b) for b in bufs])
    lens = (SIZE * 3)(*[len(w) for w in words])
    out = steal(api.list_from_utf8(ctypes.addressof(ptrs), ctypes.addressof(lens), 3))
    assert out.schema_type is List[str]
    assert out == ["alpha", "", "é\U0001f40d"]
    assert steal(api.list_from_utf8(None, None, 0)) == []
    bad = ctypes.create_string_buffer(b"\xc3", 1)
    one_ptr, one_len = (P * 1)(ctypes.addressof(bad)), (SIZE * 1)(1)
    with pytest.raises(UnicodeDecodeError):
        api.list_from_utf8(ctypes.addressof(one_ptr), ctypes.addressof(one_len), 1)
    with pytest.raises(ValueError, match="NULL"):
        api.list_from_utf8(None, None, 1)


def test_list_check_and_dtype(api):
    assert api.list_check(addr(List())) == 1
    assert api.list_check(addr([])) == 0
    assert api.list_check(None) == 0
    assert api.list_dtype(addr(List[str](["a"]))) == CODES["str"]
    assert api.list_dtype(addr(List([1, 2]))) == CODES["i64"]
    assert api.list_dtype(addr(List([1, "a"]))) == -1  # object storage
    assert api.list_dtype(addr(List())) == -1  # untyped, empty
    assert api.list_dtype(addr(List[float]())) == CODES["f64"]
    assert api.list_dtype(addr((1.0,))) == -1
    assert api.list_dtype(None) == -1


def test_borrow_reads_in_place_and_freezes(api):
    xs = List[float]([1.5, 2.5, -3.0])
    dtype, data, length, token = borrow(api, xs)
    try:
        assert (dtype, length) == (CODES["f64"], 3)
        assert data % ctypes.alignment(ctypes.c_double) == 0
        assert list((ctypes.c_double * length).from_address(data)) == [1.5, 2.5, -3.0]
        assert data == buffer_address(xs)  # zero-copy: the list's own buffer
        for mutate in (lambda: xs.append(1.0), lambda: xs.__setitem__(0, 0.0), xs.clear, xs.sort):
            with pytest.raises(BufferError):
                mutate()
        errors = []
        t = threading.Thread(target=lambda: errors.append(pytest.raises(BufferError, xs.append, 9.0)))
        t.start()
        t.join()
        assert len(errors) == 1
    finally:
        api.list_release(token)
    xs.append(4.0)
    assert xs == [1.5, 2.5, -3.0, 4.0]
    api.list_release(None)  # NULL token: no-op


def test_borrow_holds_a_reference(api):
    xs = List[int]([1, 2, 3])
    refs = sys.getrefcount(xs)
    *_, token = borrow(api, xs)
    assert sys.getrefcount(xs) == refs + 1  # the borrow keeps the list alive
    api.list_release(token)
    assert sys.getrefcount(xs) == refs


def buffer_address(obj):
    """Address of `obj`'s buffer, through a short-lived buffer export."""
    get, release = ctypes.pythonapi.PyObject_GetBuffer, ctypes.pythonapi.PyBuffer_Release
    get.argtypes, get.restype = [OBJ, P, ctypes.c_int], ctypes.c_int
    release.argtypes, release.restype = [P], None
    view = (ctypes.c_char * 256)()  # room for a Py_buffer, whose first field is `buf`
    assert get(obj, ctypes.addressof(view), 0) == 0
    try:
        return P.from_address(ctypes.addressof(view)).value
    finally:
        release(ctypes.addressof(view))


def test_borrow_errors(api):
    out = (ctypes.c_uint8(), P(), SIZE(), P())
    refs = [ctypes.byref(o) for o in out]
    with pytest.raises(TypeError, match="expected a rustique List, got list"):
        api.list_borrow(addr([1.0]), *refs)
    with pytest.raises(TypeError, match="got NULL"):
        api.list_borrow(None, *refs)
    with pytest.raises(TypeError, match="'str' strategy has no fixed-size native storage"):
        api.list_borrow(addr(List[str](["a"])), *refs)
    with pytest.raises(TypeError, match="'object' strategy"):
        api.list_borrow(addr(List([1, None])), *refs)
    with pytest.raises(TypeError, match="'empty' strategy"):
        api.list_borrow(addr(List()), *refs)
    with pytest.raises(ValueError, match="output pointer is NULL"):
        api.list_borrow(addr(List[int]([1])), None, refs[1], refs[2], refs[3])


def test_borrow_empty_typed_list(api):
    xs = List[rs.u16]()
    dtype, data, length, token = borrow(api, xs)
    try:
        assert (dtype, length) == (CODES["u16"], 0)
        assert data  # non-NULL even when empty
        with pytest.raises(BufferError):
            xs.append(1)
    finally:
        api.list_release(token)
    xs.append(1)


def test_copy_utf8_two_phase(api):
    xs = List[str](["ab", "", "été"])
    data_len, count = SIZE(), SIZE()
    rc = api.list_copy_utf8(addr(xs), None, 0, None, 0, ctypes.byref(data_len), ctypes.byref(count))
    assert (rc, data_len.value, count.value) == (1, 7, 3)
    data = ctypes.create_string_buffer(data_len.value)
    offsets = (SIZE * (count.value + 1))()
    rc = api.list_copy_utf8(
        addr(xs), data, data_len.value, offsets, count.value + 1, ctypes.byref(data_len), ctypes.byref(count)
    )
    assert rc == 0
    raw = data.raw[: data_len.value]
    assert list(offsets) == [0, 2, 2, 7]
    assert [raw[a:b].decode() for a, b in zip(offsets, offsets[1:])] == list(xs)
    # Only empty strings: no data buffer needed at all.
    blanks = List[str](["", ""])
    two = (SIZE * 3)()
    assert api.list_copy_utf8(addr(blanks), None, 0, two, 3, ctypes.byref(data_len), ctypes.byref(count)) == 0
    assert (list(two), data_len.value, count.value) == ([0, 0, 0], 0, 2)
    # Untyped empty: zero strings. Other storage: TypeError.
    one = (SIZE * 1)(99)
    assert api.list_copy_utf8(addr(List()), None, 0, one, 1, ctypes.byref(data_len), ctypes.byref(count)) == 0
    assert (one[0], data_len.value, count.value) == (0, 0, 0)
    with pytest.raises(TypeError, match="'str' strategy, got 'i64'"):
        api.list_copy_utf8(addr(List([1])), None, 0, None, 0, ctypes.byref(data_len), ctypes.byref(count))
    with pytest.raises(TypeError, match="got 'object'"):
        api.list_copy_utf8(addr(List(["a", 1])), None, 0, None, 0, ctypes.byref(data_len), ctypes.byref(count))


# --- through a separate extension module (examples/rust_extension) ---------------------------


@pytest.fixture(scope="module")
def demo():
    try:
        import rustique_demo
    except ImportError:
        pytest.skip(
            "rustique_demo is not built; run `maturin develop --uv -m examples/rust_extension/Cargo.toml` "
            "to test the C API from a separate extension module"
        )
    return rustique_demo


def test_demo_returns_genuine_rustique_lists(demo):
    p = demo.primes(100)
    assert type(p) is rustique.List
    assert isinstance(p, rustique.List[int])
    assert p == [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97]
    p.append(101)  # an ordinary, mutable rustique List
    assert p[-1] == 101


def test_demo_borrow_freezes_across_modules(demo):
    xs = List[float]([3.0, 4.0])

    def inside():
        with pytest.raises(BufferError):
            xs.append(1.0)
        return "ok"

    assert demo.while_borrowed(xs, inside) == "ok"
    assert demo.normalize(xs) == [0.6, 0.8]
    xs.append(0.0)
    assert demo.capi_version()[0] == 1


def test_demo_release_keeps_a_pending_exception(demo):
    xs = List[float]([1.0])
    assert demo.raw_release_keeps_pending_error(xs)
    xs.append(2.0)


def test_demo_strings(demo):
    words = List[str](["Rust", "Python", "über"])
    assert demo.word_lengths(words) == [4, 6, 4]
    assert demo.upper(words) == ["RUST", "PYTHON", "ÜBER"]
    assert demo.roundtrip(List[rs.f32]([0.5, math.inf])) == [0.5, math.inf]
