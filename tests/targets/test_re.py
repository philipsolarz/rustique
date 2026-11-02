"""Group `re` (docs/targets/re.md): the Rust compiler against CPython's own `re._compiler.compile`
(identical `_sre.compile` arguments, or a fallback where CPython raises or warns), the module
functions against an unpatched clone of `re`, mechanics, threads."""

from __future__ import annotations

import ast
import copy
import gzip
import json
import os
import pickle
import re
import sys
import threading
import types
import warnings
from re import _compiler, _constants, _parser
from unittest import mock

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel
HERE = os.path.dirname(os.path.abspath(__file__))
C = _constants

FLAG_COMBOS = [0, C.SRE_FLAG_IGNORECASE, C.SRE_FLAG_MULTILINE | C.SRE_FLAG_DOTALL, C.SRE_FLAG_VERBOSE,
               C.SRE_FLAG_ASCII, C.SRE_FLAG_IGNORECASE | C.SRE_FLAG_ASCII, C.SRE_FLAG_UNICODE | C.SRE_FLAG_IGNORECASE,
               C.SRE_FLAG_VERBOSE | C.SRE_FLAG_IGNORECASE | C.SRE_FLAG_MULTILINE]
NAMES = {"A": 256, "ASCII": 256, "I": 2, "IGNORECASE": 2, "M": 8, "MULTILINE": 8, "S": 16, "DOTALL": 16,
         "U": 32, "UNICODE": 32, "X": 64, "VERBOSE": 64}


@pytest.fixture(scope="module", autouse=True)
def enabled():
    rt.enable(only="re")
    bad = [t for t in rt.status()["targets"] if t["group"] == "re" and t["state"] != "installed"]
    assert not bad, bad
    rt.stats(True)
    yield
    A.disable([t["name"] for t in rt.status()["targets"] if t["group"] == "re"])


# ---------------------------------------------------------------------------------------
# The compiler: CPython's `compile` code, run unpatched, with `_sre` replaced by a recorder
# ---------------------------------------------------------------------------------------

class _Recorder:
    def __init__(self):
        self.args = None

    def __getattr__(self, name):
        import _sre
        return getattr(_sre, name)

    def compile(self, *args):
        self.args = args
        import _sre
        return _sre.compile(*args)


def reference(p, flags):
    """CPython's `re._compiler.compile(p, flags)` (the original code object, fresh function,
    `_sre` recorded): ("ok", args) / ("raise", type, str) / ("warn", [messages])."""
    rec = _Recorder()
    g = dict(_compiler.__dict__)
    g["_sre"] = rec
    f = types.FunctionType(_compiler.compile.__code__, g, "compile", _compiler.compile.__defaults__)
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        try:
            f(p, flags)
        except Exception as e:  # noqa: BLE001 - compared below
            return ("raise", type(e), str(e))
    if w:
        return ("warn", [str(x.message) for x in w])
    return ("ok", rec.args)


def normalize(args):
    p, flags, code, groups, groupindex, indexgroup = args
    return (p, int(flags), [int(x) for x in code], int(groups), list(groupindex.items()), indexgroup)


def check_pattern(p, flags):
    ours = A._re_compile_args(p, flags)
    ref = reference(p, flags)
    if ours is None:
        return "declined"
    assert ref[0] == "ok", (p, flags, ref)
    a, b = normalize(ours), normalize(ref[1])
    assert a == b, (p, flags, a, b)
    assert ours[0] is p
    assert type(ours[2]) is list and type(ours[4]) is dict and type(ours[5]) is tuple
    # indexgroup holds the very key objects of groupindex
    for k, i in ours[4].items():
        assert ours[5][i] is k
    return "same"


def test_known_patterns():
    for p in ["", "abc", r"(?P<major>[0-9]+)\.(?P<minor>[0-9]+)", r"[^.\[\]]+(?![^\[]*\])", r"\[(.*)\]$",
              "(?i)[a-z]+", "[Ā-̀]+", "(?x) a b # c", r"\bfoo\b", r"(a)(?(1)b|c)", "(?>a+)b", "a++",
              b"[\x80-\xff]+", b"(?i)abc", r"(?<=\d{3})x", "[\\s\\S]", "[^\\W\\d]"]:
        for fl in FLAG_COMBOS:
            if isinstance(p, bytes) and fl & C.SRE_FLAG_UNICODE:
                continue
            check_pattern(p, fl)


