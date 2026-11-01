"""Group `urllib` (docs/targets/urllib.md): differential tests against CPython's own
`urllib.parse` with the kernels on and off, mechanisms."""

from __future__ import annotations

import inspect
import sys
import urllib.parse as up
from unittest import mock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

NAMES = [
    "urllib.parse.quote",
    "urllib.parse.quote_plus",
    "urllib.parse.unquote",
    "urllib.parse.unquote_plus",
    "urllib.parse.parse_qsl",
    "urllib.parse.parse_qs",
    "urllib.parse._urlsplit",
]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()
    rt.check(None)


def enable():
    rt.enable(only=["urllib"])
    states = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "urllib"}
    for name in NAMES:
        assert states[name]["state"] == "installed", states[name]


def outcome(fn, args):
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001
        return ("raise", type(e), str(e))

    def ids(x):
        if isinstance(x, (list, tuple)):
            return [ids(y) for y in x]
        if isinstance(x, dict):
            return [(ids(k), ids(v)) for k, v in x.items()]
        return [x is a for a in args]

    return ("ok", type(r), r, ids(r))


def checked(calls):
    """Kernels enabled once, check mode raising on any difference from the original
    (`accel::check::verify`: values, types, identity with arguments, exceptions)."""
    rt.enable(only=["urllib"])
    rt.check("raise")
    for f, _ in calls:
        try:
            f()
        except Exception as e:  # noqa: BLE001
            assert type(e).__name__ != "RustiqueCheckError", e


def same(calls):
    out = []
    for on in (False, True):
        enable() if on else rt.disable()
        up.urlsplit.cache_clear()
        out.append([outcome(f, a) for f, a in calls])
    rt.disable()
    for i, (p, q) in enumerate(zip(*out)):
        assert q == p, (i, p, q)


STRINGS = [
    "",
    "a",
    "abc",
    "abc def",
    "/path/to/file.html",
    "a+b c+d",
    "100% sure",
    "%41%42%43",
    "%e2%82%ac %E2%82%AC",
    "%zz%4%%41%",
    "%ff%fe bad utf8 %c3%28",
    "café ünïcödé ☃ 𝄞",
    "caf%C3%A9 + ü%20x",
    "~user/_.-!*'();:@&=+$,/?#[]",
    "\x00\x01 \t\n\x7f",
    "\ud800 lone surrogate",
    "a" * 300,
]

SAFES = [None, "", "/", "/:", "~", b"/", b"", "äö/", "%"]


def test_quote_unquote_match():
    calls = []
    for s in STRINGS:
        for safe in SAFES:
            kw = {} if safe is None else {"safe": safe}
            calls.append((lambda s=s, kw=kw: up.quote(s, **kw), [s]))
            calls.append((lambda s=s, kw=kw: up.quote_plus(s, **kw), [s]))
            calls.append((lambda s=s, kw=kw: up.quote(s.encode("utf-8", "surrogatepass"), **kw), [s]))
        for enc, err in [(None, None), ("utf-8", "strict"), ("latin-1", None), (None, "ignore"), ("utf8", "replace")]:
            calls.append((lambda s=s, enc=enc, err=err: up.quote(s, encoding=enc, errors=err), [s]))
            calls.append((lambda s=s, enc=enc, err=err: up.unquote(s, encoding=enc or "utf-8", errors=err or "replace"), [s]))
            calls.append((lambda s=s, enc=enc, err=err: up.unquote_plus(s, enc or "utf-8", err or "replace"), [s]))
        for err in ["replace", "strict", "ignore", "surrogateescape", "backslashreplace"]:
            calls.append((lambda s=s, err=err: up.unquote(s, errors=err), [s]))
        calls.append((lambda s=s: up.unquote(s), [s]))
        calls.append((lambda s=s: up.unquote_plus(s), [s]))
        calls.append((lambda s=s: up.unquote(string=s), [s]))
    calls += [
        (lambda: up.quote(None), []),
        (lambda: up.quote(5), []),
        (lambda: up.quote(b"abc", encoding="utf-8"), []),
        (lambda: up.quote("abc", safe=None), []),
        (lambda: up.quote("abc", safe=5), []),
        (lambda: up.quote(bytearray(b"a b")), []),
        (lambda: up.quote_plus(b"a b", safe="/"), []),
        (lambda: up.unquote(b"a%20b"), []),
        (lambda: up.unquote(None), []),
        (lambda: up.unquote_plus(b"a+b"), []),
    ]
    same(calls)


