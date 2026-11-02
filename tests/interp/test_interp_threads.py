"""Races (free-threaded builds above all): dispatch-table writes while threads execute the
swapped handlers, enable/disable with frames inside our handlers, and class mutation racing
the attribute cache. Every worker checks its own results; any mismatch, exception or crash
fails."""

from __future__ import annotations

import subprocess
import sys
import textwrap
import threading
from typing import Any

import rustique
from _interp import enable, requires_tier

pytestmark = requires_tier


class A:
    cls_attr = 3

    def __init__(self, x: int) -> None:
        self.x = x

    def m(self) -> int:
        return self.x


class B(A):
    pass


class Meta(type):
    pass


class MA(metaclass=Meta):
    pass


class Other:
    pass


class S:
    __slots__ = ("a",)

    def __init__(self, a: int) -> None:
        self.a = a


def work() -> Any:
    objs = [A(1), B(2), A(3)]
    sl = [S(1), S(2)]
    s = 0
    for i in range(200):
        o = objs[i % 3]
        s += o.x + o.m() + o.cls_attr + isinstance(o, A) + isinstance(o, Other) + isinstance(o, MA)
        s += sl[i % 2].a + A(x=i).x
        try:
            {}[i]
        except KeyError:
            s += 1
    return s, [x * x for x in range(10)], sum(x for x in range(20))


def test_table_writes_race_with_dispatch() -> None:
    expected = work()
    stop = threading.Event()
    errors: list[Any] = []

    def worker() -> None:
        try:
            while not stop.is_set():
                r = work()
                if r != expected:
                    errors.append(r)
                    return
        except BaseException as e:  # noqa: BLE001
            errors.append(repr(e))

    ts = [threading.Thread(target=worker) for _ in range(4)]
    for t in ts:
        t.start()
    try:
        for _ in range(60):
            enable()
            rustique.disable()
    finally:
        stop.set()
        for t in ts:
            t.join()
    assert not errors, errors[:3]


def test_class_mutation_races_the_cache() -> None:
    """Threads load attributes of a class another thread keeps mutating (class attribute
    replaced, descriptor type gaining __get__): every load returns one of the values the
    class held, never anything else, and nothing crashes."""
    script = textwrap.dedent("""
        import sys, threading
        sys.path.insert(0, %r)
        from _interp import enable
        class V:
            def __init__(self, t): self.t = t
        class K:
            v = V(0)
            def m(self): return 1
        class L(K): pass
        class N(K): pass
        objs = [K(), L(), N()]
        enable()
        stop = threading.Event(); bad = []
        def reader():
            n = 0
            while not stop.is_set():
                for o in objs:
                    r = o.v
                    if not (isinstance(r, V) or r == "got"):
                        bad.append(repr(r)); return
                    n += o.m()
        ts = [threading.Thread(target=reader) for _ in range(4)]
        for t in ts: t.start()
        for i in range(3000):
            K.v = V(i)
            if i == 2000:
                V.__get__ = lambda self, o, t=None: "got"
        stop.set()
        for t in ts: t.join()
        print("bad", bad[:3])
    """) % (__import__("os").path.dirname(__file__),)
    r = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stderr[-3000:]
    assert r.stdout.strip() == "bad []", r.stdout
