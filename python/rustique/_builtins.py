"""Rust implementations behind the builtin functions ``sum``, ``min``, ``max``, ``sorted``,
``any`` and ``all``.

Nothing changes until :func:`install` is called (``rustique.enable()`` calls it). It
accelerates the builtins *in place*: the builtin function objects stay the same objects
(same identity, name, docstring, signature, pickling, and the interpreter's call
specialisations), only the C entry point behind them changes; :func:`uninstall` restores
it. Each call takes a Rust fast path when its inputs allow a result provably identical to
CPython's (builtin ``list``/``tuple``/``set``/``frozenset``/``dict`` of exact
``int``/``float``/``str``/``tuple`` elements, Rustique containers), and otherwise runs the
original C function with the original arguments.

``enumerate``, ``zip`` and ``reversed`` are types, not functions, and are deliberately not
replaced (see ``docs/ARCHITECTURE.md`` §4.6).
"""

from __future__ import annotations

from rustique._rustique import _builtins as _native

__all__ = ["REPLACED", "install", "installed", "uninstall"]

#: The builtins that :func:`install` accelerates.
REPLACED: tuple[str, ...] = ("sum", "min", "max", "sorted", "any", "all")


def install() -> tuple[str, ...]:
    """Accelerate the builtins in :data:`REPLACED`; return the names now accelerated.

    Idempotent. Process-wide (like the builtins themselves): call it before starting
    threads that should see a consistent state.
    """
    return tuple(_native.install())


def uninstall() -> None:
    """Restore the original builtin implementations."""
    _native.uninstall()


def installed() -> tuple[str, ...]:
    """Names of the builtins currently accelerated."""
    return tuple(_native.installed())
