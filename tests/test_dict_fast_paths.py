"""Differential tests for `Dict`'s raw-slot fast paths (`dict/fast.rs`) and its key
strategies: native `int` keys, `str` keys kept as objects, and `Obj` probes (floats, bools,
`None`, tuples, frozensets, identity-compared instances) decided without Python code.

Each operation runs on a builtin `dict` and on a `Dict`; results, exceptions, iteration
order and the *identity* of stored keys and values must match.
"""

from __future__ import annotations

import copy
import pickle

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import rustique as rs


class Ident:
    """Hashable by identity, compared by identity (an `Obj` probe)."""

    def __repr__(self) -> str:
        return f"Ident#{id(self) % 1000}"


class EqOne:
    """Equal to 1 through a custom `__eq__` (never a fast probe)."""

    def __hash__(self) -> int:
        return 1

    def __eq__(self, other: object) -> bool:
        return other == 1 or isinstance(other, EqOne)

    def __repr__(self) -> str:
        return "EqOne()"


class IntSub(int):
    pass


class StrSub(str):
    pass


NAN_A, NAN_B = float("nan"), float("nan")
IDENT_A, IDENT_B = Ident(), Ident()
BIG = 2**70
KEYS = [
    0, 1, -1, -2, 2, 7, 2**62, 2**63 - 1, -(2**63), BIG, -BIG,
    0.0, -0.0, 1.0, 1.5, -2.0, 7.0, float(BIG), float("inf"), NAN_A, NAN_B,
    True, False, None,
    "", "a", "b", "é", "a" * 40, "\ud800",
    (1, 2), (1.0, 2), ("a",), (), frozenset({1}), frozenset({1.0}),
    b"a", IDENT_A, IDENT_B, EqOne(), IntSub(1), IntSub(7), StrSub("a"),
]  # fmt: skip
VALUES = [object() for _ in range(6)] + [None, 1, "v"]

ops = st.lists(
    st.tuples(
        st.sampled_from(
            ["set", "get", "getitem", "in", "del", "pop", "setdefault", "iter", "items", "popitem", "update", "update_rs", "eq"]
        ),
        st.integers(0, len(KEYS) - 1),
        st.integers(0, len(VALUES) - 1),
    ),
    max_size=60,
)


def run(d, op, key, value):
    try:
        if op == "set":
            d[key] = value
            return None
        if op == "get":
            return d.get(key, "<default>")
        if op == "getitem":
            return d[key]
        if op == "in":
            return key in d
        if op == "del":
            del d[key]
            return None
        if op == "pop":
            return d.pop(key, "<default>")
        if op == "setdefault":
            return d.setdefault(key, value)
        if op == "iter":
            return list(d)
        if op == "items":
            return list(d.items())
        if op == "popitem":
            return d.popitem()
        if op in ("update", "update_rs"):
            # A few entries starting at `key`'s position in the pool.
            start = next(i for i, k in enumerate(KEYS) if k is key)
            src = {k: value for k in KEYS[start : start + 4]}
            d.update(rs.Dict(src) if op == "update_rs" and isinstance(d, rs.Dict) else src)
            return None
        if op == "eq":
            twin = {k: v for k, v in d.items()}
            return (d == twin, d == rs.Dict(twin) if isinstance(d, rs.Dict) else True)
    except Exception as e:  # noqa: BLE001
        return ("raised", type(e))
    raise AssertionError(op)


def same(a, b) -> bool:
    """Equal results, where every contained object is the identical object -- except
    exact ints, which native int keys store by value (a documented deviation)."""
    if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)) and type(a) is type(b):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if type(a) is type(b) and type(a) in (int, bool, str, type) and a == b:
        return type(a) is not str or a is b or len(a) < 2
    return a is b


@settings(max_examples=300, deadline=None)
@given(ops)
def test_fast_paths_match_dict(sequence):
    ref, d = {}, rs.Dict()
    for op, k, v in sequence:
        key, value = KEYS[k], VALUES[v]
        got, want = run(d, op, key, value), run(ref, op, key, value)
        assert same(got, want), (op, key, got, want)
        assert len(d) == len(ref)
        assert same(list(d.items()), list(ref.items()))
    assert d == ref and d.to_dict() == ref


@settings(max_examples=100, deadline=None)
@given(st.lists(st.tuples(st.integers(0, len(KEYS) - 1), st.integers(0, len(VALUES) - 1)), max_size=40))
def test_constructors_keep_identity(pairs):
    src = {}
    for k, v in pairs:
        src[KEYS[k]] = VALUES[v]
    for d in (rs.Dict(src), rs.Dict(list(src.items())), rs.Dict(iter(src.items())), rs.Dict(rs.Dict(src))):
        assert same(list(d.items()), list(src.items()))
    merged = rs.Dict(src) | {KEYS[0]: VALUES[0]}
    assert same(list(merged.items()), list((src | {KEYS[0]: VALUES[0]}).items()))
    assert same(list(rs.Dict.fromkeys(list(src)).items()), list(dict.fromkeys(list(src)).items()))


def test_numeric_key_equivalence_through_fast_paths():
    d = rs.Dict({1: "a", 2.5: "b", "x": "c"})
    assert d[1.0] == "a" and d[True] == "a" and 1.0 in d and d.get(True) == "a"
    d[True] = "z"
    assert list(d.items()) == [(1, "z"), (2.5, "b"), ("x", "c")] and type(next(iter(d))) is int
    n = rs.Dict({1: "a", 2: "b"})
    assert n._key_strategy == "i64"
    assert n[1.0] == "a" and True in n and n.get(2.0) == "b" and 1.5 not in n and None not in n
    assert (1,) not in n and IDENT_A not in n and n.get(float("nan")) is None
    big = rs.Dict({BIG: 1})
    assert big[float(BIG)] == 1 and float(BIG) in big
    nan = rs.Dict({NAN_A: 1})
    assert NAN_A in nan and NAN_B not in nan and nan.get(float("nan")) is None


