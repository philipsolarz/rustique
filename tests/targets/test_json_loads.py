"""Target `json.loads`: differential tests against CPython and mechanism tests.

CPython's own path stays reachable while the kernel is installed: `json.loads` without
keywords runs `json._default_decoder.decode(s)` (after decoding bytes), which is not
patched. Results are compared deeply: types, float bits, dict order, list/dict sizes
(`sys.getsizeof`: same growth), string kinds, single-character singletons and the
identity pattern of keys (the scanner memoizes repeated keys).
"""

from __future__ import annotations

import json
import math
import pickle
import inspect
import random
import struct
import sys
import threading
from unittest import mock

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")
from rustique import _takeover as _tk  # noqa: E402

NAME = "json.loads"
loads_captured_before_enable = json.loads


def status():
    return {d["name"]: d for d in _tk.status()["targets"]}[NAME]


def install():
    rustique.enable(only=["json", "float"])
    assert status()["state"] == "installed", status()


def counters():
    return _tk.stats()[NAME]


@pytest.fixture(autouse=True)
def installed():
    install()
    _tk.stats(enabled=True, reset=True)
    yield
    _tk.stats(enabled=False)


def cpython(s):
    if isinstance(s, (bytes, bytearray)):
        s = s.decode(json.detect_encoding(s), "surrogatepass")
    elif isinstance(s, str) and s.startswith("\ufeff"):
        return json.loads(s)  # raises before decoding (both paths: the original)
    return json._default_decoder.decode(s)


def same(a, b, keys_a, keys_b):
    assert type(a) is type(b), (a, b)
    if type(a) is float:
        assert struct.pack("<d", a) == struct.pack("<d", b) or (math.isnan(a) and math.isnan(b))
    elif type(a) is str:
        assert a == b and sys.getsizeof(a) == sys.getsizeof(b)
        if len(a) == 1 and ord(a) < 256 or len(a) == 0:
            assert a is b  # cached singletons
    elif type(a) is list:
        assert len(a) == len(b) and sys.getsizeof(a) == sys.getsizeof(b)
        for x, y in zip(a, b):
            same(x, y, keys_a, keys_b)
    elif type(a) is dict:
        assert list(a) == list(b) and sys.getsizeof(a) == sys.getsizeof(b)
        for (ka, va), (kb, vb) in zip(a.items(), b.items()):
            same(ka, kb, keys_a, keys_b)
            keys_a.append(ka)
            keys_b.append(kb)
            same(va, vb, keys_a, keys_b)
    else:
        assert a == b and (a is b) == (a is b)
        if type(a) is int and -5 <= a <= 256:
            assert a is b


def identity_pattern(keys):
    first = {}
    return [first.setdefault(id(k), i) for i, k in enumerate(keys)]


def outcome(f, s):
    try:
        return ("ok", f(s))
    except Exception as e:  # noqa: BLE001
        d = (type(e), str(e))
        if isinstance(e, json.JSONDecodeError):
            d += (e.msg, e.pos, e.lineno, e.colno, e.doc is s or e.doc == s)
        return ("raise", d)


def check(s):
    a, b = outcome(json.loads, s), outcome(cpython, s)
    assert a[0] == b[0], (s, a, b)
    if a[0] == "raise":
        assert a[1] == b[1], (s, a, b)
        return a
    ka, kb = [], []
    same(a[1], b[1], ka, kb)
    assert identity_pattern(ka) == identity_pattern(kb)
    return a


