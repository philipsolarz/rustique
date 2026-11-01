"""Mechanism tests for groups `copy` and `dataclasses` (docs/ACCELERATION.md §7.3):
captured references are accelerated, introspection is unchanged, `disable()` restores,
tools see CPython's frames, patched dependencies are honoured and re-arm, free-threaded
stress gives no wrong result.
"""

from __future__ import annotations

import copy
import dataclasses
import functools
import inspect
import pickle
import sys
import threading
import time
from copy import deepcopy as captured_deepcopy  # captured before enable()
from dataclasses import asdict as captured_asdict
from unittest import mock

import pytest

import _copy_support as S
import rustique as rt
from _mechanics import assert_tools_see_original

partial_deepcopy = functools.partial(copy.deepcopy)
SIG = str(inspect.signature(copy.deepcopy))
SRC = inspect.getsource(copy.deepcopy)
CODE = copy.deepcopy.__code__
REPR = repr(copy.copy)


@dataclasses.dataclass
class P:
    x: int
    y: list


class Plain:
    def __init__(self):
        self.a = [1, 2]
        self.b = {"k": (1, 2)}


@pytest.fixture(autouse=True)
def enabled():
    S.enable()
    S.A.stats(True, True)
    yield
    S.A.stats(False, True)
    S.disable()


def hits(name: str) -> int:
    return S.A.stats()[name]["hits"]


def fallbacks(name: str) -> dict:
    return {k: v for k, v in S.A.stats()[name]["fallback"].items() if v}


def test_installed():
    states = {t["name"]: t for t in rt.status()["targets"]}
    for n in S.ALL_TARGETS:
        assert states[n]["state"] == "installed", states[n]


def test_captured_references_accelerated():
    data = {"a": [1, 2], "b": (3, {"c": 4})}
    for f in (captured_deepcopy, partial_deepcopy, copy.deepcopy, copy.__dict__["deepcopy"]):
        before = hits("copy.deepcopy")
        assert f(data) == data
        assert hits("copy.deepcopy") == before + 1
    before = hits("dataclasses.asdict")
    assert captured_asdict(P(1, [2])) == {"x": 1, "y": [2]}
    assert hits("dataclasses.asdict") == before + 1


def test_introspection_unchanged():
    assert str(inspect.signature(copy.deepcopy)) == SIG
    assert inspect.getsource(copy.deepcopy) == SRC
    assert copy.deepcopy.__code__ is CODE
    assert repr(copy.copy) == REPR
    assert pickle.loads(pickle.dumps(copy.deepcopy)) is copy.deepcopy
    assert pickle.loads(pickle.dumps(dataclasses.asdict)) is dataclasses.asdict
    assert inspect.isfunction(copy.deepcopy)
    assert captured_deepcopy is copy.deepcopy


def test_disable_restores():
    S.disable()
    before = hits("copy.deepcopy")
    assert copy.deepcopy([1, [2]]) == [1, [2]]
    assert hits("copy.deepcopy") == before
    S.enable()
    copy.deepcopy([1])
    assert hits("copy.deepcopy") == before + 1


def _profile_events(cp, f):
    events = []

    def prof(frame, event, arg):
        if event == "call":
            events.append(frame.f_code.co_name)
        elif event == "c_call":
            events.append("c:" + getattr(arg, "__name__", "?"))

    sys.setprofile(prof)
    try:
        f(cp)
    finally:
        sys.setprofile(None)
    return events


@pytest.mark.parametrize(
    "f",
    [
        lambda cp: cp.deepcopy({"a": [1, (2, 3)], "p": Plain()}),
        lambda cp: cp.copy(Plain()),
    ],
)
def test_tools_see_cpython_frames(f):
    f(S.REF)  # warm up: class-level caches (`__slotnames__`) filled once
    ref = _profile_events(S.REF, f)
    got = _profile_events(copy, f)
    assert got == ref
    assert fallbacks("copy.deepcopy").get("tools", 0) + fallbacks("copy.copy").get("tools", 0) > 0


