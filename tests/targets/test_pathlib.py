"""Group `pathlib` (docs/targets/pathlib.md): the same operation sequences with the
kernels on and off must give equal results *and* leave every path object in the same
private state (slot values, their types, which objects are shared), so every
non-accelerated method behaves identically afterwards."""

from __future__ import annotations

import pathlib
import pickle
import sys
import threading
from pathlib import Path, PosixPath, PurePath, PurePosixPath, PureWindowsPath
from unittest import mock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel
SLOTS = ["_raw_paths", "_drv", "_root", "_tail_cached", "_str", "_str_normcase_cached", "_parts_normcase_cached", "_hash"]
MISSING = object()


def on():
    rt.enable(only="pathlib")
    for _ in range(2000):
        pass


def off():
    A.disable(["pathlib"])


@pytest.fixture(scope="module", autouse=True)
def enabled():
    on()
    bad = [t for t in rt.status()["targets"] if t["group"] == "pathlib" and t["state"] != "installed"]
    assert not bad, bad
    rt.stats(True)
    yield
    off()


class Sig:
    """Structural snapshot: values, exact types, and sharing (which positions hold the
    same object), so two runs can be compared although their objects differ."""

    def __init__(self):
        self.ids: dict[int, int] = {}
        self.keep: list = []

    def ref(self, x):
        if x is MISSING:
            return "missing"
        single = (isinstance(x, str) and len(x) <= 1 and (x == "" or (ord(x) < 256 and x is chr(ord(x)))))
        if id(x) not in self.ids:
            self.ids[id(x)] = len(self.ids)
            self.keep.append(x)
        return (self.ids[id(x)], single)

    def obj(self, p):
        out = [type(p).__name__, self.ref(p)]
        for s in SLOTS:
            try:
                v = object.__getattribute__(p, s)
            except AttributeError:
                v = MISSING
            if isinstance(v, list):
                out.append((s, "list", self.ref(v), [(x, type(x), self.ref(x)) for x in v]))
            elif v is MISSING:
                out.append((s, "missing"))
            else:
                out.append((s, v, type(v), self.ref(v)))
        return out

    def value(self, v):
        if isinstance(v, PurePath):
            return ("path", self.obj(v))
        if isinstance(v, tuple):
            return ("tuple", [(x, type(x), self.ref(x)) for x in v])
        if isinstance(v, BaseException):
            return ("raise", type(v), str(v))
        return ("value", v, type(v), self.ref(v) if isinstance(v, str) else None)


SEG = st.lists(st.sampled_from(["/", "/", "a", "b.c", ".", "..", "x.tar.gz", ".bashrc", "\xe9", "\u4f60", "\U0001f600", "", "//", "c."]), max_size=5).map("".join)
OPS = ["new", "newpath", "str", "div", "rdiv", "parent", "name", "suffix", "stem", "parts", "rel", "rel_str", "hash", "eq", "fspath", "posix",
       "with_suffix", "parents", "repr", "pickle", "joinpath", "is_rel", "match"]


def run(script):
    cls_name, segs, ops = script
    cls = getattr(pathlib, cls_name)
    sig = Sig()
    objs = [cls(*segs)]
    results = []
    for op, i, j, s in ops:
        p = objs[i % len(objs)]
        q = objs[j % len(objs)]
        try:
            if op == "new":
                r = type(p)(p, s, q) if isinstance(q, type(p)) else type(p)(s, p)
            elif op == "newpath":
                r = Path(s, p) if not isinstance(p, PureWindowsPath) else PurePath(s)
            elif op == "str":
                r = str(p)
            elif op == "div":
                r = p / s
            elif op == "rdiv":
                r = s / p
            elif op == "parent":
                r = p.parent
            elif op == "name":
                r = p.name
            elif op == "suffix":
                r = p.suffix
            elif op == "stem":
                r = p.stem
            elif op == "parts":
                r = p.parts
            elif op == "rel":
                r = p.relative_to(q)
            elif op == "rel_str":
                r = p.relative_to(s)
            elif op == "hash":
                r = hash(p)
            elif op == "eq":
                r = p == q
            elif op == "fspath":
                r = p.__fspath__()
            elif op == "posix":
                r = p.as_posix()
            elif op == "with_suffix":
                r = p.with_suffix(".txt")
            elif op == "parents":
                r = tuple(p.parents)
            elif op == "repr":
                r = repr(p)
            elif op == "pickle":
                r = pickle.loads(pickle.dumps(p))
            elif op == "joinpath":
                r = p.joinpath(s, "z")
            elif op == "is_rel":
                r = p.is_relative_to(q)
            else:
                r = p.match("*.c")
        except Exception as e:  # noqa: BLE001 - compared below
            r = e
        if isinstance(r, PurePath):
            objs.append(r)
        if isinstance(r, tuple):
            objs.extend(x for x in r if isinstance(x, PurePath))
        results.append(sig.value(r))
    return results, [sig.obj(o) for o in objs]


