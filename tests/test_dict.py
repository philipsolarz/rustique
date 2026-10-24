"""`Dict` parity with builtin `dict` (ARCHITECTURE §4.2), typed behaviour and key strategies."""

import builtins
import copy
import gc
import pickle
import threading
from decimal import Decimal
from enum import IntEnum, StrEnum
from fractions import Fraction

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from rustique import Dict, List, Set

NAN = float("nan")


def snapshot(mapping):
    """Items with their exact types, so `1`, `1.0` and `True` keys are told apart."""
    return [(type(k), k, type(v), v) for k, v in mapping.items()]


def check_same(d, ref):
    assert snapshot(d) == snapshot(ref)
    assert len(d) == len(ref)
    assert d == ref
    assert list(d) == list(ref)


# --- construction ---------------------------------------------------------------------


def test_construction_forms():
    assert Dict() == {}
    check_same(Dict({"a": 1, "b": 2}), {"a": 1, "b": 2})
    check_same(Dict([("a", 1), ["b", 2]]), dict([("a", 1), ["b", 2]]))
    check_same(Dict(["ab", "cd"]), dict(["ab", "cd"]))
    check_same(Dict({"a": 1}, b=2, a=3), dict({"a": 1}, b=2, a=3))
    check_same(Dict(x=1, y=2.5), dict(x=1, y=2.5))
    check_same(Dict((k, k * 2) for k in range(5)), {k: k * 2 for k in range(5)})
    check_same(Dict(Dict({1: "a"})), {1: "a"})
    check_same(Dict(zip("abc", [None, [1], (2,)])), dict(zip("abc", [None, [1], (2,)])))


def test_construction_from_mapping_protocol():
    class Mapping:
        def keys(self):
            return ["x", "y"]

        def __getitem__(self, k):
            return k.upper()

    check_same(Dict(Mapping()), dict(Mapping()))


def test_construction_from_frozendict():
    frozendict = getattr(builtins, "frozendict", None)
    if frozendict is None:
        pytest.skip("frozendict needs Python 3.15")
    fd = frozendict(a=1, b=2)
    check_same(Dict(fd), dict(fd))
    assert Dict(fd) == fd
    d = Dict()
    d |= fd
    assert d == {"a": 1, "b": 2}


def test_construction_errors_match_dict():
    with pytest.raises(TypeError, match="cannot convert dictionary update sequence element #1 to a sequence"):
        Dict([("a", 1), 5])
    with pytest.raises(ValueError, match="dictionary update sequence element #0 has length 3; 2 is required"):
        Dict([(1, 2, 3)])
    with pytest.raises(TypeError):
        Dict(5)
    with pytest.raises(TypeError):
        Dict({}, {})


def test_update_keeps_prefix_before_error():
    d, ref = Dict(), {}
    for target in (d, ref):
        with pytest.raises(ValueError):
            target.update([(1, 2), (3, 4, 5), (6, 7)])
    check_same(d, ref)


# --- item access ----------------------------------------------------------------------


def test_getitem_setitem_delitem():
    d = Dict()
    d["a"] = 1
    d["b"] = 2
    d["a"] = 3
    assert d["a"] == 3
    assert list(d.items()) == [("a", 3), ("b", 2)]
    del d["a"]
    assert "a" not in d
    with pytest.raises(KeyError) as exc:
        d["missing"]
    assert exc.value.args == ("missing",)
    with pytest.raises(KeyError) as exc:
        del d[(1, 2)]
    assert exc.value.args == ((1, 2),)
    assert len(d) == 1 and bool(d) and not Dict()


def test_numeric_keys_follow_python_equality():
    d = Dict({1: "int"})
    assert d[1.0] == "int" and d[True] == "int"
    assert 1.0 in d and True in d and Fraction(1) in d and Decimal(1) in d
    d[1.0] = "float"
    d[True] = "bool"
    check_same(d, {1: "bool"})
    assert type(next(iter(d))) is int

    ref = {True: "a"}
    d = Dict({True: "a"})
    for target in (d, ref):
        target[1] = "b"
        target[1.0] = "c"
    check_same(d, ref)
    assert type(next(iter(d))) is bool

    d = Dict({0: "zero"})
    assert d[-0.0] == "zero" and d[False] == "zero"
    assert 0.5 not in d