EXAMPLES = [
    "null", "true", "false", "0", "-0", "7", "-7", "256", "257", "-5", "-6", "123456789012345678",
    "1234567890123456789", "-9223372036854775808", "18446744073709551616", "1" * 400,
    "0.0", "-0.0", "1.5", "1e5", "1E+5", "1e-5", "2.5e-3", "1e400", "-1e400", "1e-400", "0.1", "1." + "1" * 100,
    "123456789012345678901234567890.5e-10",
    '""', '"a"', '"\\u00e9"', '"\\u0041"', '"x"', '"\\n"', '"\\ud83d\\ude00"', '"\\ud83d"', '"\\udc00"',
    '"\\ud83d\\u0041"', '"\\ud83dx"', '"\\/\\b\\f\\n\\r\\t\\"\\\\"', '"\xe9"', '"\u20ac"', '"\U0001F600"',
    "[]", "{}", "[[]]", '{"": ""}', ' [ 1 , 2 ] ', '\n\t{ "a" : [ 1 , { "b" : null } ] }\r\n',
    '{"a": 1, "a": 2}', '{"k": {"k": {"k": 1}}, "k2": [{"k": 2}]}', '{"\\u00e9": 1, "\xe9": 2}',
    '[1, 2.5, "x", true, false, null, [], {}]', '{"a": [1, {"b": [2, {"c": "d"}]}]}',
    # Errors: CPython's JSONDecodeError (or ValueError) through the original.
    "", " ", "[", "]", "[1,]", "{,}", '{"a":1,}', '{"a" 1}', '{a: 1}', "[1 2]", "01", "1.", "-", "+1", ".5",
    "nul", "tru", "NaN", "Infinity", "-Infinity", "[NaN]", '"abc', '"\\x"', '"\\u12"', '"\\u12g4"',
    '"a\nb"', '"a\x00b"', "[1] x", "{} {}", "\ufeff[1]", "1" * 5000, '"\\ud83d\\uZZZZ"',
]


@pytest.mark.parametrize("s", EXAMPLES, ids=range(len(EXAMPLES)))
def test_examples(s):
    check(s)
    check(s.encode("utf-8", "surrogatepass"))


@pytest.mark.parametrize(
    "b",
    [b"[1]", b"\xef\xbb\xbf[1]", "[1]".encode("utf-16"), "[1]".encode("utf-16-le"), "[1]".encode("utf-32"),
     b'"\xed\xa0\x80"', b'"\xff"', b"\x00[1]", b'["\xc3\xa9"]', b"", b"7", b"\x001", b"1\x00"],
)
def test_bytes_encodings(b):
    check(b)


def test_other_input_types():
    for s in [bytearray(b"[1]"), 1, None, memoryview(b"[1]")]:
        a, b = outcome(json.loads, s), outcome(lambda x: json.loads.__wrapped__(x) if False else cpython(x), s)
        assert a[0] == b[0]

    class S(str):
        pass

    assert json.loads(S('{"a": 1}')) == {"a": 1}


def test_hits_and_fallbacks():
    _tk.stats(reset=True)
    assert json.loads('{"a": [1, 2]}') == {"a": [1, 2]}
    assert json.loads(b"[1]") == [1]
    assert json.loads("[1]", object_hook=None, cls=None) == [1]
    assert counters()["hits"] == 3
    before = counters()["fallback"]
    json.loads("[1]", object_hook=dict)
    json.loads("[NaN]")
    with pytest.raises(json.JSONDecodeError):
        json.loads("[1,]")
    after = counters()["fallback"]
    assert after["args"] == before["args"] + 1
    assert after["deopt"] == before["deopt"] + 2


def test_key_memo_identity():
    d = json.loads('[{"id": 1, "name": "a"}, {"id": 2, "name": "b"}, {"\\u0069d": 3}]')
    ks = [k for o in d for k in o]
    assert ks[0] is ks[2] is ks[4]
    assert ks[1] is ks[3]
    # Values are not memoized.
    v = json.loads('["abc", "abc"]')
    assert v[0] == v[1] and v[0] is not v[1]


def test_int_max_str_digits():
    old = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(0)
        assert check("1" * 5000)[0] == "ok"
        sys.set_int_max_str_digits(4300)
        assert check("1" * 5000)[0] == "raise"
        assert check("[" + "1" * 4300 + "]")[0] == "ok"
    finally:
        sys.set_int_max_str_digits(old)


