"""Opt-in tiers (DECISIONS §10.2, §10.3; `TargetSpec.opt_in`), the opcode-handler site kind,
and suspension of any site kind, through the hidden `_selftest` targets."""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap

import pytest

from rustique import _rustique

A = _rustique._accel


@pytest.fixture(autouse=True)
def _cleanup():
    yield
    A.disable(["_selftest"])


def target(name: str) -> dict | None:
    for t in A.status()["targets"]:
        if t["name"] == name:
            return t
    return None


def test_opt_in_target_needs_its_tier():
    A.enable(["_selftest.tier"])
    assert "_selftest.tier" not in A.installed()
    assert A._probe_t(1) == ("orig", 1)
    A.enable(["_selftest.tier"], tiers=["_selftest_tier"])
    assert "_selftest.tier" in A.installed()
    assert A._probe_t(1) == ("kernel", 1)
    t = target("_selftest.tier")
    assert t["tier"] == "_selftest_tier" and t["state"] == "installed"
    assert "_selftest_tier" in A.status()["process"]["tiers"]
    assert target("_selftest.meth") is None or target("_selftest.meth")["tier"] is None


def test_tier_name_works_in_exclude_and_unknown_tiers_raise():
    A.enable(["_selftest.tier"], exclude=["_selftest_tier"], tiers=["_selftest_tier"])
    assert "_selftest.tier" not in A.installed()
    with pytest.raises(ValueError, match="unknown rustique tier"):
        A.enable(["_selftest.tier"], tiers=["nope"])
    A.enable(tiers=["interp", "gc"], only=["_selftest.meth"])  # known tiers, maybe empty here
    assert "_selftest.meth" in A.installed()


def test_full_disable_forgets_the_tiers():
    A.enable(["_selftest.tier"], tiers=["_selftest_tier"])
    installed = [n for n in A.installed() if not n.startswith("_selftest")]
    A.disable()
    try:
        assert A.status()["process"]["tiers"] == []
        A.enable(["_selftest.tier"])
        assert "_selftest.tier" not in A.installed()
    finally:
        if installed:
            A.enable(installed)


def test_opcode_handler_site():
    assert A._selftest_handler() in ("core", "0x0")
    A.enable(["_selftest.opcode"])
    assert A._selftest_handler() == "ours"
    t = target("_selftest.opcode")
    assert t["state"] == "installed" and t["sites"] == ["_selftest.TABLE[1] (opcode-handler)"]
    A.disable(["_selftest.opcode"])
    assert A._selftest_handler() == "core"


def test_any_site_kind_can_be_suspended_and_resumed():
    A.enable(["_selftest.opcode", "_selftest.meth"])
    A._selftest_request_suspend("_selftest.opcode")
    assert A._selftest_handler() == "core"
    assert target("_selftest.opcode")["suspended"] == "fallback"
    A._selftest_request_suspend("_selftest.meth")
    assert A._probe(1) == ("orig", 1)
    A.enable(["_selftest.opcode", "_selftest.meth"])  # the next enable() resumes
    assert A._selftest_handler() == "ours" and A._probe(1) == ("kernel", 1)
    assert "suspended" not in target("_selftest.opcode")


def run(code: str, **env: str) -> subprocess.CompletedProcess[str]:
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE") or k == "RUSTIQUE_VALIDATE"}
    e.update(env)
    return subprocess.run(
        [sys.executable, "-c", textwrap.dedent(code)], capture_output=True, text=True, env=e, timeout=120
    )


SWITCH = """
from rustique import _rustique
A = _rustique._accel
A.switch_enable(only=["_selftest.tier", "_selftest.meth"], tiers={tiers})
print(sorted(n for n in A.installed() if n.startswith("_selftest")), A.status()["process"]["switch"]["tiers"])
"""


@pytest.mark.parametrize(
    ("env", "code_tiers", "installed", "tiers"),
    [
        ({}, None, ["_selftest.meth"], ()),
        ({}, ["_selftest_tier"], ["_selftest.meth", "_selftest.tier"], ("_selftest_tier",)),
        ({"RUSTIQUE_TIERS": "_selftest_tier"}, None, ["_selftest.meth", "_selftest.tier"], ("_selftest_tier",)),
        ({"RUSTIQUE_TIERS": "none"}, ["_selftest_tier"], ["_selftest.meth"], ()),
        ({"RUSTIQUE_TIERS": ""}, ["_selftest_tier"], ["_selftest.meth"], ()),
        ({"RUSTIQUE_TIERS": "0"}, ["_selftest_tier"], ["_selftest.meth"], ()),
        ({"RUSTIQUE_EXCLUDE": "_selftest_tier"}, ["_selftest_tier"], ["_selftest.meth"], ("_selftest_tier",)),
        ({"RUSTIQUE": "0"}, ["_selftest_tier"], [], ()),
    ],
)
def test_rustique_tiers_environment(env, code_tiers, installed, tiers):
    r = run(SWITCH.format(tiers=code_tiers), **env)
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == f"{installed} {tiers}", r.stdout + r.stderr


def test_unknown_environment_tier_warns():
    r = run(SWITCH.format(tiers=None), RUSTIQUE_TIERS="_selftest_tier,bogus")
    assert r.returncode == 0, r.stderr
    assert "RUSTIQUE_TIERS: no tier named 'bogus'" in r.stderr
    assert r.stdout.strip() == "['_selftest.meth', '_selftest.tier'] ('_selftest_tier',)"
