"""Group `textwrap`, wave 2 (docs/targets/textwrap.md §1, §7): the allocation-free kernels
build their results straight into the final `str`, so object identity is decided by the
kernels themselves. These tests compare, against CPython's own `textwrap`, the values,
the types and the identity of every result object with every argument object and with the
Latin-1 one-character singletons; inputs of every PEP 393 kind; the `max_lines` /
placeholder paths that return the placeholder object itself; and concurrent callers
(per-thread scratch buffers)."""

from __future__ import annotations

import textwrap
import threading

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

SINGLETONS = [chr(i) for i in range(256)]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()


def ident(r, args):
    """Identity facts of a result: `is` each argument, `is` the singleton of its value."""
    items = r if isinstance(r, list) else [r]
    out = []
    for x in items:
        single = len(x) == 1 and ord(x) < 256 and x is SINGLETONS[ord(x)]
        out.append(([x is a for a in args], single, type(x)))
    return out


def run(fn, args):
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001
        return ("raise", type(e), str(e))
    return ("ok", type(r), r, ident(r, args))


def same(calls):
    out = []
    for on in (False, True):
        if on:
            rt.enable(only=["textwrap"])
        else:
            rt.disable()
        out.append([run(f, a) for f, a in calls])
    rt.disable()
    for i, (p, q) in enumerate(zip(*out)):
        assert q == p, (i, p, q)


KINDS = ["plain ascii words here", "café crème brûlée", "šč кириллица text",
         "emoji \U0001f600 wide \U0001f680 chars", "a", "é", " ", "\t", "　x　"]


def test_kinds_and_singletons():
    calls = []
    for t in KINDS:
        for w in (1, 2, 5, 70):
            for o in ({}, {"replace_whitespace": False, "drop_whitespace": False}, {"initial_indent": "é"},
                      {"subsequent_indent": "\U0001f600"}, {"break_on_hyphens": False}):
                if max(len(o.get("initial_indent", "")), len(o.get("subsequent_indent", ""))) >= w:
                    continue
                calls.append((lambda t=t, w=w, o=o: textwrap.wrap(t, w, **o), [t] + list(o.values())))
                calls.append((lambda t=t, w=w, o=o: textwrap.fill(t, w, **o), [t] + list(o.values())))
        calls.append((lambda t=t: textwrap.dedent(t), [t]))
        calls.append((lambda t=t: textwrap.indent(t, "é"), [t]))
    same(calls)


def test_placeholder_identity_paths():
    # CPython returns the placeholder object itself (`'' + placeholder`,
    # `'' + placeholder.lstrip()` with nothing to strip) or a fresh one-character str (an
    # empty placeholder joined to a one-character chunk): the kernels let CPython run.
    ph = "".join(["[", "..", "]"])  # a run-time object, not a constant
    calls = []
    for text in ["abcdefgh ijk", "  ", "a b c d e f", "x", "xy zw", "ab  cd", "     word"]:
        for w in (1, 2, 3, 4, 5, 8):
            for p in (ph, "", " ", "  .", "é"):
                for m in (1, 2):
                    for ind in ("", " "):
                        o = {"max_lines": m, "placeholder": p, "initial_indent": ind, "subsequent_indent": ind}
                        if len(ind) >= w:
                            continue
                        calls.append((lambda t=text, w=w, o=o: textwrap.wrap(t, w, **o), [text, p, ind]))
                        calls.append((lambda t=text, w=w, o=o: textwrap.fill(t, w, **o), [text, p, ind]))
                        calls.append((lambda t=text, w=w, o=o: textwrap.wrap(t, w, drop_whitespace=False, **o), [text, p, ind]))
    same(calls)


def test_large_input_releases_scratch():
    big = ("word " * 40000) + "end"
    same([(lambda: textwrap.fill(big, 77), [big]), (lambda: textwrap.indent(big.replace(" ", "\n"), "> "), [big]),
          (lambda: textwrap.dedent("  " + big.replace(" ", "\n  ")), [big])])


texts = st.text(alphabet=st.sampled_from(list("ab -\t\n.!?\"'") + ["é", "ā", "\U0001f600", "　", "\x85"]),
                max_size=60)


@settings(max_examples=300, deadline=None)
@given(t=texts, width=st.integers(1, 30), ii=st.sampled_from(["", "> ", "é"]),
       si=st.sampled_from(["", " ", "\U0001f600"]), ph=st.sampled_from([" [...]", "", "..", " é"]),
       ml=st.sampled_from([None, 1, 2, 3]), flags=st.lists(st.booleans(), min_size=4, max_size=4))
def test_hyp_identity(t, width, ii, si, ph, ml, flags):
    if max(len(ii), len(si)) >= width:
        return
    o = {"initial_indent": ii, "subsequent_indent": si, "placeholder": ph, "max_lines": ml,
         "replace_whitespace": flags[0], "drop_whitespace": flags[1], "expand_tabs": flags[2],
         "break_on_hyphens": flags[3]}
    args = [t, ii, si, ph]
    same([(lambda: textwrap.wrap(t, width, **o), args), (lambda: textwrap.fill(t, width, **o), args),
          (lambda: textwrap.shorten(t, width, placeholder=ph), [t, ph]),
          (lambda: textwrap.dedent(t), [t]), (lambda: textwrap.indent(t, ii), [t, ii]),
          (lambda: textwrap.TextWrapper(width=width, **o).fill(t), args)])


def test_threads_share_nothing():
    texts = [("w%d " % i) * (50 + i) + "é" * (i % 3) for i in range(16)]
    expect = [(textwrap.fill(t, 40, initial_indent="> "), textwrap.wrap(t, 33), textwrap.dedent("  " + t),
               textwrap.indent(t.replace(" ", "\n"), "+")) for t in texts]
    rt.enable(only=["textwrap"])
    errors = []

    def worker(k):
        for _ in range(200):
            for t, e in zip(texts[k:] + texts[:k], expect[k:] + expect[:k]):
                got = (textwrap.fill(t, 40, initial_indent="> "), textwrap.wrap(t, 33), textwrap.dedent("  " + t),
                       textwrap.indent(t.replace(" ", "\n"), "+"))
                if got != e:
                    errors.append((k, t))
                    return

    ts = [threading.Thread(target=worker, args=(k,)) for k in range(8)]
    for th in ts:
        th.start()
    for th in ts:
        th.join()
    assert not errors
    st_ = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "textwrap"}
    assert st_["textwrap.fill"]["state"] == "installed"


def test_reentrant_scratch_from_finalizer():
    # A finalizer calling a kernel while this thread's scratch is in use gets one-off
    # buffers (no panic); results stay CPython's.
    class D:
        def __del__(self):
            out.append(textwrap.fill("finalizer text " * 5, 20))

    out = []
    plain = textwrap.fill("main text " * 20, 30)
    rt.enable(only=["textwrap"])
    for _ in range(50):
        d = D()
        del d
        assert textwrap.fill("main text " * 20, 30) == plain
    assert out and all(x == out[0] for x in out)
