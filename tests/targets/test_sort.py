"""The sort family (`list.sort`, `builtins.sorted`): differential tests by identity.

Every case runs with CPython's originals and with the `containers` + `builtins` groups
enabled, on lists whose equal values are distinct objects, and compares the resulting
order by identity (stability), exceptions (type, message) and the list left behind.
Shapes cover what the kernel decides on: run structure (presorted, reversed, halves,
nearly sorted, organ pipe, few distinct values), sizes around the small-sort, stack and
shared-prefix thresholds, `str` keys (1/2/4-byte kinds, mixed kinds, shared prefixes
longer than a key, ties beyond 8 bytes), floats with -0.0/inf, big ints and the inputs it
must leave to CPython (NaN, mixed types, big ints mixed in, tuples, `key=`).
"""

from __future__ import annotations

import random
import sys

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")

GROUPS = ["containers", "builtins"]


@pytest.fixture(autouse=True)
def _clean():
    rustique.disable()
    yield
    rustique.disable()


def ids(xs):
    return [id(x) for x in xs]


def run(fn):
    try:
        return ("ok", fn())
    except Exception as e:  # noqa: BLE001 - compared
        return ("raise", type(e), str(e))


def both(pool, reverse=False, key=None):
    """(original, enabled) outcomes of `list.sort` and `sorted` on copies of `pool`."""
    kw = {"reverse": reverse} if key is None else {"reverse": reverse, "key": key}

    def case():
        a = list(pool)
        r1 = run(lambda: a.sort(**kw))
        r2 = run(lambda: ids(sorted(pool, **kw)))
        r3 = run(lambda: ids(sorted(tuple(pool), **kw)))
        return (r1, ids(a), r2, r3)

    plain = case()
    rustique.enable(only=GROUPS)
    try:
        on = case()
    finally:
        rustique.disable()
    return plain, on


def distinct(values):
    """Equal values as distinct objects (stability is then visible by identity)."""
    out = []
    for v in values:
        if isinstance(v, str):
            out.append("".join(list(v)) if len(v) > 1 else (v + "x")[:-1] if v else v)
        elif isinstance(v, bool):
            out.append(v)
        elif isinstance(v, int):
            out.append(int(str(v)))
        elif isinstance(v, float):
            out.append(float(repr(v)))
        else:
            out.append(v)
    return out


