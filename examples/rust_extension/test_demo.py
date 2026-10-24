"""Tests for the `rustique_demo` downstream extension (built on rustique-api).

Build and run:
    maturin develop --uv -m examples/rust_extension/Cargo.toml
    python -m pytest examples/rust_extension
"""

import array
import ctypes
import math
import threading
import time

import pytest

import rustique
import rustique._rustique as rs
import rustique_demo as demo
from rustique import List

FIXED = [rs.i8, rs.i16, rs.i32, rs.i64, rs.u8, rs.u16, rs.u32, rs.u64, rs.f32, rs.f64]

MUTATIONS = [
    ("append", lambda xs: xs.append(1.0)),
    ("setitem", lambda xs: xs.__setitem__(0, xs[0])),
    ("delitem", lambda xs: xs.__delitem__(0)),
    ("extend", lambda xs: xs.extend([1.0])),
    ("insert", lambda xs: xs.insert(0, 1.0)),
    ("pop", lambda xs: xs.pop()),
    ("sort", lambda xs: xs.sort()),
    ("reverse", lambda xs: xs.reverse()),
    ("clear", lambda xs: xs.clear()),
    ("iadd", lambda xs: xs.__iadd__([1.0])),
    ("imul", lambda xs: xs.__imul__(2)),
]


def reference_primes(n):
    sieve = bytearray([1]) * n
    sieve[:2] = b"\x00\x00"[: min(n, 2)]
    for i in range(2, math.isqrt(max(n - 1, 0)) + 1):
        if sieve[i]:
            sieve[i * i :: i] = bytes(len(range(i * i, n, i)))
    return [i for i in range(n) if sieve[i]]


# --- Rust -> Python ---------------------------------------------------------------------


def test_primes_is_a_real_rustique_list():
    p = demo.primes(100)
    assert type(p) is rustique.List
    assert isinstance(p, rustique.List[int])
    assert p.schema_type is List[int]
    assert p.strategy == "i64"
    assert p == reference_primes(100)


@pytest.mark.parametrize("n", [0, 1, 2, 3, 4, 10, 97, 98, 1000, 100_003])
def test_primes_values(n):
    assert demo.primes(n).to_list() == reference_primes(n)
    assert demo.primes_pylist(n) == reference_primes(n)


def test_primes_large():
    p = demo.primes(1_000_000)
    assert len(p) == 78498 == demo.count_primes(1_000_000)
    assert p[-1] == 999983
    assert p.sum() == sum(demo.primes_pylist(1_000_000))


def test_returned_list_is_an_ordinary_mutable_list():
    p = demo.primes(20)
    p.append(23)
    p.sort(reverse=True)
    assert p[:3] == [23, 19, 17]
    with pytest.raises(TypeError):
        p.append("x")


# --- Python -> Rust (borrow) --------------------------------------------------------------


def test_normalize():
    out = demo.normalize(List[float]([3.0, 4.0]))
    assert type(out) is rustique.List and out.schema_type is List[float]
    assert out == [0.6, 0.8]
    assert demo.normalize(List([3.0, 4.0])) == [0.6, 0.8]  # untyped, f64 strategy
    assert demo.normalize(List[float]()) == []
    assert demo.normalize(List()) == []  # untyped empty: an empty slice of any type
    with pytest.raises(TypeError, match="'object' strategy"):
        demo.normalize(List[float | None]())  # typed, empty, but not f64 storage


def test_normalize_large_parallel_path():
    xs = List[float](float(i % 1000) - 500.0 for i in range(300_000))
    out = demo.normalize(xs)
    norm = math.sqrt(math.fsum(x * x for x in xs))
    assert len(out) == len(xs)
    for i in (0, 1, 777, len(xs) - 1):
        assert out[i] == pytest.approx(xs[i] / norm, rel=1e-12)


def test_normalize_rejects_other_storage():
    with pytest.raises(TypeError, match="f64 storage"):
        demo.normalize(List[int]([1, 2]))
    with pytest.raises(TypeError, match=r"f64 storage \(e.g. List\[float\]\), got one with f32"):
        demo.normalize(List[rs.f32]([1.0]))
    with pytest.raises(TypeError, match="'object' strategy"):
        demo.normalize(List[float | None]([1.0, None]))
    with pytest.raises(TypeError, match="'str' strategy"):
        demo.normalize(List[str](["a"]))
    with pytest.raises(TypeError, match="expected a rustique List, got list"):
        demo.normalize([3.0, 4.0])
    with pytest.raises(ValueError, match="non-zero norm"):
        demo.normalize(List[float]([0.0, 0.0]))