def test_declines():
    for p, fl in [("a", C.SRE_FLAG_DEBUG), ("\\N{EM DASH}", 0), ("[[a]", 0), ("[a--b]", 0), ("(a", 0),
                  (b"(?L)a", 0), (b"a", C.SRE_FLAG_LOCALE), ("(?P<n\xe9>x)", 0), ("a", 1), ("x{99999999999}", 0)]:
        assert A._re_compile_args(p, fl) is None, (p, fl)
    # exact types only
    class S(str):
        pass
    assert A._re_compile_args(S("a"), 0) is None
    assert A._re_compile_args("a", True) is None
    assert A._re_compile_args("a", re.I) is None  # RegexFlag: _compile passes ints


def corpus():
    with gzip.open(os.path.join(HERE, "data", "re_pypi_corpus.json.gz"), "rt") as f:
        rows = json.load(f)
    for r in rows:
        p = r["p"]
        if r["b"]:
            try:
                p = p.encode("latin-1")
            except UnicodeEncodeError:
                continue
        fl = 0
        for name in r["f"]:
            fl |= NAMES.get(name, 0)
        yield p, fl


def test_pypi_corpus():
    """8,522 distinct patterns from popular PyPI packages (wave-1 static scan), with their
    own flags and with every flag combination."""
    n = same = 0
    for p, fl in corpus():
        for f in {fl, *FLAG_COMBOS}:
            if isinstance(p, bytes) and f & C.SRE_FLAG_UNICODE:
                continue
            n += 1
            same += check_pattern(p, f) == "same"
    assert n > 60000
    assert same / n > 0.9, (same, n)


def _source_constants(path, only_re_calls):
    try:
        with open(path, encoding="utf-8") as f, warnings.catch_warnings():
            warnings.simplefilter("ignore")
            tree = ast.parse(f.read())
    except (SyntaxError, UnicodeDecodeError, ValueError):
        return
    fns = {"compile", "match", "fullmatch", "search", "sub", "subn", "split", "findall", "finditer", "prefixmatch"}
    for node in ast.walk(tree):
        if only_re_calls:
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr in fns
                    and node.args and isinstance(node.args[0], ast.Constant)
                    and isinstance(node.args[0].value, (str, bytes))):
                yield node.args[0].value
        elif isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes)) and len(node.value) < 2000:
            yield node.value


def cpython_src():
    src = os.environ.get("CPYTHON_SRC")
    if not src or not os.path.isdir(os.path.join(src, "Lib", "test")):
        pytest.skip("CPYTHON_SRC not set")
    return src


def test_cpython_test_re_constants():
    """Every str/bytes constant in CPython's test_re.py and re_tests.py, as a pattern."""
    src = cpython_src()
    pats = set()
    for name in ("test_re.py", "re_tests.py"):
        pats.update(_source_constants(os.path.join(src, "Lib", "test", name), only_re_calls=False))
    assert len(pats) > 1000
    for p in pats:
        for f in (0, C.SRE_FLAG_IGNORECASE, C.SRE_FLAG_VERBOSE, C.SRE_FLAG_ASCII | C.SRE_FLAG_MULTILINE):
            check_pattern(p, f)


def test_stdlib_patterns():
    """Every literal pattern passed to a `re` function in the standard library."""
    src = cpython_src()
    pats = set()
    for root, _dirs, files in os.walk(os.path.join(src, "Lib")):
        for fn in files:
            if fn.endswith(".py"):
                pats.update(_source_constants(os.path.join(root, fn), only_re_calls=True))
    assert len(pats) > 500
    for p in pats:
        for f in (0, C.SRE_FLAG_IGNORECASE):
            check_pattern(p, f)


# Hypothesis: patterns built from regex tokens
TOKENS = ["a", "b", "z", "A", "Z", "0", "9", "_", "-", " ", "\n", "#", ".", "^", "$", "*", "+", "?", "*?", "+?",
          "*+", "{2}", "{1,3}", "{,2}", "{2,}", "{", "}", "|", "(", ")", "(?:", "(?P<g1>", "(?P<g2>", "(?P=g1)",
          "(?=", "(?!", "(?<=", "(?<!", "(?>", "(?#c)", "(?i)", "(?x)", "(?m)", "(?s)", "(?a)", "(?u)", "(?i:",
          "(?-i:", "(?x:", "(?a:", "(?(1)", "(?(g1)", "[", "]", "[^", "a-z", "0-9", "\\", "\\d", "\\D", "\\w",
          "\\W", "\\s", "\\S", "\\b", "\\B", "\\A", "\\Z", "\\z", "\\1", "\\2", "\\x41", "\\u00e9", "\\U0001f600",
          "\\0", "\\012", "\\n", "\\t", "\\.", "\\[", "\\]", "\\-", "\\q", "é", "İ", "ı", "ſ",
          "K", "k", "s", "i", "Σ", "σ", "ẞ", "\U00010400", "\U0001f600", "你", "--", "&&",
          "~~", "||", "[[", "\\N{EM DASH}"]
