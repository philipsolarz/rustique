"""The tools guard (docs/ACCELERATION.md §4.3): while any tool listens (sys.settrace,
sys.setprofile, a sys.monitoring tool with global events, local events on the original's code
object) a Python-function kernel runs the original, so the tool sees CPython's frames; when
the tool detaches the kernel takes over again (never stuck in fallback). Attach and detach
in sequence, through the hidden `_selftest.vectorcall` target, whose kernel is visible."""

from __future__ import annotations

import importlib
import sys

import pytest

from rustique import _rustique
from test_accel_registry import MOD, SOURCE

A = _rustique._accel
MON = sys.monitoring


@pytest.fixture
def m(tmp_path, monkeypatch):
    (tmp_path / f"{MOD}.py").write_text(SOURCE)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop(MOD, None)
    mod = importlib.import_module(MOD)
    A.enable(["_selftest.vectorcall"])
    A.stats(True, True)
    yield mod
    A.stats(False, True)
    A.disable(["_selftest"])
    sys.modules.pop(MOD, None)


def free_tool() -> int:
    return next(i for i in range(5, -1, -1) if MON.get_tool(i) is None)


def check(m, seen: list, attach, detach) -> None:
    assert m.ident(1) == ("kernel", 1)
    seen.clear()
    attach()
    try:
        assert m.ident(2) == ("orig", 2)
        assert seen, "the tool saw no frame of the original"
    finally:
        detach()
    seen.clear()
    assert m.ident(3) == ("kernel", 3)  # the kernel resumes at once
    assert not seen


def test_attach_detach_in_sequence_resumes_the_kernel(m):
    seen: list = []

    def tracer(frame, event, arg):
        if frame.f_code is m.ident.__code__:
            seen.append(event)
        return None

    def profiler(frame, event, arg):
        if frame.f_code is m.ident.__code__:
            seen.append(event)

    tid = free_tool()
    E = MON.events

    def mon_attach(events):
        def f():
            MON.use_tool_id(tid, "rustique-test")
            MON.register_callback(tid, E.PY_START, lambda code, off: seen.append("start") if code is m.ident.__code__ else None)
            MON.set_events(tid, events)
        return f

    def mon_detach():
        MON.set_events(tid, 0)
        MON.register_callback(tid, E.PY_START, None)
        MON.free_tool_id(tid)

    def local_attach():
        MON.use_tool_id(tid, "rustique-test")
        MON.register_callback(tid, E.PY_START, lambda code, off: seen.append("local"))
        MON.set_local_events(tid, m.ident.__code__, E.PY_START)

    def local_detach():
        MON.set_local_events(tid, m.ident.__code__, 0)
        MON.register_callback(tid, E.PY_START, None)
        MON.free_tool_id(tid)

    steps = [
        (lambda: sys.settrace(tracer), lambda: sys.settrace(None)),
        (lambda: sys.setprofile(profiler), lambda: sys.setprofile(None)),
        (mon_attach(E.PY_START), mon_detach),
        (local_attach, local_detach),
        (lambda: sys.settrace(tracer), lambda: sys.settrace(None)),
    ]
    for _ in range(3):
        for attach, detach in steps:
            check(m, seen, attach, detach)
    st = A.stats()["_selftest.vectorcall"]
    assert st["fallback"]["tools"] == 15 and st["hits"] == 30, st


def test_a_tool_on_other_events_also_counts(m):
    """Any ungrouped event (here LINE only) makes the kernel step aside."""
    seen: list = []
    tid = free_tool()
    E = MON.events

    def attach():
        MON.use_tool_id(tid, "rustique-test")
        MON.register_callback(tid, E.LINE, lambda code, line: seen.append(line) if code is m.ident.__code__ else None)
        MON.set_events(tid, E.LINE)

    def detach():
        MON.set_events(tid, 0)
        MON.register_callback(tid, E.LINE, None)
        MON.free_tool_id(tid)

    check(m, seen, attach, detach)
