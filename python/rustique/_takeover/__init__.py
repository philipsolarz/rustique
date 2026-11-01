"""The switch: ``rustique.enable()`` / ``disable()`` / ``status()`` / ``stats()``
(docs/ACCELERATION.md §3, §6, §8).

``enable()`` installs the accelerated *targets* registered in the Rust extension
(``crates/rustique-py/src/targets/``): Rust kernels behind CPython's existing entry points
(builtin methods, type slots, C accelerator functions, pure-Python stdlib functions), each
exact or falling back to CPython's original. Targets are grouped (``containers``,
``builtins``, ``json``, ...); ``only=``/``exclude=`` take group or target names. Targets
whose module is not imported yet are installed when it is (never imported eagerly).

The native tier (``native=``, the ``@native`` decorator) is separate and opt-in: it
rewrites selected modules to build Rustique containers (docs/TAKEOVER.md).

Nothing here imports a module at call time: ``enable()`` and ``status()`` run in the
extension.
"""

import sys
from _collections_abc import Iterable  # = collections.abc.Iterable, without importing `collections`

from rustique import _rustique

_accel = _rustique._accel

__all__ = ["RustiqueCheckError", "check", "disable", "enable", "native", "stats", "status"]

#: Raised by a kernel's result check in ``check("raise")`` mode (an ``AssertionError``).
RustiqueCheckError = _accel.RustiqueCheckError

_state: dict[str, object] = {"native": ()}

_UNSET = object()


def _names(value: str | Iterable[str] | None) -> list[str] | None:
    if value is None:
        return None
    if isinstance(value, str):
        value = value.split(",")
    return [v.strip() for v in value if v and v.strip()]


def enable(
    *,
    only: str | Iterable[str] | None = None,
    exclude: str | Iterable[str] | None = None,
    native: str | Iterable[str] | None = None,
    builtins: bool = True,
) -> dict[str, object]:
    """Turn acceleration on for this process. Idempotent and additive.

    ``only``: group or target names to install (default: every group); ``exclude``: names
    to leave out. Both accept a list or a comma-separated string. ``builtins=False``
    excludes the ``builtins`` group. ``native``: module-name patterns (``fnmatch`` style)
    rewritten to build Rustique containers when imported (opt-in native tier).

    Unknown names raise ``ValueError`` before anything changes. Reads no environment
    variables (the switches of docs/ACCELERATION.md §8 are the DX layer's). Returns
    :func:`status`.
    """
    excl = _names(exclude) or []
    if not builtins:
        excl.append("builtins")
    _accel.enable(_names(only), excl)
    if native:
        from rustique._takeover import _hook

        patterns = (native,) if isinstance(native, str) else tuple(native)
        _hook.install(patterns)
        _state["native"] = _hook.patterns()
    return status()


def disable() -> dict[str, object]:
    """Restore every original entry point and stop rewriting imports (modules already
    rewritten by the native tier keep their rewritten code). Returns :func:`status`."""
    _accel.disable()
    if "rustique._takeover._hook" in sys.modules:
        sys.modules["rustique._takeover._hook"].uninstall()
    _state["native"] = ()
    return status()


def status() -> dict[str, object]:
    """``{"targets": [{"name", "group", "state", "reason", "sites", "versions"}, ...],
    "process": {...}}``. ``state`` is ``"installed"``, ``"pending-import"`` (its module is
    not imported yet), ``"refused"`` (``reason`` says why) or ``"disabled"``."""
    st = _accel.status()
    st["process"]["native"] = tuple(_state["native"])  # type: ignore[arg-type,index]
    return st


def stats(enabled: bool | None = None, reset: bool = False) -> dict[str, dict[str, object]]:
    """Per-target counters ``{name: {"hits": n, "fallback": {reason: n}}}``.

    ``enabled`` switches counting on or off (off by default; ``RUSTIQUE_STATS=1``);
    ``reset`` zeroes the counters after taking the snapshot."""
    return _accel.stats(enabled, reset)


def check(mode: object = _UNSET) -> str | None:
    """Check mode: run the original again after every fast path of a *checkable* target
    and compare. ``"warn"``: a mismatch counts as ``fallback["check"]`` and the original's
    result is returned; ``"raise"``: a mismatch raises :class:`RustiqueCheckError`;
    ``None``/``False``/``"off"``: off. Without an argument, return the current mode;
    otherwise set it and return the previous one."""
    if mode is _UNSET:
        return _accel.check_mode()
    if mode is None or mode is False:
        mode = "off"
    return _accel.check(mode)


def native(obj=None, /):
    """Decorator: rewrite one function (or every function of a class) so that its literals,
    displays, comprehensions and constructor calls build Rustique containers. Needs the
    source (``inspect.getsource``). See docs/TAKEOVER.md."""
    from rustique._takeover import _rewrite

    if obj is None:
        return _rewrite.rewrite_object
    return _rewrite.rewrite_object(obj)
