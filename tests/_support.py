"""Shared helpers for the parity, differential, typed, buffer, thread and interop suites.

Implementations under test are selected with the ``RUSTIQUE_IMPLS`` environment variable,
a comma-separated subset of ``builtin,untyped,typed`` (default: all three):

* ``builtin`` -- the builtin ``list``/``dict``/``set``. Running the suites against the
  builtins proves the tests themselves encode Python's behaviour.
* ``untyped`` -- ``rustique.List`` / ``Dict`` / ``Set`` with adaptive storage strategies.
* ``typed`` -- ``rustique.List[T]`` etc., with ``T`` inferred from homogeneous test data.

If a rustique implementation is selected but ``rustique`` cannot be imported, the affected
tests fail loudly (never skip silently). ``RUSTIQUE_IMPLS=builtin`` runs only the
builtin variants and skips the rustique-only modules.
"""

from __future__ import annotations

import math
import os
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from types import ModuleType
from typing import Any

import pytest

ALL_KINDS = ("builtin", "untyped", "typed")


def selected_kinds() -> tuple[str, ...]:
    raw = os.environ.get("RUSTIQUE_IMPLS", "")
    if not raw.strip():
        return ALL_KINDS
    kinds = tuple(k.strip() for k in raw.split(",") if k.strip())
    unknown = set(kinds) - set(ALL_KINDS)
    if unknown:
        raise RuntimeError(f"RUSTIQUE_IMPLS: unknown implementation(s) {sorted(unknown)}")
    return kinds


def rustique_selected() -> bool:
    return any(k != "builtin" for k in selected_kinds())


_rustique: ModuleType | None = None
_rustique_error: BaseException | None = None


def load_rustique() -> ModuleType:
    """Import ``rustique`` or fail the calling test with the import error."""
    global _rustique, _rustique_error
    if _rustique is not None:
        return _rustique
    if _rustique_error is None:
        try:
            import rustique

            _rustique = rustique
            return rustique
        except BaseException as exc:  # PanicException derives from BaseException
            _rustique_error = exc
    pytest.fail(f"rustique is not importable: {_rustique_error!r}", pytrace=False)


def import_rustique_module_level() -> ModuleType:
    """For rustique-only test modules: skip the module when only builtins are selected,
    otherwise import rustique (failing collection loudly if it is broken)."""
    if not rustique_selected():
        pytest.skip("RUSTIQUE_IMPLS selects builtins only", allow_module_level=True)
    import rustique

    return rustique


# --------------------------------------------------------------------------------------
# Strict comparison: same types, same float bits (sign of zero), NaN equal to NaN.
# --------------------------------------------------------------------------------------


def same(a: object, b: object) -> bool:
    if type(a) is not type(b):
        return False
    if a is b:
        return True
    if type(a) is float:
        assert isinstance(a, float) and isinstance(b, float)
        if math.isnan(a):
            return math.isnan(b)
        return a == b and math.copysign(1.0, a) == math.copysign(1.0, b)
    if type(a) in (list, tuple):
        assert isinstance(a, (list, tuple)) and isinstance(b, (list, tuple))
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    return bool(a == b)


def describe(x: object) -> str:
    return f"{type(x).__name__}:{x!r}"


def assert_same_items(actual: Iterable[object], expected: Iterable[object]) -> None:
    actual = list(actual)
    expected = list(expected)
    assert len(actual) == len(expected), f"length {len(actual)} != {len(expected)}: {actual!r} vs {expected!r}"
    for i, (a, e) in enumerate(zip(actual, expected)):
        assert same(a, e), f"item {i}: {describe(a)} != {describe(e)}; full: {actual!r} vs {expected!r}"


def float_key(x: float) -> tuple[int, float]:
    """Sort key that orders NaNs last, for multiset comparisons."""
    return (1, 0.0) if math.isnan(x) else (0, x)