def test_str_keys_keep_their_objects():
    k = "".join(["ke", "y"])  # not interned
    d = rs.Dict()
    d[k] = 1
    assert next(iter(d)) is k and d._key_strategy == "str"
    probe = "".join(["k", "ey"])
    assert probe is not k and d[probe] == 1 and probe in d
    d[probe] = 2
    assert next(iter(d)) is k and d[k] == 2
    d[1] = 3
    assert d._key_strategy == "object"
    assert rs.Dict({StrSub("a"): 1})._key_strategy == "object"
    assert rs.Dict({"\ud800": 1})["\ud800"] == 1


def test_values_keep_identity_and_items_tuples_are_fresh_when_kept():
    vals = [object() for _ in range(5)]
    d = rs.Dict(zip(range(5), vals))
    assert all(d[i] is vals[i] for i in range(5))
    kept = list(d.items())
    assert [t[1] for t in kept] == vals and len({id(t) for t in kept}) == 5
    for k, v in d.items():
        assert v is vals[k]


def drain(it, n=5):
    out = []
    try:
        for _ in range(n):
            out.append(next(it))
    except Exception as e:  # noqa: BLE001
        out.append((type(e), str(e)))
    return out


@pytest.mark.parametrize(
    "mutate",
    [
        lambda d, it: d.__setitem__(3, 3),
        lambda d, it: (d.__delitem__(1), d.__setitem__(4, 4)),
        lambda d, it: (next(it), d.__delitem__(1), d.__setitem__(4, 4)),
        lambda d, it: (next(it), d.pop(2), d.__setitem__(2, 0)),
        lambda d, it: d.clear(),
    ],
)
@pytest.mark.parametrize("view", ["keys", "values", "items", "reversed"])
def test_iteration_under_mutation_matches_dict(mutate, view):
    def iterate(d):
        return iter(reversed(d)) if view == "reversed" else iter(getattr(d, view)())

    results = []
    for d in ({1: 1, 2: 2}, rs.Dict({1: 1, 2: 2})):
        it = iterate(d)
        hint = it.__length_hint__()
        mutate(d, it)
        results.append((hint, drain(it), it.__length_hint__()))
    assert results[0] == results[1]


def test_raw_methods_fall_back_with_full_semantics():
    d = rs.Dict({1: "a"})
    with pytest.raises(TypeError):
        d.get()
    with pytest.raises(TypeError):
        d.get(1, 2, 3)
    with pytest.raises(TypeError, match="unhashable"):
        d.get([])
    with pytest.raises(TypeError, match="unhashable"):
        d.setdefault([])
    with pytest.raises(KeyError):
        d[2]
    with pytest.raises(KeyError):
        del d[2]
    assert d.setdefault(2) is None and d.setdefault(3, "c") == "c" and d.setdefault(3, "x") == "c"
    assert rs.Dict.get(d, 1) == "a" and rs.Dict.setdefault(d, 1) == "a"
    t = rs.Dict[str, int]({"a": 1})
    assert t.setdefault("b", 2) == 2 and t.get("b") == 2
    with pytest.raises(TypeError):
        t.setdefault("c", "x")
    with pytest.raises(TypeError):
        t["c"] = "x"


class Sub(rs.Dict):
    pass


class Missing(rs.Dict):
    def __missing__(self, key):
        return ("missing", key)


class Init(rs.Dict):
    def __init__(self, a, b=0):
        super().__init__(a=a, b=b)


def test_subclasses():
    s = Sub({1: 2}, x=3)
    assert type(s) is Sub and s == {1: 2, "x": 3} and isinstance(s, rs.Dict)
    assert s[1] == 2 and s.get("x") == 3 and len(s) == 2
    assert Missing()[5] == ("missing", 5) and Missing().get(5) is None
    assert Init(1) == {"a": 1, "b": 0}
    assert type(Sub.fromkeys("ab")) is Sub and Sub.fromkeys("ab", 0) == {"a": 0, "b": 0}
    s.attr = "kept"
    for clone in (pickle.loads(pickle.dumps(s)), copy.copy(s), copy.deepcopy(s)):
        assert type(clone) is Sub and clone == s and clone.attr == "kept"
    assert type(s.copy()) is rs.Dict and type(s | {}) is rs.Dict
    assert repr(Sub({1: 2})) == "Sub({1: 2})"
    with pytest.raises(TypeError, match="at most 1 argument"):
        rs.Dict({}, {})
    d = rs.Dict({1: 2})
    d.__init__({3: 4}, x=5)
    assert d == {1: 2, 3: 4, "x": 5}


class Overrides(rs.Dict):
    def __getitem__(self, key):
        return "overridden"

    def __contains__(self, key):
        return True

    def __len__(self):
        return 42


def test_python_overrides_win_over_raw_slots():
    o = Overrides({1: 2})
    assert o[1] == "overridden" and 5 in o and len(o) == 42
    assert rs.Dict.__getitem__(o, 1) == 2 and rs.Dict.__len__(o) == 1


def test_mapping_patterns_match_dict_and_subclasses():
    for d in (rs.Dict({"a": 1, "b": 2}), Sub({"a": 1, "b": 2})):
        match d:
            case {"a": v, **rest}:
                assert v == 1 and rest == {"b": 2}
            case _:
                raise AssertionError("mapping pattern did not match")
    match rs.Dict({"b": 1}):
        case {"a": _}:
            raise AssertionError("matched a missing key")
        case {}:
            pass
