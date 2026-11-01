"""Target `json.dumps`: differential tests against CPython and mechanism tests.

CPython's own path is reachable while the kernel is installed through
`json.JSONEncoder(**kw).encode(obj)` (what `json.dumps` runs; only `json.dumps` itself is
patched) and through the counters: a call either hits the kernel or falls back to the
original function.
"""

from __future__ import annotations

import collections
import decimal
import enum
import inspect
import json
import json.encoder
import math
import pickle
import sys
import threading
from unittest import mock

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")
from rustique import _takeover as _tk  # noqa: E402  (status/stats until the DX layer exports them)

NAME = "json.dumps"
dumps_captured_before_enable = json.dumps  # a reference taken before enable()


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


def outcome(f, *a, **kw):
    try:
        r = f(*a, **kw)
    except Exception as e:  # noqa: BLE001
        return ("raise", type(e), str(e), getattr(e, "__notes__", None))
    return ("ok", type(r), r, sys.getsizeof(r))


def cpython(obj, **kw):
    """What CPython's json.dumps computes (its own encoder path, not patched)."""
    if kw.get("cls") is None and not kw.get("default"):
        kw = {k: v for k, v in kw.items() if k not in ("cls", "default")}
    return json.JSONEncoder(**kw).encode(obj)


def check(obj, **kw):
    a = outcome(json.dumps, obj, **kw)
    b = outcome(cpython, obj, **kw)
    assert a == b, (obj, kw)
    return a


# ---------------------------------------------------------------------------------------
# Examples
# ---------------------------------------------------------------------------------------

EXAMPLES = [
    None, True, False, 0, 1, -1, 7, 255, 256, -5, 2**63 - 1, -(2**63), 2**64, -(2**100), 10**100,
    0.0, -0.0, 1.5, 0.1, 1e16, 1e-7, 1e300, 5e-324, math.inf, -math.inf, math.nan,
    "", "a", "abc", 'q"uote\\back/slash', "\n\r\t\b\f\x00\x1f\x7f", "\xe9t\xe9", "\u20ac", "\U0001F600",
    "\ud800", "\udfff", "a\ud83d\ude00b", "\x80\xff",
    [], {}, (), [[]], [{}], {"": ""}, {"a": [1, 2, {"b": None}]}, (1, "t", (2,)),
    {1: "int key", 2.5: "float key", True: "bool key", None: "none key"},
    {"\xe9": 1, "\u20ac": 2, "\U0001F600": 3},
    [1.0, 2.25, -3.5e-9, 123456789.123],
    {"nested": {"deep": [[[[[[1]]]]]]}},
]


@pytest.mark.parametrize("obj", EXAMPLES, ids=range(len(EXAMPLES)))
@pytest.mark.parametrize(
    "kw",
    [
        {},
        {"ensure_ascii": False},
        {"indent": 2},
        {"indent": 0},
        {"indent": -1},
        {"indent": "\t"},
        {"indent": "\u2192", "ensure_ascii": False},
        {"separators": (",", ":")},
        {"separators": [" , ", " : "], "indent": 1},
        {"sort_keys": True},
        {"allow_nan": False},
        {"skipkeys": True},
        {"check_circular": False},
        {"indent": True, "sort_keys": 1, "ensure_ascii": 0},
        {"cls": None, "default": None, "indent": None, "separators": None},
    ],
    ids=lambda kw: ",".join(kw) or "nokw",
)
def test_examples(obj, kw):
    check(obj, **kw)


def test_hits_and_identity():
    _tk.stats(reset=True)
    assert json.dumps({"a": 1}) == '{"a": 1}'
    assert counters()["hits"] == 1
    # One-character results are CPython's cached singletons, like `''.join` of the writer's.
    assert json.dumps(5) is json.JSONEncoder().encode(5) is "5"  # noqa: F632
    # Fresh strings otherwise.
    a, b = json.dumps([1]), json.dumps([1])
    assert a == b and a is not b and type(a) is str
    # Canonical kinds (same size as CPython's result).
    for obj in ["\xe9", "\u20ac", "\U0001F600", ["x", "\xe9"]]:
        for ea in (True, False):
            r = json.dumps(obj, ensure_ascii=ea)
            assert sys.getsizeof(r) == sys.getsizeof(cpython(obj, ensure_ascii=ea))
    assert counters()["hits"] >= 9


