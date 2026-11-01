"""Group `posixpath` (docs/targets/posixpath.md): differential tests against an unpatched
clone of the module, identity rules, fallbacks, mechanics, free-threaded stress.

The reference is a clone of `posixpath` whose functions are fresh function objects with
the same code and their own globals: CPython's code, untouched by the kernels.
"""

from __future__ import annotations

import inspect
import os
import pickle
import posixpath
import sys
import threading
import types
from unittest import mock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel
NAMES = ["join", "split", "splitext", "basename", "dirname", "isabs", "relpath", "abspath"]
TARGETS = [f"posixpath.{n}" for n in NAMES]


def clone(mod: types.ModuleType) -> dict:
    ns = dict(mod.__dict__)
    for k, v in list(ns.items()):
        if isinstance(v, types.FunctionType):
            ns[k] = types.FunctionType(v.__code__, ns, v.__name__, v.__defaults__, v.__closure__)
            ns[k].__kwdefaults__ = v.__kwdefaults__
    return ns


REF = clone(posixpath)
ALLB = bytes(range(256))


def _consts() -> list:
    out = []
    import genericpath

    for mod in (posixpath, genericpath):
        for v in vars(mod).values():
            if isinstance(v, types.FunctionType):
                out += [c for c in v.__code__.co_consts if isinstance(c, (str, bytes))]
    return out


CONSTS = _consts()


def provenance(x, args):
    """Where a result object comes from: an argument, a code constant, a shared
    singleton, or a fresh object."""
    for i, a in enumerate(args):
        if x is a:
            return ("arg", i)
    for c in CONSTS:
        if x is c:
            return ("const", c)
    if isinstance(x, str) and len(x) <= 1:
        if x == "" or (ord(x) < 256 and x is chr(ord(x))):
            return ("single",)
    if isinstance(x, bytes) and len(x) <= 1:
        if x == b"" and x is b"":
            return ("single",)
        if len(x) == 1 and x is ALLB[x[0] : x[0] + 1]:
            return ("single",)
    return ("new",)


def outcome(f, args, kwargs=None):
    try:
        r = f(*args, **(kwargs or {}))
    except Exception as e:  # noqa: BLE001 - compared below
        return ("raise", type(e), str(e))
    sig = tuple(provenance(x, args) for x in r) if isinstance(r, tuple) else provenance(r, args)
    types_ = tuple(type(x) for x in r) if isinstance(r, tuple) else type(r)
    return ("ok", r, types_, sig)


def check(name, *args, **kwargs):
    want = outcome(REF[name], args, kwargs)
    got = outcome(getattr(posixpath, name), args, kwargs)
    assert got == want, (name, args, kwargs)


@pytest.fixture(scope="module", autouse=True)
def enabled():
    rt.enable(only="posixpath")
    bad = [t for t in rt.status()["targets"] if t["group"] == "posixpath" and t["state"] != "installed"]
    assert not bad, bad
    rt.stats(True)
    yield
    A.disable(["posixpath"])


def hits(name):
    return rt.stats()[name]["hits"]


# ---------------------------------------------------------------------------------------
# Inputs
# ---------------------------------------------------------------------------------------

PIECES = ["/", "/", "/", ".", ".", "..", "a", "bc", "x.py", "\xe9", "你", "\U0001f600", "\udc80", "\x00", " ", "~"]
BPIECES = [b"/", b"/", b"/", b".", b"..", b"a", b"bc", b"x.py", b"\x80", b"\xff", b"\x00", b"~"]
paths_str = st.lists(st.sampled_from(PIECES), max_size=10).map("".join)
paths_bytes = st.lists(st.sampled_from(BPIECES), max_size=10).map(b"".join)


class S(str):
    pass


class SB(bytes):
    pass


class PL:
    def __init__(self, p):
        self.p = p

    def __fspath__(self):
        return self.p


def wrap(kind, p):
    if kind == "s":
        return S(p) if isinstance(p, str) else SB(p)
    if kind == "pl":
        return PL(p)
    if kind == "int":
        return 1
    return p


paths_any = st.one_of(paths_str, paths_bytes)
args_strategy = st.lists(st.tuples(st.sampled_from(["", "", "", "s", "pl", "int"]), paths_any), min_size=1, max_size=5)


# ---------------------------------------------------------------------------------------
# Differential
# ---------------------------------------------------------------------------------------


