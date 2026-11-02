"""Free-threading hardening of the acceleration framework (DECISIONS §10.9,
docs/atlas/w2-threads.md §3.4): no panic escapes ``enable()`` (S1), the pin tables lock is a
leaf (S2, asserted), suspension is performed by the thread that falls back, and on
free-threaded builds a changed dependency swaps nothing in steady state.

The stress scenarios run in child processes with a deadline (a hang fails the test with every
thread's stack, through ``faulthandler``); they are short here and longer with
``RUSTIQUE_STRESS_SECS``. Uses the hidden ``_selftest`` targets
(crates/rustique-py/src/targets/_selftest.rs) through ``rustique._rustique._accel`` only.
"""

from __future__ import annotations

import importlib
import os
import subprocess
import sys
import textwrap
import threading

import pytest

from rustique import _rustique
from test_accel_registry import MOD, SOURCE

A = _rustique._accel
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()
SECS = float(os.environ.get("RUSTIQUE_STRESS_SECS", "4"))


def target(name: str) -> dict | None:
    for t in A.status()["targets"]:
        if t["name"] == name:
            return t
    return None


@pytest.fixture
def selftest_module(tmp_path, monkeypatch):
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    yield importlib.import_module(MOD)
    A.disable(["_selftest"])
    sys.modules.pop(MOD, None)


@pytest.fixture(autouse=True)
def _cleanup():
    yield
    A.disable(["_selftest"])
    A.stats(False, True)


def run_child(code: str, tmp_path, timeout: float) -> subprocess.CompletedProcess[str]:
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    prelude = textwrap.dedent(
        f"""
        import faulthandler, sys, threading, time
        faulthandler.dump_traceback_later({timeout - 5:.0f}, exit=True)
        sys.path.insert(0, {str(tmp_path)!r})
        from rustique import _rustique
        A = _rustique._accel
        SECS = {SECS}
        """
    )
    return subprocess.run(
        [sys.executable, "-c", prelude + textwrap.dedent(code)],
        capture_output=True,
        text=True,
        timeout=timeout,
    )


# --------------------------------------------------------------------------------------
# The pin tables lock is a leaf (S2, deterministic)
# --------------------------------------------------------------------------------------


def test_pin_tables_lock_is_a_leaf(selftest_module):
    """Every pin kind is activated, mutated, re-armed and released: no CPython call that
    can lock, detach or run code happens while the pin tables lock is held."""
    m = selftest_module
    before = A._lock_violations()
    A.enable(["_selftest.vectorcall", "_selftest.snapshot", "_selftest.shared_a", "_selftest.shared_b"])
    A._selftest_watch(m.__dict__, "SEP")
    A._selftest_watch_dict(m.OBJ.__dict__)
    dep = m.DEP
    m.DEP = object()
    m.K.attr = 7  # a class attribute pin (pin_attr: the type is watched)
    m.OBJ.a = 99  # snapshot pin
    m.__dict__[1] = 2  # a non-str key breaks every pin of the dict
    m.SEP = "\\"
    del m.__dict__[1]
    m.DEP, m.K.attr, m.OBJ.a, m.SEP = dep, 1000, 1, "/"
    m.K.__bases__ = (object,)  # MRO change (type watcher)
    A.pins("_selftest.vectorcall")
    A.status()
    A.disable(["_selftest"])
    installed = A.installed()
    A.enable(["_selftest.vectorcall"])
    try:
        A._exit_hook()  # teardown: every pin and watch released at once
    finally:
        if installed:
            A.enable(installed)
    assert A._lock_violations() == before


# --------------------------------------------------------------------------------------
# S1: no panic escapes enable() while another thread mutates a snapshot-pinned dict
# --------------------------------------------------------------------------------------

S1_CHILD = """
import copy
import _rustique_selftest as m
stop = threading.Event()
def churn(d, key):
    while not stop.is_set():
        d[key] = 1
        del d[key]
for d in (m.OBJ.__dict__, m.OBJ.__dict__, copy.__dict__):
    threading.Thread(target=churn, args=(d, f"_k{id(d) % 7}"), daemon=True).start()
errors, refused, cycles = {}, 0, 0
t0 = time.monotonic()
while time.monotonic() - t0 < SECS:
    try:
        A.enable(["_selftest.snapshot", "copy"])
        st = {t["name"]: t for t in A.status()["targets"] if t["name"] in ("_selftest.snapshot", "copy.deepcopy")}
        refused += sum(t["state"] != "installed" for t in st.values())
        A.disable(["_selftest", "copy"])
    except BaseException as e:  # PanicException derives from BaseException
        errors[repr(e)[:200]] = errors.get(repr(e)[:200], 0) + 1
    cycles += 1
stop.set()
print("cycles", cycles, "errors", errors, "refused", refused)
assert not errors, errors
assert refused == 0, refused
assert A._lock_violations() == 0
"""


