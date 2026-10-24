"""Regression tests for the findings of the independent safety review (hardening pass).

Each test names the finding it guards against (C = critical, H = high, M = medium,
L = low). Scenarios that would deadlock on a regression run in a fresh interpreter with a
timeout, so a regression fails the test instead of hanging the suite.
"""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap

import pytest
from _support import import_rustique_module_level

rs = import_rustique_module_level()
List = rs.List


def run_isolated(code: str, timeout: float = 30.0, env: dict[str, str] | None = None) -> str:
    """Run ``code`` in a fresh interpreter and return its stdout. A hang (deadlock), a
    crash or a non-zero exit fails the calling test."""
    script = (
        "import faulthandler\n"
        f"faulthandler.dump_traceback_later({timeout}, exit=True)\n"
        "import rustique as rs\n"
        "from rustique import List\n" + textwrap.dedent(code)
    )
    try:
        proc = subprocess.run(
            [sys.executable, "-c", script],
            capture_output=True,
            text=True,
            timeout=timeout + 30,
            env={**os.environ, **(env or {})},
        )
    except subprocess.TimeoutExpired as e:
        pytest.fail(f"scenario hung (deadlock?): {e.stdout!r} {e.stderr!r}")
    assert proc.returncode == 0, f"exit {proc.returncode}\nstdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
    return proc.stdout


# --------------------------------------------------------------------------------------
# CRITICAL (second review): `Schema::is_pure()` promised that coercion runs no Python code,
# and dict construction relies on it to coerce *borrowed* items inside the source's
# critical section. Fixed-width overflow messages called `str()`/`repr()`, i.e. an int or
# float subclass's `__str__`/`__repr__`, which could drop the source's last reference to
# the item: use-after-free (`realloc(): invalid pointer` / segfault).
# --------------------------------------------------------------------------------------

_PURE_SCHEMA_UAF = """
import sys
mode = {mode!r}
class Evil(int):
    def __str__(self):
        print("EVIL-CODE-RAN", flush=True)
        src.clear()
        return "evil"
    __repr__ = __str__
class EvilF(float):
    def __repr__(self):
        print("EVIL-CODE-RAN", flush=True)
        src.clear()
        return "evilf"
    __str__ = __repr__
if mode == "dict":
    src, target = {{"a": Evil(1000)}}, rs.Dict[str, rs.i8]()
elif mode == "pairs":
    src, target = [("a", Evil(1000))], rs.Dict[str, rs.i8]()
elif mode == "f32":
    src, target = {{"a": EvilF(1e300)}}, rs.Dict[str, rs.f32]()
elif mode == "list":
    src, target = [Evil(1000)], List[rs.i8]()
elif mode == "list-f32":
    src, target = [EvilF(1e300)], List[rs.f32]()
try:
    (target.update if hasattr(target, "update") else target.extend)(src)
except OverflowError as e:
    print("OverflowError:", e)
print("survived", len(src))
"""


@pytest.mark.parametrize("mode", ["dict", "pairs", "f32", "list", "list-f32"])
def test_critical_pure_schema_coercion_runs_no_python_code(mode: str) -> None:
    out = run_isolated(_PURE_SCHEMA_UAF.format(mode=mode), env={"PYTHONMALLOC": "debug"})
    assert "EVIL-CODE-RAN" not in out  # messages are formatted natively
    value = "1e+300 is out of range for f32" if "f32" in mode else "1000 is out of range for i8"
    assert "OverflowError: " in out and value in out, out
    assert out.endswith("survived 1\n"), out


# --------------------------------------------------------------------------------------
# C1: objects rejected by a failed operation were dropped under the write lock, so a
# `__del__` touching the list deadlocked on its own thread.
# --------------------------------------------------------------------------------------


def test_c1_extended_slice_mismatch_drops_rejected_values_unlocked() -> None:
    out = run_isolated(
        """
        l = List([1, 2, 3, 4])
        seen = []
        class D:
            def __del__(self):
                seen.append((len(l), l[0], l.to_list()))
        try:
            l[::2] = (D() for _ in range(3))
        except ValueError as e:
            print("ValueError:", e)
        assert seen == [(4, 1, [1, 2, 3, 4])] * 3, seen
        print("ok")
        """
    )
    assert "attempt to assign sequence of size 3 to extended slice of size 2" in out
    assert out.endswith("ok\n")


