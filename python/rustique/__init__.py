"""Rustique: CPython with a Rust engine (docs/USAGE.md).

``enable()`` puts Rust kernels behind CPython's own entry points, process-wide;
``status()``/``stats()`` show what runs; ``check()`` turns check mode on. The ``RUSTIQUE*``
environment variables control every enable path (``RUSTIQUE=0`` is the kill switch; see
below). Rustique's own containers (``List``/``Dict``/``Set``/``FrozenSet``/``Tuple``, the
expression placeholder ``X``, fixed-width scalars) are created on first use
(docs/ARCHITECTURE.md, docs/TAKEOVER.md).

This module is on the startup path of ``RUSTIQUE=1`` (with ``_rustique_startup`` and the
extension, and nothing else): it imports no other Python module at import or enable time.
Formatting (report at exit, CLI) and the native tier are imported on use.

Environment (read at every enable; all ignored under ``-E``/``-I``, like ``PYTHON*``):

``RUSTIQUE``          ``1`` enables at startup (``_rustique_startup``); ``0`` is the kill
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
# The containers implement the full ABC protocols. The lazy extension registers them when it
# creates the types; an extension that does not (wave 0) gets them registered here.
_ABCS = {"List": "MutableSequence", "Dict": "MutableMapping", "Set": "MutableSet", "Tuple": "Sequence",
         "FrozenSet": "Set"}


def __getattr__(name: str):
    if name in _LAZY:
        value = getattr(_rustique, name)
        if name in _ABCS:
            import _collections_abc  # loaded by the interpreter at startup

            abc = getattr(_collections_abc, _ABCS[name])
            if not issubclass(value, abc):
                abc.register(value)
        globals()[name] = value
        return value
    if name == "RustiqueCheckError":
        return _accel.RustiqueCheckError
    raise AttributeError(f"module 'rustique' has no attribute {name!r}")


def __dir__():
    return sorted(set(globals()) | _LAZY | {"RustiqueCheckError"})


# --- the switch (docs/ACCELERATION.md §8) -----------------------------------------------------

_ON = ("1", "true", "yes", "on")
_OFF = ("0", "false", "no", "off")
_VARIABLES = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT", "RUSTIQUE_CHECK")
# How Rustique was turned on in this process, and what the last enable selected.
_state: dict = {"source": None, "only": None, "exclude": (), "killed_calls": 0, "report": None}
_UNSET = object()


def _env(name: str) -> str | None:
    """A ``RUSTIQUE*`` variable, or None (unset, or ``-E``/``-I``)."""
    return None if sys.flags.ignore_environment else os.environ.get(name)


def _flag(value: str | None) -> bool | None:
    if value is None:
        return None
    v = value.strip().lower()
    return True if v in _ON else False if v in _OFF else None


def _killed() -> bool:
    """``RUSTIQUE=0`` (or ``false``/``no``/``off``): every enable path is a no-op."""
    return _flag(_env("RUSTIQUE")) is False


def _names(value) -> tuple[str, ...] | None:
    """``"json, copy"`` / ``["json", "copy"]`` -> ``("json", "copy")``; None stays None."""
    if value is None:
        return None
    if isinstance(value, str):
        value = value.replace(";", ",").split(",")
    out: list[str] = []
    for v in value:
        if not isinstance(v, str):
            raise TypeError(f"group or target names must be str, not {type(v).__name__}")
        v = v.strip()
        if v and v not in out:
            out.append(v)
    return tuple(out)


def _targets() -> list[tuple[str, str]]:
    return [(t[0], t[1]) for t in _accel.targets()]


def _known(var: str, sel: tuple[str, ...] | None) -> tuple[str, ...] | None:
    """``sel`` without names no target or group has, with one warning on stderr (an
    environment variable never raises; ``enable()`` raises on unknown names in code)."""
    if not sel:
        return sel
    targets = _targets()
    known = {n for n, _ in targets} | {g for _, g in targets}
    unknown = [n for n in sel if n not in known]
    if unknown:
        groups = ", ".join(sorted({g for _, g in targets if not g.startswith("_")}))
        sys.stderr.write(f"rustique: {var}: no target or group named {', '.join(map(repr, unknown))}, ignored "
                         f"(groups: {groups})\n")
    return tuple(n for n in sel if n in known)


def _narrow(code: tuple[str, ...] | None, env: tuple[str, ...] | None) -> tuple[str, ...] | None:
    """The intersection of two ``only`` selections of groups or target names."""
    if env is None:
        return code
    if code is None:
        return env
    return tuple(n for n, g in _targets() if (n in code or g in code) and (n in env or g in env))


def _check_mode() -> str | None:
    """What ``RUSTIQUE_CHECK`` asks for: ``"warn"``, ``"raise"``, ``"off"`` or None (unset)."""
    value = _env("RUSTIQUE_CHECK")
    if value is None or not value.strip():
        return None
    v = value.strip().lower()
    if v in ("warn", "raise"):
        return v
    flag = _flag(v)
    if flag is None:
        sys.stderr.write(f"rustique: ignoring RUSTIQUE_CHECK={value!r} (expected 1, raise or 0)\n")
        return None
    return "warn" if flag else "off"


def _report_target() -> str | None:
    """Where ``RUSTIQUE_REPORT`` sends the report: None, ``"stderr"`` or a path."""
    value = _env("RUSTIQUE_REPORT")
    if value is None or not value.strip():
        return None
    flag = _flag(value)
    if flag is False:
        return None
    if flag is True or value.strip().lower() == "stderr":
        return "stderr"
    return value.strip().replace("{pid}", str(os.getpid()))


def _enable(only=None, exclude=None, native=None, builtins=True, source="rustique.enable()") -> None:
    """``enable()`` without building the status (the startup path's quiet enable)."""
    report = _report_target()
    if report is not None:
        _report_to(report, counters=False)
    if _killed():
        _state["killed_calls"] += 1
        return
    # Unknown names in RUSTIQUE_ONLY narrow to nothing, never to everything.
    env_only = _known("RUSTIQUE_ONLY", _names(_env("RUSTIQUE_ONLY")))
    env_exclude = _known("RUSTIQUE_EXCLUDE", _names(_env("RUSTIQUE_EXCLUDE")) or ())
    sel_only = _narrow(_names(only), env_only)
    sel_exclude = tuple(dict.fromkeys((_names(exclude) or ()) + env_exclude + (() if builtins else ("builtins",))))
    mode = _check_mode()
    if mode is not None:
        _accel.check(mode)
    if report is not None or _flag(_env("RUSTIQUE_STATS")):
        _accel.stats(True)
    _accel.enable(None if sel_only is None else list(sel_only), list(sel_exclude))
    if native:
        from rustique._takeover import _hook

        _hook.install((native,) if isinstance(native, str) else tuple(native))
    if _state["source"] is None:
        _state["source"] = source
    _state["only"], _state["exclude"] = sel_only, sel_exclude


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
    _accel.disable()
    if "rustique._takeover._hook" in sys.modules:
        sys.modules["rustique._takeover._hook"].uninstall()
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
    hook = sys.modules.get("rustique._takeover._hook")
    proc["native"] = tuple(hook.patterns()) if hook is not None else ()
    proc["free_threaded"] = not getattr(sys, "_is_gil_enabled", lambda: True)()
    proc["switch"] = {
        "source": _state["source"],
        "killed": _killed(),
        "killed_calls": _state["killed_calls"],
        "only": _state["only"],
        "exclude": _state["exclude"],
        "stats": bool(proc.get("stats")),
        "report": _state["report"],
        "check": proc.get("check"),
        "env": {k: v for k in _VARIABLES if (v := _env(k)) is not None},
    }
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
    """``RUSTIQUE=1`` at interpreter startup (``_rustique_startup``)."""
    _enable(source="RUSTIQUE=1")


# --- report at exit (formatting imported only then) -------------------------------------------


def _report_to(target: str, counters: bool = True) -> None:
    """Print (``"stderr"``) or append (a path) the report at exit."""
    if _state["report"] is None:
        import atexit  # builtin module

        atexit.register(_report_at_exit)
    _state["report"] = target
    if counters:
        _accel.stats(True)


def _record() -> dict:
    """This process's status and counters as one JSON-able record."""
    return {"pid": os.getpid(), "argv": list(sys.argv), "status": status(), "stats": stats(), "counters": True}


def _report_at_exit() -> None:
    target = _state["report"]
    if target is not None:
        from rustique import _report

        _report.write(target, _record)
