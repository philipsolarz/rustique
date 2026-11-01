"""Group `textwrap` (docs/targets/textwrap.md): differential tests against CPython's own
`textwrap` with the kernels on and off, mechanisms.

Every comparison runs the same calls with `rustique._takeover.enable(only=["textwrap"])`
and disabled and compares results (values, types, identity with the input), exceptions
(type and message).
"""

from __future__ import annotations

import inspect
import sys
import textwrap
from unittest import mock

import pytest
from hypothesis import assume, given, settings
from hypothesis import strategies as st

from rustique import _takeover as rt

NAMES = [
    "textwrap.wrap",
    "textwrap.fill",
    "textwrap.shorten",
    "textwrap.dedent",
    "textwrap.indent",
    "textwrap.TextWrapper.wrap",
    "textwrap.TextWrapper.fill",
]


@pytest.fixture(autouse=True)
def _restore():
    yield
    rt.disable()
    rt.check(None)


def enable():
    rt.enable(only=["textwrap"])
    states = {t["name"]: t for t in rt.status()["targets"] if t["group"] == "textwrap"}
    for name in NAMES:
        assert states[name]["state"] == "installed", states[name]


def outcome(fn, args):
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001
        return ("raise", type(e), str(e))
    ident = [r is a for a in args] if not isinstance(r, list) else [[x is a for a in args] for x in r]
    return ("ok", type(r), r, ident)


def same(calls):
    """`calls`: list of (callable, args-for-identity). Runs all plain, then all enabled."""
    out = []
    for on in (False, True):
        enable() if on else rt.disable()
        out.append([outcome(f, a) for f, a in calls])
    rt.disable()
    for i, (p, q) in enumerate(zip(*out)):
        assert q == p, (i, p, q)


TEXTS = [
    "",
    " ",
    "   \n  ",
    "single",
    "Hello there -- you goof-ball, use the -b option!",
    "The quick brown fox jumps over the lazy dog. " * 7,
    "a\tb\tc\n  indented line\n\n",
    "supercalifragilisticexpialidocious " * 3,
    "x--y well-known self-evident co-op -- em--dash a-b-c-d ab-cd-ef-gh",
    "Mr. Smith. went home!  He said \"hi.\" Then 'bye.'\nNext line? yes.",
    "unicode: café–crème naïve-ish ŝĉ-ŝĉ ab cd  em-space 　ideographic",
    "digits 12-34 ²-ab ab-² ٣٤-ab x_y-z_w",
    "tabs\tand\x0bvertical\x0cform\rreturn",
    "-" * 30 + " " + "--a" + " a--" + " ---x--- ",
    "longwordwithhyphen-insideandmore-hyphens-here " * 2,
    " line sep para sep\x85nel\x1cfs",
]

WIDTHS = [1, 2, 3, 5, 8, 12, 20, 40, 70, 200]

OPTION_SETS = [
    {},
    {"initial_indent": "> ", "subsequent_indent": "  "},
    {"initial_indent": "***********", "subsequent_indent": ""},
    {"break_on_hyphens": False},
    {"break_on_hyphens": 1},
    {"break_long_words": False},
    {"drop_whitespace": False},
    {"replace_whitespace": False},
    {"expand_tabs": False},
    {"tabsize": 4},
    {"tabsize": 0},
    {"fix_sentence_endings": True},
    {"max_lines": 1},
    {"max_lines": 2, "placeholder": "..."},
    {"max_lines": 3, "placeholder": " [more]", "initial_indent": "* "},
    {"max_lines": 2, "placeholder": "   "},
    {"replace_whitespace": False, "expand_tabs": False, "drop_whitespace": False},
]


def hangs(t, w, o):
    """CPython's `_wrap_chunks` never returns for whitespace-only text whose indent is at
    least as wide as the width (the kernels detect it and let CPython loop)."""
    ws = " \t\n\x0b\x0c\r"
    indent = max(len(o.get("initial_indent", "")), len(o.get("subsequent_indent", "")))
    return not t.strip(ws) and indent >= w


def test_module_functions_match():
    calls = []
    for t in TEXTS:
        for w in WIDTHS:
            for o in OPTION_SETS:
                if hangs(t, w, o):
                    continue
                calls.append((lambda t=t, w=w, o=o: textwrap.wrap(t, w, **o), [t]))
                calls.append((lambda t=t, w=w, o=o: textwrap.fill(t, width=w, **o), [t]))
                if "max_lines" not in o:
                    calls.append((lambda t=t, w=w, o=o: textwrap.shorten(t, w, **o), [t]))
        calls.append((lambda t=t: textwrap.dedent(t), [t]))
        calls.append((lambda t=t: textwrap.indent(t, "  "), [t]))
        calls.append((lambda t=t: textwrap.indent(t, ""), [t]))
        calls.append((lambda t=t: textwrap.indent(text=t, prefix="+"), [t]))
        calls.append((lambda t=t: textwrap.indent(t, "> ", None), [t]))
    same(calls)