@settings(max_examples=400)
@given(st.lists(paths_str, min_size=1, max_size=6))
def test_join_str(parts):
    check("join", *parts)


@settings(max_examples=300)
@given(st.lists(paths_bytes, min_size=1, max_size=6))
def test_join_bytes(parts):
    check("join", *parts)


@settings(max_examples=400)
@given(args_strategy)
def test_join_mixed(items):
    check("join", *[wrap(k, p) for k, p in items])


@pytest.mark.parametrize("name", ["split", "splitext", "basename", "dirname", "isabs", "abspath"])
@settings(max_examples=300)
@given(p=paths_any, kind=st.sampled_from(["", "", "s", "pl"]))
def test_one_arg(name, p, kind):
    check(name, wrap(kind, p))


@settings(max_examples=300)
@given(p=paths_any, s=st.one_of(st.none(), paths_any), kind=st.sampled_from(["", "", "s", "kw"]))
def test_relpath(p, s, kind):
    if kind == "kw":
        check("relpath", p, start=s)
    elif kind == "s":
        check("relpath", wrap("s", p), wrap("s", s) if s is not None else s)
    else:
        check("relpath", p, s)


def test_edge_cases():
    for args in [("",), ("", ""), ("a/", ""), ("/", ""), ("a", ""), ("", "b"), ("a", "/b", ""), ("//", "a")]:
        check("join", *args)
        check("join", *[a.encode() for a in args])
        check("join", *[S(a) for a in args])
    for p in ["", "/", "//", "///", "a", "a/", "/a", "a//b", ".", "..", ".a", "a.", "a.b.c", "/x/.bashrc", "....", "a/.b"]:
        for f in ["split", "splitext", "basename", "dirname", "isabs", "abspath"]:
            check(f, p)
            check(f, p.encode())
            check(f, S(p))
    for a, b in [("/a/b", "/a"), ("/a", "/a/b/c"), ("a", "b"), ("/", "/"), ("x", None), (b"/a/b", b"/c")]:
        check("relpath", a, b)
    check("relpath", "")
    check("relpath", "a", b"b")
    check("join", "a", b"b")
    check("join", b"a", "b")
    check("join", 1)


def test_kind_widening():
    # Result kind follows the widest piece (PyUnicode_Concat semantics).
    for parts in [("/a", "\xe9"), ("/你", "b"), ("\U0001f600", "a", "b"), ("ascii", "/\xff")]:
        check("join", *parts)


# ---------------------------------------------------------------------------------------
# Subclasses: plain ones run natively, overriding ones see CPython's calls
# ---------------------------------------------------------------------------------------


def test_plain_subclass_hits():
    rt.stats(reset=True)
    posixpath.join(S("/a"), S("b"))
    posixpath.split(S("/a/b"))
    assert hits("posixpath.join") == 1 and hits("posixpath.split") == 1


@pytest.mark.parametrize("meth", ["startswith", "endswith", "rfind", "__add__", "__radd__", "__len__", "__bool__", "__getitem__"])
def test_overriding_subclass_falls_back(meth):
    log = []

    def hook(self, *a):
        log.append(meth)
        return getattr(str, meth)(self, *a) if meth != "__bool__" else len(self) > 0

    T = type("T", (str,), {meth: hook})
    for name, args in [("join", (T("/a"), T("b"))), ("split", (T("/a/b"),)), ("splitext", (T("a.b"),)), ("dirname", (T("/a/b"),))]:
        log.clear()
        want = outcome(REF[name], args)
        ref_log = list(log)
        log.clear()
        got = outcome(getattr(posixpath, name), args)
        assert got == want and log == ref_log, (meth, name)


def test_instance_attribute_shadowing_falls_back():
    s = S("/a/b")
    s.rfind = lambda *a: 0
    check("split", s)
    check("basename", s)


def test_class_attribute_falls_back():
    class C(str):
        __class__ = bytes  # isinstance(x, bytes) consults __class__

    check("split", C("/a/b"))
    check("join", C("/a"), "b")


# ---------------------------------------------------------------------------------------
# Mechanics
# ---------------------------------------------------------------------------------------