def test_fallbacks_are_counted():
    _tk.stats(reset=True)

    class D(dict):
        pass

    class Color(enum.IntEnum):
        RED = 1

    cases = [
        (lambda: json.dumps({"a": decimal.Decimal(1)}, default=str), "args"),
        (lambda: json.dumps(collections.OrderedDict(a=1)), "deopt"),
        (lambda: json.dumps(D(a=1)), "deopt"),
        (lambda: json.dumps({"a": {1, 2}}), "deopt"),
        (lambda: json.dumps([math.nan], allow_nan=False), "deopt"),
        (lambda: json.dumps({"a": 1, 2: 3}, sort_keys=True), "deopt"),
        (lambda: json.dumps([1], cls=json.JSONEncoder), "args"),
        (lambda: json.dumps([1], foo=1), "args"),
        (lambda: json.dumps(obj=[1]), "args"),
        (lambda: json.dumps([1], indent=10**6), "args"),
        (lambda: json.dumps([1], separators=(",", 1)), "args"),
    ]
    for f, reason in cases:
        before = dict(counters()["fallback"])
        outcome(f)
        after = counters()["fallback"]
        assert after[reason] == before[reason] + 1, (reason, after)
    # IntEnum and str subclasses are encoded by value by the C encoder: in the subset.
    assert json.dumps([Color.RED]) == "[1]"


def test_errors_are_cpythons():
    circular = [1]
    circular.append(circular)
    d = {}
    d["self"] = d
    for obj, kw in [
        (circular, {}),
        (d, {}),
        ({"k": {1, 2}}, {}),
        ([1, [2, {"x": object()}]], {}),
        ([math.nan], {"allow_nan": False}),
        ({math.inf: 1}, {"allow_nan": False}),
        ({(1, 2): 1}, {}),
        ({"a": 1, 2: 3}, {"sort_keys": True}),
        ([1], {"separators": (",",)}),
        ([10**5000], {}),
    ]:
        assert check(obj, **kw)[0] == "raise"
    deep = []
    for _ in range(100_000):
        deep = [deep]
    # RecursionError at the C stack limit: CPython's depth depends on the C stack in use
    # when it is called, and the fallback runs one C frame deeper, so only the type,
    # message and kind of notes are compared, not how many levels were noted.
    a, b = outcome(json.dumps, deep), outcome(cpython, deep)
    assert a[:3] == b[:3] and a[1] is RecursionError
    assert set(a[3]) == set(b[3]) == {"when serializing list item 0"}


def test_int_max_str_digits():
    big = 10**5000
    old = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(0)
        assert check([big])[0] == "ok"
        sys.set_int_max_str_digits(4300)
        assert check([big])[0] == "raise"
    finally:
        sys.set_int_max_str_digits(old)


# ---------------------------------------------------------------------------------------
# Hypothesis differential
# ---------------------------------------------------------------------------------------

