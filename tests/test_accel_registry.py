"""The acceleration framework (docs/ACCELERATION.md §3–§6, §9) through the hidden
`_selftest` targets (crates/rustique-py/src/targets/_selftest.rs), whose kernels are
deliberately visible: they return ``("kernel", x)`` where the originals return
``("orig", x)``.
"""

from __future__ import annotations

import builtins
import importlib
import sys
import textwrap
from unittest import mock

import pytest

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel
MOD = "_rustique_selftest"
SOURCE = textwrap.dedent(
    """
    DEP = object()

    class K:
        attr = 1000

        def __repr__(self):
            return "orig"

    def ident(x):
        return ("orig", x)
    """
)


def state(name: str) -> tuple[str, str | None]:
    for t in rt.status()["targets"]:
        if t["name"] == name:
            return t["state"], t["reason"]
    return "absent", None


@pytest.fixture
def selftest_module(tmp_path, monkeypatch):
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    yield
    A.disable(["_selftest"])
    sys.modules.pop(MOD, None)


@pytest.fixture(autouse=True)
def _cleanup():
    yield
    A.disable(["_selftest"])
    A.stats(False, True)


def test_hidden_group_needs_explicit_selection():
    before = set(A.installed())
    rt.enable()
    try:
        assert not any(n.startswith("_selftest") for n in A.installed())
        assert all(t["group"] != "_selftest" for t in rt.status()["targets"])
    finally:
        A.disable([n for n in A.installed() if n not in before] or ["_selftest"])


def test_unknown_names_raise_before_anything_changes():
    before = A.installed()
    with pytest.raises(ValueError, match="unknown rustique target or group: nope"):
        rt.enable(only=["nope"])
    with pytest.raises(ValueError):
        rt.enable(exclude="containers,nope")
    assert A.installed() == before


def test_ml_meth_site_install_and_restore():
    assert A._probe(1) == ("orig", 1)
    rt.enable(only=["_selftest.meth"])
    assert state("_selftest.meth") == ("installed", None)
    assert A._probe(1) == ("kernel", 1)
    assert A._probe("a") == ("orig", "a")  # kernel falls back outside its subset
    A.stats(True, True)
    A._probe(2), A._probe(None)
    s = A.stats()["_selftest.meth"]
    assert s["hits"] == 1 and s["fallback"]["args"] == 1
    rt.enable(only=["_selftest.meth"])  # idempotent
    A.disable(["_selftest.meth"])
    assert state("_selftest.meth")[0] == "absent"  # hidden and disabled: not listed
    assert A._probe(1) == ("orig", 1)
    assert A._selftest_probe_state() == [True, True, True]


def test_refusal_is_per_target_with_reason():
    rt.enable(only=["_selftest.refuse", "_selftest.meth"])
    assert state("_selftest.refuse") == ("refused", "selftest refusal")
    assert state("_selftest.meth")[0] == "installed"


def test_all_or_nothing_cas():
    try:
        rt.enable(only=["_selftest.cas"])
        st, reason = state("_selftest.cas")
        assert st == "refused" and "changed by someone else" in reason
        # The first site was rolled back, the second left as "someone else" set it.
        assert A._selftest_probe_state() == [True, True, False]
    finally:
        A._selftest_untamper()
    assert A._selftest_probe_state() == [True, True, True]


def test_lazy_activation_on_import(selftest_module):
    rt.enable(only=["_selftest.vectorcall", "_selftest.slot"])
    assert state("_selftest.vectorcall") == ("pending-import", None)
    assert MOD not in sys.modules  # never imported by enable()
    m = importlib.import_module(MOD)
    assert state("_selftest.vectorcall") == ("installed", None)
    assert state("_selftest.slot") == ("installed", None)
    assert m.ident(1) == ("kernel", 1)
    assert m.ident(x=1) == ("orig", 1)
    assert repr(m.K()) == "kernel"

    class Sub(m.K):
        pass

    assert repr(Sub()) == "orig"  # exact type only
    A.disable(["_selftest"])
    assert m.ident(1) == ("orig", 1) and repr(m.K()) == "orig"
    assert A.func_version(m.ident) >= 2  # specializer version given back


def test_already_imported_module_is_patched_at_enable(selftest_module):
    m = importlib.import_module(MOD)
    from _rustique_selftest import ident  # captured before enable

    rt.enable(only=["_selftest.vectorcall"])
    assert state("_selftest.vectorcall") == ("installed", None)
    assert ident(2) == ("kernel", 2) and m.ident is ident


def test_fingerprint_mismatch_refuses(selftest_module):
    importlib.import_module(MOD)
    rt.enable(only=["_selftest.badfp"])
    st, reason = state("_selftest.badfp")
    assert st == "refused" and "fingerprint" in reason


def test_dependency_pins_fall_back_and_rearm(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    assert m.ident(1) == ("kernel", 1)

    dep = m.DEP
    with mock.patch.object(m, "DEP", object()):  # module global replaced
        assert not A._selftest_deps_ok() and m.ident(1) == ("orig", 1)
    assert A._selftest_deps_ok() and m.ident(1) == ("kernel", 1)
    assert m.DEP is dep

    m.SHADOW = 1  # pinned absent
    assert m.ident(1) == ("orig", 1)
    del m.SHADOW
    assert m.ident(1) == ("kernel", 1)

    m.len = len  # shadows the builtin (pin_global)
    assert m.ident(1) == ("orig", 1)
    del m.len
    assert m.ident(1) == ("kernel", 1)

    with mock.patch.object(builtins, "len", lambda x: 0):  # the builtin itself
        assert m.ident(1) == ("orig", 1)
    assert m.ident(1) == ("kernel", 1)

    with mock.patch.object(m.K, "attr", 1):  # class attribute (pin_attr)
        assert m.ident(1) == ("orig", 1)
    assert m.ident(1) == ("kernel", 1)

    A.stats(True, True)
    m.DEP = None
    m.ident(1)
    assert A.stats()["_selftest.vectorcall"]["fallback"]["deps"] == 1
    m.DEP = dep
    pins = A.pins("_selftest.vectorcall")
    assert pins and all(intact and not broken for _, _, intact, broken in pins)


def test_tools_see_the_original(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    seen = []

    def tracer(frame, event, arg):
        if event == "call" and frame.f_code is m.ident.__code__:
            seen.append(event)
        return None

    sys.settrace(tracer)
    try:
        assert m.ident(1) == ("orig", 1)
    finally:
        sys.settrace(None)
    assert seen == ["call"]
    assert m.ident(1) == ("kernel", 1)