def test_big_and_negative_int_keys():
    keys = [2**70, -(2**70), -1, -2, 2**63, -(2**63), 2**61 - 1, 0]
    d, ref = Dict(), {}
    for k in keys:
        d[k] = str(k)
        ref[k] = str(k)
    check_same(d, ref)
    assert d[-1] == "-1" and d[-2] == "-2"  # hash(-1) == hash(-2)
    assert d[float(2**70)] == str(2**70)


def test_nan_keys_use_identity():
    d = Dict({NAN: 1})
    assert d[NAN] == 1
    with pytest.raises(KeyError):
        d[float("nan")]
    d[float("nan")] = 2
    assert len(d) == 2


def test_enum_and_numeric_tower_keys_on_native_tables():
    class Color(IntEnum):
        RED = 1

    class Name(StrEnum):
        A = "a"

    d = Dict({1: "one", 2: "two"})
    assert d[Color.RED] == "one"
    assert d[Fraction(4, 2)] == "two"
    assert d[Decimal("2")] == "two"
    assert d[complex(2, 0)] == "two"
    assert Fraction(1, 2) not in d

    s = Dict({"a": 1})
    assert s[Name.A] == 1
    s[Name.A] = 2
    check_same(s, {"a": 2})
    t, ref = Dict(), {}
    for target in (t, ref):
        target[Name.A] = 1
        target["a"] = 2
    check_same(t, ref)
    assert type(next(iter(t))) is Name


def test_custom_eq_objects_match_native_keys():
    class LooksLikeA:
        def __hash__(self):
            return hash("a")

        def __eq__(self, other):
            return other == "a"

    class LooksLikeTwo:
        def __hash__(self):
            return 2

        def __eq__(self, other):
            return other == 2

    assert Dict({"a": 1, "b": 2})[LooksLikeA()] == 1
    assert Dict({1: "x", 2: "y"})[LooksLikeTwo()] == "y"
    d = Dict({1: "x", 2: "y"})
    d[LooksLikeTwo()] = "z"
    check_same(d, {1: "x", 2: "z"})
    assert d._key_strategy == "i64"


def test_unhashable_keys_raise_like_dict():
    d = Dict({"a": 1})
    message = r"cannot use 'list' as a dict key \(unhashable type: 'list'\)"
    for op in (
        lambda: d[[1]],
        lambda: d.__setitem__([1], 2),
        lambda: [1] in d,
        lambda: d.get([1]),
        lambda: d.pop([1]),
        lambda: d.__delitem__([1]),
        lambda: d.setdefault([1]),
        lambda: Dict({(1, [2]): 3}),
    ):
        with pytest.raises(TypeError):
            op()
    with pytest.raises(TypeError, match=message):
        d[[1]] = 1
    assert d == {"a": 1}


def test_hash_errors_propagate():
    class Boom:
        def __hash__(self):
            raise ValueError("no hash")

    with pytest.raises(ValueError, match="no hash"):
        Dict()[Boom()] = 1


def test_eq_errors_propagate_and_mutation_during_eq_is_safe():
    class Bad:
        def __hash__(self):
            return 1

        def __eq__(self, other):
            raise RuntimeError("eq failed")

    d = Dict({Bad(): 1})
    with pytest.raises(RuntimeError, match="eq failed"):
        d[Bad()]

    class Mutator:
        def __init__(self, target):
            self.target = target

        def __hash__(self):
            return 7

        def __eq__(self, other):
            self.target.clear()
            return False

    victim = Dict()
    victim[Mutator(victim)] = 1
    assert Mutator(Dict()) not in victim
    assert len(victim) == 0


# --- strategies and demotion ----------------------------------------------------------


