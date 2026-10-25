"""Free-threaded stress tests for `Dict`'s raw-slot fast paths (`dict/fast.rs`): `str` keys
kept as objects, `Obj` probes, lock-free `len()`, the lock-free iterator state and the
recycled items tuple. On GIL builds they run as smoke tests with fast thread switching.

Invariants: no crash, no deadlock, every single operation is atomic, and every value read
belongs to its key.
"""

from __future__ import annotations

import os
import sys
import threading
from collections.abc import Callable
from typing import Any

import pytest

import rustique as rs

N_THREADS = max(4, min(8, (os.cpu_count() or 4)))
SCALE = int(os.environ.get("RUSTIQUE_THREAD_SCALE", "1"))
JOIN_TIMEOUT = 120.0


def run_threads(targets: list[Callable[[], Any]]) -> list[Any]:
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
    old = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    try:
        for t in threads:
            t.start()
        for t in threads:
            t.join(JOIN_TIMEOUT)
    finally:
        sys.setswitchinterval(old)
    alive = [t.name for t in threads if t.is_alive()]
    if alive:
        pytest.fail(f"deadlock suspected: threads {alive} still running")
    if errors:
        raise errors[0]
    return results


class Tag:
    """Identity-compared key (an `Obj` probe)."""

    def __init__(self, n: int) -> None:
        self.n = n


@pytest.mark.parametrize("kind", ["str", "obj", "float"])
def test_writers_readers_and_iterators(kind: str) -> None:
    k = 2000 * SCALE
    writers = N_THREADS // 2
    if kind == "str":
        keys = [[f"k{t}-{i}" for i in range(k)] for t in range(writers)]
    elif kind == "obj":
        keys = [[Tag(t * k + i) for i in range(k)] for t in range(writers)]
    else:
        keys = [[t * k + i + 0.5 for i in range(k)] for t in range(writers)]
    value_of = {id(key): (t, i) for t in range(writers) for i, key in enumerate(keys[t])}
    d = rs.Dict()
    done = threading.Event()

    def writer(t: int) -> Callable[[], None]:
        def run() -> None:
            for i, key in enumerate(keys[t]):
                d[key] = (t, i)
                if i % 3 == 0:
                    assert d.pop(key) == (t, i)
                    assert d.setdefault(key, (t, i)) == (t, i)
                if i % 5 == 0:
                    del d[key]
                    d[key] = (t, i)

        return run

    def reader() -> int:
        n = 0
        while not done.is_set():
            for t in range(writers):
                key = keys[t][n % k]
                v = d.get(key)
                assert v is None or v == value_of[id(key)]
                assert (key in d) in (True, False)
            try:
                for key, v in d.items():
                    assert v == value_of[id(key)]
            except RuntimeError:
                pass  # "dictionary changed size during iteration" is allowed
            assert 0 <= len(d) <= writers * k
            n += 1
        return n

    def writers_then_done() -> None:
        try:
            run_threads([writer(t) for t in range(writers)])
        finally:
            done.set()

    run_threads([writers_then_done] + [reader for _ in range(N_THREADS - writers)])
    assert len(d) == writers * k
    for t in range(writers):
        for i, key in enumerate(keys[t]):
            assert d[key] == (t, i)


def test_one_iterator_shared_by_threads() -> None:
    n = 20000 * SCALE
    d = rs.Dict({f"k{i}": i for i in range(n)})
    for make in (lambda: iter(d.items()), lambda: iter(d), lambda: iter(d.values())):
        it = make()

        def drain() -> list[Any]:
            out = []
            try:
                for item in it:
                    out.append(item)
            except RuntimeError as e:
                # Racing threads may consume one position twice, which (as with CPython's
                # free-threaded dict iterators) can end in this error.
                assert "keys changed" in str(e)
            return out

        results = run_threads([drain for _ in range(N_THREADS)])
        seen = [x for r in results for x in r]
        # Threads sharing one iterator may repeat or skip items (like CPython's free-threaded
        # dict iterators), but every item is a consistent entry of the dict.
        for x in seen:
            if isinstance(x, tuple):
                assert d[x[0]] == x[1]
            elif isinstance(x, str):
                assert x in d
            else:
                assert 0 <= x < n


def test_kept_item_tuples_are_never_recycled_concurrently() -> None:
    d = rs.Dict({i: i for i in range(5000 * SCALE)})

    def keep() -> int:
        kept = list(d.items())
        assert all(k == v for k, v in kept)
        return len(kept)

    assert run_threads([keep for _ in range(N_THREADS)]) == [len(d)] * N_THREADS