def test_local_breakpoint_on_helper():
    """A debugger's local LINE events on `_deepcopy_dict` fire as in CPython."""
    mon = sys.monitoring
    tool = 4
    mon.use_tool_id(tool, "rustique-test")
    lines = []
    try:
        mon.register_callback(tool, mon.events.LINE, lambda code, line: lines.append(line))
        mon.set_local_events(tool, copy._deepcopy_dict.__code__, mon.events.LINE)
        copy.deepcopy({"a": {"b": 1}})
        got = list(lines)
        lines.clear()
        S.disable()
        copy.deepcopy({"a": {"b": 1}})
        assert got == lines and got
    finally:
        mon.set_local_events(tool, copy._deepcopy_dict.__code__, 0)
        mon.register_callback(tool, mon.events.LINE, None)
        mon.free_tool_id(tool)


@pytest.mark.parametrize(
    "target,attr",
    [
        (copy, "_reconstruct"),
        (copy, "_keep_alive"),
        (copy, "deepcopy"),
        (copy, "_deepcopy_dispatch"),
        (copy, "dispatch_table"),
        (copy, "_atomic_types"),
    ],
)
def test_patched_dependency_is_honoured(target, attr):
    orig = getattr(target, attr)
    calls = []
    if callable(orig) and not isinstance(orig, (dict, frozenset, set)):

        def wrapper(*a, **k):
            calls.append(1)
            return orig(*a, **k)

        new = wrapper
    else:
        new = type(orig)(orig)
    with mock.patch.object(target, attr, new):
        y = copy.deepcopy([Plain(), {"k": [1]}])
        assert isinstance(y[0], Plain) and y[1] == {"k": [1]}
        if callable(orig) and attr != "deepcopy":
            assert calls
    # Restored: re-armed.
    before = hits("copy.deepcopy")
    copy.deepcopy([1])
    assert hits("copy.deepcopy") == before + 1


def test_helper_code_replaced():
    orig = copy._deepcopy_list.__defaults__
    seen = []

    def spy(x, memo):
        seen.append(x)
        return copy.deepcopy(x, memo)

    copy._deepcopy_list.__defaults__ = (spy,)
    try:
        assert copy.deepcopy([[1], 2]) == [[1], 2]
        assert seen == [[1], 1, 2]
    finally:
        copy._deepcopy_list.__defaults__ = orig


def test_newobj_patched():
    calls = []
    orig = sys.modules["copyreg"].__newobj__

    def newobj(cls, *args):
        calls.append(cls)
        return orig(cls, *args)

    with mock.patch("copyreg.__newobj__", newobj):
        copy.deepcopy(Plain())
        copy.copy(Plain())
    assert calls == [Plain, Plain]


def test_dataclasses_dependency_patched():
    calls = []
    orig = dataclasses._asdict_inner

    def inner(obj, f):
        calls.append(type(obj).__name__)
        return orig(obj, f)

    with mock.patch.object(dataclasses, "_asdict_inner", inner):
        assert dataclasses.asdict(P(1, [2])) == {"x": 1, "y": [2]}
    assert calls[0] == "P"
    before = hits("dataclasses.asdict")
    dataclasses.asdict(P(1, [2]))
    assert hits("dataclasses.asdict") == before + 1


@pytest.mark.skipif(sys.version_info >= (3, 15), reason="3.14: copy's type sets are mutable sets")
def test_mutable_atomic_set_314():
    class Atom:
        pass

    a = Atom()
    copy._atomic_types.add(Atom)
    try:
        assert copy.deepcopy([a])[0] is a
    finally:
        copy._atomic_types.discard(Atom)
    assert copy.deepcopy([a])[0] is not a
    copy._copy_atomic_types.add(Atom)
    try:
        assert copy.copy(a) is a
    finally:
        copy._copy_atomic_types.discard(Atom)


def test_free_threaded_stress():
    """Concurrent deep copies of shared graphs (and concurrent mutation of other
    graphs): no crash, and every unmutated copy equals its source."""
    shared = {"a": [1, 2, {"b": (3, [4])}], "p": P(1, [Plain()])}
    mutated: list = [1, 2, 3]
    errors = []
    stop = threading.Event()

    def copier():
        try:
            for _ in range(100):
                y = copy.deepcopy(shared)
                assert y["a"] == shared["a"] and y["p"].y[0].a == [1, 2]
                try:
                    copy.deepcopy([mutated, {"m": mutated}])
                except RuntimeError:
                    pass
                dataclasses.asdict(shared["p"])
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    def mutator():
        # Bounded, and yielding: on GIL builds a spinning mutator starves the copiers.
        for i in range(20_000):
            if stop.is_set():
                break
            mutated.append(1)
            if len(mutated) > 50:
                del mutated[3:]
            if i % 64 == 0:
                time.sleep(0)

    threads = [threading.Thread(target=copier) for _ in range(4)]
    m = threading.Thread(target=mutator)
    m.start()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    stop.set()
    m.join()
    assert not errors, errors


