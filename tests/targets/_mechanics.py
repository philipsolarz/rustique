"""Shared mechanism checks for accelerated targets (docs/ACCELERATION.md §7.3).

Import from a target test (pytest puts `tests/targets/` on `sys.path`):

    from _mechanics import (
        enabled, counting, references, assert_references_accelerated,
        assert_introspection_unchanged, assert_disable_restores, assert_tools_see_original,
        assert_patch_falls_back, ft_stress, settle,
    )

    import posixpath
    from posixpath import join as captured_join          # captured before enable()
    REFS = references(posixpath.join)                    # alias, partial, closure, ...

    def test_mechanics():
        with enabled("posixpath.join"):
            assert_references_accelerated("posixpath.join", {**REFS, "from": captured_join},
                                          ("/usr", "lib"))
        assert_introspection_unchanged(posixpath.join, "posixpath.join")
        assert_disable_restores("posixpath.join", posixpath.join, lambda: posixpath.join("a", "b"))
        assert_tools_see_original("posixpath.join", posixpath.join, lambda: posixpath.join("a", "b"))
        assert_patch_falls_back("posixpath.join", mock.patch("posixpath._get_sep", ...),
                                lambda: posixpath.join("a", "b"))
        ft_stress(["posixpath.join"], lambda: posixpath.join("a", "b"), "a/b")

Every helper leaves the target disabled (it was disabled when the helper started) and
resets the counters it used. Counting is switched on inside the helpers only.
"""

from __future__ import annotations

import contextlib
import functools
import inspect
import pickle
import sys
import threading
from collections.abc import Callable, Iterator
from typing import Any

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel
FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()

__all__ = [
    "A",
    "FREE_THREADED",
    "Counts",
    "assert_disable_restores",
    "assert_introspection_unchanged",
    "assert_patch_falls_back",
    "assert_references_accelerated",
    "assert_tools_see_original",
    "counting",
    "enabled",
    "ft_stress",
    "introspection",
    "references",
    "settle",
    "status_of",
]


# --------------------------------------------------------------------------------------
# Basics
# --------------------------------------------------------------------------------------


def settle() -> None:
    """Let pending calls run (the registry suspends/resumes vectorcall targets in a pending
    call after a pin flips): they run at eval-breaker checks, e.g. on backward jumps."""
    for _ in range(2000):
        pass


def status_of(name: str) -> dict[str, Any] | None:
    for t in rt.status()["targets"]:
        if t["name"] == name:
            return t
    return None


@contextlib.contextmanager
def enabled(*names: str, require: bool = True) -> Iterator[dict[str, Any]]:
    """`enable(only=names)` for the duration; `require`: every named *target* (dotted
    names; groups are not checked) must be installed or pending its import."""
    st = rt.enable(only=list(names))
    try:
        if require:
            for n in names:
                t = status_of(n)
                if t is not None:
                    assert t["state"] in ("installed", "pending-import"), t
        yield st
    finally:
        A.disable(list(names))
        settle()


class Counts:
    """Counter deltas of one target since the start of a `counting()` block."""

    def __init__(self, name: str) -> None:
        self.name = name
        self._base = self._now()

    def _now(self) -> dict[str, Any]:
        s = A.stats().get(self.name, {"hits": 0, "fallback": {}, "suspensions": 0})
        return {"hits": s["hits"], "suspensions": s.get("suspensions", 0), **{f"fb_{k}": v for k, v in s["fallback"].items()}}

    @property
    def hits(self) -> int:
        return self._now()["hits"] - self._base["hits"]

    def fallback(self, reason: str | None = None) -> int:
        now = self._now()
        keys = [f"fb_{reason}"] if reason else [k for k in now if k.startswith("fb_")]
        return sum(now.get(k, 0) - self._base.get(k, 0) for k in keys)

    def reset(self) -> None:
        self._base = self._now()

    def __repr__(self) -> str:
        now = self._now()
        return f"Counts({self.name}: " + ", ".join(f"{k}={now[k] - self._base.get(k, 0)}" for k in now) + ")"


@contextlib.contextmanager
def counting(name: str) -> Iterator[Counts]:
    """Switch counting on and yield a :class:`Counts` for `name`."""
    was = A.stats()  # snapshot only, to keep other counters
    del was
    A.stats(True)
    try:
        yield Counts(name)
    finally:
        A.stats(False)


# --------------------------------------------------------------------------------------
# Captured references (§7.3): call them all, every one must reach the kernel
# --------------------------------------------------------------------------------------