def test_deep_nesting():
    for depth in (10, 500, 100_000):
        s = "[" * depth + "]" * depth
        a, b = outcome(json.loads, s), outcome(cpython, s)
        assert a[0] == b[0]
        if a[0] == "raise":
            assert a[1][0] is b[1][0] is RecursionError
        else:
            same(a[1], b[1], [], [])


# ---------------------------------------------------------------------------------------
# Hypothesis differential
# ---------------------------------------------------------------------------------------

SURR = st.sampled_from(["\ud800", "\udfff", "\ud83d\ude00", "\x7f", "\x00", "\u2028", "\xe9", "\u20ac"])
TEXT = st.one_of(st.text(max_size=12), SURR, st.text(alphabet="ab\"\\\n\t\xe9\u20ac\U0001F600", max_size=8))
FLOATS = st.one_of(st.floats(allow_nan=False, allow_infinity=False), st.sampled_from([0.0, -0.0, 5e-324, 1e308]))
SCALARS = st.one_of(st.none(), st.booleans(), st.integers(), st.integers(-(2**200), 2**200), FLOATS, TEXT)
VALUES = st.recursive(
    SCALARS,
    lambda c: st.one_of(st.lists(c, max_size=6), st.dictionaries(TEXT, c, max_size=6)),
    max_leaves=40,
)
DUMP_KW = st.fixed_dictionaries(
    {},
    optional={
        "ensure_ascii": st.booleans(),
        "indent": st.one_of(st.none(), st.integers(0, 4), st.sampled_from(["\t", " \r\n"])),
        "separators": st.sampled_from([(",", ":"), (", ", ": "), (" ,", " : "), ("\n,", "\t:")]),
    },
)
HS = settings(max_examples=1500, suppress_health_check=[HealthCheck.too_slow, HealthCheck.data_too_large])


@given(VALUES, DUMP_KW)
@HS
def test_hypothesis_roundtrip_documents(v, kw):
    s = json.JSONEncoder(**kw).encode(v)
    check(s)
    check(s.encode("utf-8", "surrogatepass"))


@given(VALUES, DUMP_KW, st.data())
@HS
def test_hypothesis_mutated_documents(v, kw, data):
    s = json.JSONEncoder(**kw).encode(v)
    chars = list(s)
    for _ in range(data.draw(st.integers(1, 3))):
        op = data.draw(st.sampled_from(["del", "ins", "rep"]))
        i = data.draw(st.integers(0, len(chars)))
        c = data.draw(st.sampled_from(list('{}[],:"\\ 0123456789.eE+-truefalsnNaI\x00\n\ufeff\xe9')))
        if op == "del" and chars:
            del chars[min(i, len(chars) - 1)]
        elif op == "ins":
            chars.insert(i, c)
        elif chars:
            chars[min(i, len(chars) - 1)] = c
    m = "".join(chars)
    check(m)
    check(m.encode("utf-8", "surrogatepass"))


@given(st.text(alphabet='{}[],:" \t\n\\u0123456789abcdefABCDEF.eE+-truefalsn', max_size=40))
@settings(max_examples=2000)
def test_hypothesis_random_text(s):
    check(s)


@given(st.from_regex(r"-?(0|[1-9][0-9]{0,30})(\.[0-9]{1,30})?([eE][+-]?[0-9]{1,4})?", fullmatch=True))
@settings(max_examples=3000)
def test_hypothesis_numbers(s):
    check(s)
    check(f"[{s}]")


@given(VALUES)
@settings(max_examples=300, suppress_health_check=[HealthCheck.too_slow])
def test_check_mode_agrees(v):
    prev = _tk.check("raise")
    try:
        json.loads(json.JSONEncoder().encode(v))
    finally:
        _tk.check(prev)
    assert counters()["fallback"]["check"] == 0


def test_float_parsing_matches_float():
    rng = random.Random(1234)
    for _ in range(20000):
        digits = "".join(rng.choice("0123456789") for _ in range(rng.randint(1, 25)))
        frac = "".join(rng.choice("0123456789") for _ in range(rng.randint(0, 25)))
        e = rng.randint(-340, 320)
        s = f"{digits.lstrip('0') or '0'}{'.' + frac if frac else ''}e{e}"
        assert struct.pack("<d", json.loads(s)) == struct.pack("<d", float(s)), s