BTOKENS = [t.encode("latin-1") for t in TOKENS if all(ord(c) < 256 for c in t)] + [b"\x80", b"\xff", b"\xe9",
                                                                                     b"(?L)"]
flag_values = st.sampled_from(FLAG_COMBOS + [C.SRE_FLAG_DOTALL, C.SRE_FLAG_UNICODE])


@settings(max_examples=6000, deadline=None, suppress_health_check=[HealthCheck.too_slow])
@given(st.lists(st.sampled_from(TOKENS), max_size=14).map("".join), flag_values)
def test_hypothesis_str(p, flags):
    check_pattern(p, flags)


@settings(max_examples=3000, deadline=None, suppress_health_check=[HealthCheck.too_slow])
@given(st.lists(st.sampled_from(BTOKENS), max_size=14).map(b"".join),
       st.sampled_from([f for f in FLAG_COMBOS if not f & C.SRE_FLAG_UNICODE]))
def test_hypothesis_bytes(p, flags):
    check_pattern(p, flags)


@settings(max_examples=1500, deadline=None)
@given(st.text(max_size=12), flag_values)
def test_hypothesis_any_text(p, flags):
    check_pattern(p, flags)


@settings(max_examples=800, deadline=None)
@given(st.lists(st.sampled_from(["[", "]", "^", "-", "a", "z", "\\d", "\\w", "\\S", "Ā", "ſ", "K",
                                 "\U00010400", "￿", "\x00", "\\x7f", "\\u0400", "\\U00010428"]),
                min_size=1, max_size=10).map(lambda t: "[" + "".join(t) + "]"), flag_values)
def test_hypothesis_sets(p, flags):
    check_pattern(p, flags)


def test_large_and_unicode_sets():
    import random
    rnd = random.Random(7)
    for _ in range(300):
        chars = "".join(chr(rnd.choice([rnd.randrange(0x80, 0x800), rnd.randrange(0x800, 0x10000),
                                        rnd.randrange(0x10000, 0x110000), rnd.randrange(0x20, 0x7f)]))
                        for _ in range(rnd.randrange(1, 40)))
        chars = chars.replace("\\", "").replace("]", "").replace("[", "").replace("-", "")
        for neg in ("", "^"):
            for f in (0, C.SRE_FLAG_IGNORECASE):
                check_pattern("[" + neg + chars + "]", f)
                check_pattern("[" + neg + "a-" + chars[:1] + chars + "]", f) if chars and chars[0] >= "a" else None


# ---------------------------------------------------------------------------------------
# The compile kernel in place
# ---------------------------------------------------------------------------------------

def hits(name):
    return rt.stats()[name]["hits"]


def rearm():
    """Fallback-heavy tests suspend the module-function targets (net-benefit suspension,
    re-armed by enable()); re-arm and reset the counters before counting hits."""
    rt.enable(only="re")
    rt.stats(reset=True)


def test_kernel_is_used_and_patterns_equal():
    re.purge()
    before = hits("re._compiler.compile")
    p = re.compile(r"(?P<k>\w+)=(?P<v>[^;]*)", re.I)
    assert hits("re._compiler.compile") == before + 1
    ref = reference(p.pattern, p.flags & ~C.SRE_FLAG_UNICODE)
    assert ref[0] == "ok"
    import _sre
    q = _sre.compile(*ref[1])
    assert p == q and p.groupindex == q.groupindex and p.groups == q.groups
    assert list(p.groupindex) == ["k", "v"]
    assert p.match("a=1;").groupdict() == {"k": "a", "v": "1"}
    assert re.compile(r"(?P<k>\w+)=(?P<v>[^;]*)", re.I) is p  # cached
    re.purge()
    assert re.compile(r"(?P<k>\w+)=(?P<v>[^;]*)", re.I) is not p