def test_c1_typed_nested_lists_rejected_by_slice_assignment() -> None:
    out = run_isolated(
        """
        import gc
        active = True
        class Foo:
            def __del__(self):
                if active:
                    print("del", len(L), type(L[0]).__name__, flush=True)
        L = List[List[Foo]]([[Foo()]])
        gc.collect()
        try:
            L[10] = (Foo() for _ in range(1))          # IndexError after building List[Foo]
        except IndexError as e:
            print("IndexError", e)
        try:
            L[::2] = [(Foo() for _ in range(1)), (Foo() for _ in range(1))]
        except ValueError as e:
            print("ValueError", e)
        L[::-1] = [[Foo()]]                            # displaced nested list dies unlocked
        active = False
        print("done")
        """
    )
    lines = out.splitlines()
    assert lines[-1] == "done"
    assert "IndexError list assignment index out of range" in lines
    assert any(line.startswith("ValueError attempt to assign sequence of size 2") for line in lines)
    assert lines.count("del 1 List") == 4


def test_c1_rejected_values_are_not_leaked_or_stored() -> None:
    import weakref

    class Obj:
        pass

    lst = List([1, 2, 3, 4])
    objs = [Obj() for _ in range(3)]
    refs = [weakref.ref(o) for o in objs]
    with pytest.raises(ValueError, match="extended slice of size 2"):
        lst[::2] = objs
    assert lst.to_list() == [1, 2, 3, 4]
    del objs
    assert all(r() is None for r in refs)


# --------------------------------------------------------------------------------------
# H1: remove() through the Python-equality path re-located the matched element with the
# untyped classification, which rejects u64 values above i64::MAX: 64 retries, then a
# spurious RuntimeError although the list never changed.
# --------------------------------------------------------------------------------------


class _EqBig:
    def __eq__(self, other: object) -> bool:
        return other == 2**64 - 1

    __hash__ = None  # type: ignore[assignment]


@pytest.mark.parametrize("value", [rs.u64(2**64 - 1), _EqBig(), 2**64 - 1], ids=["u64", "custom-eq", "int"])
def test_h1_remove_relocates_u64_above_i64_max(value: object) -> None:
    lst = List[rs.u64]([1, 2**64 - 1, 7, 2**64 - 1])
    assert lst.index(value) == 1
    lst.remove(value)
    assert lst.to_list() == [1, 7, 2**64 - 1]
    lst.remove(value)
    assert lst.to_list() == [1, 7]
    with pytest.raises(ValueError, match=r"list.remove\(x\): x not in list"):
        lst.remove(value)


def test_h1_remove_python_path_on_every_native_dtype() -> None:
    class EqAll:
        def __init__(self, target: object) -> None:
            self.target = target

        def __eq__(self, other: object) -> bool:
            return other == self.target

        __hash__ = None  # type: ignore[assignment]

    cases = [
        (List[rs.u64], [0, 2**63, 2**64 - 1]),
        (List[rs.i64], [-(2**63), 0, 2**63 - 1]),
        (List[rs.f32], [0.5, -1.5, 3.0]),
        (List[float], [float("nan"), 1.0, -0.0]),
        (List[bool], [False, True]),
        (List[str], ["a", "é", "z"]),
    ]
    for alias, values in cases:
        for target in values:
            lst = alias(values)
            expected = list(values)
            lst.remove(EqAll(target) if target == target else target)
            expected.remove(target) if target == target else expected.pop(0)
            assert repr(lst.to_list()) == repr(expected), (alias, target)


# --------------------------------------------------------------------------------------
# M1: contended waiters acquired (or queued on) the lock while detached from the
# interpreter: a waiting writer set the writer bit, so `__traverse__`'s `try_read` could
# succeed in one stop-the-world GC phase and fail in another (live objects treated as
# garbage). The deterministic regression tests are Rust unit tests in `sync.rs`
# (`waiting_writer_leaves_the_lock_alone`, starvation tests); this is the end-to-end smoke
# test: GC under contention never finalizes an element that is still in the list.
# --------------------------------------------------------------------------------------

FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


