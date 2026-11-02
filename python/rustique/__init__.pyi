"""Type stubs for Rustique's public API (docs/USAGE.md).

Rustique adds no types of its own: it accelerates CPython's. The opt-in native types of
0.2 (``List``, ``Dict``, ``X``, the scalars, ``native``) were removed in 0.3
(docs/MIGRATION.md).
"""

from collections.abc import Iterable
from typing import Any, Final, Literal, overload

__all__ = ["RustiqueCheckError", "__version__", "check", "disable", "enable", "stats", "status"]

__version__: Final[str]

def enable(
    *,
    only: str | Iterable[str] | None = None,
    exclude: str | Iterable[str] | None = None,
    tiers: str | Iterable[str] | None = None,
    builtins: bool = True,
) -> dict[str, Any]:
    """Put Rust kernels behind CPython's own entry points, process-wide (groups or target
    names in ``only``/``exclude``; opt-in ``tiers`` such as ``["interp", "gc"]``). A no-op
    under ``RUSTIQUE=0``. Returns :func:`status`."""

def disable() -> dict[str, Any]:
    """Undo :func:`enable`. Returns :func:`status`."""

def status() -> dict[str, Any]:
    """``{"targets": [{name, group, state, reason, sites, versions}], "process": {...}}``."""

def stats(enabled: bool | None = None, reset: bool = False) -> dict[str, dict[str, Any]]:
    """Per-target counters ``{name: {"hits": n, "fallback": {reason: n}}}``."""

class RustiqueCheckError(AssertionError):
    """Check mode ``"raise"``: a fast path returned something else than CPython's original."""

@overload
def check() -> str | None:
    """The current check mode (``"warn"``, ``"raise"`` or ``"off"``)."""
@overload
def check(mode: Literal["warn", "raise", "off"] | bool | None, /) -> str | None:
    """Set the check mode; returns the previous one."""
