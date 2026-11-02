"""Runtime safety of the acceleration framework: the 3.15 JIT's copied `ml_meth`
pointers (stale traces), subinterpreters, free-threaded enable/disable under load.
(docs/ACCELERATION.md §3.8, §3.10, §3.11; docs/atlas/mechanisms.md §4–§6.)
"""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap
import threading

import pytest

from rustique import _rustique
import rustique as rt

A = _rustique._accel
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()


def run(code: str, **env: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-c", textwrap.dedent(code)],
        capture_output=True,
        text=True,
        env={**os.environ, **env},
        timeout=300,
    )


# --------------------------------------------------------------------------------------
# JIT: 3.15's tier 2 copies `ml_meth` into traces (_CALL_METHOD_DESCRIPTOR_*_INLINE);
# the registry clears the executors after every ml_meth install and uninstall.
# --------------------------------------------------------------------------------------

JIT_SCRIPT = """
import sys
jit = getattr(sys, "_jit", None)
if not (jit and jit.is_available() and jit.is_enabled()):
    print("NOJIT")
    raise SystemExit
import rustique as rt
from rustique import _rustique
A = _rustique._accel

def f(l, n):
    c = 0
    for _ in range(n):
        c += l.count(2) + l.index(3)
    return c

def g(l, n):  # same body; warmed only while enabled
    c = 0
    for _ in range(n):
        c += l.count(2) + l.index(3)
    return c

def hits():
    s = A.stats(None, True)
    return tuple(s.get(k, {"hits": 0})["hits"] for k in ("list.count", "list.index"))

l = [1, 2, 3, 2]
assert f(l, 50000) == 50000 * 4          # traces built with the originals
A.stats(True, True)
rt.enable(only=["containers"])
assert f(l, 1000) == 4000
pre = hits()                             # stale: (1, 1) without clearing
assert g(l, 50000) == 50000 * 4          # traces built with the kernels
rt.disable()
hits()
assert g(l, 1000) == 4000
post = hits()                            # stale: (1000, 1000) without clearing
print(pre, post)
"""


def test_jit_traces_follow_install_and_uninstall():
    r = run(JIT_SCRIPT, PYTHON_JIT="1")
    assert r.returncode == 0, r.stderr
    if r.stdout.strip() == "NOJIT":
        pytest.skip("JIT not available in this build")
    # Kernels reached by traces built before enable(); none after disable().
    assert r.stdout.strip() == "(1000, 1000) (0, 0)", r.stdout


# --------------------------------------------------------------------------------------
# Subinterpreters: the extension refuses to load in any of them (§3.11).
# --------------------------------------------------------------------------------------

_interpreters = pytest.importorskip("_interpreters")


def _run_in(config: str, code: str) -> str | None:
    """Run `code` in a fresh subinterpreter; return the exception type name or None."""
    interp = _interpreters.create(config)
    try:
        exc = _interpreters.run_string(interp, textwrap.dedent(code))
    finally:
        _interpreters.destroy(interp)
    return None if exc is None else f"{exc.type.__name__}: {exc.msg}"


@pytest.mark.parametrize("config", ["isolated", "legacy"])
def test_extension_refuses_subinterpreters(config):
    err = _run_in(config, "import rustique._rustique")
    assert err is not None and err.startswith("ImportError"), err
    assert "subinterpreters" in err


def test_process_global_kernels_are_correct_in_subinterpreters():
    rt.enable(only=["containers", "builtins"])
    try:
        err = _run_in(
            "legacy",
            """
            l = [3, 1, 2, 'x'][:3]
            assert l.index(2) == 2 and l.count(1) == 1 and (2 in l) and ('y' not in l)
            assert sorted(l) == [1, 2, 3] and sum(l) == 6 and min(l) == 1 and max(l) == 3
            assert (1, 2).index(2) == 1 and any([0, 1]) and not all([0, 1])
            try:
                l.index(9)
            except ValueError as e:
                assert str(e) == 'list.index(x): x not in list', str(e)
            else:
                raise AssertionError('no error')
            """,
        )
        assert err is None, err
    finally:
        A.disable(["containers", "builtins"])


# --------------------------------------------------------------------------------------
# Free-threaded stress: enable/disable while threads call the kernels (§3.10).
# --------------------------------------------------------------------------------------


def test_enable_disable_under_concurrent_calls():
    stop = threading.Event()
    errors: list[str] = []
    calls = [0]
    data = [5, 3, 9, 1, 7, 3]
    words = ["b", "a", "c"]

    def worker():
        n = 0
        while not stop.is_set():
            try:
                ok = (
                    data.index(9) == 2
                    and data.count(3) == 2
                    and (7 in data)
                    and (4 not in data)
                    and sorted(data) == [1, 3, 3, 5, 7, 9]
                    and sum(data) == 28
                    and min(data) == 1
                    and max(words) == "c"
                    and (1, 2, 3).index(3) == 2
                    and any(data)
                    and all(words)
                )
                if not ok:
                    errors.append("wrong result")
            except Exception as e:  # noqa: BLE001
                errors.append(repr(e))
            n += 1
        calls[0] += n

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for t in threads:
        t.start()
    try:
        for _ in range(200 if FREE_THREADED else 50):
            rt.enable(only=["containers", "builtins"])
            rt.disable()
    finally:
        stop.set()
        for t in threads:
            t.join()
    assert not errors, errors[:5]
    assert calls[0] > 0


def test_concurrent_enable_status_stats_from_threads():
    errors: list[str] = []

    def worker():
        try:
            for _ in range(20):
                rt.enable(only=["containers"])
                rt.status()
                rt.stats()
                A.disable(["containers"])
        except Exception as e:  # noqa: BLE001
            errors.append(repr(e))

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors, errors


def test_lazy_activation_with_concurrent_imports(tmp_path, monkeypatch):
    """Threads import the target module (and other modules) while targets are pending:
    the module is patched exactly once, calls are always correct."""
    (tmp_path / "_rustique_selftest.py").write_text("SEP = '/'\n\ndef same(x):\n    return x\n")
    for i in range(20):
        (tmp_path / f"_rq_other_{i}.py").write_text(f"V = {i}\n")
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop("_rustique_selftest", None)
    errors: list[str] = []
    start = threading.Barrier(5)

    def worker(k: int) -> None:
        import importlib

        start.wait()
        try:
            for i in range(k, 20, 4):
                importlib.import_module(f"_rq_other_{i}")
            m = importlib.import_module("_rustique_selftest")
            for j in range(2000):
                if m.same(j) != j:
                    errors.append("wrong")
        except Exception as e:  # noqa: BLE001
            errors.append(repr(e))

    rt.enable(only=["_selftest.same"])
    try:
        threads = [threading.Thread(target=worker, args=(k,)) for k in range(4)]
        for t in threads:
            t.start()
        start.wait()
        for t in threads:
            t.join()
        assert not errors, errors[:5]
        st = {t["name"]: t["state"] for t in rt.status()["targets"]}
        assert st["_selftest.same"] == "installed"
        import _rustique_selftest as m

        assert not A.vectorcall_is_original(m.same)
    finally:
        A.disable(["_selftest"])
        sys.modules.pop("_rustique_selftest", None)
        for i in range(20):
            sys.modules.pop(f"_rq_other_{i}", None)