def test_identity_cases():
    # CPython returns its input when nothing changes; so must the kernels.
    cases = ["single", "", "abc def", "  x", "x\n", "\t", "  \n", "a\nb"]
    calls = []
    for t in cases:
        calls.append((lambda t=t: textwrap.wrap(t, replace_whitespace=False), [t]))
        calls.append((lambda t=t: textwrap.fill(t, replace_whitespace=False), [t]))
        calls.append((lambda t=t: textwrap.fill(t, 3, replace_whitespace=False, expand_tabs=False), [t]))
        calls.append((lambda t=t: textwrap.shorten(t, 20, replace_whitespace=False), [t]))
        calls.append((lambda t=t: textwrap.dedent(t), [t]))
        calls.append((lambda t=t: textwrap.indent(t, "> "), [t]))
    same(calls)


def test_errors_and_fallbacks_match():
    class S(str):
        pass

    calls = [
        (lambda: textwrap.wrap("abc", 0), []),
        (lambda: textwrap.wrap("abc", -3), []),
        (lambda: textwrap.wrap("abc def", 5, max_lines=1, placeholder="  much too long  "), []),
        (lambda: textwrap.wrap("abc", 4.5), []),
        (lambda: textwrap.wrap("abc", width="4"), []),
        (lambda: textwrap.wrap(b"abc"), []),
        (lambda: textwrap.wrap(None), []),
        (lambda: textwrap.wrap(S("abc def ghi"), 4), []),
        (lambda: textwrap.wrap("abc", nonsense=1), []),
        (lambda: textwrap.wrap("abc", 3, 4), []),
        (lambda: textwrap.wrap("abc", 3, width=4), []),
        (lambda: textwrap.shorten("abc", 3, max_lines=2), []),
        (lambda: textwrap.shorten("abc"), []),
        (lambda: textwrap.fill("abc def", 4, initial_indent=b"x"), []),
        (lambda: textwrap.fill("abc def", 4, tabsize=2.0), []),
        (lambda: textwrap.fill("abc def", 4, drop_whitespace=[]), []),
        (lambda: textwrap.dedent(None), []),
        (lambda: textwrap.dedent(b"  x"), []),
        (lambda: textwrap.dedent(S("  x\n  y")), []),
        (lambda: textwrap.indent("a\nb", "> ", lambda line: True), []),
        (lambda: textwrap.indent(b"a", b"> "), []),
        (lambda: textwrap.indent("a", 5), []),
    ]
    same(calls)


def test_textwrapper_methods_match():
    class Sub(textwrap.TextWrapper):
        pass

    class Custom(textwrap.TextWrapper):
        def _split(self, text):
            return text.split()

    calls = []
    for cls in (textwrap.TextWrapper, Sub, Custom):
        for t in TEXTS[:10]:
            for o in OPTION_SETS:
                for w in (5, 13, 40):
                    if hangs(t, w, o):
                        continue
                    calls.append((lambda cls=cls, t=t, o=o, w=w: cls(width=w, **o).wrap(t), [t]))
                    calls.append((lambda cls=cls, t=t, o=o, w=w: cls(width=w, **o).fill(t), [t]))

    def shadowed(t):
        w = textwrap.TextWrapper(width=10)
        w._wrap_chunks = lambda chunks: ["shadow"]
        return w.wrap(t), w.fill(t)

    def changed_attrs(t):
        w = textwrap.TextWrapper()
        w.width = 7
        w.placeholder = "~"
        w.max_lines = 2
        w.break_on_hyphens = 0
        return w.wrap(t), w.fill(t)

    def deleted_attr(t):
        w = textwrap.TextWrapper()
        del w.tabsize
        return w.wrap(t)

    def materialized(t):
        w = textwrap.TextWrapper(width=9)
        vars(w)
        return w.wrap(t), w.fill(t)

    for t in TEXTS:
        for f in (shadowed, changed_attrs, deleted_attr, materialized):
            calls.append((lambda f=f, t=t: f(t), [t]))
    same(calls)


def test_class_patching_is_honoured():
    def op():
        out = []
        with mock.patch.object(textwrap.TextWrapper, "_split", lambda self, text: text.split()):
            out.append(textwrap.wrap("ab-cd ef", 3))
        with mock.patch.object(textwrap.TextWrapper, "wordsep_re", textwrap.TextWrapper.wordsep_simple_re):
            out.append(textwrap.wrap("ab-cd ef", 3))
        with mock.patch.object(textwrap.TextWrapper.__init__, "__defaults__", (5,) + textwrap.TextWrapper.__init__.__defaults__[1:]):
            out.append(textwrap.wrap("abcdefgh ij"))
        with mock.patch.dict(textwrap.TextWrapper.unicode_whitespace_trans, {ord("x"): ord("y")}):
            out.append(textwrap.wrap("axb cxd", 3))
        with mock.patch.object(textwrap, "TextWrapper", type("TW", (textwrap.TextWrapper,), {"_split": lambda s, t: [t]})):
            out.append(textwrap.wrap("ab cd", 3))
            out.append(textwrap.shorten("ab cd ef", 6))
        out.append(textwrap.wrap("ab-cd ef", 3))
        return out

    same([(op, [])])


