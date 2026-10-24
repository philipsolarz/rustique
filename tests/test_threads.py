"""Concurrency stress tests (ARCHITECTURE §1.5, §6).

Designed for free-threaded CPython (3.15t), where they exercise real parallelism; on GIL
builds they still run as a smoke test with a tiny switch interval to force interleaving.
Each test starts N threads behind a barrier and joins them with a timeout, so a deadlock
fails the test instead of hanging the run (threads are daemons).

Invariants: no crash, no deadlock, no exception from single operations on shared
containers (Rustique containers are frozen pyclasses with interior locking -- never
"Already borrowed"), every single operation is atomic (append, extend, setitem, ...), and
the final contents are exactly the expected multiset. Read-modify-write *sequences* are
not atomic (not for builtins either) and are never asserted.

The builtin containers are included as a reference implementation of the same invariants.
"""

from __future__ import annotations

import os
import random
import sys
import threading
from collections import Counter
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from typing import Any

import pytest
from _support import ListImpl, load_rustique, selected_kinds

N_THREADS = max(4, min(8, (os.cpu_count() or 4)))
SCALE = int(os.environ.get("RUSTIQUE_THREAD_SCALE", "1"))
JOIN_TIMEOUT = 120.0
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


@contextmanager
def fast_switching() -> Iterator[None]:
    old = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    try:
        yield
    finally:
        sys.setswitchinterval(old)


def run_threads(targets: list[Callable[[], Any]], timeout: float = JOIN_TIMEOUT) -> list[Any]:
    """Run each target in its own daemon thread, released together by a barrier.
    Re-raises the first exception from any thread; fails on a suspected deadlock."""
    barrier = threading.Barrier(len(targets))
    results: list[Any] = [None] * len(targets)
    errors: list[BaseException] = []

    def wrap(i: int, f: Callable[[], Any]) -> None:
        try:
            barrier.wait()
            results[i] = f()
        except BaseException as e:  # noqa: BLE001 -- reported below
            errors.append(e)

    threads = [threading.Thread(target=wrap, args=(i, f), daemon=True) for i, f in enumerate(targets)]
    with fast_switching():
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout)
    alive = [t.name for t in threads if t.is_alive()]
    if alive:
        pytest.fail(f"deadlock suspected: threads {alive} still running after {timeout}s")
    if errors:
        raise errors[0]
    return results


def impls(typed: bool = True) -> list[Any]:
    out = []
    for k in selected_kinds():
        if k == "typed" and not typed:
            continue
        out.append(pytest.param(ListImpl(k), id=ListImpl(k).id))
    return out


def new_list(impl: ListImpl, data: list[int] = []) -> Any:  # noqa: B006
    if impl.is_builtin:
        return list(data)
    rs = load_rustique()
    return rs.List[int](data) if impl.kind == "typed" else rs.List(data)


def new_dict(kind: str) -> Any:
    if kind == "builtin":
        return {}
    rs = load_rustique()
    return rs.Dict[int, int]() if kind == "typed" else rs.Dict()


def new_set(kind: str) -> Any:
    if kind == "builtin":
        return set()
    rs = load_rustique()
    return rs.Set[int]() if kind == "typed" else rs.Set()


# ======================================================================================
# List
# ======================================================================================


@pytest.mark.parametrize("impl", impls())
def test_concurrent_append(impl: ListImpl) -> None:
    k = 10_000 * SCALE
    lst = new_list(impl)

    def worker(t: int) -> Callable[[], None]:
        def run() -> None:
            for i in range(k):
                lst.append(t * k + i)

        return run

    run_threads([worker(t) for t in range(N_THREADS)])
    got = list(lst)
    assert len(lst) == len(got) == N_THREADS * k
    assert sorted(got) == list(range(N_THREADS * k))
    # Each thread's appends are linearised in program order.
    for t in range(N_THREADS):
        mine = [x for x in got if t * k <= x < (t + 1) * k]
        assert mine == sorted(mine)


