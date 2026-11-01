"""Mechanism tests (docs/ACCELERATION.md §7.3) of the utils groups' Python-function targets
(`random`, `textwrap`, `urllib`), with the shared helpers of `_mechanics`: captured
references, introspection, `disable()` restores, tools see the original, dependency
patches fall back and re-arm, free-threaded stress."""

from __future__ import annotations

import random
import textwrap
import urllib.parse as up
from random import choice as captured_choice  # captured before enable()
from random import randint as captured_randint
from textwrap import dedent as captured_dedent
from textwrap import fill as captured_fill
from unittest import mock
from urllib.parse import quote as captured_quote
from urllib.parse import unquote as captured_unquote

import pytest
from _mechanics import (
    assert_disable_restores,
    assert_introspection_unchanged,
    assert_patch_falls_back,
    assert_references_accelerated,
    assert_tools_see_original,
    enabled,
    ft_stress,
    references,
)

RNG = random.Random(1)
TW = textwrap.TextWrapper(width=12)
TEXT = "The quick brown fox jumps over the lazy dog, twice."

# (target, function object, call, references + `from` import, args)
CASES = [
    ("random.randint", random.Random.randint, lambda: random.Random(3).randint(1, 6),
     {**references(random.randint), "from": captured_randint, "bound": RNG.randint}, (1, 6)),
    ("random.randrange", random.Random.randrange, lambda: random.Random(3).randrange(10),
     {**references(random.randrange), "bound": RNG.randrange}, (10,)),
    ("random.choice", random.Random.choice, lambda: random.Random(3).choice("abc"),
     {**references(random.choice), "from": captured_choice}, ("abc",)),
    ("random.shuffle", random.Random.shuffle, lambda: (lambda l: (random.Random(3).shuffle(l), l)[1])([1, 2, 3]),
     references(random.shuffle), ([3, 1, 2],)),
    ("random.sample", random.Random.sample, lambda: random.Random(3).sample(range(100), 3),
     references(random.sample), (range(10), 2)),
    ("random.choices", random.Random.choices, lambda: random.Random(3).choices("abc", k=3),
     references(random.choices), ("abc",)),
    ("textwrap.wrap", textwrap.wrap, lambda: textwrap.wrap(TEXT, 10), references(textwrap.wrap), (TEXT, 10)),
    ("textwrap.fill", textwrap.fill, lambda: textwrap.fill(TEXT, 10),
     {**references(textwrap.fill), "from": captured_fill}, (TEXT, 10)),
    ("textwrap.shorten", textwrap.shorten, lambda: textwrap.shorten(TEXT, 20), references(textwrap.shorten), (TEXT, 20)),
    ("textwrap.dedent", textwrap.dedent, lambda: textwrap.dedent("  a\n  b"),
     {**references(textwrap.dedent), "from": captured_dedent}, ("  x\n   y",)),
    ("textwrap.indent", textwrap.indent, lambda: textwrap.indent("a\nb", "> "), references(textwrap.indent), ("a", "> ")),
    ("textwrap.TextWrapper.wrap", textwrap.TextWrapper.wrap, lambda: TW.wrap(TEXT), {"bound": TW.wrap}, (TEXT,)),
    ("textwrap.TextWrapper.fill", textwrap.TextWrapper.fill, lambda: TW.fill(TEXT), {"bound": TW.fill}, (TEXT,)),
    ("urllib.parse.quote", up.quote, lambda: up.quote("a b/c"), {**references(up.quote), "from": captured_quote}, ("a b",)),
    ("urllib.parse.quote_plus", up.quote_plus, lambda: up.quote_plus("a b"), references(up.quote_plus), ("a b",)),
    ("urllib.parse.unquote", up.unquote, lambda: up.unquote("a%20b"),
     {**references(up.unquote), "from": captured_unquote}, ("a%20b",)),
    ("urllib.parse.unquote_plus", up.unquote_plus, lambda: up.unquote_plus("a+b"), references(up.unquote_plus), ("a+b",)),
    ("urllib.parse.parse_qsl", up.parse_qsl, lambda: up.parse_qsl("a=1&b=2"), references(up.parse_qsl), ("a=1",)),
    ("urllib.parse.parse_qs", up.parse_qs, lambda: up.parse_qs("a=1&b=2"), references(up.parse_qs), ("a=1",)),
    ("urllib.parse._urlsplit", up._urlsplit, lambda: up._urlsplit("http://h/p?q#f"), references(up._urlsplit),
     ("http://h/p",)),
]

IDS = [c[0] for c in CASES]


@pytest.mark.parametrize("case", CASES, ids=IDS)
def test_references_accelerated(case):
    name, _func, _call, refs, args = case
    with enabled(name):
        assert_references_accelerated(name, refs, args)


@pytest.mark.parametrize("case", CASES, ids=IDS)
def test_introspection_unchanged(case):
    assert_introspection_unchanged(case[1], case[0])


@pytest.mark.parametrize("case", CASES, ids=IDS)
def test_disable_restores(case):
    assert_disable_restores(case[0], case[1], case[2])


