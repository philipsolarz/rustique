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