def test_errors_and_warnings_are_cpythons():
    for p in ["(", "[a", "a{3,2}", "(?P<1>x)", "x**", "\\", "(?<=a*)b", "(?P=nope)"]:
        re.purge()
        with pytest.raises(re.error) as ei:
            re.compile(p)
        tb = ei.traceback
        assert any(e.frame.code.raw.co_name == "compile" and "_compiler" in str(e.path) for e in tb), p
        ref = reference(p, 0)
        assert ref[0] == "raise" and type(ei.value) is ref[1] and str(ei.value) == ref[2], (p, ref)
    with pytest.warns(FutureWarning, match="Possible nested set"):
        re.purge()
        re.compile("[[a]")
    with pytest.raises(OverflowError):
        re.compile("x{99999999999}")


def test_debug_flag_prints(capsys):
    re.purge()
    re.compile("ab", re.DEBUG)
    out = capsys.readouterr().out
    assert "LITERAL 97" in out


def test_compile_called_directly():
    p = _compiler.compile("a+b", 0)
    assert p.match("aab") and p.pattern == "a+b"
    # a parsed SubPattern (re.Scanner's way) falls back
    sp = _parser.parse("a|b", 0)
    assert _compiler.compile(sp, 0).match("b")
    sc = re.Scanner([(r"\d+", lambda s, t: int(t)), (r"\s+", None)])
    assert sc.scan("1 22 333") == ([1, 22, 333], "")


def test_dependency_patch_falls_back():
    calls = []
    import _sre
    real = _sre.compile

    def spy(*a):
        calls.append(a)
        return real(*a)

    with mock.patch.object(_sre, "compile", spy):
        re.purge()
        re.compile("abc+")
    assert calls  # CPython's compile ran (reads _sre.compile at call time)
    re.purge()
    before = hits("re._compiler.compile")
    re.compile("abc+")
    assert hits("re._compiler.compile") == before + 1  # re-armed


def test_tools_see_cpython_frames():
    seen = []

    def tracer(frame, event, arg):
        if event == "call":
            seen.append(frame.f_code.co_name)
        return None

    re.purge()
    sys.settrace(tracer)
    try:
        re.compile("x(y)z")
    finally:
        sys.settrace(None)
    assert "_parse_sub" in seen and "_compile_info" in seen


def test_introspection_unchanged():
    f = _compiler.compile
    assert f.__name__ == "compile" and f.__module__ == "re._compiler"
    import inspect
    assert str(inspect.signature(f)) == "(p, flags=0)"
    assert pickle.loads(pickle.dumps(re.compile("ab(c)"))).pattern == "ab(c)"
    assert copy.deepcopy(re.compile("x")) is re.compile("x")


# ---------------------------------------------------------------------------------------
# Module functions against an unpatched clone of `re`
# ---------------------------------------------------------------------------------------

def _clone(fn):
    return types.FunctionType(fn.__code__, fn.__globals__, fn.__name__, fn.__defaults__, fn.__closure__)


REF = {}
for _n in ("match", "fullmatch", "search", "sub", "subn", "split", "findall", "finditer", "compile"):
    _f = getattr(re, _n)
    _g = _clone(_f)
    _g.__kwdefaults__ = _f.__kwdefaults__
    REF[_n] = _g


def outcome(f, *args, **kw):
    try:
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            r = f(*args, **kw)
    except Exception as e:  # noqa: BLE001 - compared below
        return ("raise", type(e), str(e))
    if isinstance(r, re.Match):
        r = ("match", r.span(), r.groups(), r.groupdict(), r.lastindex, r.re, r.string is args[1])
    elif type(r).__name__ == "callable_iterator":
        r = ("iter", [(m.span(), m.groups()) for m in r])
    return ("ok", r, type(r), [str(x.message) for x in w])


PATTERNS = [r"a+", r"(\w+)@(\w+)", r"(?P<x>\d+)-(?P<y>\d+)", r"\s*,\s*", "x*", "(?i)hello", r"(a)|(b)", "^$", "",
            b"[a-z]+", b"(\\d+)", "é+", r"(?m)^\w+$"]
SUBJECTS = ["", "aaa", "bob@host and al@x", "12-34 5-6", "a , b,c", "HeLLo hello", "ab", "été",
            b"abc 123", b"", "line1\nline2", bytearray(b"xy 42")]
REPLS = ["-", "", r"<\1>", r"\g<0>", r"\g<x>", r"\2", r"\n", "\\", r"\q", b"#", br"\1", lambda m: "!", r"\g<y>-\g<x>"]


