"""The user-facing switches (docs/ACCELERATION.md §8, docs/USAGE.md).

``rustique.enable``/``disable``/``status``/``stats``/``native`` are defined here, on top of the
acceleration framework (``rustique._framework``). This layer adds what the environment
controls:

``RUSTIQUE``
    ``1`` enables at interpreter startup (``_rustique_startup``). ``0`` is the **kill
    switch**: every enable path (startup, ``python -m rustique``, ``rustique.enable()`` in
    code, ``rustique.native``, the pytest plugin) becomes a no-op, so the process runs as
    plain CPython would.
``RUSTIQUE_ONLY`` / ``RUSTIQUE_EXCLUDE``
    Comma-separated groups or target names (``json,copy``; ``re``; ``json.dumps``). They
    narrow every enable path: ``only`` intersects with the code's ``only=``, ``exclude`` adds
    to its ``exclude=``. The environment can always turn things off, never on.
``RUSTIQUE_STATS``
    ``1`` switches the per-target counters on when Rustique is enabled.
``RUSTIQUE_REPORT``
    ``1`` (or ``stderr``) prints a report at exit to stderr; a path appends it to that file
    (``{pid}`` in the path is replaced by the process id; a ``.json`` path gets one JSON
    object per line). Implies ``RUSTIQUE_STATS=1``.
``RUSTIQUE_CHECK``
    ``1`` (``warn``): check mode. Every fast path of a *checkable* target (no user code, no
    side effects in its exact subset) also runs CPython's original and compares; a
    mismatch counts as ``fallback["check"]`` and the original's result is returned.
    ``raise``: a mismatch raises ``RustiqueCheckError``. About 2x per checked call: a CI
    and debugging mode.

Like ``PYTHON*`` variables, all of them are ignored under ``-E`` and ``-I``.
"""

import os
import sys

from rustique import _framework as _fw

ON = ("1", "true", "yes", "on")
OFF = ("0", "false", "no", "off")
VARIABLES = ("RUSTIQUE", "RUSTIQUE_ONLY", "RUSTIQUE_EXCLUDE", "RUSTIQUE_STATS", "RUSTIQUE_REPORT", "RUSTIQUE_CHECK")

# How Rustique was enabled in this process, and what the last enable() selected.
_state: dict = {"source": None, "only": None, "exclude": (), "killed_calls": 0, "report": None}


def getenv(name: str) -> str | None:
    """A ``RUSTIQUE*`` variable, or None (unset, or ``-E``/``-I``)."""
    if sys.flags.ignore_environment:
        return None
    return os.environ.get(name)


def _flag(value: str | None) -> bool | None:
    if value is None or not value.strip():
        return None
    v = value.strip().lower()
    return True if v in ON else False if v in OFF else None


def killed() -> bool:
    """``RUSTIQUE=0`` (or ``false``/``no``/``off``): every enable path is a no-op."""
    return _flag(getenv("RUSTIQUE")) is False


def names(value) -> tuple[str, ...] | None:
    """``"json, copy"`` / ``["json", "copy"]`` -> ``("json", "copy")``; None stays None."""
    if value is None:
        return None
    if isinstance(value, str):
        value = value.replace(";", ",").split(",")
    out = []
    for v in value:
        if not isinstance(v, str):
            raise TypeError(f"group or target names must be str, not {type(v).__name__}")
        v = v.strip()
        if v and v not in out:
            out.append(v)
    return tuple(out)


def check_mode() -> str | None:
    """What ``RUSTIQUE_CHECK`` asks for: ``"warn"``, ``"raise"``, ``"off"`` or None (unset)."""
    value = getenv("RUSTIQUE_CHECK")
    if value is None or not value.strip():
        return None
    v = value.strip().lower()
    if v in ("raise", "warn"):
        return v
    flag = _flag(v)
    if flag is None:
        sys.stderr.write(f"rustique: ignoring RUSTIQUE_CHECK={value!r} (expected 1, raise or 0)\n")
        return None
    return "warn" if flag else "off"


def report_target() -> str | None:
    """Where ``RUSTIQUE_REPORT`` sends the report: None, ``"stderr"`` or a path."""
    value = getenv("RUSTIQUE_REPORT")
    if value is None or not value.strip():
        return None
    flag = _flag(value)
    if flag is False:
        return None
    if flag is True or value.strip().lower() == "stderr":
        return "stderr"
    return value.strip().replace("{pid}", str(os.getpid()))


def _matches(t: dict, sel: tuple[str, ...]) -> bool:
    return t["name"] in sel or t["group"] in sel


def _narrow(code: tuple[str, ...] | None, env: tuple[str, ...] | None) -> tuple[str, ...] | None:
    """The intersection of two ``only`` selections (groups or target names)."""
    if env is None:
        return code
    if code is None:
        return env
    targets = _fw.status().get("targets", [])
    if not targets:  # nothing to resolve against: intersect the names
        return tuple(n for n in code if n in env)
    return tuple(t["name"] for t in targets if _matches(t, code) and _matches(t, env))


def _warn_unknown(var: str, sel: tuple[str, ...] | None) -> None:
    if not sel:
        return
    targets = _fw.status().get("targets", [])
    known = {t["name"] for t in targets} | {t["group"] for t in targets}
    unknown = [n for n in sel if n not in known]
    if unknown and targets:
        groups = ", ".join(sorted({t["group"] for t in targets}))
        sys.stderr.write(f"rustique: {var}: no target or group named {', '.join(map(repr, unknown))} "
                         f"(groups: {groups})\n")