def test_enable_disable_under_load():
    errors = []
    stop = threading.Event()
    data = {"a": [1, {"b": 2}], "c": (3, 4)}

    def worker():
        try:
            for _ in range(5_000):
                if stop.is_set():
                    break
                assert copy.deepcopy(data) == data
                assert copy.copy(data) == data
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=worker) for _ in range(3)]
    for t in threads:
        t.start()
    for _ in range(20):
        S.disable()
        S.enable()
    stop.set()
    for t in threads:
        t.join()
    assert not errors, errors


@pytest.mark.parametrize(
    "name,func,call",
    [
        ("copy.deepcopy", copy.deepcopy, lambda: copy.deepcopy({"a": [1, (2, 3)], "b": P(1, [2])})),
        ("copy.copy", copy.copy, lambda: copy.copy([1, 2, {"a": 1}])),
        ("dataclasses.asdict", dataclasses.asdict, lambda: dataclasses.asdict(P(1, [2, {"k": 3}]))),
        ("dataclasses.astuple", dataclasses.astuple, lambda: dataclasses.astuple(P(1, (2, 3)))),
    ],
)
def test_tools_guard_shared(name, func, call):
    S.disable()
    assert_tools_see_original(name, func, call)


def test_dataclasses_without_copy_imported():
    """Fresh process: the dataclasses group installs before `copy` is imported (3.15's
    `dataclasses` imports it lazily), and asdict's leaf `copy.deepcopy` resolves the lazy
    import exactly as LOAD_GLOBAL does."""
    import subprocess
    import textwrap

    code = textwrap.dedent(
        """
        import sys, rustique
        rustique.enable(only=["copy", "dataclasses"])
        import dataclasses, datetime
        st = {t["name"]: t["state"] for t in rustique.status()["targets"]}
        assert st["dataclasses.asdict"] == "installed", st
        before = "copy" in sys.modules
        @dataclasses.dataclass
        class P:
            when: datetime.datetime
            tags: list
        d = dataclasses.asdict(P(datetime.datetime(2024, 1, 2), [frozenset({"a"})]))
        assert d == {"when": datetime.datetime(2024, 1, 2), "tags": [frozenset({"a"})]}, d
        assert "copy" in sys.modules
        assert type(dataclasses.__dict__["copy"]).__name__ == "module"
        print(before, st["copy.deepcopy"])
        """
    )
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


class _Hook:
    def __init__(self):
        self.seen = None

    def __deepcopy__(self, memo):
        # What the hook sees is CPython's memo: the entries `copy.py` made before reaching
        # it, nothing left over from the kernel's abandoned attempt.
        self.seen = sorted(type(v).__name__ for k, v in memo.items() if k != id(memo))
        return self


def test_restart_counts_deopt_and_leaves_no_trace():
    h = _Hook()
    root = [{"a": [1, 2]}, Plain(), h]
    memo = {}
    out = copy.deepcopy(root, memo)
    assert out[2] is h and out[0] == root[0] and out[0] is not root[0]
    assert fallbacks("copy.deepcopy") == {"deopt": 1}
    assert hits("copy.deepcopy") == 0
    assert h.seen == ["Plain", "dict", "list"]
    # The memo the caller gets back is CPython's (same keys, keep-alive list, order).
    ref = {}
    S.REF.deepcopy([{"a": [1, 2]}, Plain(), _Hook()], ref)
    assert [type(v).__name__ for v in memo.values()] == [type(v).__name__ for v in ref.values()]
    # A pure call afterwards is native again.
    copy.deepcopy([1, {"a": 2}])
    assert hits("copy.deepcopy") == 1


def test_restart_streak_suspends_and_hit_resets():
    h = _Hook()
    for _ in range(10):
        copy.deepcopy([h])
        copy.deepcopy(7)
    st = S.A.stats()["copy.deepcopy"]
    assert st["fallback"]["deopt"] == 10 and st["hits"] == 10 and st["suspensions"] == 0