def references(obj: Callable[..., Any]) -> dict[str, Callable[..., Any]]:
    """Ways user code holds `obj` (call this *before* `enable()`, e.g. at import time):
    alias, `functools.partial`, a closure, a default argument, a container entry. For a
    `from m import f` reference, add your own module-level import to the dict."""
    alias = obj

    def closure(*a: Any, **k: Any) -> Any:
        return obj(*a, **k)

    def default(*a: Any, _f: Callable[..., Any] = obj, **k: Any) -> Any:
        return _f(*a, **k)

    table = {"f": obj}
    return {
        "direct": obj,
        "alias": alias,
        "partial": functools.partial(obj),
        "closure": closure,
        "default-arg": default,
        "container": table["f"],
    }


def assert_references_accelerated(
    name: str, refs: dict[str, Callable[..., Any]], args: tuple[Any, ...] = (), kwargs: dict[str, Any] | None = None
) -> None:
    """Inside `enabled(name)`: each reference reaches the kernel (one hit per call)."""
    kwargs = kwargs or {}
    with counting(name) as c:
        for label, f in refs.items():
            c.reset()
            f(*args, **kwargs)
            assert c.hits == 1, f"{label}: {c}"


# --------------------------------------------------------------------------------------
# Introspection (§2.8)
# --------------------------------------------------------------------------------------


def _try(f: Callable[[], Any]) -> Any:
    try:
        return f()
    except Exception as e:  # noqa: BLE001
        return ("raises", type(e).__name__)


def introspection(obj: Any) -> dict[str, Any]:
    """What user code can observe about `obj` without calling it."""
    return {
        "id": id(obj),
        "type": type(obj),
        "repr": _try(lambda: repr(obj)),
        "name": getattr(obj, "__name__", None),
        "qualname": getattr(obj, "__qualname__", None),
        "module": getattr(obj, "__module__", None),
        "doc": getattr(obj, "__doc__", None),
        "text_signature": getattr(obj, "__text_signature__", None),
        "signature": _try(lambda: str(inspect.signature(obj))),
        "code": id(getattr(obj, "__code__", None)),
        "defaults": getattr(obj, "__defaults__", None),
        "kwdefaults": getattr(obj, "__kwdefaults__", None),
        "source": _try(lambda: inspect.getsource(obj)),
        "isfunction": inspect.isfunction(obj),
        "isbuiltin": inspect.isbuiltin(obj),
        "pickle": _try(lambda: pickle.loads(pickle.dumps(obj)) is obj),
        "dict": dict(getattr(obj, "__dict__", {}) or {}),
    }


def assert_introspection_unchanged(obj: Any, name: str) -> None:
    """Identity, repr, names, doc, signature, code, defaults, source and pickling of `obj`
    are the same with `name` enabled as without."""
    before = introspection(obj)
    with enabled(name):
        during = introspection(obj)
    after = introspection(obj)
    assert during == before, {k: (before[k], during[k]) for k in before if before[k] != during[k]}
    assert after == before


# --------------------------------------------------------------------------------------
# disable() restores (§3.9)
# --------------------------------------------------------------------------------------


def assert_disable_restores(name: str, obj: Any, call: Callable[[], Any]) -> None:
    """`call()` reaches the kernel while enabled and never after `disable()`; a Python
    function gets its original vectorcall and a specializer version back."""
    expected = call()
    with enabled(name):
        with counting(name) as c:
            assert call() == expected
            assert c.hits >= 1, c
    with counting(name) as c:
        assert call() == expected
        assert c.hits == 0 and c.fallback() == 0, c
    if inspect.isfunction(obj):
        assert A.vectorcall_is_original(obj), "vectorcall not restored"
        assert A.func_version(obj) >= 2, "func_version not restored"
    t = status_of(name)
    assert t is None or t["state"] == "disabled", t


# --------------------------------------------------------------------------------------
# Tools see the original (§4.3)
# --------------------------------------------------------------------------------------


