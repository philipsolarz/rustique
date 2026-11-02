"""Mechanics of the interp tier (ACCELERATION §7.3 for the opcode-handler site kind):
verification, install all-or-nothing, disable() restores every entry, refusal reasons,
idempotence, tools (sys.monitoring / settrace / setprofile) see CPython's own handlers,
subinterpreters, the JIT, and net-benefit suspension."""

from __future__ import annotations

import subprocess
import sys
import textwrap

import pytest

import rustique
from _interp import FREE_THREADED, INFO, VERIFIED, enable, expected_installed, requires_tier, states, swapped, tier


def test_info_reports_a_reason_when_refused() -> None:
    if VERIFIED:
        assert INFO["release"] == "%d.%d.%d" % sys.version_info[:3]
        assert len(INFO["build_id"]) == 40
    else:
        assert INFO["reason"]
        assert INFO["category"] in {"version", "platform", "build", "conflict", "selftest", "other"}


def test_refused_targets_say_why() -> None:
    enable()
    try:
        st = states()
        if VERIFIED:
            assert {n for n, t in st.items() if t["state"] == "installed"} == expected_installed()
            if FREE_THREADED:
                assert st["interp.kwinit"]["state"] == "refused"
        else:
            assert all(t["state"] == "refused" and t["reason"] for t in st.values())
    finally:
        rustique.disable()


@requires_tier
def test_install_and_disable_restore_every_entry() -> None:
    assert all(not v for v in swapped().values())
    with tier():
        sw = swapped()
        assert "LOAD_ATTR" in sw["interp.attr"] and len(sw["interp.attr"]) == 14
        assert sw["interp.isinstance"] == ["CALL_ISINSTANCE"]
        assert sw["interp.setattr"] == ["CALL_NON_PY_GENERAL"]
        assert sw["interp.kwinit"] == ([] if FREE_THREADED else ["CALL_KW_NON_PY"])
    assert all(not v for v in swapped().values())


@requires_tier
def test_enable_is_idempotent_and_excludes_by_target() -> None:
    enable()
    enable()
    rustique.disable()
    enable(["interp.isinstance"])
    try:
        sw = swapped()
        assert sw["interp.isinstance"] == ["CALL_ISINSTANCE"]
        assert sw["interp.attr"] == []
    finally:
        rustique.disable()


@requires_tier
def test_not_enabled_by_default() -> None:
    rustique.enable()
    try:
        assert all(not v for v in swapped().values())
    finally:
        rustique.disable()


def _program() -> list[object]:
    class A:
        def __init__(self, x: int) -> None:
            self.x = x

        def m(self) -> int:
            return self.x

    class B(A):
        pass

    out: list[object] = []
    for o in [A(1), B(2), A(3)] * 30:
        out.append((o.x, o.m(), isinstance(o, B), isinstance(o, (int, A))))
    out.append(A(x=5).x)
    return out


@requires_tier
@pytest.mark.parametrize("tool", ["settrace", "setprofile", "monitoring"])
def test_tools_see_cpython_handlers(tool: str) -> None:
    """Instrumented code runs INSTRUMENTED_* opcodes, which are never swapped: the events a
    tool sees with the tier on are those of plain CPython."""

    def record() -> tuple[list[object], list[tuple[str, str]]]:
        events: list[tuple[str, str]] = []
        if tool == "settrace":
            def tracer(frame, event, arg):  # type: ignore[no-untyped-def]
                events.append((event, frame.f_code.co_name))
                return tracer
            sys.settrace(tracer)
            try:
                out = _program()
            finally:
                sys.settrace(None)
        elif tool == "setprofile":
            def prof(frame, event, arg):  # type: ignore[no-untyped-def]
                events.append((event, getattr(arg, "__name__", frame.f_code.co_name)))
            sys.setprofile(prof)
            try:
                out = _program()
            finally:
                sys.setprofile(None)
        else:
            mon = sys.monitoring
            tid = mon.PROFILER_ID
            mon.use_tool_id(tid, "interp-test")
            ev = mon.events
            mon.register_callback(tid, ev.PY_START, lambda code, off: events.append(("start", code.co_name)))
            mon.register_callback(tid, ev.CALL, lambda code, off, c, a: events.append(("call", getattr(c, "__name__", "?"))))
            mon.set_events(tid, ev.PY_START | ev.CALL)
            try:
                out = _program()
            finally:
                mon.set_events(tid, 0)
                mon.free_tool_id(tid)
        return out, events

    plain = record()
    with tier():
        accelerated = record()
    assert accelerated == plain


@requires_tier
def test_subinterpreters_get_cpython_behaviour() -> None:
    """The dispatch table is process-global, so swapped handlers run in subinterpreters too;
    every fast path checks the interpreter and falls back to CPython's code there."""
    interpreters = pytest.importorskip("concurrent.interpreters")
    code = textwrap.dedent("""
        class A:
            def __init__(self, x): self.x = x
        class B(A): pass
        class Meta(type): pass
        class M(metaclass=Meta): pass
        out = []
        for o in [A(1), B(2), M()] * 50:
            out.append((getattr(o, 'x', None), isinstance(o, A), isinstance(o, M), type(A(x=3)).__name__))
        assert out[-1] == (None, False, True, 'A'), out[-1]
    """)
    with tier():
        interp = interpreters.create()
        try:
            interp.exec(code)
        finally:
            interp.close()
        assert _program() == _program()


@requires_tier
def test_jit_process() -> None:
    """Under the JIT, tier 1 dispatches through the table, so the overrides run there; traces
    use their own micro-ops. The tier installs and computes the same results."""
    script = textwrap.dedent("""
        import sys
        sys.path.insert(0, %r)
        from _interp import enable, swapped
        import test_interp_mechanics as t
        plain = t._program()
        enable()
        assert swapped()["interp.attr"], swapped()
        for _ in range(50):
            assert t._program() == plain
        print("ok")
    """) % (__import__("os").path.dirname(__file__),)
    r = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True,
                       env={**__import__("os").environ, "PYTHON_JIT": "1"}, timeout=120)
    assert r.returncode == 0 and r.stdout.strip().endswith("ok"), r.stderr[-3000:]


@requires_tier
def test_attr_group_suspends_when_it_does_not_pay() -> None:
    """A generic LOAD_ATTR stream the cache cannot serve (a Python data descriptor at a
    polymorphic site) makes the attribute group restore CPython's handlers after ~1M
    executions; results are unchanged throughout."""
    script = textwrap.dedent("""
        import sys
        sys.path.insert(0, %r)
        from _interp import enable, info, swapped
        class D:
            def __get__(self, o, t=None): return 1
            def __set__(self, o, v): pass
        classes = [type(f"K{i}", (), {"d": D()}) for i in range(4)]
        objs = [k() for k in classes]
        enable()
        s = 0
        for _ in range(400_000):
            for o in objs:
                s += o.d
        g = info()["groups"]["interp.attr"]
        print(g["suspended"], swapped()["interp.attr"], s)
    """) % (__import__("os").path.dirname(__file__),)
    r = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stderr[-3000:]
    assert r.stdout.split()[0] == "True" and "[]" in r.stdout and r.stdout.split()[-1] == "1600000", r.stdout