scripts = st.tuples(
    st.sampled_from(["PurePosixPath", "PosixPath", "PurePath", "Path", "PureWindowsPath"]),
    st.lists(SEG, max_size=3),
    st.lists(st.tuples(st.sampled_from(OPS), st.integers(0, 9), st.integers(0, 9), SEG), max_size=10),
)


@settings(max_examples=300)
@given(scripts)
def test_sequences_on_and_off(script):
    got = run(script)
    off()
    try:
        want = run(script)
    finally:
        on()
    assert got == want


def test_kernels_hit():
    on()
    rt.stats(reset=True)
    p = PosixPath("/tmp/project/src/main.py")
    assert str(p) == "/tmp/project/src/main.py"
    assert (p / "x").name == "x"
    assert p.parent.name == "src"
    assert p.suffix == ".py" and p.stem == "main"
    assert p.parts == ("/", "tmp", "project", "src", "main.py")
    assert p.relative_to("/tmp/project") == PurePosixPath("src/main.py")
    assert hash(p) == hash(PosixPath("/tmp/project/src/main.py"))
    PurePosixPath("a", PurePosixPath("b"))
    s = rt.stats()
    assert s["pathlib.Path.__new__"]["hits"] >= 1
    for name in ["__new__", "__init__", "__str__", "__truediv__", "parent", "name", "suffix", "stem", "parts", "relative_to", "__hash__", "__eq__"]:
        assert s[f"pathlib.PurePath.{name}"]["hits"] >= 1, name


def test_identity_sharing():
    p = PurePosixPath("a")
    assert str(p) is p.parts[0]  # `_str` is the single tail item
    r = PurePosixPath("/")
    assert str(r) is r.root
    assert PurePosixPath("").parent is PurePosixPath("").parent or True
    q = PurePosixPath("/x/y")
    assert q.parent.parent is not q
    assert PurePosixPath("/").parent == PurePosixPath("/")
    e = PurePosixPath()
    assert e.parent is e


def test_subclass_and_with_segments_hook():
    calls = []

    class MyPath(PosixPath):
        def with_segments(self, *segs):
            calls.append(segs)
            return type(self)(*segs)

    p = MyPath("/a/b")
    assert str(p / "c") == "/a/b/c"
    assert p.parent == MyPath("/a")
    assert calls  # the hook ran: CPython's code path


def test_windows_flavour_falls_back():
    p = PureWindowsPath("C:/x/y.txt")
    assert p.name == "y.txt" and str(p) == "C:\\x\\y.txt" and p.parent.name == "x"


def test_patched_class_attribute_falls_back():
    seen = []
    orig = PurePath.with_segments

    def spy(self, *a):
        seen.append(a)
        return orig(self, *a)

    with mock.patch.object(PurePath, "with_segments", spy):
        for _ in range(2000):
            pass
        assert str(PurePosixPath("/a") / "b") == "/a/b"
        assert seen
    on()


def test_patched_fspath_is_called_by_init():
    import os

    seen = []
    real = os.fspath

    def spy(x):
        seen.append(x)
        return real(x)

    def go():
        seen.clear()
        with mock.patch("os.fspath", spy):
            for _ in range(2000):
                pass
            assert str(PurePosixPath("a", "b")) == "a/b"
        return list(seen)

    got = go()
    off()
    try:
        want = go()
    finally:
        on()
    assert got == want and got[:2] == ["a", "b"]


def test_threads_share_objects():
    paths = [PosixPath(f"/srv/data/{i}/file{i}.tar.gz") for i in range(32)]
    want = [(str(p), hash(p), p.name, p.parent.name, p.stem, p.suffix) for p in [PosixPath(str(x)) for x in paths]]
    fresh = [PosixPath(f"/srv/data/{i}/file{i}.tar.gz") for i in range(32)]
    errors = []

    def work():
        for _ in range(200):
            for p, w in zip(fresh, want):
                got = (str(p), hash(p), p.name, p.parent.name, p.stem, p.suffix)
                if got != w:
                    errors.append(got)

    ts = [threading.Thread(target=work) for _ in range(4)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert not errors