def _tool_contexts(code: Any) -> dict[str, Callable[[list[Any]], contextlib.AbstractContextManager[Any]]]:
    @contextlib.contextmanager
    def settrace(seen: list[Any]) -> Iterator[None]:
        def tracer(frame: Any, event: str, arg: Any) -> Any:
            if event == "call" and frame.f_code is code:
                seen.append(event)

        old = sys.gettrace()
        sys.settrace(tracer)
        try:
            yield
        finally:
            sys.settrace(old)

    @contextlib.contextmanager
    def setprofile(seen: list[Any]) -> Iterator[None]:
        def prof(frame: Any, event: str, arg: Any) -> None:
            if event == "call" and frame.f_code is code:
                seen.append(event)

        old = sys.getprofile()
        sys.setprofile(prof)
        try:
            yield
        finally:
            sys.setprofile(old)

    @contextlib.contextmanager
    def cprofile(seen: list[Any]) -> Iterator[None]:
        import cProfile
        import pstats

        p = cProfile.Profile()
        p.enable()
        try:
            yield
        finally:
            p.disable()
            st = pstats.Stats(p)
            for (filename, line, fname), row in st.stats.items():  # type: ignore[attr-defined]
                if fname == code.co_name and line == code.co_firstlineno:
                    seen.extend(["call"] * row[1])

    @contextlib.contextmanager
    def monitoring_global(seen: list[Any]) -> Iterator[None]:
        mon = sys.monitoring
        tool = 5
        mon.use_tool_id(tool, "rustique-mechanics")
        try:
            mon.register_callback(tool, mon.events.PY_START, lambda c, off: seen.append("start") if c is code else None)
            mon.set_events(tool, mon.events.PY_START)
            yield
        finally:
            mon.set_events(tool, 0)
            mon.register_callback(tool, mon.events.PY_START, None)
            mon.free_tool_id(tool)

    @contextlib.contextmanager
    def monitoring_local(seen: list[Any]) -> Iterator[None]:
        mon = sys.monitoring
        tool = 5
        mon.use_tool_id(tool, "rustique-mechanics")
        try:
            mon.register_callback(tool, mon.events.LINE, lambda c, line: seen.append("line"))
            mon.set_local_events(tool, code, mon.events.LINE)
            yield
        finally:
            mon.set_local_events(tool, code, 0)
            mon.register_callback(tool, mon.events.LINE, None)
            mon.free_tool_id(tool)

    return {
        "settrace": settrace,
        "setprofile": setprofile,
        "cProfile": cprofile,
        "sys.monitoring PY_START": monitoring_global,
        "sys.monitoring local LINE": monitoring_local,
    }


def assert_tools_see_original(name: str, func: Any, call: Callable[[], Any], *, calls: int = 3) -> None:
    """Python-function targets: under each tool (settrace, setprofile, cProfile,
    sys.monitoring global PY_START and local LINE events on the original's code) the call
    runs the original (the tool sees its frame; the kernel counts a `tools` fallback), and
    the kernel is reached again once the tool stops."""
    code = func.__code__
    expected = call()
    with enabled(name):
        for label, ctx in _tool_contexts(code).items():
            seen: list[Any] = []
            with counting(name) as c:
                with ctx(seen):
                    for _ in range(calls):
                        assert call() == expected, label
                assert c.hits == 0, (label, c)
                assert c.fallback("tools") >= calls, (label, c)
            assert seen, f"{label}: the tool saw no frame of the original"
            with counting(name) as c:
                assert call() == expected
                assert c.hits == 1, (label, "kernel not reached after the tool stopped", c)


# --------------------------------------------------------------------------------------
# Dependency pins (§4.2)
# --------------------------------------------------------------------------------------


def assert_patch_falls_back(
    name: str, patcher: contextlib.AbstractContextManager[Any], call: Callable[[], Any], expected: Any = None
) -> None:
    """With a pinned dependency patched (e.g. `mock.patch("posixpath._get_sep", f)`) the
    kernel is not used (deps fallback, or the target is suspended and CPython's function
    runs); when the patch exits the kernel is used again. `expected`: the result under the
    patch (default: not checked)."""
    with enabled(name):
        with counting(name) as c:
            call()
            assert c.hits == 1, c
        with patcher:
            with counting(name) as c:
                r = call()
                settle()
                r2 = call()
                assert c.hits == 0, c
            if expected is not None:
                assert r == expected and r2 == expected
        settle()
        with counting(name) as c:
            call()
            assert c.hits == 1, ("not re-armed after the patch exited", c)


# --------------------------------------------------------------------------------------
# Free-threaded stress (§3.10)
# --------------------------------------------------------------------------------------


def ft_stress(
    names: list[str],
    fn: Callable[[], Any],
    expected: Any,
    *,
    threads: int = 4,
    toggles: int | None = None,
    check: Callable[[Any], bool] | None = None,
) -> int:
    """Call `fn` from `threads` threads while the main thread enables and disables `names`
    `toggles` times; every result must equal `expected` (or satisfy `check`) and nothing
    may raise. Returns the number of calls made. Leaves `names` disabled."""
    toggles = toggles if toggles is not None else (200 if FREE_THREADED else 50)
    stop = threading.Event()
    errors: list[str] = []
    counts = [0] * threads

    def worker(i: int) -> None:
        n = 0
        while not stop.is_set():
            try:
                r = fn()
                if not (check(r) if check else r == expected):
                    errors.append(f"wrong result {r!r}")
            except Exception as e:  # noqa: BLE001
                errors.append(repr(e))
            n += 1
        counts[i] = n

    ts = [threading.Thread(target=worker, args=(i,)) for i in range(threads)]
    for t in ts:
        t.start()
    try:
        for _ in range(toggles):
            rt.enable(only=names)
            A.disable(names)
    finally:
        stop.set()
        for t in ts:
            t.join()
    assert not errors, errors[:5]
    return sum(counts)