def test_strategies_and_demotion_preserve_order():
    d = Dict({1: "a", 2: "b"})
    assert d._key_strategy == "i64"
    d["x"] = "c"
    assert d._key_strategy == "object"
    check_same(d, {1: "a", 2: "b", "x": "c"})

    s = Dict({"a": 1})
    assert s._key_strategy == "str"
    s[None] = 2
    assert s._key_strategy == "object"
    check_same(s, {"a": 1, None: 2})

    b = Dict({1: 1})
    b[True] = 2
    assert b._key_strategy == "i64"
    b[2.5] = 3
    assert b._key_strategy == "object"
    check_same(b, {1: 2, 2.5: 3})

    assert Dict()._key_strategy == "empty"
    assert Dict({(1, 2): 0})._key_strategy == "object"
    assert Dict({2**64: 0})._key_strategy == "object"
    assert Dict({True: 0})._key_strategy == "object"

    c = Dict({1: 1})
    c.clear()
    c["a"] = 1
    assert c._key_strategy == "str"


def test_value_strategies():
    assert Dict({"a": 1})._value_strategy == "i64"
    assert Dict({"a": 1.5})._value_strategy == "f64"
    d = Dict({"a": 1})
    d["b"] = "x"
    assert d._value_strategy == "object"
    check_same(d, {"a": 1, "b": "x"})


# --- order, deletion, churn -----------------------------------------------------------


def test_delete_preserves_order_and_reinsert_appends():
    d = Dict({k: k for k in "abcdef"})
    ref = dict(d.items())
    for target in (d, ref):
        del target["b"]
        target.pop("d")
        target["b"] = 9
    check_same(d, ref)


def test_heavy_churn_matches_dict():
    d, ref = Dict(), {}
    for i in range(5000):
        k = (i * 7919) % 211
        if i % 3 == 0:
            assert d.pop(k, None) == ref.pop(k, None)
        else:
            d[k] = i
            ref[k] = i
    check_same(d, ref)
    keys = list(d)
    for k in keys[::2]:
        del d[k]
        del ref[k]
    check_same(d, ref)


def test_popitem_is_lifo():
    d = Dict({1: "a", 2: "b", 3: "c"})
    del d[3]
    assert d.popitem() == (2, "b")
    assert d.popitem() == (1, "a")
    with pytest.raises(KeyError, match=r"popitem\(\): dictionary is empty"):
        d.popitem()
    d[5] = "e"
    assert list(d.items()) == [(5, "e")]


# --- methods --------------------------------------------------------------------------


def test_get_pop_setdefault():
    d = Dict({"a": 1})
    assert d.get("a") == 1 and d.get("z") is None and d.get("z", 5) == 5
    assert d.pop("a") == 1
    assert d.pop("a", None) is None
    with pytest.raises(KeyError) as exc:
        d.pop("a")
    assert exc.value.args == ("a",)
    with pytest.raises(TypeError):
        d.pop("a", 1, 2)
    assert d.setdefault("k", [1]) == [1]
    assert d.setdefault("k", [2]) == [1]
    assert d.setdefault("n") is None
    check_same(d, {"k": [1], "n": None})


def test_update_forms():
    d, ref = Dict({"a": 1}), {"a": 1}
    for target in (d, ref):
        target.update({"b": 2})
        target.update([("c", 3)], d=4)
        target.update(e=5)
        target.update(Dict({"a": 0}))
        target.update()
    check_same(d, ref)


def test_clear_copy_fromkeys_to_dict():
    d = Dict({"a": [1]})
    c = d.copy()
    assert c == d and c is not d and type(c) is Dict
    c["b"] = 2
    assert "b" not in d
    assert c["a"] is d["a"]
    d.clear()
    assert d == {} and len(d) == 0
    check_same(Dict.fromkeys("abc"), dict.fromkeys("abc"))
    check_same(Dict.fromkeys([1, 2, 1.0], 0), dict.fromkeys([1, 2, 1.0], 0))
    out = Dict({1: "a"}).to_dict()
    assert type(out) is dict and out == {1: "a"}
    assert copy.copy(Dict({1: 2})) == {1: 2}


def test_or_operators():
    a, b = Dict({"x": 1, "y": 2}), {"y": 3, "z": 4}
    check_same(a | b, {"x": 1, "y": 2} | b)
    check_same(b | a, b | {"x": 1, "y": 2})
    assert type(b | a) is Dict
    assert type(a | Dict(b)) is Dict
    a |= [("w", 0)]
    check_same(a, {"x": 1, "y": 2, "w": 0})
    with pytest.raises(TypeError):
        a | [("q", 1)]
    with pytest.raises(TypeError):
        a | 1


