"""Rustique: CPython with a Rust engine (docs/USAGE.md).

``enable()`` puts Rust kernels behind CPython's own entry points, process-wide;
``status()``/``stats()`` show what runs; ``check()`` turns check mode on. The ``RUSTIQUE*``
environment variables control every enable path (``RUSTIQUE=0`` is the kill switch; see
below). Rustique's own containers (``List``/``Dict``/``Set``/``FrozenSet``/``Tuple``, the
expression placeholder ``X``, fixed-width scalars) are created on first use
(docs/ARCHITECTURE.md, docs/TAKEOVER.md).

This module is the startup path of ``RUSTIQUE=1`` (with the extension, and nothing else):
the ``_rustique_startup.pth`` line that ``site`` runs at every interpreter start imports it
only when ``RUSTIQUE`` is set to something other than ``0``::

    import os; os.environ.get('RUSTIQUE', '0') != '0' and __import__('rustique')._startup()

It imports no other Python module at import or enable time and keeps its own code small
(every code object is unmarshalled at each start): the common path (``RUSTIQUE=1`` or
``enable()`` with no selection, no ``RUSTIQUE_ONLY``/``_EXCLUDE``/``_STATS``/``_REPORT``/
``_CHECK``) is here; anything configured goes through ``rustique._switch``, imported then.

Environment (read at every enable; all ignored under ``-E``/``-I``, like ``PYTHON*``):

``RUSTIQUE``          ``1`` enables at startup (the ``.pth`` line above); ``0`` is the kill
                      switch: every enable path (startup, ``python -m rustique``,
                      ``enable()`` in code, ``native``, ``pytest --rustique``) is a no-op.
``RUSTIQUE_ONLY``     groups or target names; intersected with ``enable(only=)``.
``RUSTIQUE_EXCLUDE``  groups or target names; added to ``enable(exclude=)``. The
                      environment can turn things off, never on; unknown names warn.
``RUSTIQUE_STATS``    ``1``: counters on.
``RUSTIQUE_REPORT``   ``1``/``stderr``: report at exit on stderr; a path: appended to it
                      (``{pid}`` replaced; ``*.json``: one JSON record per process).
``RUSTIQUE_CHECK``    ``1``/``warn``: check mode (mismatches counted, CPython's result
                      used); ``raise``: a mismatch raises ``RustiqueCheckError``.

PYTEST_DONT_REWRITE (pytest would warn that this package, imported at startup under
``RUSTIQUE=1``, cannot be rewritten: it ships the ``rustique.pytest_plugin`` entry point).
"""

import os
import sys

from rustique import _rustique
from rustique._rustique import __version__

# The framework's primitives (accel-core, crates/rustique-py/src/accel/py.rs), by module
# attribute so that tests can substitute a fake.
_accel = _rustique._accel

__all__ = [
    "Dict",
    "DictType",
    "Expr",
    "FrozenSet",
    "FrozenSetType",
    "List",
    "ListType",
    "RustiqueCheckError",
    "Set",
    "SetType",
    "Tuple",
    "X",
    "__version__",
    "arange",
    "check",
    "disable",
    "enable",
    "f32",
    "f64",
    "i8",
    "i16",
    "i32",
    "i64",
    "native",
    "stats",
    "status",
    "u8",
    "u16",
    "u32",
    "u64",
]

# --- the opt-in API, created on first use (PEP 562) -------------------------------------------

_LAZY = frozenset({
    "Dict", "DictType", "Expr", "FrozenSet", "FrozenSetType", "List", "ListType", "Set", "SetType", "Tuple",
    "X", "arange", "f32", "f64", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64",
})


def __getattr__(name: str):
    if name in _LAZY:
        value = getattr(_rustique, name)  # the extension creates it (and registers its ABC)
        globals()[name] = value
        return value
    if name == "RustiqueCheckError":
        return _accel.RustiqueCheckError
    raise AttributeError(f"module 'rustique' has no attribute {name!r}")


def __dir__():
    return sorted(set(globals()) | _LAZY | {"RustiqueCheckError"})


# --- the switch (docs/ACCELERATION.md §8; the configured paths: rustique._switch) ------------

# How Rustique was turned on in this process, and what the last enable selected.
_state: dict = {"source": None, "only": None, "exclude": (), "killed_calls": 0, "report": None}
_UNSET = object()
_CONFIG = ("RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT", "RUSTIQUE_CHECK")


def _killed() -> bool:
    """``RUSTIQUE=0`` (or ``false``/``no``/``off``): every enable path is a no-op."""
    v = None if sys.flags.ignore_environment else os.environ.get("RUSTIQUE")
    return v is not None and v.strip().lower() in ("0", "false", "no", "off")