@settings(max_examples=3000, deadline=None)
@given(st.sampled_from(PATTERNS), st.sampled_from(SUBJECTS), st.sampled_from(REPLS),
       st.sampled_from([0, re.I, re.M, re.A, 2, None]), st.sampled_from([0, 1, 2, -1, None]))
def test_functions_against_clone(pattern, subject, repl, flags, count):
    if pattern in ("(?i)hello",) and flags == re.A:
        flags = re.I
    fk = {} if flags is None else {"flags": flags}
    for name in ("match", "fullmatch", "search", "findall", "finditer"):
        assert outcome(getattr(re, name), pattern, subject, **fk) == outcome(REF[name], pattern, subject, **fk)
    ck = dict(fk, **({} if count is None else {"count": count}))
    for name in ("sub", "subn"):
        assert outcome(getattr(re, name), pattern, repl, subject, **ck) == outcome(REF[name], pattern, repl, subject,
                                                                                 **ck)
    sk = dict(fk, **({} if count is None else {"maxsplit": count}))
    assert outcome(re.split, pattern, subject, **sk) == outcome(REF["split"], pattern, subject, **sk)
    assert outcome(re.compile, pattern, **fk) == outcome(REF["compile"], pattern, **fk)


def test_functions_hit():
    re.purge()
    rearm()
    re.compile(r"(\d+)")
    re.sub(r"(\d+)", r"<\1>", "a12")  # the first template sub of a process runs the original
    before = {n: hits(n) for n in ("re.match", "re.search", "re.sub", "re.findall", "re.split", "re.compile")}
    assert re.match(r"(\d+)", "12a").group(1) == "12"
    assert re.search(r"(\d+)", "a12").span() == (1, 3)
    assert re.sub(r"(\d+)", r"<\1>", "a12") == "a<12>"
    assert re.findall(r"(\d+)", "1 2") == ["1", "2"]
    assert re.split(r"(\d+)", "a1b") == ["a", "1", "b"]
    assert re.compile(r"(\d+)") is re.compile(r"(\d+)")
    for n, b in before.items():
        assert hits(n) > b, n


def test_fallbacks_are_cpythons():
    # callable replacement: user code sees re.sub's frame
    frames = []

    def repl(m):
        frames.append(sys._getframe(1).f_code.co_name)
        return "x"

    assert re.sub("a", repl, "banana") == "bxnxnx"
    assert frames and frames[0] == "sub"
    # positional count warns from re.sub
    with pytest.warns(DeprecationWarning):
        assert re.sub("a", "b", "aaa", 1) == "baa"
    # type mismatch raises from re.match's frame
    with pytest.raises(TypeError) as ei:
        re.match("a", b"a")
    assert any(e.frame.code.raw.co_name in ("match", "prefixmatch") for e in ei.traceback)
    # compiled pattern with flags
    with pytest.raises(ValueError):
        re.match(re.compile("a"), "a", re.I)
    assert re.match(re.compile("a"), "abc").span() == (0, 1)
    # bad template raises from re.sub's frame
    with pytest.raises(re.error):
        re.sub("(a)", r"\2", "a")


def test_captured_references():
    from re import search as s0
    m = s0(r"b+", "abbc")
    assert m.span() == (1, 3)
    import functools
    p = functools.partial(re.findall, r"\d")
    assert p("a1b2") == ["1", "2"]


def test_purge_clears_caches():
    p = re.compile("purge-me")
    assert re.search("purge-me", "x purge-me") is not None
    re.purge()
    assert re.compile("purge-me") is not p
    assert re._cache2 and re._cache  # CPython's own caches are used


def test_mock_patch_of_cache_and_compile():
    seen = []
    real = re._compile

    def spy(p, f):
        seen.append(p)
        return real(p, f)

    with mock.patch.object(re, "_compile", spy):
        assert re.match("a", "a")
    assert seen == ["a"]
    rearm()
    assert re.match("a", "a")
    assert hits("re.match") == 1


