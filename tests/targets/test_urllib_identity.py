"""Group `urllib`, wave 2 (docs/targets/urllib.md §1, §7): the allocation-free kernels build
their results straight into the final `str`, so object identity is decided by the kernels.
CPython's builders differ: slices and `bytes.decode()` give the Latin-1 singleton for one
character, `''.join` of several parts and `str.replace` give a new object even then, and a
one-byte `quote` returns the `_Quoter` cache's shared object. These tests compare values,
types and identity (with every argument and with the singletons) against CPython for
inputs of every PEP 393 kind, and run concurrent callers (per-thread scratch buffers)."""

from __future__ import annotations

import threading
import urllib.parse as up

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

SINGLETONS = [chr(i) for i in range(256)]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()


def flat(r):
    if isinstance(r, str):
        return [r]
    if isinstance(r, dict):
        return [x for k, v in r.items() for x in [k, *v]]
    if isinstance(r, (list, tuple)):
        return [y for x in r for y in flat(x)]
    return []


def facts(r, args):
    out = []
    for x in flat(r):
        if x is None:
            continue
        single = len(x) == 1 and ord(x) < 256 and x is SINGLETONS[ord(x)]
        out.append(([x is a for a in args], single))
    return out


def run(fn, args):
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001
        return ("raise", type(e), str(e))
    return ("ok", type(r), r, facts(r, args))


def same(calls, twice=True):
    out = []
    for on in (False, True):
        if on:
            rt.enable(only=["urllib"])
        else:
            rt.disable()
        res = [run(f, a) for f, a in calls]
        if twice:  # identity across calls (caches) too
            res = [(r, run(f, a), (lambda r1, r2: [x is y for x, y in zip(flat(r1[2]), flat(r2[2]))] if r1[0] == r2[0] == "ok" else None)(r, run(f, a)))
                   for r, (f, a) in zip(res, calls)]
        out.append(res)
    rt.disable()
    for i, (p, q) in enumerate(zip(*out)):
        assert q == p, (i, calls[i][1], p, q)


STRS = ["", "a", "A", "%41", "%", "+", " ", "a b", "%41%42", "a%20b", "café", "%C3%A9", "é%41",
        "ā%20x", "\U0001f600+%2B", "%e2%82%ac", "%E2%82", "a+b%2", "é", "ā", "~._-", "/", "?"]


def test_quote_unquote_identity():
    calls = []
    for s in STRS:
        b = s.encode("utf-8", "surrogatepass")
        for safe in (None, "", "/", "é/", b"/"):
            kw = {} if safe is None else {"safe": safe}
            calls.append((lambda s=s, kw=kw: up.quote(s, **kw), [s]))
            calls.append((lambda s=s, kw=kw: up.quote_plus(s, **kw), [s]))
            calls.append((lambda b=b, kw=kw: up.quote(b, **kw), [b]))
        for errors in (None, "strict", "ignore", "surrogateescape"):
            kw = {} if errors is None else {"errors": errors}
            calls.append((lambda s=s, kw=kw: up.unquote(s, **kw), [s]))
            calls.append((lambda s=s, kw=kw: up.unquote_plus(s, **kw), [s]))
    same(calls)


def test_parse_and_split_identity():
    qs = ["a=b", "%41=%42", "a=+", "=", "a", "x=1&y=%41&z=+&w", "é=ā&%41=\U0001f600", "a=b;c=d", "k=v&&k=w"]
    urls = ["a\t", "  a", "http://h/p?q#f", "\t x", "h:", "//n", "?", "#", "a\nb", "HTTP://H/é?ā#\U0001f600",
            "mailto:x", "x", ""]
    calls = []
    for q in qs:
        for kw in ({}, {"keep_blank_values": True}, {"separator": ";"}, {"separator": "&&"}):
            calls.append((lambda q=q, kw=kw: up.parse_qsl(q, **kw), [q]))
            calls.append((lambda q=q, kw=kw: up.parse_qs(q, **kw), [q]))
    for u in urls:
        for sch in (None, "", "s", " s\t", "s\tc"):
            for frag in (True, False):
                calls.append((lambda u=u, sch=sch, frag=frag: up._urlsplit(u, sch, frag), [u, sch]))
    same(calls)


alphabet = st.sampled_from(list("ab%+ =&;#?/:\t") + ["%41", "%C3%A9", "%e2%82%ac", "é", "ā", "\U0001f600",
                                                     "%zz", "%F0%9F", "\ud800"])
texty = st.lists(alphabet, max_size=16).map("".join)


@settings(max_examples=400, deadline=None)
@given(s=texty, safe=st.sampled_from([None, "", "/", "+ é"]), errors=st.sampled_from([None, "strict", "ignore"]))
def test_hyp_identity(s, safe, errors):
    kw = {} if safe is None else {"safe": safe}
    ekw = {} if errors is None else {"errors": errors}
    same([(lambda: up.quote(s, **kw), [s]), (lambda: up.quote_plus(s, **kw), [s]),
          (lambda: up.unquote(s, **ekw), [s]), (lambda: up.unquote_plus(s, **ekw), [s]),
          (lambda: up.parse_qsl(s, keep_blank_values=True, **ekw), [s]), (lambda: up.parse_qs(s, **ekw), [s]),
          (lambda: up._urlsplit(s), [s]), (lambda: up._urlsplit(s, "x\t"), [s])])


def test_threads_share_nothing():
    data = [("/p/%d/café x?y" % i, "a=%d&b=%%41+%d&c=é" % (i, i), "http://h%d.example/p?q=%d#f" % (i, i))
            for i in range(32)]
    expect = [(up.quote(a), up.unquote(up.quote(a)), up.parse_qsl(b), up.parse_qs(b), up._urlsplit(c)) for a, b, c in data]
    rt.enable(only=["urllib"])
    bad = []

    def worker(k):
        for _ in range(300):
            for (a, b, c), e in zip(data, expect):
                if (up.quote(a), up.unquote(up.quote(a)), up.parse_qsl(b), up.parse_qs(b), up._urlsplit(c)) != e:
                    bad.append(k)
                    return

    ts = [threading.Thread(target=worker, args=(k,)) for k in range(8)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert not bad
