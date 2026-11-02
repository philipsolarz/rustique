"""Differential tests of the call overrides: keyword instantiation pushed inline
(CALL_KW_NON_PY) and `object.__setattr__` called directly (CALL_NON_PY_GENERAL). Results,
exception types and messages, and traceback frames are compared with plain CPython."""

from __future__ import annotations

import dataclasses
import sys
import traceback
from typing import Any

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from _interp import requires_tier, tier

pytestmark = requires_tier


class P:
    def __init__(self, a, b=0, *args, c=3, **kw):  # type: ignore[no-untyped-def]
        self.a, self.b, self.args, self.c, self.kw = a, b, args, c, kw

    def __repr__(self) -> str:
        return f"P({self.a!r}, {self.b!r}, {self.args!r}, {self.c!r}, {self.kw!r})"


class Bad:
    def __init__(self, a, b=0):  # type: ignore[no-untyped-def]
        return 5  # noqa: PLE0101


class Raises:
    def __init__(self, a, b=0):  # type: ignore[no-untyped-def]
        raise ValueError(f"bad {a} {b}")


class Slots:
    __slots__ = ("a", "b")

    def __init__(self, a, b=0):  # type: ignore[no-untyped-def]
        self.a = a
        self.b = b

    def __repr__(self) -> str:
        return f"Slots({self.a}, {self.b})"


class Sub(P):
    pass


class Frame:
    def __init__(self, a, b=0):  # type: ignore[no-untyped-def]
        f = sys._getframe(1)
        self.caller = f.f_code.co_name
        self.depth = len(traceback.extract_stack())


def deep(n: int) -> Any:
    return deep(n + 1) if n < 10**6 else P(1, b=n)


def show(f: Any) -> Any:
    try:
        r = f()
        return ("ok", repr(r) if not isinstance(r, Frame) else (r.caller, r.depth))
    except BaseException as e:  # noqa: BLE001
        tb = traceback.format_exception(e)
        return ("exc", type(e).__name__, str(e)[:200],
                [l.strip().split(",")[-1] for l in tb if l.strip().startswith("File")][-6:])


SHAPES = {
    "kw": lambda: P(1, b=2),
    "kw extra": lambda: P(1, b=2, z=9),
    "kw only": lambda: P(a=1, c=4),
    "kw varargs": lambda: P(1, 2, 3, c=4),
    "bad return": lambda: Bad(1, b=2),
    "raises": lambda: Raises(1, b=2),
    "unexpected kw": lambda: Slots(1, zz=2),
    "missing": lambda: Slots(b=2),
    "dup": lambda: Slots(1, a=2),
    "slots": lambda: Slots(1, b=2),
    "subclass": lambda: Sub(1, b=2),
    "frames": lambda: Frame(1, b=2),
    "recursion": lambda: deep(0),
}


def run_shapes() -> list[Any]:
    out = []
    for _ in range(3):   # warm the specializer
        for name, f in SHAPES.items():
            out.append((name, show(f)))
    return out


def test_keyword_instantiation_shapes() -> None:
    plain = run_shapes()
    with tier():
        accelerated = run_shapes()
    assert accelerated == plain


def test_keyword_instantiation_mutated_class() -> None:
    """A class whose __init__ is replaced, or whose metaclass/new changes, between calls."""

    def program() -> list[Any]:
        class K:
            def __init__(self, a, b=1):  # type: ignore[no-untyped-def]
                self.v = (a, b)

        out = []
        for i in range(30):
            out.append(show(lambda: K(i, b=2).v))
            if i == 10:
                K.__init__ = lambda self, a, b=1: setattr(self, "v", ("new", a, b))  # type: ignore[method-assign]
            if i == 20:
                K.__new__ = lambda cls, *a, **k: object.__new__(cls)  # type: ignore[method-assign]
        return out

    plain = program()
    with tier():
        accelerated = program()
    assert accelerated == plain


@dataclasses.dataclass(frozen=True)
class F:
    a: int
    b: str = "x"


class Guarded:
    def __setattr__(self, name: str, value: Any) -> None:
        raise AttributeError(f"no {name}")


setattr_values = st.one_of(st.integers(), st.text(max_size=3), st.none())
setattr_names = st.one_of(st.sampled_from(["a", "b", "__dict__", "__class__"]), st.integers(0, 3))


@settings(max_examples=100)
@given(st.lists(st.tuples(st.sampled_from(["frozen", "plain", "type", "guarded", "slots", "dict"]),
                          setattr_names, setattr_values), min_size=1, max_size=8))
def test_object_setattr_differential(ops: list[tuple[str, Any, Any]]) -> None:
    def program() -> list[Any]:
        class Plain:
            pass

        class T:
            pass

        targets = {"frozen": F(1), "plain": Plain(), "type": T, "guarded": Guarded(),
                   "slots": Slots(1), "dict": {}}
        out = []
        for _ in range(3):
            for kind, name, value in ops:
                o = targets[kind]
                if kind == "dict":
                    out.append(show(lambda: dict.__setitem__(o, name, value)))
                else:
                    out.append(show(lambda: object.__setattr__(o, name, value)))
                out.append(show(lambda: getattr(o, name) if isinstance(name, str) else None))
        return out

    plain = program()
    with tier():
        accelerated = program()
    assert accelerated == plain


def test_frozen_dataclass_errors() -> None:
    def program() -> list[Any]:
        out = []
        for i in range(30):
            f = F(i)
            out.append(show(lambda: f.a))
            out.append(show(lambda: setattr(f, "a", 3)))
            out.append(show(lambda: F(i, b=str(i))))
        return out

    plain = program()
    with tier():
        accelerated = program()
    assert accelerated == plain


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
