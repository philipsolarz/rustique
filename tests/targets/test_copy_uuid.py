"""`uuid.UUID` in `copy.deepcopy` and `dataclasses.asdict` (native `__getstate__` /
`__setstate__` emulation): differential against CPython's own run, memo contents and
counters included; every input outside the subset must restart (`deopt`) with CPython's
result."""

from __future__ import annotations

import copy
import dataclasses
import sys
import uuid
from unittest import mock

import pytest

rustique = pytest.importorskip("rustique")


@pytest.fixture(autouse=True)
def _clean():
    rustique.disable()
    yield
    rustique.disable()


def shape(o, memo=None):
    """A comparable view of a copy (and of a caller's memo)."""

    def v(x):
        if isinstance(x, uuid.UUID):
            return ("UUID", type(x).__name__, x.int, x.is_safe, id(x))
        if isinstance(x, dict):
            return ("dict", [(k, v(val)) for k, val in x.items()])
        if isinstance(x, (list, tuple)):
            return (type(x).__name__, [v(i) for i in x])
        return ("other", repr(x))

    out = v(o)
    m = None
    if memo is not None:
        m = sorted((type(val).__name__, len(val) if isinstance(val, (list, dict)) else 0) for val in memo.values())
    return out, m


def both(fn):
    a = fn()
    rustique.enable(only=["copy", "dataclasses"])
    rustique.stats(True, reset=True)
    try:
        b = fn()
        st = rustique.stats()
    finally:
        rustique.stats(False)
        rustique.disable()
    return a, b, st


def canon_ids(view):
    """Replace object ids by first-seen order (identity pattern only)."""
    seen: dict[int, int] = {}

    def walk(x):
        if isinstance(x, tuple) and len(x) == 5 and x[0] == "UUID":
            return x[:4] + (seen.setdefault(x[4], len(seen)),)
        if isinstance(x, (list, tuple)):
            return type(x)(walk(i) for i in x)
        return x

    return walk(view)


U1 = uuid.UUID(int=12345)
U2 = uuid.uuid4()


@dataclasses.dataclass
class Customer:
    id: uuid.UUID
    name: str
    tags: list


def test_deepcopy_uuid_matches_and_hits():
    def run():
        x = [U1, {"a": U2, "b": [U1]}, (U2,)]
        y = copy.deepcopy(x)
        assert y[0] is not U1 and y[0] == U1 and y[0] is y[1]["b"][0]
        return canon_ids(shape(y))

    a, b, st = both(run)
    assert a == b
    assert st["copy.deepcopy"]["hits"] >= 1
    assert st["copy.deepcopy"]["fallback"]["deopt"] == 0


def test_identity_and_type_of_copies():
    rustique.enable(only=["copy"])
    y = copy.deepcopy([U1, U1])
    rustique.disable()
    assert type(y[0]) is uuid.UUID and y[0] is y[1] and y[0] is not U1
    assert y[0].int == U1.int and y[0].is_safe is uuid.SafeUUID.unknown
    assert hash(y[0]) == hash(U1) and str(y[0]) == str(U1)
    with pytest.raises(TypeError):
        y[0].x = 1


def test_caller_memo_contents_match():
    def run():
        memo = {}
        y = copy.deepcopy({"k": U1}, memo)
        return canon_ids(shape(y, memo))

    a, b, _ = both(run)
    assert a == b


def test_asdict_with_uuid_hits():
    c = Customer(U1, "Ada", ["x"])

    def run():
        return canon_ids(shape(dataclasses.asdict(c)))

    a, b, st = both(run)
    assert a == b
    assert st["dataclasses.asdict"]["hits"] >= 1
    assert st["dataclasses.asdict"]["fallback"]["deopt"] == 0


class SubUUID(uuid.UUID):
    pass


@pytest.mark.parametrize(
    "make",
    [
        lambda: SubUUID(int=7),
        lambda: uuid.UUID(int=7, is_safe=uuid.SafeUUID.safe),
        lambda: uuid.UUID(int=7, is_safe=uuid.SafeUUID.unsafe),
    ],
    ids=["subclass", "safe", "unsafe"],
)
def test_outside_the_subset_restarts_with_cpythons_result(make):
    def run():
        x = make()
        y = copy.deepcopy([x])
        return canon_ids(shape(y)), type(y[0]).__name__, y[0].is_safe

    a, b, st = both(run)
    assert a == b
    assert st["copy.deepcopy"]["fallback"]["deopt"] >= 1


def test_changed_class_or_enum_falls_back():
    def run():
        y = copy.deepcopy(U1)
        return y.int, y.is_safe

    calls = []
    orig = uuid.UUID.__getstate__

    def spy(self):
        calls.append(1)
        return orig(self)

    with mock.patch.object(uuid.UUID, "__getstate__", spy):
        a, b, st = both(run)
    assert a == b and len(calls) == 2  # CPython's __getstate__ ran in both legs
    with mock.patch.object(uuid, "SafeUUID", uuid.SafeUUID):  # same object: still native
        a, b, st = both(run)
    assert a == b


def test_tracer_sees_cpythons_frames():
    events_a, events_b = [], []

    def run(events):
        def tracer(frame, event, arg):
            if event == "call":
                events.append(frame.f_code.co_name)

        sys.settrace(tracer)
        try:
            copy.deepcopy(U1)
        finally:
            sys.settrace(None)

    run(events_a)
    rustique.enable(only=["copy"])
    try:
        run(events_b)
    finally:
        rustique.disable()
    assert events_a == events_b
    assert "__getstate__" in events_a and "__setstate__" in events_a
