"""Regression tests for the Dict/Set/expression safety review (crashes, deadlocks under
container locks, livelocks, iteration semantics, expression limits).

Scenarios that used to crash or deadlock run in a subprocess with a timeout, so a
regression fails cleanly instead of taking the test run down with it.
"""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap

import pytest

import rustique as rs

FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


def run_isolated(code: str, *flags: str, timeout: float = 60, env: dict[str, str] | None = None) -> str:
    """Run `code` in a fresh interpreter; fail on a crash, a hang or a non-zero exit."""
    script = "import faulthandler; faulthandler.enable()\n" + textwrap.dedent(code)
    try:
        proc = subprocess.run(
            [sys.executable, *flags, "-c", script],
            capture_output=True,
            text=True,
            timeout=timeout,
            env={**os.environ, **(env or {})},
        )
    except subprocess.TimeoutExpired as e:
        pytest.fail(f"timed out after {timeout}s (deadlock?)\nstdout: {e.stdout}\nstderr: {e.stderr}")
    assert proc.returncode == 0, f"exit {proc.returncode}\nstdout: {proc.stdout}\nstderr: {proc.stderr}"
    return proc.stdout


# --- C1: no conversion that may run Python code inside a source's critical section ----------


@pytest.mark.parametrize("mode", ["dict", "list", "f32"])
def test_update_from_builtin_never_converts_borrowed_subclass_values(mode):
    out = run_isolated(
        f"""
        import rustique as rs

        class Evil(int):
            def __str__(self):
                src.clear()
                return "evil"

            __repr__ = __str__

        class EvilF(float):
            def __repr__(self):
                src.clear()
                return "evilf"

            __str__ = __repr__

        mode = {mode!r}
        if mode == "dict":
            src, d = {{"a": Evil(1000)}}, rs.Dict[str, rs.i8]()
        elif mode == "list":
            src, d = [("a", Evil(1000))], rs.Dict[str, rs.i8]()
        else:
            src, d = {{"a": EvilF(1e300)}}, rs.Dict[str, rs.f32]()
        try:
            d.update(src)
        except OverflowError:
            print("overflow")
        print(len(d))
        """,
        env={"PYTHONMALLOC": "debug"},
    )
    assert out.split() == ["overflow", "0"]


def test_update_from_builtin_still_converts_valid_values():
    d = rs.Dict[str, rs.i8]()
    d.update({"a": 1, "b": -128})
    d.update([("c", 127), ("d", True)])
    assert d.to_dict() == {"a": 1, "b": -128, "c": 127, "d": 1}

    class Small(int):
        pass

    d.update({"e": Small(5)})
    assert d["e"] == 5
    with pytest.raises(OverflowError):
        d.update({"f": 300})
    u = rs.Dict({"x": 1.5, "y": "s", "z": None})
    assert u.to_dict() == {"x": 1.5, "y": "s", "z": None}


# --- H1: Python objects are never released while a container lock is held -------------------


def test_setdefault_releases_an_unused_default_after_the_lock():
    out = run_isolated(
        """
        import rustique as rs

        d = rs.Dict[str, rs.List]()

        class Bomb:
            def __del__(self):
                print("del", len(d), flush=True)  # needs d's lock

        def gen():
            d["k"] = []  # the key appears while the default is being converted
            yield Bomb()

        print("got", d.setdefault("k", gen()), flush=True)
        """,
        timeout=30,
    )
    assert out.split() == ["del", "1", "got", "List([])"]


def test_failed_batch_releases_pending_items_after_the_lock():
    # Under -bb the bytes/str comparison raises; the value in flight and the items not yet
    # inserted are released (finalizers touching the dict run) only after the lock.
    out = run_isolated(
        """
        import rustique as rs

        d = rs.Dict({b"a": 1})

        class Bomb:
            def __del__(self):
                print("del", len(d), flush=True)

        try:
            d.update(iter([("a", Bomb()), ("z", Bomb())]))
        except BytesWarning:
            print("BytesWarning", flush=True)
        print("len", len(d))
        """,
        "-bb",
        timeout=30,
    )
    assert out.split() == ["del", "1", "del", "1", "BytesWarning", "len", "1"]


def test_failed_eq_mid_batch_keeps_prefix_and_releases_rest():
    class BadEq:
        def __hash__(self):
            return hash("x")

        def __eq__(self, other):
            raise ValueError("no eq")

    released = []

    class Value:
        def __init__(self, n):
            self.n = n

        def __del__(self):
            released.append((self.n, len(d)))

    d = rs.Dict({"x": 0})
    with pytest.raises(ValueError, match="no eq"):
        d.update([("a", Value(1)), (BadEq(), Value(2)), ("b", Value(3))])
    assert list(d) == ["x", "a"]
    assert sorted(released) == [(2, 2), (3, 2)]