def test_equality():
    d = Dict({"a": 1, 2: [3]})
    assert d == {"a": 1, 2: [3]} and {"a": 1, 2: [3]} == d
    assert d == Dict({2: [3], "a": 1})
    assert d != {"a": 1}
    assert d != {"a": 2, 2: [3]}
    assert d != [("a", 1), (2, [3])]
    assert (d == 5) is False
    assert Dict({1: 1}) == {1.0: 1.0} == Dict({True: 1})
    assert Dict[str, int]({"a": 1}) == Dict({"a": 1})
    assert Dict({"a": NAN}) == Dict({"a": NAN})
    assert Dict({"a": NAN}) == {"a": NAN}
    with pytest.raises(TypeError):
        d < d


def test_hash_raises():
    with pytest.raises(TypeError):
        hash(Dict())


def test_repr():
    assert repr(Dict()) == "Dict({})"
    assert repr(Dict({"a": 1, 2: 0.5, None: [1]})) == "Dict({'a': 1, 2: 0.5, None: [1]})"
    assert repr(Dict[str, int]({"a": 1})) == "Dict[str, int]({'a': 1})"
    assert repr(Dict[str, int]()) == "Dict[str, int]({})"
    d = Dict()
    d["self"] = d
    assert repr(d) == "Dict({'self': Dict({...})})"


def test_pickle_and_deepcopy():
    for d in (Dict({"a": 1, 2: [3]}), Dict[str, float]({"a": 1.5})):
        for proto in range(pickle.HIGHEST_PROTOCOL + 1):
            back = pickle.loads(pickle.dumps(d, proto))
            assert back == d and type(back) is Dict
            assert back.schema_type is d.schema_type
    deep = copy.deepcopy(Dict({"a": [1]}))
    assert deep == {"a": [1]}


def test_schema_type():
    assert Dict().schema_type is Dict
    assert Dict[str, int]().schema_type is Dict[str, int]


# --- views ----------------------------------------------------------------------------


def test_views_are_live():
    d = Dict({"a": 1, "b": 2})
    keys, values, items = d.keys(), d.values(), d.items()
    d["c"] = 3
    assert len(keys) == len(values) == len(items) == 3
    assert list(keys) == ["a", "b", "c"]
    assert list(values) == [1, 2, 3]
    assert list(items) == [("a", 1), ("b", 2), ("c", 3)]
    assert list(reversed(keys)) == ["c", "b", "a"]
    assert list(reversed(items)) == [("c", 3), ("b", 2), ("a", 1)]
    assert list(reversed(d)) == ["c", "b", "a"]
    assert "a" in keys and ("a", 1) in items and ("a", 2) not in items and 2 in values
    assert ("a",) not in items and "a" not in items
    assert repr(keys) == "DictKeys(['a', 'b', 'c'])"
    assert repr(values) == "DictValues([1, 2, 3])"
    assert repr(items) == "DictItems([('a', 1), ('b', 2), ('c', 3)])"
    assert keys.mapping is d


def test_view_set_operations():
    d = Dict({"a": 1, "b": 2, "c": 3})
    ref = {"a": 1, "b": 2, "c": 3}
    other = {"b", "z"}
    assert d.keys() & other == ref.keys() & other
    assert d.keys() | other == ref.keys() | other
    assert d.keys() - other == ref.keys() - other
    assert d.keys() ^ other == ref.keys() ^ other
    assert other - d.keys() == other - ref.keys()
    assert other & d.keys() == other & ref.keys()
    assert d.keys() & ["a", "q"] == {"a"}
    assert d.items() & {("a", 1), ("b", 9)} == {("a", 1)}
    assert d.keys() == {"a", "b", "c"}
    assert d.keys() == ref.keys()
    assert d.keys() <= {"a", "b", "c", "d"}
    assert d.keys() < {"a", "b", "c", "d"}
    assert not d.keys().isdisjoint(["c"])
    assert d.keys() != ["a", "b", "c"]
    with pytest.raises(TypeError):
        hash(d.keys())


