"""Pytest configuration shared by the Rustique test-suite.

See ``_support.py`` for implementation selection (``RUSTIQUE_IMPLS``). Hypothesis
profiles: ``default`` (Hypothesis defaults, no deadline), ``quick`` (smoke runs on a busy
machine), ``thorough`` (CI / pre-merge); select with e.g. ``HYPOTHESIS_PROFILE=thorough``.
"""

from __future__ import annotations

import os
import sys
from types import ModuleType
from typing import Any

import pytest
from hypothesis import HealthCheck, settings

sys.path.insert(0, os.path.dirname(__file__))

from _support import ListImpl, list_impls, load_rustique, rustique_selected  # noqa: E402

settings.register_profile(
    "default",
    deadline=None,
    suppress_health_check=[HealthCheck.too_slow],
)
settings.register_profile(
    "quick",
    max_examples=20,
    stateful_step_count=30,
    deadline=None,
    suppress_health_check=[HealthCheck.too_slow],
)
settings.register_profile(
    "thorough",
    max_examples=600,
    stateful_step_count=80,
    deadline=None,
    suppress_health_check=[HealthCheck.too_slow],
)
settings.load_profile(os.environ.get("HYPOTHESIS_PROFILE", "default"))

FREE_THREADED = bool(getattr(sys, "_is_gil_enabled", lambda: True)() is False)


def pytest_report_header(config: pytest.Config) -> list[str]:
    from _support import selected_kinds

    gil = "disabled (free-threaded)" if FREE_THREADED else "enabled"
    return [f"rustique implementations: {', '.join(selected_kinds())}; GIL {gil}"]


@pytest.fixture(params=list_impls())
def impl(request: pytest.FixtureRequest) -> ListImpl:
    """Every list implementation: builtin, untyped List, typed List[T]."""
    impl: ListImpl = request.param
    if not impl.is_builtin:
        load_rustique()
    return impl


@pytest.fixture(params=list_impls(typed=False))
def uimpl(request: pytest.FixtureRequest) -> ListImpl:
    """Implementations that accept heterogeneous data: builtin list and untyped List."""
    impl: ListImpl = request.param
    if not impl.is_builtin:
        load_rustique()
    return impl


@pytest.fixture
def rs() -> ModuleType:
    """The rustique module (skips when only builtins are selected)."""
    if not rustique_selected():
        pytest.skip("RUSTIQUE_IMPLS selects builtins only")
    return load_rustique()


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "slow: long-running stress tests")


def pytest_collection_modifyitems(config: pytest.Config, items: list[Any]) -> None:
    # Keep thread stress tests last so a hang (guarded by timeouts) cannot mask other results.
    items.sort(key=lambda item: "test_threads" in item.nodeid)
