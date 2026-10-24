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
