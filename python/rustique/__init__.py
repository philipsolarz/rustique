"""Rustique: CPython with a Rust engine (docs/USAGE.md).

``enable()`` puts Rust kernels behind CPython's own entry points, process-wide;
``status()``/``stats()`` show what runs; ``check()`` turns check mode on. The ``RUSTIQUE*``
environment variables control every enable path (``RUSTIQUE=0`` is the kill switch; see
below). The opt-in native types of Rustique 0.2 (``List``, ``Dict``, ``X``, the scalars,
``native``) were removed in 0.3: docs/MIGRATION.md.

The ``RUSTIQUE=1`` startup path does not import this package: the ``_rustique_startup.pth``
line that ``site`` runs at every interpreter start imports only the extension (top-level
``_rustique``) and calls its ``startup()``, and only when ``RUSTIQUE`` is set to something
other than ``0`` (docs/targets/dx.md §1). The environment semantics have one
implementation, in the extension (``_accel.switch_enable``), used by the startup path and by
:func:`enable` alike. This module imports no other Python module at import or enable time
and keeps its own code small (every code object is unmarshalled at each import).

Environment (read at every enable; all ignored under ``-E``/``-I``, like ``PYTHON*``):

``RUSTIQUE``          ``1`` enables at startup (the ``.pth`` line above); ``0`` is the kill
                      switch: every enable path (startup, ``python -m rustique``,
                      ``enable()`` in code, ``pytest --rustique``) is a no-op.
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

# The extension: top-level `_rustique` in every installed layout (a release wheel's
# extension itself, or the `_rustique.py` shim of the in-package layout); the
# package-relative import is the last resort, for a tree without the shim. Any other
# ImportError (a refused subinterpreter, a broken build) propagates as it is.
try:
    import _rustique
except ModuleNotFoundError as e:
    if e.name != "_rustique":
        raise
    from rustique import _rustique

__version__ = _rustique.__version__

# The framework's primitives (crates/rustique-py/src/accel/py.rs), by module attribute so
# that tests can substitute a fake.
_accel = _rustique._accel

__all__ = ["RustiqueCheckError", "__version__", "check", "disable", "enable", "stats", "status"]

_MIGRATION = "https://github.com/philipsolarz/rustique/blob/main/docs/MIGRATION.md"


def __getattr__(name: str):
    if name == "RustiqueCheckError":
        return _accel.RustiqueCheckError
    if name in {
        "List", "Dict", "Set", "Tuple", "FrozenSet", "ListType", "DictType", "SetType", "FrozenSetType",
        "Expr", "X", "arange", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "native",
    }:
        raise AttributeError(
            f"module 'rustique' has no attribute {name!r}: the opt-in native types were removed in "
            f"Rustique 0.3 (pin rustique<0.3 to keep them); see {_MIGRATION}",
            name=name,
        )
    raise AttributeError(f"module 'rustique' has no attribute {name!r}", name=name)


def __dir__():
    return sorted(set(globals()) | {"RustiqueCheckError"})


# --- the switch (docs/ACCELERATION.md §8; semantics in the extension: _accel.switch_enable) ----

_UNSET = object()


def _killed() -> bool:
    """``RUSTIQUE=0`` (or ``false``/``no``/``off``): every enable path is a no-op."""
    return _accel.killed()


def _enable(
    only=None, exclude=None, native=None, builtins=True, source="rustique.enable()", tiers=None
) -> None:
    """``enable()`` without building the status: the environment semantics (kill switch,
    ``RUSTIQUE_ONLY``/``_EXCLUDE`` narrowing, ``_STATS``, ``_CHECK``, ``_REPORT``) run in the
    extension. ``native`` must be empty (removed in 0.3; the parameter goes in 0.4)."""
    if native:
        raise TypeError(f"rustique: native= was removed in Rustique 0.3; see {_MIGRATION}")
    _accel.switch_enable(only, exclude, builtins, source, tiers)


def enable(*, only=None, exclude=None, tiers=None, builtins: bool = True, native=None) -> dict:
    """Turn acceleration on for this process (idempotent, additive); returns :func:`status`.

    ``only``/``exclude``: group or target names (a list, or a comma-separated string);
    unknown names raise ``ValueError``. ``builtins=False`` is ``exclude=["builtins"]``. A
    no-op under ``RUSTIQUE=0``; the other ``RUSTIQUE*`` variables apply (module docstring).
    ``tiers``: opt-in tiers to turn on as well (``["interp", "gc"]``), in union with
    ``RUSTIQUE_TIERS``; ``RUSTIQUE_TIERS=none`` turns every tier off even when code asks.
    ``native=`` was removed in 0.3 (``TypeError``).
    """
    _enable(only, exclude, native, builtins, tiers=tiers)
    return _accel.status()


def disable() -> dict:
    """Restore every original entry point. Returns :func:`status`."""
    _accel.disable(quiet=True)
    return _accel.status()


def status() -> dict:
    """``{"targets": [{"name", "group", "state", "reason", "refusal", "sites", "versions",
    ...}], "process": {...}}`` (docs/ACCELERATION.md §6.2). ``state``: ``"installed"``,
    ``"pending-import"``, ``"refused"`` (``reason``, ``refusal`` category) or
    ``"disabled"``. ``process["switch"]``: how Rustique was turned on, the kill switch, the
    selection and the ``RUSTIQUE*`` variables in effect."""
    return _accel.status()


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