def test_borrow_sum_matches():
    xs = List[float]([0.5] * 1000)
    assert demo.fsum_borrowed(xs) == 500.0
    assert demo.fsum_extracted(xs) == 500.0
    assert demo.fsum_extracted([0.5] * 1000) == 500.0


def test_word_lengths():
    words = List[str](["", "a", "héllo", "日本語", "🐍x"])
    out = demo.word_lengths(words)
    assert out.schema_type is List[int]
    assert out == [len(w) for w in words]
    assert demo.word_lengths(List(["ab", "c"])) == [2, 1]
    assert demo.word_lengths(List()) == []
    with pytest.raises(TypeError, match="'str' strategy"):
        demo.word_lengths(List[int]([1]))


def test_upper_builds_str_lists():
    out = demo.upper(List[str](["abc", "straße", ""]))
    assert out.schema_type is List[str]
    assert out == ["ABC", "STRASSE", ""]


@pytest.mark.parametrize("scalar", FIXED, ids=lambda t: t.__name__)
def test_roundtrip_fixed_width(scalar):
    xs = List[scalar]([0, 1, 2, 100, 7])
    out = demo.roundtrip(xs)
    # i64/f64 data comes back as the Python-native aliases List[int]/List[float].
    assert out.schema_type is {rs.i64: List[int], rs.f64: List[float]}.get(scalar, List[scalar])
    assert out == xs
    assert demo.storage_dtype(out) == scalar.__name__


@pytest.mark.parametrize(
    "values", [[True, False, True], [1, -2, 2**62], [0.5, -1e300, math.inf], ["x", "", "é"]], ids=repr
)
def test_roundtrip_builtin_types(values):
    xs = List[type(values[0])](values)
    out = demo.roundtrip(xs)
    assert out.schema_type is xs.schema_type
    assert out == values


def test_roundtrip_nan_and_negative_zero():
    out = demo.roundtrip(List[float]([math.nan, -0.0]))
    assert math.isnan(out[0]) and math.copysign(1.0, out[1]) == -1.0


def test_storage_dtype_and_is_list():
    assert demo.storage_dtype(List[int]([1])) == "i64"
    assert demo.storage_dtype(List[int]([2**70])) is None  # demoted to object storage
    assert demo.storage_dtype(List()) is None
    assert demo.storage_dtype(List[str]()) == "str"
    assert demo.storage_dtype([1, 2]) is None
    assert demo.is_rustique_list(List())
    assert not demo.is_rustique_list([])
    assert not demo.is_rustique_list(List[int])


# --- freeze semantics ----------------------------------------------------------------------


@pytest.mark.parametrize("name,mutate", MUTATIONS, ids=[m[0] for m in MUTATIONS])
def test_borrowed_list_is_frozen_then_released(name, mutate):
    xs = List[float]([3.0, 1.0, 2.0])

    def inside():
        with pytest.raises(BufferError, match="buffer export"):
            mutate(xs)
        return "done"

    assert demo.while_borrowed(xs, inside) == "done"
    assert xs == [3.0, 1.0, 2.0]
    mutate(xs)  # released: works again


def test_mutation_from_another_thread_during_borrow():
    xs = List[rs.u8]([1, 2, 3])
    errors = []

    def other_thread():
        try:
            xs.append(4)
        except BufferError as e:
            errors.append(e)

    def inside():
        t = threading.Thread(target=other_thread)
        t.start()
        t.join()

    demo.while_borrowed(xs, inside)
    assert len(errors) == 1
    assert xs == [1, 2, 3]
    xs.append(4)
    assert xs == [1, 2, 3, 4]


def test_concurrent_mutation_while_rust_reads_detached():
    xs = List[float]([1.0] * 100_000)
    result = []
    worker = threading.Thread(target=lambda: result.append(demo.hold_and_sum(xs, 0.5)))
    worker.start()
    saw_frozen = False
    deadline = time.monotonic() + 5
    while worker.is_alive() and time.monotonic() < deadline:
        try:
            xs[0] = 1.0  # no-op write: still a mutation
        except BufferError:
            saw_frozen = True
            break
    worker.join()
    assert saw_frozen, "the list was never observed frozen during the borrow"
    assert result == [100_000.0]
    xs[0] = 2.0
    assert xs[0] == 2.0