def test_captured_references_and_introspection():
    from os.path import join as captured  # bound at test import time, before enable()

    assert captured is posixpath.join is os.path.join
    rt.stats(reset=True)
    assert captured("/a", "b") == "/a/b"
    assert hits("posixpath.join") == 1
    assert pickle.loads(pickle.dumps(posixpath.join)) is posixpath.join
    assert str(inspect.signature(posixpath.join)) == str(inspect.signature(REF["join"]))
    assert posixpath.join.__code__ is REF["join"].__code__
    assert inspect.getsource(posixpath.join) == inspect.getsource(REF["join"])


def test_tools_see_the_original():
    calls = []

    def prof(frame, event, arg):
        if event == "call" and frame.f_code is posixpath.split.__code__:
            calls.append(event)

    rt.stats(reset=True)
    sys.setprofile(prof)
    try:
        posixpath.split("/a/b")
    finally:
        sys.setprofile(None)
    assert calls == ["call"]
    assert rt.stats()["posixpath.split"]["fallback"]["tools"] == 1


def settle():
    for _ in range(2000):
        pass


def test_patched_dependency_falls_back_and_rearms():
    seen = []
    real = os.fspath

    def spy(x):
        seen.append(x)
        return real(x)

    with mock.patch("os.fspath", spy):
        settle()
        assert posixpath.basename("/a/b") == "b"
        assert seen == ["/a/b"]
    settle()
    rt.stats(reset=True)
    assert posixpath.basename("/a/b") == "b"
    assert hits("posixpath.basename") == 1


def test_get_sep_patched():
    with mock.patch.object(posixpath, "_get_sep", lambda p: "/"):
        settle()
        assert posixpath.join("a", "b") == "a/b"
        assert posixpath.split("a/b") == ("a", "b")
    settle()


def test_builtin_shadowed_in_module_globals():
    posixpath.isinstance = lambda o, t: False  # posixpath code now resolves this one
    try:
        settle()
        check("split", b"/a/b")
    finally:
        del posixpath.isinstance
    settle()


def test_disable_restores():
    A.disable(["posixpath"])
    try:
        rt.stats(reset=True)
        posixpath.join("a", "b")
        assert hits("posixpath.join") == 0
    finally:
        rt.enable(only="posixpath")
    rt.stats(reset=True)
    posixpath.join("a", "b")
    assert hits("posixpath.join") == 1


def test_getcwd_failure_falls_back(tmp_path):
    d = tmp_path / "gone"
    d.mkdir()
    old = os.getcwd()
    os.chdir(d)
    try:
        d.rmdir()
        with pytest.raises(FileNotFoundError):
            posixpath.abspath("x")
        with pytest.raises(FileNotFoundError):
            posixpath.relpath("x")
    finally:
        os.chdir(old)


def test_normpath_port():
    for p in ["", "/", "//", "///a//b/./c/..", "a/../..", "/../a", "./..", "\x00/a/../\x00", "\U0001f600/.."]:
        assert A._paths_normpath(p) == posixpath.normpath(p)
        assert A._paths_normpath(p.encode("utf-8", "surrogateescape")) == posixpath.normpath(
            p.encode("utf-8", "surrogateescape")
        )


@settings(max_examples=500)
@given(paths_any)
def test_normpath_port_fuzz(p):
    assert A._paths_normpath(p) == posixpath.normpath(p)


# ---------------------------------------------------------------------------------------
# Free-threaded stress: concurrent calls while the targets are disabled and re-enabled
# ---------------------------------------------------------------------------------------


def test_threads_with_enable_disable():
    cases = [("join", ("/a", "b", "\xe9")), ("split", ("/x/y//z",)), ("splitext", ("a/b.c",)), ("relpath", ("/a/b", "/a/c")),
             ("dirname", (b"/a/b",)), ("abspath", ("/a/./b",))]
    expected = [REF[n](*a) for n, a in cases]
    errors = []
    stop = threading.Event()

    def worker():
        while not stop.is_set():
            for (n, a), want in zip(cases, expected):
                got = getattr(posixpath, n)(*a)
                if got != want:
                    errors.append((n, a, got))

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for t in threads:
        t.start()
    try:
        for _ in range(30):
            A.disable(["posixpath"])
            rt.enable(only="posixpath")
    finally:
        stop.set()
        for t in threads:
            t.join()
    assert not errors
    assert all(t["state"] == "installed" for t in rt.status()["targets"] if t["group"] == "posixpath")
