"""Adapter between the user-facing layer (``rustique._switch``) and the acceleration
framework (``rustique._takeover``, owned by accel-core; docs/ACCELERATION.md §6, §9).

The framework API this layer is written against::

    enable(*, only=None, exclude=None, native=None, builtins=True) -> dict
    disable() -> dict
    status() -> {"targets": [{name, group, state, reason, sites, versions}], "process": {...}}
    stats(enabled=None, reset=False) -> {name: {"hits": n, "fallback": {reason: n}}}

Until the target registry lands, ``rustique._takeover`` has the wave-0 API (``enable(native=,
builtins=)``, a ``status()`` of ``{"safe", "kernels", "builtins", "native"}`` and no counters);
this module emulates the new API on top of it, with the existing kernels as targets of the
groups ``list``, ``tuple`` and ``builtins``. Delete the legacy branches once the registry is
merged.
"""

import sys

from rustique import _takeover as _fw

#: True when the framework implements the contract API itself.
NATIVE_API = hasattr(_fw, "stats")

# Wave-0 kernels as targets: (name, group, sites).
_LEGACY_TAKEOVER = (
    ("list.sort", "list", ["list.sort (ml_meth)"]),
    ("list.index", "list", ["list.index (ml_meth)"]),
    ("list.count", "list", ["list.count (ml_meth)"]),
    ("x in list", "list", ["list.sq_contains"]),
    ("tuple.index", "tuple", ["tuple.index (ml_meth)"]),
    ("tuple.count", "tuple", ["tuple.count (ml_meth)"]),
    ("x in tuple", "tuple", ["tuple.sq_contains"]),
)
_LEGACY_BUILTINS = ("sum", "min", "max", "sorted", "any", "all")


def _legacy_targets() -> list[dict]:
    out = [{"name": n, "group": g, "sites": s} for n, g, s in _LEGACY_TAKEOVER]
    out += [{"name": f"builtins.{n}", "group": "builtins", "sites": [f"builtins.{n} (ml_meth)"]}
            for n in _LEGACY_BUILTINS]
    return out


def _selected(t: dict, only, exclude) -> bool:
    if only is not None and t["name"] not in only and t["group"] not in only:
        return False
    return t["name"] not in exclude and t["group"] not in exclude


def enable(*, only=None, exclude=None, native=None, builtins=True) -> dict:
    if NATIVE_API:
        return _fw.enable(only=only, exclude=exclude, native=native, builtins=builtins)
    # Wave 0: the list/tuple kernels install together; the builtins are one unit.
    exclude = set(exclude or ())
    only = None if only is None else set(only)
    groups = {t["group"] for t in _legacy_targets() if _selected(t, only, exclude)}
    names = {t["name"] for t in _legacy_targets() if _selected(t, only, exclude)}
    takeover = {n for n, _, _ in _LEGACY_TAKEOVER}
    if takeover <= names:
        _fw.enable(native=native, builtins=builtins and "builtins" in groups)
        _LEGACY_REFUSED.clear()
    else:
        for n in takeover & names:
            _LEGACY_REFUSED[n] = "wave-0 kernels install together: select both groups list and tuple"
        if builtins and "builtins" in groups:
            from rustique import _builtins

            _builtins.install()
        if native:
            from rustique._takeover import _hook

            _hook.install((native,) if isinstance(native, str) else tuple(native))
    return status()


_LEGACY_REFUSED: dict[str, str] = {}


def disable() -> dict:
    if NATIVE_API:
        return _fw.disable()
    _fw.disable()
    if "rustique._builtins" in sys.modules:
        sys.modules["rustique._builtins"].uninstall()
    _LEGACY_REFUSED.clear()
    return status()


def status() -> dict:
    st = dict(_fw.status())
    if NATIVE_API:
        return st
    active = set(_fw._kernels.installed())
    if "rustique._builtins" in sys.modules:
        active |= {f"builtins.{n}" for n in sys.modules["rustique._builtins"].installed()}
    versions = [f"{sys.version_info[0]}.{sys.version_info[1]}"]
    targets = []
    for t in _legacy_targets():
        refused = _LEGACY_REFUSED.get(t["name"])
        state = "installed" if t["name"] in active else "refused" if refused else "disabled"
        targets.append({**t, "state": state, "reason": refused if state == "refused" else None,
                        "versions": versions})
    st["targets"] = targets
    st["process"] = {}
    return st


def stats(enabled=None, reset=False) -> dict:
    if NATIVE_API:
        return _fw.stats(enabled=enabled, reset=reset)
    return {}  # wave 0 kernels have no counters


def counters_available() -> bool:
    return NATIVE_API
