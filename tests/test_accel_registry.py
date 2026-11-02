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
import rustique as rt

A = _rustique._accel
MOD = "_rustique_selftest"
SOURCE = textwrap.dedent(
    """
    DEP = object()

    class Obj:
        shadow = "class"

    OBJ = Obj()
    OBJ.a = 1
    OBJ.b = "two"

    def ident2(x):
        return ("orig", x)

    SEP = "/"

    def same_a(x):
        return x

    def same_b(x):
        return x

    class K:
        attr = 1000

        def __repr__(self):
            return "orig"

    def ident(x):
        return ("orig", x)
    """
)


def target(name: str) -> dict | None:
    for t in rt.status()["targets"]:
        if t["name"] == name:
            return t
    return None


def state(name: str) -> tuple[str, str | None]:
    t = target(name)
    return (t["state"], t["reason"]) if t else ("absent", None)


def settle():
    """Let pending calls (deopt-while-unpinned resyncs) run: they run at eval-breaker
    checks, e.g. on backward jumps."""
    for _ in range(1000):
        pass


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
    rt.check(None)


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
    assert target("_selftest.refuse")["refusal"] == "other"
    assert state("_selftest.meth")[0] == "installed"
    assert target("_selftest.meth")["refusal"] is None


def test_all_or_nothing_cas():
    try:
        rt.enable(only=["_selftest.cas"])
        st, reason = state("_selftest.cas")
        assert st == "refused" and "changed by someone else" in reason
        assert target("_selftest.cas")["refusal"] == "conflict"
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
    assert target("_selftest.badfp")["refusal"] == "fingerprint"


def test_dependency_pins_fall_back_and_rearm(selftest_module):
    # A pin's flag flips synchronously (the kernel falls back at once); restoring the
    # kernel's vectorcall after a re-arm happens in a pending call (settle()).
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    assert m.ident(1) == ("kernel", 1)

    dep = m.DEP
    with mock.patch.object(m, "DEP", object()):  # module global replaced
        assert not A._selftest_deps_ok() and m.ident(1) == ("orig", 1)
    assert A._selftest_deps_ok() and m.DEP is dep
    settle()
    assert m.ident(1) == ("kernel", 1)

    m.SHADOW = 1  # pinned absent
    assert not A._selftest_deps_ok() and m.ident(1) == ("orig", 1)
    del m.SHADOW
    assert A._selftest_deps_ok()

    m.len = len  # shadows the builtin (pin_global)
    assert not A._selftest_deps_ok() and m.ident(1) == ("orig", 1)
    del m.len
    assert A._selftest_deps_ok()

    with mock.patch.object(builtins, "len", lambda x: 0):  # the builtin itself
        assert not A._selftest_deps_ok() and m.ident(1) == ("orig", 1)
    assert A._selftest_deps_ok()

    with mock.patch.object(m.K, "attr", 1):  # class attribute (pin_attr)
        assert not A._selftest_deps_ok() and m.ident(1) == ("orig", 1)
    assert A._selftest_deps_ok()
    settle()
    assert m.ident(1) == ("kernel", 1)

    A.stats(True, True)
    m.DEP = None
    m.ident(1)  # before the pending resync: the kernel's deps guard falls back
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


def test_check_mode():
    rt.enable(only=["_selftest.meth"])
    assert target("_selftest.meth")["checkable"]
    assert rt.check() is None
    assert A._probe(1) == ("kernel", 1)  # the self-test kernel deliberately differs
    assert rt.check("warn") is None and rt.check() == "warn"
    assert A._probe(1) == ("orig", 1)  # mismatch: the original's result wins
    assert A.stats()["_selftest.meth"]["fallback"]["check"] == 1
    rt.check("raise")
    with pytest.raises(rt.RustiqueCheckError, match="kernel .* != original"):
        A._probe(1)
    assert isinstance(rt.RustiqueCheckError("x"), AssertionError)
    assert rt.check(False) == "raise" and rt.check() is None
    assert A._probe(1) == ("kernel", 1)
    with pytest.raises(ValueError):
        rt.check("loud")


