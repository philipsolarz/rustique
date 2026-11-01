"""Group `fnmatch` (docs/targets/fnmatch.md): the pattern port against `re` itself, the
kernels against an unpatched clone of the module, fallbacks, mechanics, threads."""

from __future__ import annotations

import fnmatch
import os
import pickle
import re
import sys
import threading
import types
import warnings
from unittest import mock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel


def clone(mod):
    ns = dict(mod.__dict__)
    for k, v in list(ns.items()):
        if isinstance(v, types.FunctionType):
            ns[k] = types.FunctionType(v.__code__, ns, v.__name__, v.__defaults__, v.__closure__)
    return ns


REF = clone(fnmatch)


@pytest.fixture(scope="module", autouse=True)
def enabled():
    rt.enable(only="fnmatch")
    bad = [t for t in rt.status()["targets"] if t["group"] == "fnmatch" and t["state"] != "installed"]
    assert not bad, bad
    rt.stats(True)
    yield
    A.disable(["fnmatch"])


def hits(name):
    return rt.stats()[name]["hits"]


def outcome(f, *args):
    try:
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            r = f(*args)
    except Exception as e:  # noqa: BLE001 - compared below
        return ("raise", type(e), str(e))
    return ("ok", r, type(r), [str(x.message) for x in w])


def regex_match(name, pat):
    """CPython's meaning: the translated regex, compiled fresh (warnings as errors)."""
    if isinstance(pat, bytes):
        res = fnmatch.translate(str(pat, "ISO-8859-1")).encode("ISO-8859-1")
    else:
        res = fnmatch.translate(pat)
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        return re.compile(res).match(name) is not None


PAT_CHARS = list("**??[[]]]!!--^\\ab.&~|") + ["\xe9", "你", "\U0001f600", "\n", "z", "0", "9", "A"]
NAME_CHARS = list("ab.-]![\\^&~|z09A\n") + ["\xe9", "你", "\U0001f600", "\udc80", "\x00"]
patterns = st.lists(st.sampled_from(PAT_CHARS), max_size=12).map("".join)
names = st.lists(st.sampled_from(NAME_CHARS), max_size=12).map("".join)
BPAT = list(b"**??[[]]]!!--^\\ab.&~|\x80\xff\n")
bpatterns = st.lists(st.sampled_from(BPAT), max_size=12).map(bytes)
bnames = st.lists(st.sampled_from(list(b"ab.-]![\\^&~|\x80\xff\n\x00")), max_size=12).map(bytes)


@settings(max_examples=3000)
@given(names, patterns)
def test_port_equals_regex(name, pat):
    got = A._paths_fnmatch_port(name, pat)
    if got is None:
        return  # outside the reproduced grammar: the kernel falls back
    assert got == regex_match(name, pat), (name, pat)


@settings(max_examples=1500)
@given(bnames, bpatterns)
def test_port_equals_regex_bytes(name, pat):
    got = A._paths_fnmatch_port(name, pat)
    if got is None:
        return
    assert got == regex_match(name, pat), (name, pat)


def test_port_covers_realistic_patterns():
    for pat in ["*.py", "*.pyc", "test_*.py", "*~", ".DS_Store", "[!_]*", "[a-z]*.txt", "*.[ch]", "file?.log",
                "[0-9][0-9]*", "*[!.pyc]", "coverage.*", "*.egg-info", "[]x]", "[!]]", "a[-]b", "[a-]"]:
        assert A._paths_fnmatch_port("x", pat) is not None, pat


@pytest.mark.parametrize(
    "name,pat",
    [("a", "[[:alpha:]]"), ("a", "[a--b]"), ("x", "[a&&b]"), ("x", "[[]"), ("ab", "*[[]*")],
)
def test_warning_patterns_fall_back_or_agree(name, pat):
    got = A._paths_fnmatch_port(name, pat)
    if got is not None:
        assert got == regex_match(name, pat)