def test_release_on_error_paths():
    xs = List[float]([1.0])

    def boom():
        raise KeyError("boom")

    with pytest.raises(KeyError):
        demo.while_borrowed(xs, boom)
    xs.append(2.0)  # released despite the exception

    ints = List[int]([1, 2])
    with pytest.raises(TypeError):
        demo.normalize(ints)  # borrowed, then rejected for its dtype
    ints.append(3)  # ... and released


def test_nested_and_concurrent_borrows():
    xs = List[float]([1.0, 2.0])
    assert demo.while_borrowed(xs, lambda: demo.while_borrowed(xs, lambda: demo.fsum_borrowed(xs))) == 3.0
    sums = []
    threads = [threading.Thread(target=lambda: sums.append(demo.hold_and_sum(xs, 0.05))) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert sums == [3.0] * 8
    xs.append(3.0)


def test_stress_borrows_against_writers():
    # Readers borrow and sum while writers keep trying to grow and shrink the list. Every
    # element is 1.0, so a consistent read sums to the length at borrow time; a torn read
    # (e.g. during a reallocation) would not.
    xs = List[float]([1.0] * 10_000)
    stop = threading.Event()
    bad, frozen, ok = [], [0], [0]

    def reader():
        for _ in range(300):
            total = demo.fsum_borrowed(xs)
            if total != int(total) or not 10_000 <= total <= 10_100:
                bad.append(total)

    def writer():
        while not stop.is_set():
            try:
                xs.extend([1.0] * 50)
            except BufferError:
                frozen[0] += 1
                continue
            while True:  # undo the growth as soon as the list is free again
                try:
                    del xs[-50:]
                    break
                except BufferError:
                    frozen[0] += 1
            ok[0] += 1

    writers = [threading.Thread(target=writer) for _ in range(2)]
    readers = [threading.Thread(target=reader) for _ in range(4)]
    for t in writers + readers:
        t.start()
    for t in readers:
        t.join()
    stop.set()
    for t in writers:
        t.join()
    assert not bad
    assert frozen[0] + ok[0] > 0
    assert len(xs) == 10_000 and set(xs) == {1.0}
    xs.append(1.0)


def test_borrow_coexists_with_memoryview():
    xs = List[float]([1.0, 2.0])
    mv = memoryview(xs)
    assert demo.fsum_borrowed(xs) == 3.0
    with pytest.raises(BufferError):
        xs.append(1.0)  # the memoryview still holds its own export
    mv.release()
    xs.append(1.0)


# --- raw C entry points ----------------------------------------------------------------------


def test_capi_version():
    version, abi_size = demo.capi_version()
    assert version >= 1
    assert abi_size >= 9 * ctypes.sizeof(ctypes.c_size_t)


def test_raw_from_slice_checks():
    with pytest.raises(ValueError, match="unknown dtype code 200"):
        demo.raw_from_slice(200, b"", 1)
    with pytest.raises(ValueError, match="list_from_utf8"):
        demo.raw_from_slice(11, b"abc", 1)
    with pytest.raises(ValueError, match="invalid bool byte 0x02 at index 2"):
        demo.raw_from_slice(0, b"\x00\x01\x02", 1)
    assert demo.raw_from_slice(0, b"\x00\x01", 1) == [False, True]
    assert demo.raw_from_slice(0, b"\x00\x01", 1).schema_type is List[bool]


def test_raw_from_slice_unaligned_source():
    data = b"\xff" + array.array("q", [1, -2, 2**62]).tobytes()
    out = demo.raw_from_slice(4, data, 8, offset=1)
    assert out.schema_type is List[int]
    assert out == [1, -2, 2**62]
    floats = b"\x00" * 3 + array.array("d", [0.25, -1.5]).tobytes()
    assert demo.raw_from_slice(10, floats, 8, offset=3) == [0.25, -1.5]


def test_raw_from_utf8_checks():
    out = demo.raw_from_utf8([b"ok", b"", "é".encode()])
    assert out.schema_type is List[str]
    assert out == ["ok", "", "é"]
    assert demo.raw_from_utf8([]) == []
    with pytest.raises(UnicodeDecodeError) as info:
        demo.raw_from_utf8([b"fine", b"ab\xffcd"])
    assert info.value.start == 2
    assert any("string 1" in note for note in info.value.__notes__)
    with pytest.raises(UnicodeDecodeError):
        demo.raw_from_utf8([b"\xed\xa0\x80"])  # an encoded surrogate is not UTF-8