@pytest.mark.parametrize("impl", impls())
def test_concurrent_extend_is_atomic(impl: ListImpl) -> None:
    block, rounds = 64, 500 * SCALE
    lst = new_list(impl)

    def worker(t: int) -> Callable[[], None]:
        def run() -> None:
            for r in range(rounds):
                base = (t * rounds + r) * block
                lst.extend(list(range(base, base + block)))

        return run

    run_threads([worker(t) for t in range(N_THREADS)])
    got = list(lst)
    assert len(got) == N_THREADS * rounds * block
    # Every extend landed as one contiguous block.
    for i in range(0, len(got), block):
        chunk = got[i : i + block]
        assert chunk == list(range(chunk[0], chunk[0] + block)), f"torn extend at {i}"


@pytest.mark.parametrize("impl", impls())
def test_readers_during_appends(impl: ListImpl) -> None:
    k = 8000 * SCALE
    lst = new_list(impl, [0])
    done = threading.Event()
    writers = N_THREADS // 2

    def writer(t: int) -> Callable[[], None]:
        def run() -> None:
            for i in range(k):
                lst.append(1 + t * k + i)

        return run

    def reader() -> int:
        reads = 0
        domain = writers * k + 1
        while not done.is_set():
            n = len(lst)
            assert n >= 1
            x = lst[n - 1]  # never shrinks: index n-1 stays valid
            assert 0 <= x < domain
            assert lst[0] == 0
            assert 0 in lst
            for y in lst[:50]:
                assert 0 <= y < domain
            it = iter(lst)
            for _, y in zip(range(100), it):
                assert 0 <= y < domain
            s = lst.sum() if not impl.is_builtin else sum(lst)
            assert s >= 0
            assert len(lst.copy()) >= n
            assert len(repr(lst)) > 2
            reads += 1
        return reads

    def writers_then_done() -> None:
        try:
            run_threads([writer(t) for t in range(writers)])
        finally:
            done.set()

    results = run_threads([writers_then_done] + [reader for _ in range(N_THREADS - writers)])
    assert all(r >= 1 for r in results[1:])
    assert len(lst) == writers * k + 1
    assert sorted(lst) == list(range(writers * k + 1))


@pytest.mark.parametrize("impl", impls())
def test_mixed_mutations_preserve_multiset(impl: ListImpl) -> None:
    """Random append/insert/pop/setitem/remove/sort/reverse/extend/clear-free mix. Every
    value ever added is tracked, so the final multiset is exactly predictable."""
    ops_per_thread = 5000 * SCALE
    initial = list(range(1000))
    lst = new_list(impl, initial)

    def worker(t: int) -> Callable[[], tuple[Counter[int], Counter[int]]]:
        def run() -> tuple[Counter[int], Counter[int]]:
            rng = random.Random(t)
            added: Counter[int] = Counter()
            removed: Counter[int] = Counter()
            for i in range(ops_per_thread):
                v = 10_000 + t * ops_per_thread + i
                op = rng.randrange(9)
                if op == 0:
                    lst.append(v)
                    added[v] += 1
                elif op == 1:
                    lst.insert(rng.randrange(-5, 5), v)
                    added[v] += 1
                elif op == 2:
                    try:
                        removed[lst.pop()] += 1
                    except IndexError:
                        pass
                elif op == 3:
                    try:
                        removed[lst.pop(0)] += 1
                    except IndexError:
                        pass
                elif op == 4:
                    lst.extend([v, v])
                    added[v] += 2
                elif op == 5 and i % 50 == 0:
                    lst.sort()
                elif op == 6 and i % 50 == 0:
                    lst.reverse()
                elif op == 7:
                    try:
                        x = lst[rng.randrange(-3, 3)]
                        assert isinstance(x, int)
                    except IndexError:
                        pass
                elif op == 8:
                    assert lst.__iadd__([v]) is lst
                    added[v] += 1
            return added, removed

        return run

    results = run_threads([worker(t) for t in range(N_THREADS)])
    expected: Counter[int] = Counter(initial)
    for added, removed in results:
        expected.update(added)
        expected.subtract(removed)
    assert all(c >= 0 for c in expected.values())
    assert Counter(lst) == +expected
    assert len(lst) == sum(expected.values())


