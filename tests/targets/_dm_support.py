"""Shared helpers of the data-model groups' tests (`enum`, `generated`, `contextlib`).

`both(scenario)` runs `scenario()` twice in this process — once with every target of the
groups disabled (CPython's own code) and once enabled — and returns both logs. A scenario
defines its classes itself (generated methods get their kernels when the class is created
while the group is enabled) and records what user code can observe: results (repr, type,
identity relations), exceptions (type, message, the function names of the traceback),
side-effect logs of user hooks.
"""

from __future__ import annotations

import re
import sys
import traceback
from collections.abc import Callable
from typing import Any

from rustique import _rustique
from rustique import _takeover as rt

A = _rustique._accel
GROUPS = ["enum", "generated", "contextlib", "functools"]


def group_targets(group: str) -> dict[str, dict[str, Any]]:
    return {t["name"]: t for t in rt.status()["targets"] if t["group"] == group}


FREE_THREADED = not getattr(sys, "_is_gil_enabled", lambda: True)()
#: Targets not validated on free-threaded builds this wave (refused there by the gate).
NOT_ON_FT = {
    "enum.property.__get__",
    # dynamic sites (PyFunction_SetVectorcall stops the world on free-threaded builds)
    "collections.namedtuple",
    "dataclasses.__init__",
    "dataclasses.__init__.positional",
    "contextlib.contextmanager",
    "functools.singledispatch",
}
ft_only_skip = __import__("pytest").mark.skipif(FREE_THREADED, reason="target not validated on free-threaded builds this wave")


def enable(groups: list[str] | None = None) -> None:
    groups = groups or GROUPS
    rt.enable(only=groups)
    for g in groups:
        for name, t in group_targets(g).items():
            if FREE_THREADED and name in NOT_ON_FT:
                assert t["state"] == "refused", (name, t)
                continue
            assert t["state"] in ("installed", "pending-import"), (name, t)


def disable() -> None:
    rt.disable()


def outcome(fn: Callable[[], Any]) -> tuple[Any, ...]:
    """What a call shows: its result (repr and type name) or its exception (type, message,
    traceback function names below this frame)."""
    try:
        r = fn()
    except Exception as e:  # noqa: BLE001 - compared, not handled
        frames = [f.name for f in traceback.extract_tb(e.__traceback__)[1:]]
        return ("raise", type(e).__name__, norm(str(e)), tuple(frames))
    return ("ok", norm(repr(r)), type(r).__name__)


def norm(text: str) -> str:
    """A repr without object addresses."""
    return re.sub(r" at 0x[0-9a-f]+", " at 0x?", text)


def both(scenario: Callable[[], list[Any]], groups: list[str] | None = None) -> tuple[list[Any], list[Any]]:
    disable()
    plain = scenario()
    enable(groups)
    try:
        fast = scenario()
    finally:
        disable()
    return plain, fast


def hits(name: str) -> int:
    return A.stats().get(name, {"hits": 0})["hits"]


class Counting:
    """Counter deltas over a block (counting switched on for the block)."""

    def __init__(self, *names: str) -> None:
        self.names = names

    def __enter__(self) -> Counting:
        A.stats(True)
        self.base = {n: dict(A.stats().get(n, {"hits": 0, "fallback": {}})) for n in self.names}
        return self

    def __exit__(self, *exc: Any) -> None:
        self.end = {n: dict(A.stats().get(n, {"hits": 0, "fallback": {}})) for n in self.names}
        A.stats(False)

    def hits(self, name: str) -> int:
        now = A.stats().get(name, {"hits": 0}) if not hasattr(self, "end") else self.end[name]
        return now["hits"] - self.base[name]["hits"]

    def fallbacks(self, name: str) -> int:
        now = A.stats().get(name, {"fallback": {}}) if not hasattr(self, "end") else self.end[name]
        return sum(now["fallback"].values()) - sum(self.base[name].get("fallback", {}).values())