@pytest.mark.skipif(not FREE_THREADED, reason="needs a free-threaded build with the GIL disabled")
def test_m1_gc_under_contention_never_finalizes_live_elements() -> None:
    out = run_isolated(
        """
        import gc, threading, time
        class E:
            __slots__ = ("finalized", "__weakref__")
            def __init__(self): self.finalized = False
            def __del__(self): self.finalized = True
        lst = List([E() for _ in range(2000)])
        stop = time.time() + 2.0
        bad = []
        def writer():
            while time.time() < stop:
                lst.append(E())
                try: lst.pop(0)
                except IndexError: pass
        def reader():
            while time.time() < stop:
                for x in lst:
                    if x.finalized: bad.append(1)
        def collector():
            while time.time() < stop:
                gc.collect(0)
        ts = [threading.Thread(target=f) for f in [writer] * 3 + [reader] * 3 + [collector]]
        for t in ts: t.start()
        for t in ts: t.join()
        print("bad", len(bad))
        """,
        timeout=60,
    )
    assert out.strip() == "bad 0"


# --------------------------------------------------------------------------------------
# M2: arange() computed its length and last element with unchecked i128 arithmetic
# (`b - a - 1`, `-s`, `(n - 1) * s`): PanicException in debug builds, silent wrap-around
# in release builds.
# --------------------------------------------------------------------------------------

M = 2**127


@pytest.mark.parametrize(
    "args",
    [(-M, M - 1), (M - 1, -M, -1), (0, M - 1, 2**126), (-M, 0, M - 1), (-M, M - 1, M - 1), (0, 2**100, 2**99)],
)
@pytest.mark.parametrize("dtype", [None, rs.i8, rs.u64])
def test_m2_arange_extreme_bounds_raise_overflow(args: tuple[int, ...], dtype: object) -> None:
    with pytest.raises(OverflowError):
        rs.arange(*args, dtype=dtype)


def test_m2_arange_extreme_steps_match_range() -> None:
    assert rs.arange(0, -1, -M).to_list() == list(range(0, -1, -M))
    assert rs.arange(0, 1, M - 1).to_list() == [0]
    assert rs.arange(5, 6, -M).to_list() == []
    assert rs.arange(2**64 - 3, 2**64, 1, rs.u64).to_list() == list(range(2**64 - 3, 2**64))
    assert rs.arange(-(2**63), -(2**63) + 2, dtype=rs.i64).to_list() == [-(2**63), -(2**63) + 1]
    # Float targets take any int bounds; elements whose products overflow i128 stay exact.
    got = rs.arange(-M, M - 1, M - 1, dtype=float).to_list()
    assert got == [float(x) for x in range(-M, M - 1, M - 1)]


def test_m2_arange_huge_lengths_fail_cleanly() -> None:
    with pytest.raises(OverflowError):
        rs.arange(2**64)  # more items than fit an index (like list(range(2**64)))
    with pytest.raises(MemoryError):
        rs.arange(2**62)
    with pytest.raises(MemoryError):
        rs.arange(0.0, 1e300, 1e-3, float)


# --------------------------------------------------------------------------------------
# M3: converting a `range` into a fixed-width list scanned linearly (attached, without
# checking for signals) for the first element out of range: `List[rs.u32](range(2**33))`
# took over a minute.
# --------------------------------------------------------------------------------------


def test_m3_range_out_of_range_is_found_arithmetically() -> None:
    import time

    t0 = time.perf_counter()
    with pytest.raises(OverflowError, match=r"List\[u32\]: 4294967296 is out of range for u32 at index 4294967296"):
        List[rs.u32](range(2**33))
    with pytest.raises(OverflowError, match=r"at index 256$"):
        List[rs.u8](range(2**62))
    with pytest.raises(OverflowError, match=r"-129 is out of range for i8 at index 129$"):
        List[rs.i8](range(0, -(2**62), -1))
    # arange reports the first offending element too.
    with pytest.raises(OverflowError, match=r"^arange\(\) value 256 is out of range for u8$"):
        rs.arange(0, 300, 1, rs.u8)
    with pytest.raises(OverflowError, match=r"^arange\(\) value -2 is out of range for u32$"):
        rs.arange(10, -(2**60), -3, rs.u32)
    assert time.perf_counter() - t0 < 5


@pytest.mark.parametrize(
    ("alias", "rng"),
    [
        ("u8", range(250, 300)),
        ("u8", range(300, 250, -1)),
        ("u8", range(-5, 5)),
        ("u8", range(10, -10, -3)),
        ("i8", range(-200, 0, 7)),
        ("i16", range(0, 10**6, 999)),
        ("u32", range(2**32 - 5, 2**32 + 5, 3)),
        ("i32", range(2**31 - 1, -(2**31) - 10, -(2**30))),
    ],
)
def test_m3_range_errors_match_element_wise_conversion(alias: str, rng: range) -> None:
    typed = List[getattr(rs, alias)]
    with pytest.raises(OverflowError) as fast:
        typed(rng)
    with pytest.raises(OverflowError) as slow:
        typed(list(rng))
    assert str(fast.value) == str(slow.value)