QS = [
    "",
    "a=1",
    "a=1&b=2&a=3",
    "a=1;b=2",
    "a&b=&=c&&d=4",
    "x=%41+%42&y=caf%C3%A9&z=%zz",
    "name=hello+world&empty=&flag",
    "k=v=w&k2==",
    "ü=ö&%E2%82%AC=€",
    "a=1&" * 20,
]


def test_parse_qs_match():
    calls = []
    for q in QS:
        for kw in [
            {},
            {"keep_blank_values": True},
            {"keep_blank_values": 1},
            {"separator": ";"},
            {"separator": "&&"},
            {"max_num_fields": 3},
            {"max_num_fields": 100},
            {"strict_parsing": True},
            {"errors": "strict"},
            {"encoding": "latin-1"},
            {"separator": ""},
            {"separator": b"&"},
        ]:
            calls.append((lambda q=q, kw=kw: up.parse_qsl(q, **kw), [q]))
            calls.append((lambda q=q, kw=kw: up.parse_qs(q, **kw), [q]))
    calls += [
        (lambda: up.parse_qsl(None), []),
        (lambda: up.parse_qsl(b"a=1"), []),
        (lambda: up.parse_qs("a=1", _stacklevel=2), []),
        (lambda: up.parse_qsl("a=1", _stacklevel=2), []),
    ]
    same(calls)


URLS = [
    "",
    "http://www.example.com/path?q=1#frag",
    "HTTPS://User:Pw@Example.COM:8080/a/b;p?x=1&y=2#f#g",
    "//netloc/path",
    "/abs/path?x",
    "rel/path#frag?notquery",
    "mailto:someone@example.com",
    "c:\\windows\\path",
    "1http://x",
    "http:",
    "http://",
    "  \t http://ex\tam\nple.com/\r",
    "\x00\x1fhttp://a",
    "http://[::1]:80/",
    "http://[::1/",
    "http://ünïcode.example/",
    "http://exa\u2100mple/",
    "svn+ssh://host/repo",
    "a-b.c+d:rest",
    "?only=query",
    "#only-fragment",
    "path?a=1?b=2#c#d",
]


def test_urlsplit_urlparse_match():
    calls = []
    for u in URLS:
        for scheme in [None, "", "https", "  ftp \t"]:
            for frag in [True, False, 0, 1]:
                calls.append((lambda u=u, s=scheme, f=frag: up._urlsplit(u, s, f), [u, scheme]))
        calls.append((lambda u=u: up.urlsplit(u), [u]))
        calls.append((lambda u=u: up.urlparse(u), [u]))
        calls.append((lambda u=u: up.urljoin("http://a/b/c/d;p?q", u), [u]))
        calls.append((lambda u=u: up.urlsplit(u, scheme="x", allow_fragments=False), [u]))
    calls.append((lambda: up._urlsplit(b"http://x"), []))
    calls.append((lambda: up._urlsplit("http://x", 5), []))
    same(calls)


def test_urlsplit_cache_info_unchanged():
    def op():
        up.urlsplit.cache_clear()
        for u in URLS[:8] * 3:
            up.urlsplit(u)
        return up.urlsplit.cache_info()

    same([(op, [])])


# ---------------------------------------------------------------------------------------
# Hypothesis
# ---------------------------------------------------------------------------------------