@settings(max_examples=500)
@given(name=st.one_of(names, bnames), pat=st.one_of(patterns, bpatterns), kind=st.sampled_from(["", "", "s", "pl", "int"]))
def test_kernels_differential(name, pat, kind):
    class S(type(name)):
        pass

    class PL:
        def __fspath__(self):
            return name

    n = {"": name, "s": S(name), "pl": PL(), "int": 3}[kind]
    for f in ["fnmatch", "fnmatchcase"]:
        assert outcome(getattr(fnmatch, f), n, pat) == outcome(REF[f], n, pat), (f, n, pat)


@settings(max_examples=300)
@given(st.lists(st.one_of(names, bnames), max_size=8), st.one_of(patterns, bpatterns), st.sampled_from(["list", "tuple", "gen"]))
def test_filter_differential(ns, pat, container):
    make = {"list": list, "tuple": tuple, "gen": lambda x: (y for y in x)}[container]
    want = outcome(REF["filter"], make(ns), pat)
    got = outcome(fnmatch.filter, make(ns), pat)
    assert got == want
    if got[0] == "ok":
        # Same objects, same order.
        ref_items = REF["filter"](ns, pat)
        assert all(a is b for a, b in zip(got[1], ref_items))


def test_hits_on_realistic_calls():
    rt.stats(reset=True)
    assert fnmatch.fnmatch("main.py", "*.py")
    assert not fnmatch.fnmatchcase("main.pyc", "*.py")
    assert fnmatch.filter(["a.py", "b.txt", "c.py"], "*.py") == ["a.py", "c.py"]
    assert fnmatch.fnmatch(b"x.c", b"*.[ch]")
    s = rt.stats()
    assert s["fnmatch.fnmatch"]["hits"] == 2
    assert s["fnmatch.fnmatchcase"]["hits"] == 1
    assert s["fnmatch.filter"]["hits"] == 1


def test_mechanics():
    from fnmatch import fnmatch as captured  # bound before enable() (module import)

    rt.stats(reset=True)
    assert captured("a.py", "*.py")
    assert hits("fnmatch.fnmatch") == 1
    assert pickle.loads(pickle.dumps(fnmatch.filter)) is fnmatch.filter
    assert fnmatch.fnmatch.__code__ is REF["fnmatch"].__code__


def settle():
    for _ in range(2000):
        pass


def test_patched_normcase_is_called():
    seen = []
    real = os.path.normcase

    def spy(s):
        seen.append(s)
        return real(s)

    with mock.patch("os.path.normcase", spy):
        settle()
        assert fnmatch.fnmatch("A.PY", "*.PY")
        assert seen == ["A.PY", "*.PY"]
    settle()
    rt.stats(reset=True)
    fnmatch.fnmatch("a", "a")
    assert hits("fnmatch.fnmatch") == 1


def test_patched_translate_is_called():
    seen = []
    real = fnmatch.translate

    def spy(p):
        seen.append(p)
        return real(p)

    fnmatch._compile_pattern.cache_clear()
    with mock.patch.object(fnmatch, "translate", spy):
        settle()
        assert fnmatch.fnmatchcase("abc", "a*c")
        assert seen == ["a*c"]
    settle()


def test_tools_see_the_original():
    calls = []

    def prof(frame, event, arg):
        if event == "call" and frame.f_code is fnmatch.fnmatchcase.__code__:
            calls.append(1)

    sys.setprofile(prof)
    try:
        fnmatch.fnmatchcase("a", "a")
    finally:
        sys.setprofile(None)
    assert calls == [1]


def test_filter_concurrent_mutation_is_safe():
    pool = [f"f{i}.py" if i % 2 else f"f{i}.txt" for i in range(64)]
    names_ = list(pool)
    stop = threading.Event()
    errors = []

    def mutate():
        k = 0
        while not stop.is_set():
            names_[k % len(names_)] = pool[(k * 7) % len(pool)]
            k += 1

    def run():
        for _ in range(3000):
            r = fnmatch.filter(names_, "*.py")
            if not all(isinstance(x, str) and x.endswith(".py") for x in r):
                errors.append(r)

    ts = [threading.Thread(target=mutate)] + [threading.Thread(target=run) for _ in range(3)]
    for t in ts:
        t.start()
    for t in ts[1:]:
        t.join()
    stop.set()
    ts[0].join()
    assert not errors
