"""Validate on first call (docs/ACCELERATION.md §3.2, w2-startup §1.7, w2-floors §4): a target
is *armed* at install (sites resolved, first-call stubs swapped in, no fingerprint, self-test
or pin yet) and validates in full at its first call, which is then dispatched to the kernel or,
on a refusal, to the original. GIL builds only (free-threaded builds validate eagerly)."""

from __future__ import annotations

import importlib
import sys

import pytest

from rustique import _rustique
from test_accel_registry import MOD, SOURCE

A = _rustique._accel
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()

pytestmark = pytest.mark.skipif(FREE_THREADED, reason="free-threaded builds validate eagerly")


def state(name: str) -> tuple[str, str | None, str | None]:
    for t in A.status()["targets"]:
        if t["name"] == name:
            return t["state"], t["reason"], t["refusal"]
    return ("absent", None, None)


@pytest.fixture(autouse=True)
def first_call():
    prev = A._set_validation("first-call")
    yield
    A.disable(["_selftest"])
    A._set_validation(prev)


@pytest.fixture
def m(tmp_path, monkeypatch):
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    yield importlib.import_module(MOD)
    A.disable(["_selftest"])
    sys.modules.pop(MOD, None)


def test_ml_meth_target_is_armed_then_validated_by_its_first_call():
    A.enable(["_selftest.meth"])
    assert state("_selftest.meth")[0] == "armed"
    assert A.status()["process"]["validate"] == "first-call"
    assert "_selftest.meth" in A.installed()  # armed counts as installed there
    assert A._probe("a") == ("orig", "a")  # the first call: validated, then the kernel falls back
    assert state("_selftest.meth")[0] == "installed"
    assert A._probe(1) == ("kernel", 1)
    A.disable(["_selftest.meth"])
    assert A._probe(1) == ("orig", 1) and A._selftest_probe_state() == [True, True, True]


def test_first_call_is_dispatched_to_the_new_kernel(m):
    A.enable(["_selftest.vectorcall", "_selftest.slot"])
    assert state("_selftest.vectorcall")[0] == "armed" and state("_selftest.slot")[0] == "armed"
    assert m.ident(1) == ("kernel", 1)  # validated by this very call
    assert state("_selftest.vectorcall")[0] == "installed"
    assert repr(m.K()) == "kernel"
    assert state("_selftest.slot")[0] == "installed"


def test_lazy_install_arms_and_validates_at_first_call(tmp_path, monkeypatch):
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    try:
        A.enable(["_selftest.vectorcall"])
        assert state("_selftest.vectorcall")[0] == "pending-import"
        m = importlib.import_module(MOD)
        assert state("_selftest.vectorcall")[0] == "armed"
        assert m.ident(x=2) == ("orig", 2)
        assert state("_selftest.vectorcall")[0] == "installed"
        assert m.ident(2) == ("kernel", 2)
    finally:
        A.disable(["_selftest"])
        sys.modules.pop(MOD, None)


def test_refusal_at_first_call_restores_the_original(m):
    A.enable(["_selftest.badfp"])
    assert state("_selftest.badfp")[0] == "armed"  # fingerprints are checked at the first call
    assert m.ident(3) == ("orig", 3)
    st, reason, refusal = state("_selftest.badfp")
    assert st == "refused" and refusal == "fingerprint" and "fingerprint" in reason
    assert A._selftest_vectorcall(m.ident) == "original"


def test_disable_while_armed_restores_everything(m):
    A.enable(["_selftest.meth", "_selftest.vectorcall"])
    assert A._selftest_vectorcall(m.ident) == "other"  # the first-call stub
    A.disable(["_selftest"])
    assert A._selftest_vectorcall(m.ident) == "original"
    assert A._selftest_probe_state() == [True, True, True]
    assert m.ident(1) == ("orig", 1) and A._probe(1) == ("orig", 1)
    assert A.func_version(m.ident) >= 2


def test_validate_now_and_eager_enable_validate_armed_targets(m):
    A.enable(["_selftest.meth", "_selftest.vectorcall"])
    A.validate(["_selftest.meth"])
    assert state("_selftest.meth")[0] == "installed" and state("_selftest.vectorcall")[0] == "armed"
    A._set_validation("eager")
    A.enable(["_selftest.vectorcall"])
    assert state("_selftest.vectorcall")[0] == "installed"


def test_conflict_found_while_arming_validates_now():
    try:
        A.enable(["_selftest.cas"])
        st, reason, refusal = state("_selftest.cas")
        assert st == "refused" and refusal == "conflict"
        assert A._selftest_probe_state()[:3] == [True, True, False]
    finally:
        A._selftest_untamper()