@pytest.mark.parametrize("impl", impls())
def test_concurrent_setitem_is_atomic(impl: ListImpl) -> None:
    n = 256
    lst = new_list(impl, [0] * n)
    rounds = 600 * SCALE

    def worker(t: int) -> Callable[[], None]:
        def run() -> None:
            for r in range(rounds):
                for i in range(t, n, N_THREADS):
                    lst[i] = t * 1_000_000 + r
                lst[r % n] = lst[r % n]  # racy copy of an existing slot is still one valid value

        return run

    run_threads([worker(t) for t in range(N_THREADS)])
    assert len(lst) == n
    for x in lst:
        assert x == 0 or (x % 1_000_000) < rounds


@pytest.mark.parametrize("impl", impls(typed=False))
def test_concurrent_demotion(impl: ListImpl) -> None:
    """Native-to-object demotion racing with native appends and reads."""
    k = 4000 * SCALE
    lst = new_list(impl, [0])

    def worker(t: int) -> Callable[[], None]:
        def run() -> None:
            for i in range(k):
                if t == 0 and i == k // 2:
                    lst.append("demote")
                else:
                    lst.append(i)
                _ = lst[-1]
                _ = 5 in lst

        return run

    run_threads([worker(t) for t in range(N_THREADS)])
    assert len(lst) == N_THREADS * k + 1
    assert lst.count("demote") == 1
    if not impl.is_builtin:
        assert lst.strategy == "object"
    assert Counter(x for x in lst if isinstance(x, int)) == Counter([0] + [i for _ in range(N_THREADS) for i in range(k)]) - Counter([k // 2])


@pytest.mark.parametrize("impl", impls())
def test_concurrent_sort_and_iterate(impl: ListImpl) -> None:
    rng = random.Random(7)
    data = [rng.randrange(1000) for _ in range(5000)]
    lst = new_list(impl, data)

    def sorter() -> None:
        for _ in range(20 * SCALE):
            lst.sort()
            lst.sort(reverse=True)
            lst.reverse()

    def iterator() -> None:
        for _ in range(20 * SCALE):
            for x in lst:
                assert 0 <= x < 1000

    def searcher() -> None:
        for _ in range(200 * SCALE):
            assert lst.count(-1) == 0
            assert (-1 in lst) is False
            n = len(lst)
            assert n in (0, len(data))  # builtin sort shows an empty list meanwhile

    run_threads([sorter, sorter, iterator, iterator, searcher, searcher])
    assert sorted(lst) == sorted(data)


def test_concurrent_map_filter_kernels(rs: Any) -> None:
    lst = rs.List[int](range(100_000))
    X = rs.X

    def worker() -> None:
        for _ in range(5 * SCALE):
            assert lst.sum() == 4_999_950_000
            assert lst.map(X * 2).sum() == 9_999_900_000
            assert len(lst.filter(X % 3 == 0)) == 33_334
            assert lst.min() == 0 and lst.max() == 99_999

    def appender() -> None:
        for _ in range(5 * SCALE):
            other = rs.List[int](range(10))
            other.extend(range(10))
            assert len(other) == 20

    run_threads([worker] * (N_THREADS - 1) + [appender])


def test_buffer_exports_race_with_mutation(rs: Any) -> None:
    lst = rs.List[int](range(100))
    stop = threading.Event()
    blocked = Counter[str]()

    def exporter() -> None:
        while not stop.is_set():
            with memoryview(lst) as mv:
                n = mv.shape[0]
                assert mv.tolist()[:2] == [0, 1]
                assert n >= 100

    def mutator() -> None:
        try:
            for i in range(3000 * SCALE):
                try:
                    lst.append(i)
                except BufferError:
                    blocked["append"] += 1
        finally:
            stop.set()

    run_threads([exporter, exporter, mutator])
    assert len(lst) == 100 + 3000 * SCALE - blocked["append"]
    lst.append(1)  # all exports released


def test_alias_creation_is_thread_safe(rs: Any) -> None:

    def worker() -> list[Any]:
        out = []
        for _ in range(200 * SCALE):
            out.append(
                (
                    rs.List[int],
                    rs.List[rs.u8],
                    rs.List[int | None],
                    rs.Dict[str, rs.List[float]],
                    rs.Set[str],
                    rs.List[rs.List[rs.i16]],
                )
            )
        return out

    results = run_threads([worker] * N_THREADS)
    first = results[0][0]
    for per_thread in results:
        for tup in per_thread:
            assert all(a is b for a, b in zip(tup, first))


def test_typed_validation_errors_under_contention(rs: Any) -> None:
    lst = rs.List[int]()

    def worker(t: int) -> Callable[[], int]:
        def run() -> int:
            errors = 0
            for i in range(2000 * SCALE):
                if i % 7 == 0:
                    try:
                        lst.append("bad")
                    except TypeError:
                        errors += 1
                else:
                    lst.append(i)
            return errors

        return run

    errors = run_threads([worker(t) for t in range(N_THREADS)])
    per_thread = 2000 * SCALE
    bad = len(range(0, per_thread, 7))
    assert errors == [bad] * N_THREADS
    assert len(lst) == N_THREADS * (per_thread - bad)
    assert lst.strategy == "i64"


# ======================================================================================
# Dict and Set
# ======================================================================================


@pytest.mark.parametrize("kind", list(selected_kinds()))
def test_dict_concurrent_writers_and_readers(kind: str) -> None:
    d = new_dict(kind)
    k = 5000 * SCALE
    writers = N_THREADS // 2
    done = threading.Event()

    def writer(t: int) -> Callable[[], None]:
        def run() -> None:
            for i in range(k):
                key = t * k + i
                d[key] = key * 2
                if i % 3 == 0:
                    assert d.pop(key) == key * 2
                    d[key] = key * 2
                d.setdefault(key, -1)

        return run

    def reader() -> int:
        n = 0
        while not done.is_set():
            for key in (0, 1, k, writers * k - 1):
                v = d.get(key)
                assert v is None or v == key * 2
            try:
                for key in d:
                    pass
            except RuntimeError:
                pass  # "dictionary changed size during iteration" is allowed
            assert len(d) <= writers * k
            n += 1
        return n

    def writers_then_done() -> None:
        try:
            run_threads([writer(t) for t in range(writers)])
        finally:
            done.set()

    run_threads([writers_then_done] + [reader for _ in range(N_THREADS - writers)])
    assert len(d) == writers * k
    assert all(d[key] == key * 2 for key in range(writers * k))


@pytest.mark.parametrize("kind", list(selected_kinds()))
def test_dict_concurrent_update_and_popitem(kind: str) -> None:
    d = new_dict(kind)
    k = 1000 * SCALE

    def producer(t: int) -> Callable[[], None]:
        def run() -> None:
            for i in range(0, k, 10):
                d.update({t * k + j: 1 for j in range(i, i + 10)})

        return run

    def consumer() -> Counter[int]:
        got: Counter[int] = Counter()
        misses = 0
        while misses < 2000:
            try:
                key, v = d.popitem()
                assert v == 1
                got[key] += 1
                misses = 0
            except KeyError:
                misses += 1
        return got

    producers = N_THREADS // 2
    results = run_threads([producer(t) for t in range(producers)] + [consumer for _ in range(N_THREADS - producers)])
    consumed: Counter[int] = Counter()
    for r in results[producers:]:
        consumed.update(r)
    assert all(c == 1 for c in consumed.values()), "an item was popped twice"
    assert set(consumed) | set(d.keys()) == set(range(producers * k))
    assert not (set(consumed) & set(d.keys()))


@pytest.mark.parametrize("kind", list(selected_kinds()))
def test_set_concurrent_add_discard(kind: str) -> None:
    s = new_set(kind)
    k = 8000 * SCALE

    def worker(t: int) -> Callable[[], None]:
        def run() -> None:
            for i in range(k):
                v = t * k + i
                s.add(v)
                if i % 2:
                    s.discard(v)
                assert (v in s) == (i % 2 == 0)
                if i % 100 == 0:
                    assert s.__ior__({v + N_THREADS * k}) is s

        return run

    run_threads([worker(t) for t in range(N_THREADS)])
    expected = {t * k + i for t in range(N_THREADS) for i in range(0, k, 2)}
    expected |= {t * k + i + N_THREADS * k for t in range(N_THREADS) for i in range(0, k, 100)}
    assert s == expected
    assert len(s) == len(expected)


def test_report_build() -> None:
    """Make the build flavour visible in the test log."""
    print(f"free-threaded={FREE_THREADED} threads={N_THREADS} scale={SCALE}")
