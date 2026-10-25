"""Rust implementations of the builtin functions ``sum``, ``min``, ``max``, ``sorted``,
``any`` and ``all``.

Nothing changes until :func:`install` is called (``rustique.enable()`` calls it). It puts
replacements into the :mod:`builtins` module; :func:`uninstall` restores the originals.

The replacements are builtin functions with the originals' names, docstrings, signatures
and call flags, so introspection, ``help()``, pickling (by reference to ``builtins.sum``,
...) and the interpreter's call specialisations behave as before. Each call takes a Rust
fast path when its inputs allow a result provably identical to CPython's (builtin
``list``/``tuple``/``set``/``frozenset``/``dict`` of exact ``int``/``float``/``str``
elements, Rustique containers), and otherwise calls the original C function with the
original arguments.

``enumerate``, ``zip`` and ``reversed`` are types, not functions, and are deliberately not
replaced (see ``docs/ARCHITECTURE.md``, "Builtin functions").
"""

from __future__ import annotations

from rustique._rustique import _builtins as _native

__all__ = ["REPLACED", "install", "installed", "uninstall"]

#: The builtins that :func:`install` replaces.
REPLACED: tuple[str, ...] = ("sum", "min", "max", "sorted", "any", "all")


def install() -> tuple[str, ...]:
    """Replace the builtins in :data:`REPLACED`; return the names now replaced.

    Idempotent. A builtin that is no longer CPython's original (e.g. patched by other code)
    is left alone.
    """
    return tuple(_native.install())


def uninstall() -> None:
    """Restore the original builtins (where the Rustique version is still installed)."""
    _native.uninstall()


def installed() -> tuple[str, ...]:
    """Names of the builtins currently replaced."""
    return tuple(_native.installed())
