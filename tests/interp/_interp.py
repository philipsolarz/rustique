"""Shared helpers of the interp tier's tests (docs/targets/interp.md).

The tier runs only on verified python-build-standalone builds of the recorded CPython
releases; everywhere else the tests that need it skip with the tier's own refusal reason.
"""

from __future__ import annotations

import contextlib
import sys
from collections.abc import Iterator
from typing import Any

import pytest

import rustique

acc = rustique._accel


def info() -> dict[str, Any]:
    return acc._t_interp.info()


INFO = info()
VERIFIED = bool(INFO["verified"])
FREE_THREADED = getattr(sys, "_is_gil_enabled", lambda: True)() is False
requires_tier = pytest.mark.skipif(not VERIFIED, reason=f"interp tier refused: {INFO.get('reason')}")

GROUP = next(grp for name, grp, _ in acc.targets() if name.startswith("interp."))
TARGETS = ("interp.isinstance", "interp.attr", "interp.setattr", "interp.kwinit", "interp.getattr",
           "interp.hasattr")


def states() -> dict[str, dict[str, Any]]:
    return {t["name"]: t for t in acc.status()["targets"] if t["name"].startswith("interp.")}


def swapped() -> dict[str, list[str]]:
    return {k: v["swapped"] for k, v in info().get("groups", {}).items()}


def enable(only: list[str] | None = None) -> None:
    names = only or [GROUP]
    try:
        rustique.enable(only=names, tiers=["interp"])
    except TypeError:
        rustique.enable(only=names)


@contextlib.contextmanager
def tier(only: list[str] | None = None) -> Iterator[None]:
    """The tier on (only its own targets) for the block; everything off afterwards."""
    enable(only)
    try:
        yield
    finally:
        rustique.disable()


def expected_installed() -> set[str]:
    out = set(TARGETS)
    if FREE_THREADED:
        out.discard("interp.kwinit")
    return out