SURROGATES = st.sampled_from(["\ud800", "\udbff", "\udc00", "\udfff", "\ud83d\ude00", "\x7f", "\x00", "\u2028"])
TEXT = st.one_of(st.text(), st.text(st.characters(codec="utf-8")), SURROGATES, st.text(alphabet="ab\"\\\n\t\xe9\u20ac"))
FLOATS = st.one_of(st.floats(), st.sampled_from([0.0, -0.0, 1e16, 1e-5, 1e-4, 5e-324, 1.7976931348623157e308]))
INTS = st.one_of(st.integers(), st.integers(-(2**200), 2**200))
SCALARS = st.one_of(st.none(), st.booleans(), INTS, FLOATS, TEXT)
KEYS = st.one_of(TEXT, INTS, FLOATS, st.booleans(), st.none())
JSONISH = st.recursive(
    SCALARS,
    lambda children: st.one_of(
        st.lists(children, max_size=6),
        st.tuples(children, children),
        st.dictionaries(TEXT, children, max_size=6),
        st.dictionaries(KEYS, children, max_size=4),
    ),
    max_leaves=40,
)
ODD = st.sampled_from([decimal.Decimal("1.5"), {1, 2}, object, b"bytes", collections.OrderedDict(a=1), 1j])
WITH_ODD = st.recursive(
    st.one_of(SCALARS, ODD),
    lambda children: st.one_of(st.lists(children, max_size=4), st.dictionaries(TEXT, children, max_size=4)),
    max_leaves=12,
)
FLAG = st.one_of(st.booleans(), st.none(), st.integers(-1, 2))
SEP = st.sampled_from([",", ", ", " , ", ";", "\u3001"])
KSEP = st.sampled_from([":", ": ", " : ", "="])
KWARGS = st.fixed_dictionaries(
    {},
    optional={
        "skipkeys": FLAG,
        "ensure_ascii": FLAG,
        "check_circular": FLAG,
        "allow_nan": FLAG,
        "sort_keys": FLAG,
        "indent": st.one_of(st.none(), st.integers(-2, 6), st.booleans(), st.sampled_from(["", "\t", "  ", "\u2192"])),
        "separators": st.one_of(st.none(), st.tuples(SEP, KSEP), st.lists(SEP, min_size=2, max_size=2)),
    },
)

HSETTINGS = settings(max_examples=1500, suppress_health_check=[HealthCheck.too_slow, HealthCheck.data_too_large])


@given(JSONISH, KWARGS)
@HSETTINGS
def test_hypothesis_jsonish(obj, kw):
    check(obj, **kw)


@given(WITH_ODD, KWARGS)
@settings(max_examples=500, suppress_health_check=[HealthCheck.too_slow])
def test_hypothesis_with_unsupported(obj, kw):
    check(obj, **kw)


@given(JSONISH)
@settings(max_examples=300, suppress_health_check=[HealthCheck.too_slow])
def test_check_mode_agrees(obj):
    prev = _tk.check("raise")
    try:
        outcome(json.dumps, obj)
        outcome(json.dumps, obj, indent=2, sort_keys=True, ensure_ascii=False)
    finally:
        _tk.check(prev)
    assert counters()["fallback"]["check"] == 0


# ---------------------------------------------------------------------------------------
# Mechanics
# ---------------------------------------------------------------------------------------


def test_captured_references_are_accelerated():
    from json import dumps as d2

    import functools

    p = functools.partial(json.dumps, indent=None)
    _tk.stats(reset=True)
    assert dumps_captured_before_enable([1]) == "[1]"
    assert d2([2]) == "[2]"
    assert p([3]) == "[3]"
    assert counters()["hits"] == 3


def test_introspection_unchanged():
    f = json.dumps
    assert f.__name__ == "dumps" and f.__module__ == "json" and f.__qualname__ == "dumps"
    assert "skipkeys=False" in str(inspect.signature(f))
    assert "def dumps(" in inspect.getsource(f)
    assert pickle.loads(pickle.dumps(f)) is f
    assert inspect.isfunction(f)
    assert f.__code__.co_name == "dumps"


def test_disable_restores():
    _tk.disable() if hasattr(_tk, "disable") else rustique.disable()
    try:
        _tk.stats(reset=True)
        assert json.dumps([1, 2]) == "[1, 2]"
        assert counters()["hits"] == 0
        assert status()["state"] == "disabled"
    finally:
        install()


def test_tools_see_the_original():
    calls = []

    def prof(frame, event, arg):
        if event == "call" and frame.f_code.co_name in ("dumps", "encode", "iterencode"):
            calls.append(frame.f_code.co_name)

    _tk.stats(reset=True)
    sys.setprofile(prof)
    try:
        json.dumps([1])
    finally:
        sys.setprofile(None)
    assert calls[:3] == ["dumps", "encode", "iterencode"]
    assert counters()["fallback"]["tools"] == 1