def test_iterators_are_live_and_detect_size_changes():
    d = Dict({1: 1, 2: 2})
    it = iter(d)
    assert next(it) == 1
    d[1] = 10  # overwrite: no size change
    assert next(it) == 2
    d[3] = 3
    with pytest.raises(RuntimeError, match="dictionary changed size during iteration"):
        next(it)
    it = iter(Dict({1: 1}))
    assert list(it) == [1]
    with pytest.raises(StopIteration):  # once exhausted, always exhausted
        next(it)

    it = iter(d)
    next(it)
    d[4] = 4
    with pytest.raises(RuntimeError, match="dictionary changed size during iteration"):
        next(it)
    with pytest.raises(RuntimeError, match="dictionary changed size during iteration"):
        next(it)

    ref = {1: 1, 2: 2}
    for target in (Dict(ref), ref):
        with pytest.raises(RuntimeError):
            for k in target:
                del target[k]

    it = iter(d.items())
    d.pop(4)
    with pytest.raises(RuntimeError):
        next(it)


def test_iteration_sees_value_updates():
    d = Dict({"a": 1, "b": 2})
    out = []
    for k, v in d.items():
        out.append(v)
        d["b"] = 20
    assert out == [1, 20]
    assert d.values().__iter__().__length_hint__() == 2


# --- typed dicts ----------------------------------------------------------------------


def test_typed_validation_and_conversion():
    d = Dict[str, int]({"a": 1})
    with pytest.raises(TypeError, match=r"Dict\[str, int\] expects int, got str"):
        d["b"] = "x"
    with pytest.raises(TypeError, match=r"Dict\[str, int\] expects str, got int"):
        d[1] = 1
    assert d == {"a": 1}
    assert 1 not in d and d.get(1) is None and [1, 2] != d
    with pytest.raises(KeyError):
        d[2.5]

    f = Dict[str, float]({"a": 1})
    assert f["a"] == 1.0 and type(f["a"]) is float
    k = Dict[int, str]({True: "t"})
    assert list(k) == [1] and type(next(iter(k))) is int
    assert k[1.0] == "t"
    with pytest.raises(TypeError):
        k[1.5] = "x"
    fl = Dict[float, str]({1: "one"})
    assert list(fl) == [1.0] and type(next(iter(fl))) is float
    assert fl[1] == "one" and fl[True] == "one"


def test_typed_operations_keep_schema():
    d = Dict[str, int]({"a": 1})
    assert d.copy().schema_type is Dict[str, int]
    assert (d | Dict[str, int]({"b": 2})).schema_type is Dict[str, int]
    assert (d | {"b": "x"}).schema_type is Dict
    with pytest.raises(TypeError):
        d |= {"b": "x"}
    assert Dict[str, int](d).schema_type is Dict[str, int]
    assert Dict(d).schema_type is Dict


def test_typed_big_int_keys_demote_but_validate():
    d = Dict[int, int]()
    d[2**80] = 1
    d[3] = 2
    assert d == {2**80: 1, 3: 2}
    with pytest.raises(TypeError):
        d["x"] = 1


def test_nested_containers_as_values():
    d = Dict[str, List[int]]({"a": [1, 2]})
    assert type(d["a"]) is List and d["a"].to_list() == [1, 2]
    s = Dict({"s": Set([1])})
    assert s["s"] == {1}


# --- GC and threads -------------------------------------------------------------------


def test_reference_cycles_are_collected():
    collected = []

    class Node:
        def __del__(self):
            collected.append(True)

    d = Dict()
    node = Node()
    node.d = d
    d["node"] = node
    d[node] = d
    del d, node
    gc.collect()
    assert collected == [True]


