"""The data-model kernels and the opt-in `interp` tier (`interp.setattr`'s
`object.__setattr__` fast path, `interp.kwinit`'s keyword instantiation, `interp.attr`)
active together (`RUSTIQUE_TIERS=interp`): same observable results as plain CPython, and
each call handled by one of them only — a class whose generated `__init__` carries the
dataclass kernel is not a plain-`__init__` class for `interp.kwinit` (its vectorcall is not
the default one; a routed class's `tp_alloc` is not `PyType_GenericAlloc`), so keyword
calls would reach the kernel; but while `interp.setattr` and `interp.kwinit` are live the
tier runs CPython's generated `__init__` faster than the kernel (keyword calls pushed
inline, `object.__setattr__` fast path), so the dataclass hook leaves new classes to it."""

from __future__ import annotations

import json
import os
import subprocess
import sys

import pytest

HERE = os.path.dirname(os.path.abspath(__file__))


def run(mode: str) -> dict:
    env = dict(os.environ)
    env.pop("RUSTIQUE_TIERS", None)
    r = subprocess.run([sys.executable, os.path.join(HERE, "_dm_interp_scenario.py"), mode],
                       capture_output=True, text=True, env=env, timeout=300)
    assert r.returncode == 0, r.stderr[-3000:]
    return json.loads(r.stdout.strip().splitlines()[-1])


def interp_available() -> bool:
    import rustique

    rustique.enable(only=["interp"], tiers=["interp"])
    try:
        st = {t["name"]: t["state"] for t in rustique.status()["targets"] if t["name"].startswith("interp.")}
    finally:
        rustique.disable()
    return st.get("interp.setattr") == "installed"


@pytest.mark.skipif(not getattr(sys, "_is_gil_enabled", lambda: True)(), reason="dataclass kernels: GIL builds this wave")
def test_dataclass_kernels_and_interp_tier_compose():
    if not interp_available():
        pytest.skip("interp tier not available on this build")
    plain, dm, interp, both = (run(m) for m in ("plain", "dm", "interp", "both"))
    # Same observable results (values, errors, tracebacks, __post_init__ and __setattr__
    # side effects) in all four modes.
    assert plain["log"] == dm["log"] == interp["log"] == both["log"]
    # With interp.setattr and interp.kwinit live, new frozen dataclasses keep CPython's
    # generated __init__ (the tier runs it faster than the kernel would: no double
    # handling); every other data-model kernel handles exactly the calls it handles without
    # the tier (the handlers count nothing themselves; their effect is measured in
    # benches/targets/datamodel_interp_cells.py).
    assert dm["counters"]["dataclasses.__init__"][0] > 0
    assert both["counters"]["dataclasses.__init__"] == [0, 0]
    assert both["counters"]["dataclasses.__init__.positional"] == [0, 0]
    others = lambda c: {k: v for k, v in c.items() if not k.startswith("dataclasses.")}
    assert others(both["counters"]) == others(dm["counters"])
