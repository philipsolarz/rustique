"""The configured enable path (docs/USAGE.md): imported by ``rustique`` only when an
``only=``/``exclude=``/``native=`` argument or a ``RUSTIQUE_ONLY``, ``RUSTIQUE_EXCLUDE``,
``RUSTIQUE_STATS``, ``RUSTIQUE_REPORT`` or ``RUSTIQUE_CHECK`` variable is in play, and by the
CLI and the pytest plugin; the common path stays in ``rustique/__init__.py`` (startup budget,
docs/targets/dx.md §1.3).

- The environment narrows, never widens: ``RUSTIQUE_ONLY`` is intersected with ``only=``,
  ``RUSTIQUE_EXCLUDE`` added to ``exclude=``; unknown names in a variable warn on stderr and
  are dropped (unknown names in code raise ``ValueError`` in the framework).
- ``RUSTIQUE_STATS=1`` turns the counters on; ``RUSTIQUE_REPORT`` (``1``/``stderr``, a path,
  ``{pid}``, ``*.json``) registers the report at exit (formatting: ``rustique._report``);
  ``RUSTIQUE_CHECK`` (``1``/``warn``, ``raise``, ``0``) sets check mode.
"""

import os
import sys

import rustique as _r  # already imported; `_r._accel` is looked up per call (tests substitute it)

_ON = ("1", "true", "yes", "on")
_OFF = ("0", "false", "no", "off")


def env(name: str) -> str | None:
    """A ``RUSTIQUE*`` variable, or None (unset, or ``-E``/``-I``)."""
    return None if sys.flags.ignore_environment else os.environ.get(name)


def flag(value: str | None) -> bool | None:
    if value is None:
        return None
    v = value.strip().lower()
    return True if v in _ON else False if v in _OFF else None


def names(value) -> tuple[str, ...] | None:
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


def targets() -> list[tuple[str, str]]:
    return [(t[0], t[1]) for t in _r._accel.targets()]


def known(var: str, sel: tuple[str, ...] | None) -> tuple[str, ...] | None:
    """``sel`` without names no target or group has, with one warning on stderr."""
    if not sel:
        return sel
    ts = targets()
    ok = {n for n, _ in ts} | {g for _, g in ts}
    unknown = [n for n in sel if n not in ok]
    if unknown:
        groups = ", ".join(sorted({g for _, g in ts if not g.startswith("_")}))
        sys.stderr.write(f"rustique: {var}: no target or group named {', '.join(map(repr, unknown))}, ignored "
                         f"(groups: {groups})\n")
    return tuple(n for n in sel if n in ok)


def narrow(code: tuple[str, ...] | None, env_sel: tuple[str, ...] | None) -> tuple[str, ...] | None:
    """The intersection of two ``only`` selections of groups or target names."""
    if env_sel is None:
        return code
    if code is None:
        return env_sel
    return tuple(n for n, g in targets() if (n in code or g in code) and (n in env_sel or g in env_sel))


def check_mode() -> str | None:
    """What ``RUSTIQUE_CHECK`` asks for: ``"warn"``, ``"raise"``, ``"off"`` or None (unset)."""
    value = env("RUSTIQUE_CHECK")
    if value is None or not value.strip():
        return None
    v = value.strip().lower()
    if v in ("warn", "raise"):
        return v
    f = flag(v)
    if f is None:
        sys.stderr.write(f"rustique: ignoring RUSTIQUE_CHECK={value!r} (expected 1, raise or 0)\n")
        return None
    return "warn" if f else "off"


def report_target() -> str | None:
    """Where ``RUSTIQUE_REPORT`` sends the report: None, ``"stderr"`` or a path."""
    value = env("RUSTIQUE_REPORT")
    if value is None or not value.strip():
        return None
    f = flag(value)
    if f is False:
        return None
    if f is True or value.strip().lower() == "stderr":
        return "stderr"
    return value.strip().replace("{pid}", str(os.getpid()))


def enable(only=None, exclude=None, native=None, builtins=True, source="rustique.enable()") -> None:
    """``rustique._enable`` with a selection, the native tier or configuring variables."""
    state = _r._state
    report = report_target()
    if report is not None:
        report_to(report, counters=False)
    if _r._killed():
        state["killed_calls"] += 1
        return
    # Unknown names in RUSTIQUE_ONLY narrow to nothing, never to everything.
    env_only = known("RUSTIQUE_ONLY", names(env("RUSTIQUE_ONLY")))
    env_exclude = known("RUSTIQUE_EXCLUDE", names(env("RUSTIQUE_EXCLUDE")) or ())
    sel_only = narrow(names(only), env_only)
    sel_exclude = tuple(dict.fromkeys((names(exclude) or ()) + env_exclude + (() if builtins else ("builtins",))))
    mode = check_mode()
    accel = _r._accel
    if mode is not None:
        accel.check(mode)
    if report is not None or flag(env("RUSTIQUE_STATS")):
        accel.stats(True)
    accel.enable(None if sel_only is None else list(sel_only), list(sel_exclude), quiet=True)
    if native:
        from rustique._takeover import _hook

        _hook.install((native,) if isinstance(native, str) else tuple(native))
        accel._set_native(list(_hook.patterns()))
    if state["source"] is None:
        state["source"] = source
    state["only"], state["exclude"] = sel_only, sel_exclude


def report_to(target: str, counters: bool = True) -> None:
    """Print (``"stderr"``) or append (a path) the report at exit."""
    state = _r._state
    if state["report"] is None:
        import atexit

        atexit.register(_report_at_exit)
    state["report"] = target
    if counters:
        _r._accel.stats(True)


def record() -> dict:
    """This process's status and counters as one JSON-able record."""
    return {"pid": os.getpid(), "argv": list(sys.argv), "status": _r.status(), "stats": _r.stats(), "counters": True}


def _report_at_exit() -> None:
    target = _r._state["report"]
    if target is not None:
        from rustique import _report

        _report.write(target, record)
