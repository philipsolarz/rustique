"""Group `contextlib` (docs/targets/datamodel.md): constructing `@contextmanager`
context managers natively. Differential tests (results, errors and tracebacks, generator
behaviour inside `with`, side effects), mechanisms, free-threaded stress."""

from __future__ import annotations

import contextlib
import sys
import threading

import pytest
from _dm_support import A, Counting, both, disable, enable, outcome
from hypothesis import given, settings
from hypothesis import strategies as st

CM = "contextlib.contextmanager"


@pytest.fixture(autouse=True)
def _off():
    yield
    disable()


def scenario_cm():
    log = []

    @contextlib.contextmanager
    def cm(a, b=2, *rest, k=0, **kw):
        """doc of cm"""
        log.append(("enter", a, b, rest, k, sorted(kw.items())))
        try:
            yield a + b
        finally:
            log.append("exit")

    @contextlib.contextmanager
    def nodoc():
        yield 1

    @contextlib.contextmanager
    def raises_late():
        raise ValueError("inside")
        yield  # pragma: no cover

    @contextlib.contextmanager
    def not_a_generator(x):
        return iter([x])

    async def agen():
        yield 1

    for call in [
        lambda: cm(1),
        lambda: cm(1, 3, 4, 5, k=1, z=2),
        lambda: cm(),
        lambda: cm(1, a=2),
        lambda: nodoc(),
        lambda: raises_late(),
        lambda: not_a_generator(5),
        lambda: contextlib.contextmanager(agen)(),
    ]:
        o = outcome(call)
        log.append(o)
        try:
            obj = call()
        except Exception:  # noqa: BLE001
            continue
        log.append((type(obj).__name__, obj.__doc__, getattr(obj, "args", None), getattr(obj, "kwds", None)))
        log.append(outcome(lambda obj=obj: obj.__enter__()))
        log.append(outcome(lambda obj=obj: obj.__exit__(None, None, None)))

    with cm(1) as v:
        log.append(("body", v))
    try:
        with cm(2) as v:
            raise KeyError("x")
    except KeyError as e:
        log.append(("caught", repr(e)))
    gen = cm(1).gen
    log.append(type(gen).__name__)
    log.append(gen.gi_frame.f_lineno - cm.__wrapped__.__code__.co_firstlineno)

    @cm(1)
    def decorated():
        return "deco"

    log.append(outcome(decorated))
    log.append(outcome(decorated))
    return log


def test_contextmanager_differential():
    with Counting(CM) as c:
        plain, fast = both(scenario_cm, ["contextlib"])
    assert plain == fast
    assert c.hits(CM) > 3


@settings(max_examples=50)
@given(st.lists(st.tuples(st.integers(0, 4), st.lists(st.sampled_from(["a", "b", "k", "z"]), max_size=3)), max_size=8))
def test_hypothesis_contextmanager(calls):
    def scenario():
        log = []

        @contextlib.contextmanager
        def cm(a, b=1, *, k=2):
            log.append(("in", a, b, k))
            yield (a, b, k)
            log.append("out")

        for npos, kws in calls:
            kw = {n: n for n in kws}

            def go(npos=npos, kw=kw):
                with cm(*range(npos), **kw) as v:
                    return v

            log.append(outcome(go))
        return log

    plain, fast = both(scenario, ["contextlib"])
    assert plain == fast


def test_disable_restores_and_tools():
    enable(["contextlib"])

    @contextlib.contextmanager
    def cm():
        yield 1

    with Counting(CM) as c:
        with cm():
            pass
    assert c.hits(CM) == 1
    helper_code = cm.__code__
    seen = []

    def tracer(frame, event, arg):
        if event == "call" and frame.f_code is helper_code:
            seen.append(1)

    with Counting(CM) as c:
        sys.settrace(tracer)
        try:
            with cm():
                pass
        finally:
            sys.settrace(None)
    assert seen and c.hits(CM) == 0
    disable()
    with Counting(CM) as c:
        with cm():
            pass
    assert c.hits(CM) == 0
    assert A.vectorcall_is_original(cm)


def test_patched_class_falls_back():
    from unittest import mock

    enable(["contextlib"])

    @contextlib.contextmanager
    def cm():
        yield 1

    class Mine(contextlib._GeneratorContextManager):
        pass

    with mock.patch.object(contextlib, "_GeneratorContextManager", Mine):
        assert type(cm()) is Mine
    for _ in range(2000):
        pass
    assert type(cm()) is contextlib._GeneratorContextManager


def test_threads_stress():
    enable(["contextlib"])

    @contextlib.contextmanager
    def cm(x):
        yield x * 2

    def work():
        t = 0
        for i in range(300):
            with cm(i) as v:
                t += v
        return t

    expected = work()
    errors = []
    stop = threading.Event()

    def worker():
        while not stop.is_set():
            try:
                if work() != expected:
                    errors.append("wrong")
            except Exception as e:  # noqa: BLE001
                errors.append(repr(e))

    ts = [threading.Thread(target=worker) for _ in range(4)]
    for t in ts:
        t.start()
    try:
        for _ in range(20):

            @contextlib.contextmanager
            def other():
                yield

            with other():
                pass
    finally:
        stop.set()
        for t in ts:
            t.join()
    assert not errors, errors[:3]