def _enable(only=None, exclude=None, native=None, builtins=True, source="rustique.enable()") -> None:
    """``enable()`` without building the status (the quiet enable of the startup path)."""
    if (only is not None or exclude is not None or native or not builtins
            or (not sys.flags.ignore_environment and any(k in os.environ for k in _CONFIG))):
        from rustique import _switch

        return _switch.enable(only, exclude, native, builtins, source)
    if _killed():
        _state["killed_calls"] += 1
        return
    _accel.enable(None, None, quiet=True)
    if _state["source"] is None:
        _state["source"] = source
    _state["only"], _state["exclude"] = None, ()


def enable(*, only=None, exclude=None, native=None, builtins: bool = True) -> dict:
    """Turn acceleration on for this process (idempotent, additive); returns :func:`status`.

    ``only``/``exclude``: group or target names (a list, or a comma-separated string);
    unknown names raise ``ValueError``. ``builtins=False`` is ``exclude=["builtins"]``.
    ``native``: module-name patterns rewritten to build Rustique containers when imported
    (opt-in native tier, docs/TAKEOVER.md). A no-op under ``RUSTIQUE=0``; the other
    ``RUSTIQUE*`` variables apply (module docstring).
    """
    _enable(only, exclude, native, builtins)
    return status()


def disable() -> dict:
    """Restore every original entry point and stop rewriting imports (modules already
    rewritten by the native tier keep their code). Returns :func:`status`."""
    _accel.disable(quiet=True)
    _state["source"] = None
    return status()


def status() -> dict:
    """``{"targets": [{"name", "group", "state", "reason", "refusal", "sites", "versions",
    ...}], "process": {...}}`` (docs/ACCELERATION.md §6.2). ``state``: ``"installed"``,
    ``"pending-import"``, ``"refused"`` (``reason``, ``refusal`` category) or
    ``"disabled"``. ``process["switch"]``: how Rustique was turned on, the kill switch, the
    selection and the ``RUSTIQUE*`` variables in effect."""
    st = _accel.status()
    proc = st["process"]
    env = {} if sys.flags.ignore_environment else os.environ
    proc["free_threaded"] = not getattr(sys, "_is_gil_enabled", lambda: True)()
    proc["switch"] = dict(_state, killed=_killed(), stats=bool(proc.get("stats")), check=proc.get("check"),
                          env={k: env[k] for k in ("RUSTIQUE", *_CONFIG) if k in env})
    return st


def stats(enabled: bool | None = None, reset: bool = False) -> dict:
    """Per-target counters ``{name: {"hits": n, "fallback": {reason: n}}}``. ``enabled``
    switches counting on or off (off by default; ``RUSTIQUE_STATS=1``); ``reset`` zeroes
    them after the snapshot."""
    return _accel.stats(enabled, reset)


def check(mode=_UNSET) -> str | None:
    """Check mode: every fast path of a *checkable* target also runs CPython's original and
    compares. ``"warn"``: a mismatch counts as ``fallback["check"]`` and the original's
    result is returned; ``"raise"``: it raises :class:`RustiqueCheckError`;
    ``None``/``False``/``"off"``: off. Without an argument, return the current mode;
    otherwise set it and return the previous one."""
    if mode is _UNSET:
        return _accel.check_mode()
    return _accel.check("off" if mode is None or mode is False else mode)


def native(obj=None, /):
    """Decorator: rewrite one function (or every method of a class) so that its literals,
    displays, comprehensions and constructor calls build Rustique containers (needs the
    source; docs/TAKEOVER.md). Under ``RUSTIQUE=0`` it returns ``obj`` unchanged."""
    if _killed():
        return (lambda o: o) if obj is None else obj
    from rustique._takeover import _rewrite

    return _rewrite.rewrite_object if obj is None else _rewrite.rewrite_object(obj)


def _startup() -> None:
    """``RUSTIQUE=1`` at interpreter startup, called by the ``_rustique_startup.pth`` line.
    ``1``/``true``/``yes``/``on`` enable; ``0``/``false``/``no``/``off``/empty do nothing;
    anything else warns. Never raises: a failure prints one line and Python starts plain."""
    value = None if sys.flags.ignore_environment else os.environ.get("RUSTIQUE")
    v = "" if value is None else value.strip().lower()
    if v in ("1", "true", "yes", "on"):
        try:
            _enable(source="RUSTIQUE=1")
        except Exception as exc:
            sys.stderr.write(f"rustique: RUSTIQUE={value} ignored: {type(exc).__name__}: {exc}\n")
    elif v not in ("", "0", "false", "no", "off"):
        sys.stderr.write(f"rustique: ignoring RUSTIQUE={value!r} (expected 1 or 0)\n")
