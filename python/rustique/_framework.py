"""The acceleration framework as the user-facing layer (``rustique._switch``) sees it:
``rustique._takeover`` (accel-core; docs/ACCELERATION.md §6, §9)::

    enable(*, only=None, exclude=None, native=None, builtins=True) -> dict
    disable() -> dict
    status() -> {"targets": [{name, group, state, reason, sites, versions, ...}], "process": {...}}
    stats(enabled=None, reset=False) -> {name: {"hits": n, "fallback": {reason: n}}}

One indirection, so that the switch logic can be tested against a fake framework.
"""

from rustique import _takeover as _fw


def enable(*, only=None, exclude=None, native=None, builtins=True) -> dict:
    return _fw.enable(only=None if only is None else list(only), exclude=list(exclude or ()), native=native,
                      builtins=builtins)


def disable() -> dict:
    return _fw.disable()


def status() -> dict:
    return _fw.status()


def stats(enabled=None, reset=False) -> dict:
    return _fw.stats(enabled=enabled, reset=reset)


def check(mode=None):
    """Set the check mode (``"warn"``, ``"raise"``, ``"off"``); returns the previous one."""
    return _fw.check(mode)


def counters_available() -> bool:
    return True