urlish = st.text(alphabet=st.sampled_from(list("ab:/?#[]@%+=&; \t\n\x00~é€2FfZz") + ["%2", "%e2%82", "://", "//"]), max_size=40)


@settings(max_examples=400)
@given(s=urlish, safe=st.sampled_from(["/", "", ":/?", "é"]), frag=st.booleans())
def test_hyp_all(s, safe, frag):
    checked(
        [
            (lambda: up.quote(s, safe=safe), [s]),
            (lambda: up.quote_plus(s, safe), [s]),
            (lambda: up.unquote(s), [s]),
            (lambda: up.unquote_plus(s), [s]),
            (lambda: up.parse_qsl(s, keep_blank_values=frag), [s]),
            (lambda: up.parse_qs(s, frag), [s]),
            (lambda: up._urlsplit(s, None, frag), [s]),
        ]
    )


@settings(max_examples=200)
@given(s=st.text(max_size=30))
def test_hyp_any_text(s):
    checked([(lambda: up.quote(s), [s]), (lambda: up.unquote(s), [s]), (lambda: up._urlsplit(s), [s])])


# ---------------------------------------------------------------------------------------
# Mechanisms
# ---------------------------------------------------------------------------------------


def test_captured_references_and_introspection():
    from urllib.parse import quote, unquote  # captured before enable (63-90% of call sites)

    src = inspect.getsource(quote)
    enable()
    rt.stats(True, reset=True)
    quote("a b")
    unquote("a%20b")
    up.urlsplit("http://fresh.example/" + "x" * 3)
    st_ = rt.stats()
    assert st_["urllib.parse.quote"]["hits"] == 1
    assert st_["urllib.parse.unquote"]["hits"] == 1
    assert st_["urllib.parse._urlsplit"]["hits"] >= 1
    assert inspect.getsource(quote) == src and quote is up.quote
    rt.stats(False)


def test_dependency_patching_is_honoured():
    def op():
        out = []
        with mock.patch.object(up, "quote_from_bytes", lambda bs, safe="/": "patched"):
            out.append(up.quote("a b"))
        with mock.patch.object(up, "quote", lambda *a, **k: "patched-quote"):
            out.append(up.quote_plus("a b"))
            out.append(up.quote_plus("ab"))
        with mock.patch.object(up, "unquote", lambda *a, **k: "patched-unquote"):
            out.append(up.unquote_plus("a+b"))
        with mock.patch.object(up, "unquote_plus", lambda *a, **k: "U"):
            out.append(up.parse_qsl("a=1&b=2"))
        with mock.patch.object(up, "parse_qsl", lambda *a, **k: [("k", "v")]):
            out.append(up.parse_qs("a=1"))
        with mock.patch.object(up, "_checknetloc", lambda n: out.append(("checked", n))):
            out.append(up._urlsplit("http://host/p"))
        with mock.patch.object(up, "scheme_chars", "abc"):
            out.append(up._urlsplit("http://host/p"))
        up._UNSAFE_URL_BYTES_TO_REMOVE.append("x")
        try:
            out.append(up._urlsplit("http://hoxst/pxx"))
        finally:
            up._UNSAFE_URL_BYTES_TO_REMOVE.remove("x")
        out.append(up.quote("a b"))
        return out

    same([(op, [])])


def test_tools_see_original_frames():
    enable()
    seen = []
    sys.setprofile(lambda f, e, a: seen.append(f.f_code.co_name) if e == "call" else None)
    try:
        up.quote("a b")
    finally:
        sys.setprofile(None)
    assert "quote" in seen and "quote_from_bytes" in seen


def test_check_mode_agrees():
    enable()
    rt.check("raise")
    for s in STRINGS:
        if "\ud800" in s:
            continue
        up.quote(s)
        up.quote_plus(s)
        up.unquote(s)
        up.unquote_plus(s)
        up.parse_qsl(s)
        up.parse_qs(s)
        up._urlsplit(s)
    rt.check(None)