def test_threads():
    errors = []
    pats = [r"(\d+)-(%d)" % i for i in range(40)] + [r"(?i)[a-z]{%d}" % i for i in range(1, 30)]

    def work(k):
        try:
            for j in range(300):
                p = pats[(j * 7 + k) % len(pats)]
                if j % 50 == 0:
                    re.purge()
                s = "12-%d abcdefghijklmnopqrstuvwxyz" % ((j * 7 + k) % 40)
                r1 = re.search(p, s)
                r2 = REF["search"](p, s)
                assert (r1 and r1.span()) == (r2 and r2.span()), p
                c = re.compile(p)
                assert c.pattern == p
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    ts = [threading.Thread(target=work, args=(k,)) for k in range(8)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert not errors, errors[:3]


def test_disable_restores():
    names = [t["name"] for t in rt.status()["targets"] if t["group"] == "re"]
    A.disable(names)
    try:
        assert all(t["state"] == "disabled" for t in rt.status()["targets"] if t["group"] == "re")
        assert re.match("a", "a")
        re.purge()
        assert re.compile("(x)").groups == 1
    finally:
        rt.enable(only="re")
    assert all(t["state"] == "installed" for t in rt.status()["targets"] if t["group"] == "re")


TEMPLATE_TOKENS = ["a", "-", "\\", "\\1", "\\2", "\\9", "\\10", "\\0", "\\01", "\\012", "\\177", "\\400", "\\8",
                   "\\g<0>", "\\g<1>", "\\g<3>", "\\g<x>", "\\g<y>", "\\g<nope>", "\\g<1", "\\g", "\\g<>", "\\g< x>",
                   "\\g<\u00e9>", "\\n", "\\t", "\\b", "\\q", "\\Q", "\\-", "\\\\", "\\\u00e9", "<", ">", "\u00e9"]
TEMPLATE_PATTERNS = [re.compile(r"(?P<x>a)(b)"), re.compile("plain"), re.compile(r"(a)(b)(c)(?P<y>d)"),
                     re.compile(rb"(?P<x>a)(b)")]


def _template_reference(p, repl):
    try:
        _parser.parse_template(repl, p)
    except Exception:  # noqa: BLE001 - any error: the port must say no
        return False
    return True


@settings(max_examples=4000, deadline=None)
@given(st.sampled_from(TEMPLATE_PATTERNS), st.lists(st.sampled_from(TEMPLATE_TOKENS), max_size=6).map("".join))
def test_template_check_against_parse_template(p, repl):
    if isinstance(p.pattern, bytes):
        try:
            repl = repl.encode("latin-1")
        except UnicodeEncodeError:
            return
    ours = A._re_template_ok(p, repl)
    if ours:
        assert _template_reference(p, repl), (p, repl)
    elif "\u00e9" not in (repl if isinstance(repl, str) else repl.decode("latin-1")):
        # declines only where CPython raises (or for non-ASCII group names)
        assert not _template_reference(p, repl) or "\\g<\u00e9" in str(repl), (p, repl)


def test_flag_operations_keep_regexflag_unmodified():
    """Composite flags create pseudo-members at run time; the class must not count as
    modified (its version tag guards the `flags.value` read)."""
    rearm()
    flags = [re.I | re.M, re.RegexFlag(10), ~re.I & re.M, re.I ^ re.S, re.X | re.A | re.I, re.RegexFlag(2 | 64 | 8)]
    for f in flags:
        assert re.search("(?:a)", "A", f | re.I)
    assert re.search("x", "X", re.I | re.M)  # compiled (a miss)
    before = hits("re.search")
    assert re.search("x", "X", re.I | re.M)
    assert hits("re.search") == before + 1


def test_first_template_sub_runs_the_original():
    """`_sre` resolves `re._compile_template` (through the calling frame's `__import__`) on the
    first template `sub` of the process: that one runs the original (subprocess: fresh `_sre`)."""
    import subprocess
    code = (
        "import builtins, re, rustique\n"
        "rustique.enable(only=['re']); rustique.stats(True)\n"
        "seen = []\n"
        "real = builtins.__import__\n"
        "def imp(name, *a, **k):\n"
        "    seen.append((name, __import__('sys')._getframe(1).f_code.co_name))\n"
        "    return real(name, *a, **k)\n"
        "re.compile('(a)')\n"
        "builtins.__import__ = imp\n"
        "try:\n"
        "    r1 = re.sub('(a)', r'<\\1>', 'xa')\n"
        "finally:\n"
        "    builtins.__import__ = real\n"
        "r2 = re.sub('(a)', r'<\\1>', 'xa')\n"
        "print(r1, r2, seen, rustique.stats()['re.sub']['hits'])\n"
    )
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True).stdout.split()
    assert out[0] == out[1] == "x<a>"
    assert "('re'," in " ".join(out) and "'sub')" in " ".join(out)  # imported under re.sub's frame
    assert out[-1] == "1"  # the second one was accelerated