def enable(*, only=None, exclude=None, native=None, builtins: bool = True, _source: str = "rustique.enable()") -> dict:
    """Turn Rustique on for this process (idempotent); returns :func:`status`.

    ``only``/``exclude``: groups or target names (an iterable, or a comma-separated
    string). ``native``: module patterns opted into the native tier (docs/TAKEOVER.md).
    ``builtins``: also accelerate the builtin functions ``sum``/``min``/``max``/``sorted``/
    ``any``/``all``. ``RUSTIQUE=0`` makes this a no-op; ``RUSTIQUE_ONLY``/``RUSTIQUE_EXCLUDE``
    narrow the selection; ``RUSTIQUE_STATS``/``RUSTIQUE_REPORT`` switch the counters and
    the report at exit on (module docstring).
    """
    report = report_target()
    if report is not None:
        _register_report(report)
    if killed():
        _state["killed_calls"] += 1
        return status()
    env_only, env_exclude = names(getenv("RUSTIQUE_ONLY")), names(getenv("RUSTIQUE_EXCLUDE")) or ()
    _warn_unknown("RUSTIQUE_ONLY", env_only)
    _warn_unknown("RUSTIQUE_EXCLUDE", env_exclude)
    sel_only = _narrow(names(only), env_only)
    sel_exclude = tuple(dict.fromkeys((names(exclude) or ()) + env_exclude))
    check = check_mode()
    if check is not None:
        _fw.check(check)
    if report is not None or _flag(getenv("RUSTIQUE_STATS")):
        _fw.stats(enabled=True)
    _fw.enable(only=sel_only, exclude=sel_exclude, native=native, builtins=builtins)
    if _state["source"] is None:
        _state["source"] = _source
    _state["only"], _state["exclude"] = sel_only, sel_exclude
    return status()


def disable() -> dict:
    """Restore every original (``rustique.disable()``); returns :func:`status`."""
    _fw.disable()
    _state["source"] = None
    return status()


def status() -> dict:
    """``{"targets": [{name, group, state, reason, sites, versions}], "process": {...}}``
    (docs/ACCELERATION.md §6.2), plus what the switches say (``process["switch"]``)."""
    st = _fw.status()
    proc = st.setdefault("process", {})
    proc.setdefault("python", sys.version.split()[0])
    proc.setdefault("implementation", sys.implementation.name)
    proc.setdefault("free_threaded", not getattr(sys, "_is_gil_enabled", lambda: True)())
    if "rustique" not in proc:
        from rustique._rustique import __version__

        proc["rustique"] = __version__
    proc["switch"] = {
        "source": _state["source"],
        "killed": killed(),
        "killed_calls": _state["killed_calls"],
        "only": _state["only"],
        "exclude": _state["exclude"],
        "stats": bool(_flag(getenv("RUSTIQUE_STATS"))) or _state["report"] is not None,
        "report": _state["report"],
        "check": proc.get("check"),
        "env": {k: v for k in VARIABLES if (v := getenv(k)) is not None},
    }
    return st


def stats(enabled: bool | None = None, reset: bool = False) -> dict:
    """Per-target counters ``{name: {"hits": n, "fallback": {reason: n}}}``. ``enabled``
    switches counting on or off (off by default; ``RUSTIQUE_STATS=1``); ``reset`` zeroes
    them after the snapshot."""
    return _fw.stats(enabled=enabled, reset=reset)


def native(obj=None, /):
    """Decorator: rewrite one function (or the methods of a class) to build Rustique
    containers (docs/TAKEOVER.md). Under ``RUSTIQUE=0`` it returns ``obj`` unchanged."""
    if killed():
        return (lambda o: o) if obj is None else obj
    from rustique._takeover import native as _native

    return _native(obj)


def startup() -> None:
    """``RUSTIQUE=1`` at interpreter startup (called by ``_rustique_startup``)."""
    enable(_source="RUSTIQUE=1")


# --- report at exit --------------------------------------------------------------------------


def report_to(target: str) -> None:
    """Print (``"stderr"``) or append (a path) the report at exit; switches counters on."""
    _register_report(target)
    _fw.stats(enabled=True)


def _register_report(target: str) -> None:
    if _state["report"] is None:
        import atexit

        atexit.register(_report_at_exit)
    _state["report"] = target


def record() -> dict:
    """This process's status and counters, as one JSON-able record (``RUSTIQUE_REPORT=x.json``)."""
    return {"pid": os.getpid(), "argv": list(sys.argv), "status": status(),
            "stats": stats(), "counters": _fw.counters_available()}


def _report_at_exit() -> None:
    target = _state["report"]
    if target is None:
        return
    try:
        rec = record()
        if target.endswith(".json"):
            import json

            text = json.dumps(rec, default=list) + "\n"
        else:
            from rustique import _report

            text = _report.render(rec)
        if target == "stderr":
            sys.stderr.write(text)
            sys.stderr.flush()
        else:
            fd = os.open(target, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
            try:
                os.write(fd, text.encode())
            finally:
                os.close(fd)
    except Exception as exc:  # never fail the program's exit
        sys.stderr.write(f"rustique: report failed: {type(exc).__name__}: {exc}\n")
