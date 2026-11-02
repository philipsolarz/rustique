"""Pytest configuration shared by the Rustique test-suite.

Hypothesis profiles: ``default`` (Hypothesis defaults, no deadline), ``quick`` (smoke runs
on a busy machine), ``thorough`` (CI / pre-merge); select with e.g.
``HYPOTHESIS_PROFILE=thorough``.
"""

from __future__ import annotations

import os
import sys

import pytest
from hypothesis import HealthCheck, settings

sys.path.insert(0, os.path.dirname(__file__))

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

# Targets validate at install in the test-suite (their tests assert "installed" right after
# enable()); the first-call path has its own tests (tests/test_accel_firstcall.py). Child
# processes inherit the setting through the environment.
os.environ.setdefault("RUSTIQUE_VALIDATE", "eager")
try:
    from rustique import _rustique as _rq

    _rq._accel._set_validation(os.environ["RUSTIQUE_VALIDATE"])
except (ImportError, AttributeError):  # not built, or an older build
    pass


def pytest_report_header(config: pytest.Config) -> list[str]:
    return [f"GIL {'disabled (free-threaded)' if FREE_THREADED else 'enabled'}"]


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "slow: long-running stress tests")
