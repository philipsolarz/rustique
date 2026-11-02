"""Target `set.__reduce__` (group `set`): free-threaded builds only.

Differential: every probe is reduced (and deep-copied, copied, pickled) with the target
disabled, then enabled, and the two results are compared structurally (types, list order
and capacity, state). On GIL builds the target must refuse with category `build`.
"""

from __future__ import annotations

import copy
import pickle
import sys
import threading

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

rustique = pytest.importorskip("rustique")
from rustique._rustique import _accel  # noqa: E402

NAME = "set.__reduce__"
FREE_THREADED = getattr(sys, "_is_gil_enabled", lambda: True)() is False


def status():
    return {d["name"]: d for d in rustique.status()["targets"]}[NAME]


def enable():
    rustique.enable(only=["set"])


def disable():
    _accel.disable([NAME])


@pytest.fixture(autouse=True)
def _clean():
    disable()
    yield
    disable()


def test_refuses_on_gil_builds_installs_on_free_threaded():
    enable()
    st_ = status()
    if FREE_THREADED:
        assert st_["state"] == "installed", st_
        assert len(st_["sites"]) == 2, st_
    else:
        assert st_["state"] == "refused", st_
        assert st_.get("refusal") == "build", st_


ft_only = pytest.mark.skipif(not FREE_THREADED, reason="the target installs on free-threaded builds only")


# --- probes ----------------------------------------------------------------------------------


class S(set):
    pass


class F(frozenset):
    pass


class WithSlots(set):
    __slots__ = ("a", "b")


class WithState(set):
    def __getstate__(self):
        return {"custom": sorted(self, key=repr)}


class RaisingState(set):
    def __getstate__(self):
        raise ValueError("no state")


class Iter(set):
    log: list = []

    def __iter__(self):
        Iter.log.append("iter")
        return super().__iter__()


class RaisingIter(set):
    def __iter__(self):
        raise KeyError("iter")


class LoggingGetattribute(set):
    log: list = []

    def __getattribute__(self, name):
        LoggingGetattribute.log.append(name)
        return super().__getattribute__(name)


def probes():
    s = S({1, 2, 3})
    s.x = "attr"
    ws = WithSlots({4})
    ws.a = 1
    return [
        set(), {1, 2, 3}, set(range(100)), {"a", "b", (1, 2)}, frozenset(), frozenset({1.5, "x"}),
        s, F({7, 8}), ws, WithState({3, 1}), Iter({9}), LoggingGetattribute({5}),
    ]


def shape(r):
    """A comparable view of a `__reduce__` result (list capacity included)."""
    cls, args, state = r
    (keys,) = args
    return (cls, type(args), type(keys), list(keys), sys.getsizeof(keys), state)


def run_both(fn):
    disable()
    Iter.log.clear()
    LoggingGetattribute.log.clear()
    a = outcome(fn)
    side_a = (list(Iter.log), list(LoggingGetattribute.log))
    enable()
    Iter.log.clear()
    LoggingGetattribute.log.clear()
    b = outcome(fn)
    side_b = (list(Iter.log), list(LoggingGetattribute.log))
    disable()
    return (a, side_a), (b, side_b)


def outcome(fn):
    try:
        return ("ok", fn())
    except Exception as e:  # noqa: BLE001 - compared below
        return ("raise", type(e), str(e))


@ft_only
@pytest.mark.parametrize("i", range(len(probes())))
def test_reduce_matches(i):
    a, b = run_both(lambda: shape(probes()[i].__reduce__()))
    assert a == b


@ft_only
@pytest.mark.parametrize("obj", [RaisingState({1}), RaisingIter({1})], ids=["getstate-raises", "iter-raises"])
def test_errors_are_cpythons(obj):
    a, b = run_both(lambda: obj.__reduce__())
    assert a == b
    assert a[0][0] == "raise"


@ft_only
@pytest.mark.parametrize("i", range(len(probes())))
def test_copy_deepcopy_pickle_match(i):
    def run():
        o = probes()[i]
        d = copy.deepcopy(o)
        c = copy.copy(o)
        out = []
        for x in (d, c):
            out.append((type(x), sorted(x, key=repr), dict(getattr(x, "__dict__", {}))))
        for proto in (0, 2, 4, 5):
            out.append(pickle.dumps(o, proto) if type(o) in (set, frozenset) else len(pickle.dumps(o, proto)))
        return out

    a, b = run_both(run)
    assert a == b


@ft_only
def test_counters_and_reachability():
    enable()
    rustique.stats(True, reset=True)
    before = rustique.stats()[NAME]["hits"]
    {1}.__reduce__()
    frozenset({1}).__reduce__()
    copy.deepcopy({1, 2})  # copy group off: object.__reduce_ex__ -> set.__reduce__
    pickle.dumps(S({1}), 2)
    after = rustique.stats()[NAME]["hits"]
    rustique.stats(False)
    assert after - before >= 4


@ft_only
@settings(max_examples=150, deadline=None)
@given(
    st.sets(st.one_of(st.integers(), st.text(max_size=4), st.tuples(st.integers(), st.integers())), max_size=40),
    st.sampled_from([set, frozenset, S, F]),
    st.dictionaries(st.sampled_from(["a", "b", "c"]), st.integers(), max_size=3),
)
def test_hypothesis_differential(items, cls, attrs):
    def run():
        o = cls(items)
        if attrs and hasattr(o, "__dict__"):
            o.__dict__.update(attrs)
        return shape(o.__reduce__())

    a, b = run_both(run)
    assert a == b


@ft_only
def test_concurrent_reduce_and_mutation():
    enable()
    shared = set(range(500))
    errors = []
    stop = threading.Event()

    def mutate():
        k = 1000
        while not stop.is_set():
            shared.add(k)
            shared.discard(k - 50)
            k += 1

    def reduce_():
        try:
            for _ in range(2000):
                cls, (keys,), state = shared.__reduce__()
                assert cls is set and state is None and len(set(keys)) == len(keys)
                copy.deepcopy(frozenset(range(20)))
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    ts = [threading.Thread(target=mutate)] + [threading.Thread(target=reduce_) for _ in range(4)]
    for t in ts:
        t.start()
    for t in ts[1:]:
        t.join()
    stop.set()
    ts[0].join()
    disable()
    assert not errors, errors[:3]


@ft_only
def test_disable_restores_and_identity():
    m1, m2 = set.__reduce__, frozenset.__reduce__
    enable()
    assert set.__reduce__ is m1 and frozenset.__reduce__ is m2
    assert set.__reduce__.__name__ == "__reduce__" and set.__reduce__.__doc__ == m1.__doc__
    disable()
    assert status()["state"] in ("disabled", "pending-import")
    assert {1}.__reduce__() == (set, ([1],), None)