def test_enable_survives_concurrent_writes_to_snapshot_pinned_dicts(tmp_path):
    r = run_child(S1_CHILD, tmp_path, timeout=SECS * 6 + 60)
    assert r.returncode == 0, r.stdout + r.stderr[-4000:]


# --------------------------------------------------------------------------------------
# S2: activation of class-attribute pins against writers on the class (stress)
# --------------------------------------------------------------------------------------

S2_CHILD = """
import _rustique_selftest as m
stop = threading.Event()
def setattrs(cls, name):
    while not stop.is_set():
        setattr(cls, name, 1)   # type_setattro: type lock + dict section, then the dict watcher
        delattr(cls, name)
def mutate_dict():
    while not stop.is_set():
        m.DEP2 = 1
        del m.DEP2
for f, a in ((setattrs, (m.K, "_s1")), (setattrs, (m.K, "_s2")), (mutate_dict, ())):
    threading.Thread(target=f, args=a, daemon=True).start()
cycles = 0
t0 = time.monotonic()
while time.monotonic() - t0 < SECS:
    A.enable(["_selftest.vectorcall", "_selftest.shared_a"])
    assert m.ident(1) in (("kernel", 1), ("orig", 1))
    A.disable(["_selftest"])
    cycles += 1
stop.set()
print("cycles", cycles)
assert A._lock_violations() == 0
"""


def test_class_attribute_pins_against_class_writers(tmp_path):
    r = run_child(S2_CHILD, tmp_path, timeout=SECS * 6 + 60)
    assert r.returncode == 0, r.stdout + r.stderr[-4000:]


# --------------------------------------------------------------------------------------
# Suspension is performed by the thread that falls back
# --------------------------------------------------------------------------------------


def test_worker_fallbacks_suspend_while_main_thread_waits(selftest_module):
    """The main thread blocks in join(): it runs no pending call. The worker's 64th
    consecutive fallback (no hit since armed: the early exit) must suspend the target itself."""
    m = selftest_module
    A.enable(["_selftest.vectorcall"])
    seen: list[str] = []

    def worker():
        for _ in range(64):
            m.ident(x=1)  # keyword argument: outside the subset
        seen.append(A._selftest_vectorcall(m.ident))
        seen.append(target("_selftest.vectorcall").get("suspended"))

    t = threading.Thread(target=worker)
    t.start()
    t.join()
    assert seen == ["original", "fallback"]


@pytest.mark.skipif(not FREE_THREADED, reason="free-threaded policy")
def test_changed_dependency_swaps_nothing_until_net_benefit(selftest_module):
    """On free-threaded builds a pinned dependency change does not swap the site (a swap
    stops the world): calls fall back through the deps guard and debit the score; a
    dependency that stays changed ends in one net-benefit suspension, resumed by enable()."""
    m = selftest_module
    A.enable(["_selftest.vectorcall"])
    dep = m.DEP
    m.DEP = object()
    for _ in range(10):
        assert m.ident(1) == ("orig", 1)
    assert A._selftest_vectorcall(m.ident) == "kernel"
    assert "suspended" not in target("_selftest.vectorcall")
    m.DEP = dep  # re-armed: hits again, no swap happened
    assert m.ident(1) == ("kernel", 1)
    m.DEP = object()
    for _ in range(300):
        m.ident(1)
    assert A._selftest_vectorcall(m.ident) == "original"
    assert target("_selftest.vectorcall")["suspended"] == "fallback"
    m.DEP = dep
    for _ in range(10):
        assert m.ident(1) == ("orig", 1)  # suspended until the next enable()
    A.enable(["_selftest.vectorcall"])
    assert m.ident(1) == ("kernel", 1)


# --------------------------------------------------------------------------------------
# Lazy activation: never inside the sys.modules watcher on free-threaded builds
# --------------------------------------------------------------------------------------


def test_lazy_activation_completes_without_a_registry_call(tmp_path, monkeypatch):
    """Imported on a worker thread while the main thread waits: the module's targets are
    activated by a pending call (any thread) or the import path, before the next registry
    operation."""
    (tmp_path / f"{MOD}.py").write_text("SEP = '/'\n\ndef same(x):\n    return x\n")
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    A.enable(["_selftest.same"])
    out: list[object] = []

    def worker():
        m = importlib.import_module(MOD)
        for _ in range(200):  # backward jumps: eval-breaker checks run the pending call
            pass
        out.append(A.vectorcall_is_original(m.same))

    t = threading.Thread(target=worker)
    t.start()
    t.join()
    try:
        assert out == [False]
    finally:
        A.disable(["_selftest"])
        sys.modules.pop(MOD, None)
