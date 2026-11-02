"""Group `functools` (docs/targets/datamodel.md): the `singledispatch` cache lookup and
`singledispatchmethod.__get__`. Differential tests (dispatch results incl. ABC
registration and cache-token changes, registering after first call, classmethod/
staticmethod implementations, unusual metaclasses), mechanisms, free-threaded stress."""

from __future__ import annotations

import abc
import collections.abc
import functools
import sys
import threading

import pytest
from _dm_support import A, Counting, both, disable, enable, outcome
from hypothesis import given, settings
from hypothesis import strategies as st

SD = "functools.singledispatch"
GET = "functools.singledispatchmethod.__get__"


@pytest.fixture(autouse=True)
def _off():
    yield
    disable()


def scenario_dispatch():
    log = []

    @functools.singledispatch
    def fmt(x):
        return ("object", type(x).__name__)

    @fmt.register
    def _(x: int):
        return ("int", x)

    @fmt.register(list)
    def _(x):
        return ("list", len(x))

    class MyInt(int):
        pass

    class Meta(type):
        def __eq__(cls, other):
            log.append("meta eq")
            return type.__eq__(cls, other)

        def __hash__(cls):
            log.append("meta hash")
            return type.__hash__(cls)

    class WithMeta(metaclass=Meta):
        pass

    for v in [1, True, MyInt(3), "s", [1, 2], (1,), 1.5, None, WithMeta(), 2, "t", [3]]:
        log.append(outcome(lambda v=v: fmt(v)))
        log.append(outcome(lambda v=v: fmt.dispatch(type(v)).__name__))

    # ABC registration: the cache token changes and the cache is cleared.
    class Seq(abc.ABC):
        pass

    @fmt.register(Seq)
    def _(x):
        return ("Seq",)

    class Thing:
        pass

    log.append(outcome(lambda: fmt(Thing())))
    Seq.register(Thing)
    log.append(outcome(lambda: fmt(Thing())))
    log.append(outcome(lambda: fmt(Thing())))

    @fmt.register(collections.abc.Mapping)
    def _(x):
        return ("Mapping",)

    log.append(outcome(lambda: fmt({})))
    log.append(outcome(lambda: fmt({})))
    log.append(outcome(lambda: fmt.dispatch(5)))
    log.append(outcome(lambda: fmt()))
    log.append(outcome(lambda: sorted(k.__name__ for k in fmt.registry)))
    fmt._clear_cache()
    log.append(outcome(lambda: fmt(1)))
    return log


def test_dispatch_differential():
    with Counting(SD) as c:
        plain, fast = both(scenario_dispatch, ["functools"])
    assert plain == fast
    assert c.hits(SD) > 5


def scenario_method():
    log = []

    class Handler:
        @functools.singledispatchmethod
        def handle(self, arg):
            return ("default", arg)

        @handle.register
        def _(self, arg: int):
            return ("int", arg)

        @handle.register
        @classmethod
        def _(cls, arg: str):
            return ("str classmethod", cls.__name__, arg)

        @handle.register
        @staticmethod
        def _(arg: float):
            return ("float static", arg)

        @functools.singledispatchmethod
        @classmethod
        def cm(cls, arg):
            return ("cm", cls.__name__, arg)

        @functools.singledispatchmethod
        @staticmethod
        def sm(arg):
            return ("sm", arg)

    h = Handler()
    for v in [1, "s", 2.5, None, [1]]:
        log.append(outcome(lambda v=v: h.handle(v)))
        log.append(outcome(lambda v=v: Handler.handle(h, v)))
        log.append(outcome(lambda v=v: Handler.cm(v)))
        log.append(outcome(lambda v=v: h.sm(v)))
    bound = h.handle
    log.append(outcome(lambda: (bound._obj is h, bound._cls is Handler, bound.__module__, bound.__doc__)))
    log.append(outcome(lambda: getattr(bound, "_dispatch_arg_index", "absent")))
    log.append(outcome(lambda: Handler.handle._dispatch_arg_index if hasattr(Handler.handle, "_dispatch_arg_index") else "n/a"))
    log.append(outcome(lambda: bound.__name__))
    log.append(outcome(lambda: bound.__wrapped__.__name__))
    log.append(outcome(lambda: repr(Handler.__dict__["handle"].__get__(None, Handler))))
    log.append(outcome(lambda: h.handle()))
    return log


def test_method_differential():
    with Counting(SD, GET) as c:
        plain, fast = both(scenario_method, ["functools"])
    assert plain == fast
    assert c.hits(GET) > 3


@settings(max_examples=40)
@given(st.lists(st.sampled_from(["int", "str", "list", "bool", "float", "tuple", "reg_float", "clear"]), max_size=20))
def test_hypothesis_dispatch(ops):
    def scenario():
        log = []

        @functools.singledispatch
        def f(x):
            return "object"

        @f.register(int)
        def _(x):
            return "int"

        vals = {"int": 1, "str": "s", "list": [], "bool": True, "float": 1.0, "tuple": ()}
        for op in ops:
            if op == "reg_float":
                f.register(float, lambda x: "float")
            elif op == "clear":
                f._clear_cache()
            else:
                log.append(outcome(lambda op=op: f(vals[op])))
        return log

    plain, fast = both(scenario, ["functools"])
    assert plain == fast


def test_mechanisms():
    enable(["functools"])

    @functools.singledispatch
    def f(x):
        return 0

    class H:
        @functools.singledispatchmethod
        def m(self, x):
            return 1

    with Counting(SD, GET) as c:
        f(1)
        f(1)
        H().m(1)
    assert c.hits(SD) >= 1 and c.hits(GET) == 1
    dcode = f.dispatch.__code__
    seen = []

    def tracer(frame, event, arg):
        if event == "call" and frame.f_code is dcode:
            seen.append(1)

    with Counting(SD) as c:
        sys.settrace(tracer)
        try:
            f(1)
        finally:
            sys.settrace(None)
    assert seen and c.hits(SD) == 0
    disable()
    with Counting(SD, GET) as c:
        f(1)
        H().m(1)
    assert c.hits(SD) == 0 and c.hits(GET) == 0
    assert A.vectorcall_is_original(f.dispatch)
    assert A.vectorcall_is_original(functools.singledispatchmethod.__get__)


def test_threads_stress():
    enable(["functools"])

    @functools.singledispatch
    def f(x):
        return "o"

    @f.register(int)
    def _(x):
        return "i"

    class H:
        @functools.singledispatchmethod
        def m(self, x):
            return "d"

        @m.register
        def _(self, x: str):
            return "s"

    h = H()

    def work():
        r = None
        for _ in range(300):
            r = (f(1), f("x"), h.m("a"), h.m(2))
        return r

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
        for _ in range(30):
            f._clear_cache()

            class N:
                pass

            f.register(N, lambda x: "n")
    finally:
        stop.set()
        for t in ts:
            t.join()
    assert not errors, errors[:3]