def test_snapshot_pin(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.snapshot"])
    assert m.ident2(1) == ("kernel", 1)
    assert A._selftest_vectorcall(m.ident2) == "kernel"
    m.OBJ.a = 2  # replaced
    assert not A._selftest_snapshot_ok()
    m.OBJ.c = 3  # added: a second deviation
    m.OBJ.a = 1  # first undone (same small int object)
    assert not A._selftest_snapshot_ok()
    del m.OBJ.c
    assert A._selftest_snapshot_ok()
    with mock.patch.object(m.OBJ, "shadow", "instance"):  # absent key that shadows a class attr
        assert not A._selftest_snapshot_ok()
    assert A._selftest_snapshot_ok()
    b = m.OBJ.b
    del m.OBJ.b
    assert not A._selftest_snapshot_ok()
    m.OBJ.b = b
    assert A._selftest_snapshot_ok()
    settle()
    assert m.ident2(1) == ("kernel", 1)
    m.OBJ.__dict__.clear()  # wholesale: broken until re-pinned
    assert not A._selftest_snapshot_ok()
    m.OBJ.a, m.OBJ.b = 1, b
    assert not A._selftest_snapshot_ok()


def test_generation_watch(selftest_module):
    m = importlib.import_module(MOD)
    d = m.__dict__
    A._selftest_watch(d, "DEP")
    A._selftest_watch(d, "DEP")  # idempotent
    g = A._selftest_gen()
    dep = m.DEP
    m.DEP = dep  # same object: CPython sends no event
    assert A._selftest_gen() == g
    m.DEP = object()
    assert A._selftest_gen() == g + 1
    m.OTHER = 1  # other key: no bump
    assert A._selftest_gen() == g + 1
    del m.DEP
    m.DEP = dep
    assert A._selftest_gen() == g + 3


@pytest.mark.skipif(
    not getattr(sys, "_is_gil_enabled", lambda: True)(),
    reason="GIL builds only: free-threaded builds swap nothing for a dependency (test_accel_threads)",
)
def test_deopt_while_unpinned(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    assert A._selftest_vectorcall(m.ident) == "kernel"
    dep = m.DEP
    m.DEP = object()
    settle()
    assert A._selftest_vectorcall(m.ident) == "original"
    assert A.func_version(m.ident) >= 2  # specializable again while suspended
    t = target("_selftest.vectorcall")
    assert t["state"] == "installed" and t["suspended"] == "deps"
    assert m.ident(1) == ("orig", 1)
    m.DEP = dep
    settle()
    assert A._selftest_vectorcall(m.ident) == "kernel"
    assert not target("_selftest.vectorcall").get("suspended")
    assert m.ident(1) == ("kernel", 1)
    # Unstable dependencies: give up after too many suspensions.
    for _ in range(20):
        m.DEP = object()
        settle()
        m.DEP = dep
        settle()
    t = target("_selftest.vectorcall")
    assert t["state"] == "refused" and t["refusal"] == "deps" and "unstable" in t["reason"]
    assert A._selftest_vectorcall(m.ident) == "original"
    assert m.ident(1) == ("orig", 1)


def test_persistent_fallback_suspends_until_next_enable(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    A.stats(True, True)
    for _ in range(63):
        assert m.ident(x=1) == ("orig", 1)  # keyword argument: outside the subset
    settle()
    assert A._selftest_vectorcall(m.ident) == "kernel"
    m.ident(x=1)  # the 64th consecutive fallback from a fresh install
    settle()
    assert A._selftest_vectorcall(m.ident) == "original"
    assert A.func_version(m.ident) >= 2
    t = target("_selftest.vectorcall")
    assert t["state"] == "installed" and t["suspended"] == "fallback"
    st = A.stats()["_selftest.vectorcall"]
    assert st["suspensions"] == 1 and st["fallback"]["args"] == 64 and st["hits"] == 0
    for _ in range(1000):  # suspended: CPython's own call path, no kernel
        assert m.ident(1) == ("orig", 1)
    assert A.stats()["_selftest.vectorcall"]["hits"] == 0
    rt.enable(only=["_selftest.vectorcall"])  # re-arms, with a fresh score
    assert A._selftest_vectorcall(m.ident) == "kernel"
    assert "suspended" not in target("_selftest.vectorcall")
    assert m.ident(1) == ("kernel", 1)
    for _ in range(63):
        m.ident(x=1)
    settle()
    assert A._selftest_vectorcall(m.ident) == "kernel"


def _mixed(m, every, calls):
    """One hit, then `every - 1` fallbacks, repeated for `calls` calls."""
    for n in range(calls):
        if n % every == 0:
            m.ident(1)
        else:
            m.ident(x=1)
    settle()


@pytest.mark.parametrize("every", [2, 5])
def test_mixed_traffic_that_pays_off_never_suspends(selftest_module, every):
    # 1:1 alternation, and 20% hits (the break-even of the default 4 / 1 policy).
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    _mixed(m, every, 5000)
    assert A._selftest_vectorcall(m.ident) == "kernel"
    assert "suspended" not in target("_selftest.vectorcall")


@pytest.mark.parametrize(("every", "still_on"), [(10, 500), (100, 200)])
def test_mostly_fallback_traffic_suspends(selftest_module, every, still_on):
    # Starting from the ceiling (a long run of hits), with hits interleaved so that
    # (for every=10) no run of fallbacks is anywhere near 64 long.
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    A.stats(True, True)
    for _ in range(200):
        m.ident(1)
    _mixed(m, every, still_on)
    assert A._selftest_vectorcall(m.ident) == "kernel"  # still paying off its credit
    _mixed(m, every, 1000)
    assert A._selftest_vectorcall(m.ident) == "original"
    assert target("_selftest.vectorcall")["suspended"] == "fallback"
    assert A.stats()["_selftest.vectorcall"]["suspensions"] == 1


def test_override_verdict_cache():
    class Base:
        def join(self):
            pass

    class Plain(Base):
        pass

    class Over(Base):
        def __fspath__(self):
            return ""

    class Deep(Over):
        pass

    class Other:
        pass

    assert A._selftest_overrides(Plain, Base) is False
    assert A._selftest_overrides(Over) is True
    assert A._selftest_overrides(Deep) is True
    assert A._selftest_overrides(Other) is None
    assert A._selftest_overrides(Plain) is False  # cached
    Plain.join = lambda self: None  # modification resets the version tag
    assert A._selftest_overrides(Plain) is True
    del Plain.join
    assert A._selftest_overrides(Plain) is False
    Base.__fspath__ = lambda self: ""  # defined on the base itself: not an override
    assert A._selftest_overrides(Plain) is False


def test_whole_dict_generation_watch():
    d = {"a": 1}
    g = A._selftest_watch_dict(d)
    assert A._selftest_watch_dict(d) == g  # idempotent, no event
    d["b"] = 2
    d["a"] = 3
    del d["b"]
    assert A._selftest_watch_dict(d) == g + 3
    d["a"] = 3  # same object: CPython sends no event
    assert A._selftest_watch_dict(d) == g + 3
    d.clear()
    assert A._selftest_watch_dict(d) == g + 4


def test_shared_group_declarations(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.shared_a", "_selftest.shared_b"])
    a, b = target("_selftest.shared_a"), target("_selftest.shared_b")
    assert a["state"] == b["state"] == "installed", (a, b)
    assert a["shared"] == b["shared"] == "_selftest.shared" and a["deps_intact"]
    assert A._selftest_vectorcall(m.same_a) == "other" or not A.vectorcall_is_original(m.same_a)
    sep = m.SEP
    m.SEP = "\\"  # the shared pin guards both targets
    assert not A._selftest_shared_ok()
    m.SEP = sep
    assert A._selftest_shared_ok()
    A.disable(["_selftest.shared_a"])  # the other user keeps the shared pins
    assert A.vectorcall_is_original(m.same_a) and not A.vectorcall_is_original(m.same_b)
    m.SEP = "\\"
    assert not A._selftest_shared_ok()
    m.SEP = sep
    A.disable(["_selftest.shared_b"])  # the last user releases them
    m.SEP = "\\"
    assert A._selftest_shared_ok()  # a flag without pins is true: nothing watches SEP now
    m.SEP = sep


def test_shared_group_refusal_refuses_every_user(selftest_module):
    m = importlib.import_module(MOD)
    m.REFUSE_SHARED = True
    rt.enable(only=["_selftest.shared_a", "_selftest.shared_b"])
    for n in ("_selftest.shared_a", "_selftest.shared_b"):
        st, reason = state(n)
        assert st == "refused" and reason == "shared refusal"
    assert A.vectorcall_is_original(m.same_a) and A.vectorcall_is_original(m.same_b)


def test_degrade_annotates_status():
    rt.enable(only=["_selftest.meth"])
    assert "degraded" not in target("_selftest.meth")
    A._selftest_degrade("_selftest.meth", "lazy fingerprint mismatch")
    A._selftest_degrade("_selftest.meth", "second reason")  # idempotent: the first stays
    t = target("_selftest.meth")
    assert t["state"] == "installed" and t["degraded"] == "lazy fingerprint mismatch"
    A.disable(["_selftest.meth"])
    rt.enable(only=["_selftest.meth"])
    assert "degraded" not in target("_selftest.meth")


# --- interpreter exit (the registry's atexit hook) -------------------------------------


def test_exit_hook_releases_everything(selftest_module):
    m = importlib.import_module(MOD)
    before = A.installed()
    base = (sys.getrefcount(m.DEP), sys.getrefcount(m.__dict__), sys.getrefcount(m.K), sys.getrefcount(m.ident))
    rt.enable(only=["_selftest.vectorcall", "_selftest.snapshot", "_selftest.same"])
    A._selftest_watch(m.__dict__, "SEP")  # a process-lifetime generation watch
    assert A.exit_hook_registered()
    assert A._selftest_vectorcall(m.ident) == "kernel" and A._selftest_deps_ok()
    held = (sys.getrefcount(m.DEP), sys.getrefcount(m.__dict__), sys.getrefcount(m.K), sys.getrefcount(m.ident))
    assert all(h > b for h, b in zip(held, base)), (held, base)
    released, gen = A._selftest_released(), A._selftest_gen()
    try:
        A._exit_hook()
        assert not A.exit_hook_registered()
        assert not any(n.startswith("_selftest") for n in A.installed())
        assert A._selftest_released() == released + 1  # one call for the shared function
        assert A._selftest_gen() > gen  # cached validations revalidate once more
        now = (sys.getrefcount(m.DEP), sys.getrefcount(m.__dict__), sys.getrefcount(m.K), sys.getrefcount(m.ident))
        assert now == base, (now, base)
        # Restored, and a kernel reached on a stale path falls back (its flag is false).
        assert A._selftest_vectorcall(m.ident) == "original" and m.ident(1) == ("orig", 1)
        assert not A._selftest_deps_ok()
        assert A._selftest_call_kernel(m.ident, 1) == ("orig", 1)
        # A later enable() works again and registers the hook again.
        rt.enable(only=["_selftest.vectorcall"])
        assert A._selftest_vectorcall(m.ident) == "kernel" and A._selftest_deps_ok() and A.exit_hook_registered()
    finally:
        if before:
            A.enable(before)


def test_non_str_and_str_subclass_keys_reach_the_pins(selftest_module):
    m = importlib.import_module(MOD)
    rt.enable(only=["_selftest.vectorcall"])
    dep = m.DEP

    class S(str):
        pass

    m.__dict__[S("DEP")] = object()  # replaces DEP's value through an equal str subclass key
    assert not A._selftest_deps_ok()
    m.DEP = dep
    assert A._selftest_deps_ok()
    m.__dict__[1] = 2  # a non-str key could alias any pinned key: every pin breaks
    assert not A._selftest_deps_ok()
    rt.enable(only=["_selftest.vectorcall"])  # stays broken while installed
    assert not A._selftest_deps_ok()
    del m.__dict__[1]
    A.disable(["_selftest"])
    rt.enable(only=["_selftest.vectorcall"])
    assert A._selftest_deps_ok()