@pytest.mark.parametrize("case", CASES, ids=IDS)
def test_tools_see_original(case):
    assert_tools_see_original(case[0], case[1], case[2])


PATCHES = [
    ("random.randint", lambda: mock.patch("random._index", lambda x: x), lambda: random.Random(3).randint(1, 6)),
    ("random.randrange", lambda: mock.patch("random._ONE", 2), lambda: random.Random(3).randrange(10, 20)),
    ("random.choice", lambda: mock.patch("random.len", len, create=True), lambda: random.Random(3).choice("ab")),
    ("random.shuffle", lambda: mock.patch("builtins.reversed", lambda x, _r=reversed: _r(x)), lambda: random.Random(3).shuffle([1, 2])),
    ("random.sample", lambda: mock.patch("random._ceil", lambda x: 1), lambda: random.Random(3).sample(range(99), 9)),
    ("random.choices", lambda: mock.patch("random._floor", int), lambda: random.Random(3).choices("ab", k=2)),
    ("textwrap.fill", lambda: mock.patch.object(textwrap.TextWrapper, "_split", textwrap.TextWrapper._split),
     lambda: textwrap.fill(TEXT, 10)),
    ("textwrap.wrap", lambda: mock.patch("textwrap.TextWrapper", type("T", (textwrap.TextWrapper,), {})),
     lambda: textwrap.wrap(TEXT, 10)),
    ("textwrap.dedent", lambda: mock.patch("textwrap.min", min, create=True), lambda: textwrap.dedent(" a\n b")),
    ("textwrap.TextWrapper.wrap",
     lambda: mock.patch.object(textwrap.TextWrapper, "wordsep_re", textwrap.TextWrapper.wordsep_simple_re), lambda: TW.wrap(TEXT)),
    ("urllib.parse.quote", lambda: mock.patch("urllib.parse.quote_from_bytes", up.quote_from_bytes),
     lambda: up.quote("a b")),
    ("urllib.parse.quote_plus", lambda: mock.patch("urllib.parse.quote", up.quote), lambda: up.quote_plus("a b")),
    ("urllib.parse.unquote_plus", lambda: mock.patch("urllib.parse.unquote", up.unquote),
     lambda: up.unquote_plus("a+%20")),
    ("urllib.parse.parse_qsl", lambda: mock.patch("urllib.parse.unquote_plus", up.unquote_plus),
     lambda: up.parse_qsl("a=1")),
    ("urllib.parse.parse_qs", lambda: mock.patch("urllib.parse.parse_qsl", up.parse_qsl), lambda: up.parse_qs("a=1")),
    ("urllib.parse._urlsplit", lambda: mock.patch("urllib.parse._checknetloc", up._checknetloc),
     lambda: up._urlsplit("http://h/p")),
]


@pytest.mark.parametrize("case", PATCHES, ids=[p[0] + ":" + str(i) for i, p in enumerate(PATCHES)])
def test_patch_falls_back(case):
    name, patcher, call = case
    # The replacement is a different object (a wrapper, a new class, another value) even
    # when it behaves the same: the pin must notice, and re-arm when the patch exits.
    assert_patch_falls_back(name, _replaced(patcher()), call)


class _replaced:
    """Make every patch install a *different* object than the original (identity pins)."""

    def __init__(self, p):
        self.p = p
        if getattr(p, "new", None) is not None and callable(p.new) and not isinstance(p.new, type):
            f = p.new
            p.new = lambda *a, **k: f(*a, **k)

    def __enter__(self):
        return self.p.__enter__()

    def __exit__(self, *a):
        return self.p.__exit__(*a)


def test_ft_stress_random():
    names = ["random.randint", "random.choice", "random.sample", "random.shuffle", "random.choices"]

    def fn():
        r = random.Random(7)
        l = list(range(10))
        r.shuffle(l)
        return (r.randint(1, 10**6), r.choice("abcdef"), r.sample(range(1000), 3), l, r.choices("xyz", k=4), r.getstate())

    rt_disabled = fn()
    ft_stress(names, fn, rt_disabled)


def test_ft_stress_shared_generator():
    """One generator and one list shared by all threads: results stay valid and nothing
    crashes. (Only membership is checked: CPython's own `shuffle` swaps with two stores, so
    another thread may observe a duplicate between them on free-threaded builds.)"""
    shared = random.Random(5)
    data = list(range(50))

    def fn():
        shared.shuffle(data)
        x = shared.choice(data)
        s = shared.sample(data, 5)
        return x in range(50) and len(s) == 5 and all(v in range(50) for v in s)

    ft_stress(["random"], fn, True)


def test_ft_stress_text_and_urls():
    def fn():
        return (textwrap.fill(TEXT, 9), textwrap.dedent("  a\n  b"), up.quote("a b/ü"), up.parse_qs("a=1&a=2"),
                up._urlsplit("http://x/y?z#w"))

    ft_stress(["textwrap", "urllib"], fn, fn())