# ---------------------------------------------------------------------------------------
# Hypothesis
# ---------------------------------------------------------------------------------------

alphabet = st.sampled_from(list("ab -\t\n.!?\"'_2²٣éZ  \x0b\x85") + ["--", "  ", "x-y", "Mr. "])
texts = st.lists(alphabet, max_size=60).map("".join)
indents = st.sampled_from(["", " ", "> ", "\t", "xxxxxxxxxx"])


@settings(max_examples=400)
@given(
    t=texts,
    width=st.integers(1, 30),
    ii=indents,
    si=indents,
    flags=st.lists(st.booleans(), min_size=7, max_size=7),
    tabsize=st.integers(0, 9),
    max_lines=st.one_of(st.none(), st.integers(1, 5)),
    placeholder=st.sampled_from([" [...]", "...", "", "  ~ "]),
)
def test_hyp_wrap(t, width, ii, si, flags, tabsize, max_lines, placeholder):
    assume(not hangs(t, width, {"initial_indent": ii, "subsequent_indent": si}))
    kw = dict(
        initial_indent=ii,
        subsequent_indent=si,
        expand_tabs=flags[0],
        replace_whitespace=flags[1],
        fix_sentence_endings=flags[2],
        break_long_words=flags[3],
        drop_whitespace=flags[4],
        break_on_hyphens=flags[5],
        tabsize=tabsize,
        max_lines=max_lines,
        placeholder=placeholder,
    )
    calls = [
        (lambda: textwrap.wrap(t, width, **kw), [t]),
        (lambda: textwrap.fill(t, width, **kw), [t]),
        (lambda: textwrap.TextWrapper(width, **kw).wrap(t), [t]),
    ]
    if max_lines is None:
        kw2 = {k: v for k, v in kw.items() if k != "max_lines"}
        calls.append((lambda: textwrap.shorten(t, width + 5, **kw2), [t]))
    same(calls)


@settings(max_examples=300)
@given(lines=st.lists(st.text(alphabet=" \tab \x0c#", max_size=8), max_size=8), prefix=indents)
def test_hyp_dedent_indent(lines, prefix):
    t = "\n".join(lines)
    t2 = "\r\n".join(lines) + "\x0b" + " ".join(lines)
    same(
        [
            (lambda: textwrap.dedent(t), [t]),
            (lambda: textwrap.dedent(t2), [t2]),
            (lambda: textwrap.indent(t, prefix), [t]),
            (lambda: textwrap.indent(t2, prefix), [t2]),
        ]
    )


@settings(max_examples=200)
@given(t=st.text(max_size=80), width=st.integers(1, 25))
def test_hyp_any_unicode(t, width):
    same([(lambda: textwrap.wrap(t, width), [t]), (lambda: textwrap.dedent(t), [t]), (lambda: textwrap.indent(t, "|"), [t])])


# ---------------------------------------------------------------------------------------
# Mechanisms
# ---------------------------------------------------------------------------------------


def test_captured_references_and_introspection():
    from textwrap import dedent, fill  # captured before enable

    srcs = {n: inspect.getsource(f) for n, f in (("fill", fill), ("dedent", dedent))}
    enable()
    rt.stats(True, reset=True)
    fill("a b c", 3)
    dedent("  x")
    textwrap.TextWrapper(width=3).fill("a b")
    st_ = rt.stats()
    assert st_["textwrap.fill"]["hits"] == 1
    assert st_["textwrap.dedent"]["hits"] == 1
    assert st_["textwrap.TextWrapper.fill"]["hits"] == 1
    assert inspect.getsource(fill) == srcs["fill"] and fill is textwrap.fill
    assert str(inspect.signature(textwrap.wrap)) == "(text, width=70, **kwargs)"
    rt.stats(False)


def test_tools_see_original_frames():
    enable()
    seen = []
    sys.setprofile(lambda f, e, a: seen.append(f.f_code.co_name) if e == "call" else None)
    try:
        textwrap.fill("a b c d", 3)
    finally:
        sys.setprofile(None)
    assert "fill" in seen and "_wrap_chunks" in seen


def test_check_mode_agrees():
    enable()
    rt.check("raise")
    for t in TEXTS:
        textwrap.wrap(t, 7)
        textwrap.fill(t, 9, max_lines=2)
        textwrap.shorten(t, 15)
        textwrap.dedent(t)
        textwrap.indent(t, "> ")
        textwrap.TextWrapper(width=11).fill(t)
    rt.check(None)