def test_concurrent_writers_lose_nothing():
    d = Dict()

    def work(base):
        for i in range(2000):
            d[base + i] = i
            d[f"s{base + i}"] = i

    threads = [threading.Thread(target=work, args=(t * 10_000,)) for t in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(d) == 4 * 2000 * 2


def test_concurrent_readers_and_writers_with_object_keys():
    d = Dict({(i, i): i for i in range(200)})
    errors = []

    def reader():
        try:
            for _ in range(20):
                for i in range(200):
                    d.get((i, i))
        except Exception as e:  # pragma: no cover - reported below
            errors.append(e)

    def writer():
        for j in range(2000):
            d[(j % 300, j % 300)] = j
            d.pop((j % 250, j % 250), None)

    threads = [threading.Thread(target=reader) for _ in range(3)] + [threading.Thread(target=writer)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors


# --- differential (Hypothesis) --------------------------------------------------------

KEYS = st.one_of(
    st.integers(-5, 5),
    st.sampled_from([1.0, 2.5, -0.0, True, False, 2**64, -(2**63), NAN, None, "a", "b", "", (1, 2), (1, "a")]),
    st.text(max_size=3),
)
VALUES = st.one_of(st.integers(-3, 3), st.floats(allow_nan=False), st.text(max_size=2), st.none())

OPS = st.one_of(
    st.tuples(st.just("set"), KEYS, VALUES),
    st.tuples(st.just("del"), KEYS),
    st.tuples(st.just("pop"), KEYS),
    st.tuples(st.just("get"), KEYS),
    st.tuples(st.just("in"), KEYS),
    st.tuples(st.just("setdefault"), KEYS, VALUES),
    st.tuples(st.just("update"), st.lists(st.tuples(KEYS, VALUES), max_size=4)),
    st.tuples(st.just("popitem")),
    st.tuples(st.just("clear")),
)


def apply(target, op):
    name = op[0]
    try:
        if name == "set":
            target[op[1]] = op[2]
            return None
        if name == "del":
            del target[op[1]]
            return None
        if name == "pop":
            return target.pop(op[1], "default")
        if name == "get":
            return target.get(op[1], "default")
        if name == "in":
            return op[1] in target
        if name == "setdefault":
            return target.setdefault(op[1], op[2])
        if name == "update":
            return target.update(op[1])
        if name == "popitem":
            return target.popitem()
        if name == "clear":
            return target.clear()
    except KeyError as e:
        return ("KeyError", e.args)
    raise AssertionError(name)


def typed(value):
    if isinstance(value, tuple):
        return tuple(typed(v) for v in value)
    return (type(value), value)


@settings(max_examples=300, deadline=None)
@given(st.lists(OPS, max_size=40))
def test_differential_against_dict(ops):
    d, ref = Dict(), {}
    for op in ops:
        assert typed(apply(d, op)) == typed(apply(ref, op)), op
        assert snapshot(d) == snapshot(ref), op
    assert d == ref
    assert list(reversed(d)) == list(reversed(ref))
    assert Dict(ref) == d and d.copy() == ref


@settings(max_examples=150, deadline=None)
@given(st.lists(st.tuples(st.text(max_size=3), st.integers()), max_size=30), st.lists(st.text(max_size=3), max_size=10))
def test_differential_typed_str_int(pairs, deletions):
    d, ref = Dict[str, int](pairs), dict(pairs)
    for k in deletions:
        assert d.pop(k, None) == ref.pop(k, None)
    check_same(d, ref)


def test_finalizers_never_run_under_the_lock():
    # A duplicate key object that dies inside `update` must be released only after the
    # dict's lock is dropped: its `__del__` reads the dict (re-entering would deadlock).
    seen = []

    class K:
        def __init__(self, owner):
            self.owner = owner

        def __hash__(self):
            return 1

        def __eq__(self, other):
            return isinstance(other, K)

        def __del__(self):
            seen.append(len(self.owner))

    d = Dict()
    d[K(d)] = 0
    d.update((K(d), i) for i in range(3))
    assert len(d) == 1 and d[K(d)] == 2
    assert seen
    del d


def test_native_equality_with_builtin_dict_follows_python():
    assert Dict({"a": 1}) == {"a": 1.0} == Dict({"a": True})
    assert Dict({"a": 1.5}) != {"a": 1}
    assert Dict({"a": "1"}) != {"a": 1}
    assert Dict({1: 2**53}) != {1: float(2**53) + 2}
    assert Dict({1: 1, 2: 2}) != {1: 1, 3: 2}
    assert Dict({1: [1]}) == {1: [1]}
    assert Dict({"x": 0.0}) == {"x": -0.0}
    assert Dict({"k": NAN}) == {"k": float("nan")}  # native floats have no identity


def test_untyped_values_keep_identity_in_object_storage():
    marker = "text-value"
    d = Dict({"a": 1})
    d["b"] = [1]
    d["c"] = marker
    assert d._value_strategy == "object"
    assert d["c"] is marker
    d["c"] = marker
    assert d["c"] is marker