# --------------------------------------------------------------------------------------
# M4: every operation bumped the mutation counter, even failed or no-op ones (pop from an
# empty list, an out-of-range assignment, sorting an empty list), so a key function that
# called `l.sort()` made the outer sort raise "list modified during sort". Python-level
# sorts now also behave like CPython's: the list appears empty while keys are computed
# and elements compared, changes made meanwhile are discarded and reported.
# --------------------------------------------------------------------------------------

LIST_TYPES = [list, List, List[int]]
LIST_IDS = ["builtin", "List", "List[int]"]


@pytest.mark.parametrize("cls", LIST_TYPES, ids=LIST_IDS)
def test_m4_key_function_sorting_the_list_itself(cls: type) -> None:
    lst = cls([3, 1, 2])
    lst.sort(key=lambda x: (lst.sort(), x)[1])
    assert list(lst) == [1, 2, 3]


NO_OPS = {
    "pop": lambda l: l.pop(),
    "pop0": lambda l: l.pop(0),
    "setitem": lambda l: l.__setitem__(0, 1),
    "delitem": lambda l: l.__delitem__(0),
    "remove": lambda l: l.remove(1),
    "index": lambda l: l.index(1),
    "clear": lambda l: l.clear(),
    "reverse": lambda l: l.reverse(),
    "sort": lambda l: l.sort(),
    "sort-key": lambda l: l.sort(key=lambda x: -x),
    "extend-empty": lambda l: l.extend([]),
    "iadd-empty": lambda l: l.__iadd__(()),
    "slice-assign-empty": lambda l: l.__setitem__(slice(None), []),
    "del-slice": lambda l: l.__delitem__(slice(None)),
    "imul-1": lambda l: l.__imul__(1),
    "imul-0": lambda l: l.__imul__(0),
    "ext-slice-mismatch": lambda l: l.__setitem__(slice(None, None, 2), [1]),
}


@pytest.mark.parametrize("op", sorted(NO_OPS))
@pytest.mark.parametrize("cls", LIST_TYPES, ids=LIST_IDS)
def test_m4_failed_or_no_op_mutations_during_sort_are_not_modifications(cls: type, op: str) -> None:
    lst = cls([3, 1, 2])
    seen = []

    def key(x: int) -> int:
        seen.append(len(lst))  # the list appears empty while it is sorted
        try:
            NO_OPS[op](lst)
        except (IndexError, ValueError):
            pass
        return x

    lst.sort(key=key)
    assert list(lst) == [1, 2, 3]
    assert seen == [0, 0, 0]


@pytest.mark.parametrize("cls", LIST_TYPES, ids=LIST_IDS)
def test_m4_changes_during_sort_are_discarded_and_reported(cls: type) -> None:
    lst = cls([3, 1, 2])
    with pytest.raises(ValueError, match="list modified during sort"):
        lst.sort(key=lambda x: (lst.append(0), x)[1])
    assert list(lst) == [1, 2, 3]  # CPython keeps the sorted elements, drops the appends
    lst = cls([3, 1, 2])
    with pytest.raises(ZeroDivisionError):
        lst.sort(key=lambda x: (lst.append(0), 1 // (x - 1))[1])  # the key error wins
    assert sorted(lst) == [1, 2, 3]


@pytest.mark.parametrize("cls", LIST_TYPES, ids=LIST_IDS)
def test_m4_list_is_empty_while_sorted(cls: type) -> None:
    lst = cls([5, 4, 3])
    views = []

    def key(x: int) -> int:
        views.append((len(lst), list(lst), bool(lst), 4 in lst, lst == cls()))
        with pytest.raises(IndexError):
            lst[0]
        return x

    lst.sort(key=key, reverse=True)
    assert views == [(0, [], False, False, True)] * 3
    assert list(lst) == [5, 4, 3]


def test_m4_no_buffer_export_while_sorted() -> None:
    lst = List[int]([3, 1, 2])
    errors = []

    def key(x: int) -> int:
        try:
            memoryview(lst)
        except BufferError as e:
            errors.append(str(e))
        return x

    lst.sort(key=key)
    assert errors == ["cannot export a List while it is being sorted"] * 3
    with memoryview(lst) as view:
        assert view.tolist() == [1, 2, 3]