def multiset(items: Iterable[object]) -> list[str]:
    """Order-insensitive, type-strict fingerprint of a collection."""
    return sorted(f"{type(x).__name__}:{x!r}" for x in items)


# --------------------------------------------------------------------------------------
# List implementations
# --------------------------------------------------------------------------------------

_SCALAR_ELEMS: tuple[type, ...] = (bool, int, float, str)


def infer_elem(data: Iterable[object]) -> type | None:
    """The element type for a typed variant, or None if the data is not homogeneous."""
    types = {type(x) for x in data}
    if len(types) == 1:
        (t,) = types
        if t in _SCALAR_ELEMS:
            return t
    return None


@dataclass(frozen=True)
class ListImpl:
    """A list implementation under test. ``kind`` is builtin / untyped / typed."""

    kind: str

    @property
    def id(self) -> str:
        return {"builtin": "list", "untyped": "List", "typed": "List[T]"}[self.kind]

    @property
    def is_builtin(self) -> bool:
        return self.kind == "builtin"

    @property
    def rs(self) -> ModuleType:
        return load_rustique()

    @property
    def cls(self) -> type:
        return list if self.is_builtin else self.rs.List

    def new(self, data: Iterable[Any] = (), elem: type | None = None) -> Any:
        """Build a container holding ``data``. ``elem`` overrides typed inference."""
        if self.kind == "builtin":
            return list(data)
        if self.kind == "untyped":
            return self.rs.List(data)
        data = list(data)
        t = elem if elem is not None else (infer_elem(data) if data else int)
        if t is None:
            pytest.fail(f"typed variant used with heterogeneous data {data!r}")
        return self.rs.List[t](data)

    def elem_name(self, lst: Any) -> str | None:
        if self.kind != "typed":
            return None
        alias = lst.schema_type
        r = repr(alias)
        assert r.startswith("List[") and r.endswith("]"), r
        return r[5:-1]

    def expected_repr(self, lst: Any, items: list[Any]) -> str:
        if self.kind == "builtin":
            return repr(items)
        if self.kind == "untyped":
            return f"List({items!r})"
        return f"List[{self.elem_name(lst)}]({items!r})"

    def to_list(self, lst: Any) -> list[Any]:
        if self.is_builtin:
            return list(lst)
        out = lst.to_list()
        assert type(out) is list
        return out

    # Native kernels vs their builtin equivalents.
    def sum(self, lst: Any) -> Any:
        return sum(lst) if self.is_builtin else lst.sum()

    def min(self, lst: Any) -> Any:
        return min(lst) if self.is_builtin else lst.min()

    def max(self, lst: Any) -> Any:
        return max(lst) if self.is_builtin else lst.max()

    def map(self, lst: Any, f: Callable[[Any], Any]) -> Any:
        return [f(x) for x in lst] if self.is_builtin else lst.map(f)

    def filter(self, lst: Any, f: Callable[[Any], Any]) -> Any:
        return [x for x in lst if f(x)] if self.is_builtin else lst.filter(f)

    def check(self, lst: Any, expected: list[Any], *, eq: bool = True) -> None:
        """Assert ``lst`` holds exactly ``expected`` (types and values), via every
        observation path: len, iteration, indexing, reversed, to_list and ==."""
        assert type(lst) is self.cls
        assert len(lst) == len(expected)
        assert_same_items(iter(lst), expected)
        assert_same_items([lst[i] for i in range(len(lst))], expected)
        assert_same_items(reversed(lst), expected[::-1])
        if not self.is_builtin:
            assert_same_items(self.to_list(lst), expected)
        if eq:
            assert lst == expected
            assert expected == lst
            assert not (lst != expected)


def list_impls(*, typed: bool = True) -> list[Any]:
    kinds = [k for k in selected_kinds() if typed or k != "typed"]
    return [pytest.param(ListImpl(k), id=ListImpl(k).id) for k in kinds]