# ---------------------------------------------------------------------------------------
# Mechanics
# ---------------------------------------------------------------------------------------


def test_captured_references_are_accelerated():
    from json import loads as l2

    _tk.stats(reset=True)
    assert loads_captured_before_enable("[1]") == [1]
    assert l2("[2]") == [2]
    assert counters()["hits"] == 2


def test_introspection_unchanged():
    f = json.loads
    assert f.__name__ == "loads" and f.__module__ == "json"
    assert "object_hook=None" in str(inspect.signature(f))
    assert "def loads(" in inspect.getsource(f)
    assert pickle.loads(pickle.dumps(f)) is f


def test_tools_see_the_original():
    calls = []

    def prof(frame, event, arg):
        if event == "call" and frame.f_code.co_name in ("loads", "decode", "raw_decode"):
            calls.append(frame.f_code.co_name)

    _tk.stats(reset=True)
    sys.setprofile(prof)
    try:
        json.loads("[1]")
    finally:
        sys.setprofile(None)
    assert calls[:3] == ["loads", "decode", "raw_decode"]
    assert counters()["fallback"]["tools"] == 1


@pytest.mark.parametrize(
    "target,attr,value",
    [
        ("json", "_default_decoder", json.JSONDecoder(parse_float=str)),
        ("json", "detect_encoding", lambda b: "latin-1"),
        ("json.codecs", "BOM_UTF8", b"[1"),
        ("json.JSONDecoder", "decode", lambda self, s: "patched"),
        ("json.JSONDecoder", "raw_decode", lambda self, s, idx=0: ("patched", len(s))),
        ("json._default_decoder", "scan_once", lambda s, i: ("patched", len(s))),
        ("json._default_decoder", "decode", lambda s: "patched"),
        ("json.decoder", "len", lambda s: 0),
    ],
)
def test_dependency_patching_falls_back_and_rearms(target, attr, value):
    doc = '{"a": [1, 2.5, "x"]}'
    owner = eval(target, {"json": json})  # noqa: S307
    with mock.patch.object(owner, attr, value, create=True):
        want = outcome(lambda s: json._default_decoder.decode(s) if attr != "detect_encoding" else json.loads(s), doc)
        got = outcome(json.loads, doc)
        assert got[0] == want[0] and repr(got[1]) == repr(want[1]), (got, want)
        if attr in ("detect_encoding", "BOM_UTF8"):
            raw = doc.encode()
            assert outcome(json.loads, raw)[0] == outcome(lambda b: json.loads.__code__ and cpython_patched(b), raw)[0]
    assert status()["deps_intact"]
    _tk.stats(reset=True)
    assert json.loads(doc) == {"a": [1, 2.5, "x"]}
    assert counters()["hits"] == 1


def cpython_patched(b):
    return json._default_decoder.decode(b.decode(json.detect_encoding(b), "surrogatepass"))


def test_decode_defaults_patching():
    f = json.JSONDecoder.decode
    old = f.__defaults__
    import re

    f.__defaults__ = (re.compile(r"[ \t]*").match,)
    try:
        assert outcome(json.loads, "\n[1]")[0] == "raise"
    finally:
        f.__defaults__ = old
    assert json.loads("\n[1]") == [1]


def test_free_threaded_stress():
    docs = [json.dumps({"i": i, "xs": list(range(i % 7)), "f": i / 3, "s": "\xe9" * (i % 3)}) for i in range(64)]
    expected = [cpython(d) for d in docs]
    errors = []
    stop = threading.Event()

    def worker():
        while not stop.is_set():
            for d, e in zip(docs, expected):
                if json.loads(d) != e:
                    errors.append(d)

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for t in threads:
        t.start()
    try:
        for _ in range(50):
            rustique.disable()
            install()
    finally:
        stop.set()
        for t in threads:
            t.join()
    assert not errors
