"""Site support added in wave 2 (docs/ACCELERATION.md §1, §3): slots of heap types defined in
Python (`Prepare::heap_slot`), dynamic vectorcall sites (`accel::dynamic::install`) with the
`on_install` hook, and the per-class cache keyed by type version tags (`accel::ClassCache`,
`guard::watch_dict_invalidates`), through the hidden `_selftest` targets."""

from __future__ import annotations

import gc
import importlib
import sys

import pytest

from rustique import _rustique
from test_accel_registry import MOD, SOURCE

A = _rustique._accel


def target(name: str) -> dict | None:
    for t in A.status()["targets"]:
        if t["name"] == name:
            return t
    return None


@pytest.fixture
def m(tmp_path, monkeypatch):
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    yield importlib.import_module(MOD)
    A.disable(["_selftest"])
    sys.modules.pop(MOD, None)


def test_heap_type_slot_install_restore_and_subclasses(m):
    obj = m.Holder()
    assert obj.p == ("orig", obj)
    A.enable(["_selftest.heap"])
    t = target("_selftest.heap")
    assert t["state"] == "installed" and t["sites"] == ["Prop.tp_descr_get (slot)"], t
    assert obj.p == ("kernel", obj)

    class Sub(m.Prop):  # created while installed: inherits the kernel pointer
        pass

    class H2:
        q = Sub()

    h = H2()
    assert h.q == ("orig", h)  # not the exact type: the original dispatches
    A.disable(["_selftest.heap"])
    assert obj.p == ("orig", obj) and h.q == ("orig", h)  # Sub keeps the kernel: still exact


def test_heap_type_slot_rewritten_by_cpython_is_a_conflict(m):
    obj = m.Holder()
    A.enable(["_selftest.heap"])
    assert obj.p == ("kernel", obj)
    m.Prop.__get__ = lambda self, o, c=None: ("new", o)  # update_slot rewrites tp_descr_get
    assert obj.p == ("new", obj)
    t = target("_selftest.heap")
    assert t["state"] == "installed" and t["suspended"] == "conflict"
    A.disable(["_selftest.heap"])
    assert obj.p == ("new", obj)  # CPython's pointer is left alone


def test_heap_slot_refuses_a_c_dunder_and_a_different_reference(m):
    del m.Prop2.__get__  # Prop2 no longer has the dispatcher: the cross-check fails
    A.enable(["_selftest.heap"])
    t = target("_selftest.heap")
    assert t["state"] == "refused" and t["refusal"] == "conflict", t


def test_dynamic_sites_adopt_install_count_and_restore(m):
    assert m.made_before(1) == ("orig", 1)
    A.enable(["_selftest.dyn"])
    t = target("_selftest.dyn")
    assert t["state"] == "installed", t
    assert m.made_before(1) == ("kernel", 1)  # adopted by on_install
    ns: dict = {}
    exec("def made_after(x):\n    return ('orig', x)\n", ns)
    f = ns["made_after"]
    assert A._selftest_dyn_install(f) is True
    assert A._selftest_dyn_install(f) is False  # already patched
    assert f(2) == ("kernel", 2) and f(x=2) == ("orig", 2)
    assert target("_selftest.dyn")["dynamic_sites"] == 2
    del ns, f
    gc.collect()  # no reference is held: the function goes
    A.disable(["_selftest.dyn"])
    # Retired lazily: the next call gives the function CPython's vectorcall back.
    assert A._selftest_vectorcall(m.made_before) == "other"
    assert m.made_before(1) == ("orig", 1)
    assert A._selftest_vectorcall(m.made_before) == "original"
    assert A.func_version(m.made_before) >= 2  # specializable again
    assert target("_selftest.dyn") is None or target("_selftest.dyn").get("dynamic_sites", 0) == 1
    g = lambda x: ("orig", x)  # noqa: E731
    assert A._selftest_dyn_install(g) is False  # target not installed


def test_class_cache_keyed_by_version_tag():
    class C:
        pass

    class D(C):
        pass

    assert A._selftest_class_cache(C) is None
    assert A._selftest_class_cache(C, 7) == 7
    assert A._selftest_class_cache(C) == 7
    C.x = 1  # any modification resets the tag
    assert A._selftest_class_cache(C) is None
    assert A._selftest_class_cache(D, 9) == 9
    C.y = 2  # ... of a base too
    assert A._selftest_class_cache(D) is None


def test_class_cache_dict_invalidation():
    class C:
        pass

    d = {"a": 1}
    A._selftest_class_cache_watch(d, C)
    assert A._selftest_class_cache(C, 3) == 3
    d["a"] = 1  # same object: no event
    assert A._selftest_class_cache(C) == 3
    d["b"] = 2
    assert A._selftest_class_cache(C) is None
    assert A._selftest_class_cache(C, 4) == 4
    del d  # deallocated: a last invalidation, and the weak interest goes with it
    gc.collect()
    assert A._selftest_class_cache(C) is None
    assert A._selftest_class_cache(C, 5) == 5
    e = {"a": 1}  # possibly at the same address: not watched
    e["b"] = 2
    assert A._selftest_class_cache(C) == 5