@pytest.mark.parametrize(
    "target,attr,value",
    [
        ("json", "_default_encoder", json.JSONEncoder(indent=1)),
        ("json.encoder", "c_make_encoder", None),
        ("json.encoder", "encode_basestring_ascii", json.encoder.py_encode_basestring_ascii),
        ("json.encoder", "encode_basestring", json.encoder.py_encode_basestring),
        ("json.encoder", "float", type("F", (float,), {})),
        ("json.encoder", "isinstance", lambda a, b: isinstance(a, b)),
        ("json.JSONEncoder", "item_separator", ";"),
        ("json.JSONEncoder", "key_separator", "="),
        ("json.JSONEncoder", "encode", lambda self, o: "patched"),
        ("json.JSONEncoder", "iterencode", lambda self, o, _one_shot=False: ["patched"]),
        ("json.JSONEncoder", "indent", property(lambda self: 3)),
        ("json._default_encoder", "indent", 4),
        ("json._default_encoder", "encode", lambda o: "patched"),
        ("json._default_encoder", "item_separator", "|"),
    ],
)
def test_dependency_patching_falls_back_and_rearms(target, attr, value):
    obj = {"a": [1, 2.5, "x"], "b": {"c": None}}
    plain = json.dumps(obj)
    owner = eval(target, {"json": json})  # noqa: S307
    with mock.patch.object(owner, attr, value, create=True):
        patched = outcome(lambda o: json._default_encoder.encode(o), obj)
        got = outcome(json.dumps, obj)
        assert got[0] == patched[0] and got[2] == patched[2], (got, patched)
    assert status()["deps_intact"]
    _tk.stats(reset=True)
    assert json.dumps(obj) == plain
    assert counters()["hits"] == 1


def test_kwdefaults_and_code_patching():
    obj = [1, {"a": 2}]
    kwd = json.dumps.__kwdefaults__
    with mock.patch.dict(kwd, {"indent": 2}):
        assert json.dumps(obj) == json.JSONEncoder(indent=2).encode(obj)
    assert json.dumps(obj) == "[1, {\"a\": 2}]"
    code = json.dumps.__code__
    try:
        json.dumps.__code__ = (lambda obj, **kw: "replaced").__code__
        assert json.dumps(obj) == "replaced"
    finally:
        json.dumps.__code__ = code
    assert json.dumps(obj) == "[1, {\"a\": 2}]"


def test_class_assignment_of_default_encoder():
    class Sub(json.JSONEncoder):
        def encode(self, o):
            return "sub"

    enc = json._default_encoder
    enc.__class__ = Sub
    try:
        assert json.dumps([1]) == "sub"
    finally:
        enc.__class__ = json.JSONEncoder
    assert json.dumps([1]) == "[1]"


def test_free_threaded_stress():
    shared = {"list": list(range(50)), "d": {str(i): i * 0.5 for i in range(50)}}
    expected = cpython(shared)
    errors = []
    stop = threading.Event()

    def reader():
        while not stop.is_set():
            r = json.dumps(shared)
            if r != expected:
                errors.append(r)

    def mutator():
        local = {"x": [1, 2, 3]}
        while not stop.is_set():
            local["x"].append(4)
            json.dumps(local)
            local["x"].pop()

    threads = [threading.Thread(target=reader) for _ in range(3)] + [threading.Thread(target=mutator)]
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


def test_concurrent_mutation_is_safe():
    """Free-threaded builds: encoding a list another thread mutates never crashes and
    always yields a valid snapshot-like document (CPython gives the same guarantee)."""
    data = [{"i": i} for i in range(100)]
    stop = threading.Event()

    def mutate():
        while not stop.is_set():
            data.append({"i": -1})
            data.pop()

    t = threading.Thread(target=mutate)
    t.start()
    try:
        for _ in range(300):
            json.loads(json.dumps(data))
    finally:
        stop.set()
        t.join()