def shapes(base, rng):
    n = len(base)
    s = sorted(base)
    yield s
    yield s[::-1]
    h = n // 2
    yield s[h:] + s[:h]
    nearly = s[:]
    for _ in range(max(1, n // 50)):
        if n:
            i, j = rng.randrange(n), rng.randrange(n)
            nearly[i], nearly[j] = nearly[j], nearly[i]
    yield nearly
    yield s[: n // 2] + s[n // 2 :][::-1]  # organ pipe
    r = base[:]
    rng.shuffle(r)
    yield r


SIZES = [0, 1, 2, 3, 5, 15, 16, 17, 20, 21, 31, 33, 63, 64, 65, 127, 128, 129, 300]


def families(n, rng):
    yield "int", [rng.randrange(-1000, 1000) for _ in range(n)]
    yield "int-dups", [rng.randrange(3) for _ in range(n)]
    yield "bigint", [rng.randrange(-(2**62), 2**62) for _ in range(n)]
    yield "float", [rng.choice([0.0, -0.0, 1.5, -1.5, float("inf"), float("-inf"), rng.uniform(-9, 9)]) for _ in range(n)]
    yield "str", [f"k{rng.randrange(10**5):05d}" for _ in range(n)]
    yield "str-short", ["".join(rng.choice("ab") for _ in range(rng.randrange(4))) for _ in range(n)]
    yield "str-prefix", [f"https://example.com/path/{rng.randrange(10**6):07d}/x" for _ in range(n)]
    yield "str-prefix-ties", [f"key-{rng.randrange(4):08d}-{rng.choice('ab')}" for _ in range(n)]
    yield "str-ucs2", [f"ā{rng.randrange(10**4):05d}" for _ in range(n)]
    yield "str-ucs4", [f"\U0001f600{rng.randrange(100)}" for _ in range(n)]
    yield "str-mixed-kinds", [rng.choice(["a", "é", "€", "\U0001f600", "ab", "a\x00", ""]) + str(rng.randrange(9)) for _ in range(n)]


@pytest.mark.parametrize("n", SIZES)
def test_shapes_by_identity(n):
    rng = random.Random(n)
    for name, base in families(n, rng):
        for shape in shapes(base, rng):
            pool = distinct(shape)
            for reverse in (False, True):
                plain, on = both(pool, reverse)
                assert plain == on, (name, n, reverse, pool[:12])


def test_declines_and_errors_match():
    nan = float("nan")
    cases = [
        [3, 1.5, 2, 0.5],  # int/float mix
        [1, 2**70, 3, -(2**80)],  # big ints
        [1.0, nan, 0.5, nan, 2.0],  # NaN: CPython's own order
        [1, "a", 2],  # TypeError
        ["b", "a", 1],
        [(1, 2), (0, 3)],
        [True, 1, False, 0, 1.0],
        [None, None],
        ["a"] * 70 + [b"x"],
    ]
    for pool in cases:
        for reverse in (False, True):
            plain, on = both(distinct(pool), reverse)
            assert plain == on, (pool, reverse)
    plain, on = both(distinct([3, 1, 2]), key=lambda v: -v)
    assert plain == on


def test_argument_forms_match():
    forms = [
        lambda: sorted([3, 1], reverse=1),
        lambda: sorted([3, 1], reverse=2**100),
        lambda: sorted([3, 1], reverse="yes"),
        lambda: sorted([3, 1], reverse=None),
        lambda: sorted([3, 1], key=None),
        lambda: sorted([3, 1], 1),
        lambda: sorted(),
        lambda: sorted([2, 1], cmp=None),
        lambda: sorted(iterable=[2, 1]),
        lambda: sorted({3, 1, 2}),
        lambda: sorted({"b": 1, "a": 2}),
        lambda: sorted(x for x in (3, 1)),
        lambda: [3, 1].sort(reverse=1.0),
    ]
    for f in forms:
        a = run(f)
        rustique.enable(only=GROUPS)
        try:
            b = run(f)
        finally:
            rustique.disable()
        assert a == b


def test_sorted_returns_a_fresh_list_with_cpythons_capacity():
    for pool in ([3, 1, 2], tuple(range(50, 0, -1)), [f"s{i}" for i in range(40, 0, -1)]):
        a = sorted(pool)
        rustique.enable(only=GROUPS)
        try:
            b = sorted(pool)
        finally:
            rustique.disable()
        assert a == b and type(b) is list and b is not pool
        assert sys.getsizeof(a) == sys.getsizeof(b)


def test_counters():
    rustique.enable(only=GROUPS)
    rustique.stats(True, reset=True)
    try:
        sorted([3, 1, 2])
        sorted(tuple(f"s{i}" for i in range(5)))
        sorted([1, 2.5])  # declines after the copy: list.sort's original on it
        [f"s{i}" for i in range(5)].sort()  # short str list: CPython's
        st_ = rustique.stats()
    finally:
        rustique.stats(False)
        rustique.disable()
    assert st_["builtins.sorted"]["hits"] == 3
    assert st_["builtins.sorted"]["fallback"]["deopt"] == 1
    assert st_["list.sort"]["fallback"]["args"] >= 1


orderables = st.one_of(
    st.lists(st.integers(-20, 20), max_size=80),
    st.lists(st.integers(-(2**63), 2**63 - 1), max_size=80),
    st.lists(st.sampled_from([0.0, -0.0, 1.5, -1.5, float("inf"), float("-inf")]), max_size=80),
    st.lists(st.floats(allow_nan=False), max_size=80),
    st.lists(st.text(max_size=12), max_size=80),
    st.lists(st.text(alphabet="abā\U0001f600", max_size=6), max_size=80),
    st.lists(st.sampled_from(["pre/fix/aaaa", "pre/fix/aaab", "pre/fix/", "pre/fix/aaaa\x00", "pre"]), max_size=80),
)


@settings(max_examples=250, deadline=None)
@given(orderables, st.booleans(), st.sampled_from(["as-is", "sorted", "reversed", "halves"]))
def test_hypothesis_differential(values, reverse, order):
    if order == "sorted":
        values = sorted(values)
    elif order == "reversed":
        values = sorted(values, reverse=True)
    elif order == "halves":
        s = sorted(values)
        values = s[len(s) // 2 :] + s[: len(s) // 2]
    plain, on = both(distinct(values), reverse)
    assert plain == on
