"""The takeover: ``rustique.enable()`` / ``rustique.disable()`` (docs/TAKEOVER.md).

Two tiers:

* **Safe tier (default, process-wide).** Containers stay exact builtins. ``enable()``
  installs exact-semantics Rust kernels behind builtin container methods
  (``list.sort`` - and so ``sorted`` -, ``list.index``/``count``, ``x in list``, the same for
  ``tuple``; ``crates/rustique-py/src/takeover.rs``) and the replacement builtin functions of
  ``rustique._builtins`` when that module is present. Every kernel falls back to the
  original C implementation whenever its fast path does not provably apply, so behaviour
  is CPython's in every observable respect. Code compiled before ``enable()`` (the
  ``__main__`` script, earlier imports, C extensions) is covered too.

* **Native tier (opt-in, per module).** ``enable(native=["pkg.*"])`` (or
  ``python -m rustique --native 'pkg.*' app.py``, or the ``@rustique.native`` decorator)
  rewrites the selected modules so that literals, displays, comprehensions and
  ``list()``/``dict()``/``set()`` calls build Rustique containers (``rustique.List`` etc.).
  Those are *not* builtins: see docs/TAKEOVER.md for what breaks when they reach code that
  did not opt in. Never the default.
"""

from __future__ import annotations

import sys
from collections.abc import Iterable

from rustique import _rustique

_kernels = _rustique._takeover

__all__ = ["disable", "enable", "native", "status"]

_state: dict[str, object] = {"safe": False, "builtins": False, "native": ()}


def _builtins_hooks():
    """WS5's replacement builtin functions, when present: (install, uninstall) or None."""
    try:
        from rustique._builtins import install, uninstall  # type: ignore[import-not-found]
    except ImportError:
        return None
    return install, uninstall


def enable(*, native: str | Iterable[str] | None = None, builtins: bool = True) -> dict[str, object]:
    """Turn the takeover on for this process. Idempotent.

    ``native``: module-name patterns (``fnmatch`` style, e.g. ``"myapp.*"``) whose modules
    are rewritten to build Rustique containers when they are imported from now on.
    ``builtins``: also install the replacement builtin functions (``rustique._builtins``).
    Returns :func:`status`.
    """
    _kernels.install()
    _state["safe"] = True
    if builtins and not _state["builtins"]:
        hooks = _builtins_hooks()
        if hooks is not None:
            hooks[0]()
            _state["builtins"] = True
    if native:
        from rustique._takeover import _hook

        patterns = (native,) if isinstance(native, str) else tuple(native)
        _hook.install(patterns)
        _state["native"] = _hook.patterns()
    return status()


def disable() -> dict[str, object]:
    """Undo :func:`enable`: restore the original builtin implementations and stop rewriting
    imports. Modules already rewritten keep their rewritten code."""
    if _state["builtins"]:
        hooks = _builtins_hooks()
        if hooks is not None:
            hooks[1]()
        _state["builtins"] = False
    _kernels.uninstall()
    _state["safe"] = False
    if "rustique._takeover._hook" in sys.modules:
        sys.modules["rustique._takeover._hook"].uninstall()
    _state["native"] = ()
    return status()


def status() -> dict[str, object]:
    """What is active: ``{"safe": bool, "kernels": [...], "builtins": bool, "native": (...)}``."""
    return {
        "safe": bool(_state["safe"]),
        "kernels": list(_kernels.installed()),
        "builtins": bool(_state["builtins"]),
        "native": tuple(_state["native"]),  # type: ignore[arg-type]
    }


def native(obj=None, /):
    """Decorator: rewrite one function (or every function of a class) so that its literals,
    displays, comprehensions and constructor calls build Rustique containers. Needs the
    source (``inspect.getsource``). See docs/TAKEOVER.md."""
    from rustique._takeover import _rewrite

    if obj is None:
        return _rewrite.rewrite_object
    return _rewrite.rewrite_object(obj)
