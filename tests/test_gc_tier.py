"""The `gc` tier (docs/targets/gc.md): mechanics, criterion (a) and stepping aside (d).

Every test runs its program in a fresh interpreter (the tier acts on the process-wide
collector), with the tier requested through `RUSTIQUE_TIERS=gc` and enabled alone
(`rustique.enable(only=["gc"])`) before the program runs, as a launch-time switch would.
`RQ_GC_TIERS=gc,interp RQ_GC_ONLY=gc,interp` runs the same tests with the `interp` tier on too.
Differential tests run the same program plain and with the tier and compare the output.

Ported from the prototype's `benches/runtime/test_gcadapt.py` (a1 configuration round-trip,
a2 inventory, a3 explicit collections, d stepping aside), plus what the product adds: no
entry in `gc.callbacks` (the forwarder) and truthful `gc.get_threshold()`.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap

import pytest

rustique = pytest.importorskip("rustique")

FT = not getattr(sys, "_is_gil_enabled", lambda: True)()
SKIP = 77
# Refusal categories that mean "not validated for this interpreter": the suite skips.
EXPECTED = ("version", "build", "jit", "platform", "symbol")

PRELUDE = """
import gc, json, os, sys
import rustique
ONLY = __import__("os").environ.get("RQ_GC_ONLY", "gc").split(",")
_accel = rustique._accel
T = _accel._t_gc
PARAMS = {params!r}
if PARAMS:
    T.set_params(**PARAMS)
{before}
rustique.enable(only=ONLY)
_st = {{t["name"]: t for t in rustique.status()["targets"]}}["gc.adaptive"]
if _st["state"] != "installed":
    print(json.dumps({{"refused": _st.get("refusal"), "reason": _st["reason"]}}))
    sys.exit(77)
R = T.report

class Leaf:
    __slots__ = ("a",)
    def __init__(self, a): self.a = a

def grow(n):  # long-lived, acyclic: full collections find nothing -> the policy backs off
    return [[Leaf(i) for i in range(4)] for _ in range(n // 5)]

def steady(rounds=8, base=200_000, peak=600_000):
    # a live heap, then large acyclic structures that reach the old generation and are
    # dropped: full collections find nothing; the heap returns below its peak (free-threaded
    # build: deferral needs room under 1.1x the largest legitimate collection, since its
    # trigger counts net allocations)
    keep = grow(base)
    for _ in range(rounds):
        tmp = grow(peak)
        del tmp
    return keep

class Cyc:
    def __init__(self): self.me = self
"""

PLAIN = """
import gc, json, os, sys
class Leaf:
    __slots__ = ("a",)
    def __init__(self, a): self.a = a
def grow(n):
    return [[Leaf(i) for i in range(4)] for _ in range(n // 5)]
def steady(rounds=8, base=200_000, peak=600_000):
    keep = grow(base)
    for _ in range(rounds):
        tmp = grow(peak)
        del tmp
    return keep
class Cyc:
    def __init__(self): self.me = self
"""


def _env(tier: bool) -> dict:
    env = dict(os.environ)
    env.pop("RUSTIQUE", None)
    env.pop("RUSTIQUE_ONLY", None)
    env.pop("RUSTIQUE_EXCLUDE", None)
    if tier:
        env["RUSTIQUE_TIERS"] = os.environ.get("RQ_GC_TIERS", "gc")
    else:
        env.pop("RUSTIQUE_TIERS", None)
    return env


def run(body: str, *, before: str = "", params: dict | None = None, tier: bool = True, check: bool = True) -> str:
    """Run `body` in a fresh interpreter, with the tier (default) or plain."""
    head = PRELUDE.format(params=params or {}, before=textwrap.dedent(before)) if tier else PLAIN
    src = head + textwrap.dedent(body)
    r = subprocess.run([sys.executable, "-c", src], capture_output=True, text=True, env=_env(tier), timeout=600)
    if r.returncode == SKIP:
        info = json.loads(r.stdout.strip().splitlines()[-1])
        if info["refused"] in EXPECTED:
            pytest.skip(f"gc tier refused here: {info['reason']}")
        pytest.fail(f"gc tier refused unexpectedly: {info}")
    if check:
        assert r.returncode == 0, r.stderr[-4000:]
    return r.stdout


def both(body: str, **kw) -> tuple[str, str]:
    """The same program plain and with the tier."""
    return run(body, tier=False, **kw), run(body, **kw)


# ---------------------------------------------------------------------------------------
# Install, status, the forwarder's invisibility
# ---------------------------------------------------------------------------------------


def test_not_installed_unless_requested():
    src = ("import json, rustique; rustique.enable(); "
           "t = {t['name']: t for t in rustique.status()['targets']}.get('gc.adaptive'); "
           "print(json.dumps(t))")
    r = subprocess.run([sys.executable, "-c", src], capture_output=True, text=True, env=_env(False), timeout=120)
    assert r.returncode == 0, r.stderr
    t = json.loads(r.stdout)
    assert t is not None and t["state"] in ("refused", "disabled"), t
    assert "RUSTIQUE_TIERS=gc" in (t["reason"] or ""), t


def test_installed_armed_and_invisible():
    probe = "print(sys.getrefcount(gc.callbacks), len(gc.get_referrers(gc.callbacks)), len(gc.get_referrers(gc.garbage)))\n"
    out = run(probe + textwrap.dedent("""
        cb = gc.callbacks
        rep = R()
        assert rep["forwarding"] and rep["hooks"] and rep["armed"], rep
        # gcstate->callbacks points at the private list; the program's list is the one the
        # gc module exposes, untouched.
        assert rep["callbacks_ptr"] == rep["private_list"] != id(cb), rep
        assert rep["program_list"] == id(cb), rep
        assert cb == [] and len(gc.callbacks) == 0
        # neither the observer nor its list is visible to the collector's inventory
        ids = {id(o) for o in gc.get_objects()}
        assert rep["private_list"] not in ids
        assert not any(getattr(o, "__name__", "") == "_rustique_gc_observer"
                       for o in gc.get_objects() if callable(o))
        assert gc.get_threshold() == (2000, 10, 10)
    """))
    plain = run(probe, tier=False)
    assert out.split() == plain.split()


# ---------------------------------------------------------------------------------------
# (a1) configuration round-trip: thresholds untouched, deferral through long_lived_total
# ---------------------------------------------------------------------------------------


def test_threshold_truthful_while_deferring():
    out = run("""
        keep = steady()
        rep = R()
        assert rep["armed"] and rep["k"] > 0, rep
        assert rep["dirty"] and rep["long_lived_total"] > rep["real_total"], rep
        assert gc.get_threshold() == (2000, 10, 10) == tuple(rep["thresholds"]), rep
        assert gc.isenabled() and gc.get_debug() == 0 and gc.get_freeze_count() == 0
        print(rep["k"])
    """)
    assert int(out) > 0


def test_reinflated_after_every_full_collection():
    run("""
        keep = steady()
        for _ in range(3):
            assert R()["k"] > 0, R()
            gc.collect()          # CPython resets long_lived_total to the survivors ...
            rep = R()
            if not FT:            # ... and the observer's stop callback re-applies the policy
                assert rep["long_lived_total"] == rep["real_total"] << rep["k"], rep
            else:
                assert rep["long_lived_total"] >= rep["real_total"], rep
            assert rep["dirty"] == (rep["long_lived_total"] != rep["real_total"]), rep
    """.replace("FT", repr(FT)))


def test_defers_full_collections():
    body = """
        keep = steady(rounds=20)
        print(gc.get_stats()[0 if {ft} else 2]["collections"])
    """.format(ft=FT)
    plain, tier = both(body)
    assert int(tier) < int(plain), (plain, tier)


# ---------------------------------------------------------------------------------------
# (a2) inventory, (a3) explicit collections
# ---------------------------------------------------------------------------------------


def test_no_freeze_inventory_exact():
    run("""
        keep = [grow(200_000) for _ in range(6)]
        assert gc.get_freeze_count() == 0
        x = Leaf(1); holder = [x]
        assert any(o is holder for o in gc.get_referrers(x))
        assert any(o is holder for o in gc.get_objects())
    """)


def test_explicit_collect_is_cpythons():
    run("""
        keep = [grow(200_000) for _ in range(6)]
        assert R()["armed"]
        junk = [Cyc() for _ in range(5000)]
        del junk
        n = gc.collect()
        assert n >= 5000, n
        assert gc.collect() == 0
        for g in (0, 1, 2):
            before = gc.get_stats()[g]["collections"]
            gc.collect(g)
            assert gc.get_stats()[g]["collections"] == before + 1
        assert R()["armed"]   # explicit collections never make the policy step aside
    """)


# ---------------------------------------------------------------------------------------
# (d) stepping aside
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("call,expect,why", [
    ("gc.set_threshold(5000)", (5000, 10, 10), "gc.set_threshold"),
    ("gc.set_threshold(700, 20, 30)", (700, 20, 30), "gc.set_threshold"),
    ("gc.disable(); gc.enable()", (2000, 10, 10), "gc.disable"),
    ("gc.enable()", (2000, 10, 10), "gc.enable"),
    ("gc.freeze(); gc.unfreeze()", (2000, 10, 10), "gc.freeze"),
    ("gc.unfreeze()", (2000, 10, 10), "gc.unfreeze"),
    ("gc.set_debug(0)", (2000, 10, 10), "gc.set_debug"),
    ("from gc import set_threshold as st; st(3000)", (3000, 10, 10), "gc.set_threshold"),
])
def test_step_aside_on_each_tuning_call(call, expect, why):
    out = run(f"""
        keep = steady()
        assert R()["k"] > 0 and R()["dirty"], R()
        {call}
        rep = R()
        assert not rep["active"] and not rep["armed"], rep
        assert rep["stepped_aside"] == {why!r}, rep
        # CPython's own value is back at once, and the thresholds are exactly the program's
        assert not rep["dirty"] and rep["long_lived_total"] == rep["real_total"], rep
        assert tuple(gc.get_threshold()) == {expect!r} == tuple(rep["thresholds"]), rep
        keep2 = steady(rounds=4)
        assert not R()["dirty"] and not R()["armed"], R()
        print(rep["stepped_aside"])
    """)
    assert out.strip() == why


def test_set_threshold_errors_are_cpythons_and_do_not_step_aside():
    body = """
        out = []
        for bad in ((), (1, 2, 3, 4), ("x",), (2**40,)):
            try:
                gc.set_threshold(*bad)
            except Exception as e:
                out.append((type(e).__name__, str(e)))
        try:
            gc.set_debug("x")
        except Exception as e:
            out.append((type(e).__name__, str(e)))
        print(json.dumps(out))
    """
    plain, tier = both(body)
    assert plain == tier
    run(textwrap.dedent(body) + "\nassert R()['armed'] and R()['stepped_aside'] is None, R()\n")


@pytest.mark.parametrize("before,why", [
    ("gc.disable()", "GC disabled before the tier was enabled"),
    ("gc.set_threshold(5000)", "thresholds changed before the tier was enabled"),
    ("gc.set_debug(gc.DEBUG_SAVEALL); gc.set_debug(gc.DEBUG_SAVEALL)", "debug flags set before the tier was enabled"),
    ("gc.freeze()", "objects frozen before the tier was enabled"),
])
def test_passive_when_tuned_before_enable(before, why):
    out = run("""
        rep = R()
        assert rep["forwarding"] and not rep["armed"], rep
        print(rep["stepped_aside"])
    """, before=before)
    assert out.strip() == why


def test_hooks_do_nothing_in_a_subinterpreter():
    run("""
        try:
            from concurrent import interpreters
        except ImportError:
            import _interpreters as _i
            iid = _i.create()
            _i.exec(iid, "import gc; gc.set_threshold(5000); gc.disable(); gc.enable()")
        else:
            it = interpreters.create()
            it.exec("import gc; gc.set_threshold(5000); gc.disable(); gc.enable()")
        rep = R()
        assert rep["armed"] and rep["stepped_aside"] is None, rep
        assert gc.get_threshold() == (2000, 10, 10)
    """)


# ---------------------------------------------------------------------------------------
# disable() / enable() / exit
# ---------------------------------------------------------------------------------------


def test_disable_restores_and_reenable_rearms():
    run("""
        keep = steady()
        assert R()["dirty"], R()
        cb = gc.callbacks
        rustique.disable()
        rep = R()   # everything restored at once (on_disable)
        assert not rep["hooks"] and not rep["armed"], rep
        assert not rep["dirty"] and rep["long_lived_total"] == rep["real_total"], rep
        assert not rep["forwarding"] and rep["callbacks_ptr"] == id(cb), rep
        hits = []
        gc.callbacks.append(lambda p, i: hits.append(p))
        gc.collect(0)
        assert hits == ["start", "stop"], hits
        gc.set_threshold(2000, 10, 10)           # plain CPython function again: no step-aside
        assert R()["stepped_aside"] is None
        rustique.enable(only=ONLY)
        rep = R()
        assert rep["armed"] and rep["k"] == 0 and rep["forwarding"], rep
        assert rep["callbacks_ptr"] == rep["private_list"] and rep["program_list"] == id(cb), rep
        gc.collect(0)
        assert hits == ["start", "stop"] * 2, hits
    """)


def test_exit_restores_the_program_list():
    out = run("""
        keep = steady()
        print(json.dumps(R()["dirty"]))
    """, before="""
        import atexit
        cb0 = gc.callbacks
        def last():   # registered before rustique's exit hook: runs after it (LIFO)
            rep = T.report()
            print(json.dumps([rep["forwarding"], rep["callbacks_ptr"] == id(cb0), rep["dirty"]]))
        atexit.register(last)
    """)
    assert out.split() == ["true", '[false,', "true,", "false]"], out


def test_exit_hook_releases_everything():
    run("""
        A = rustique._accel
        keep = steady()
        cb = gc.callbacks
        A._exit_hook()
        rep = R()
        assert not rep["forwarding"] and rep["callbacks_ptr"] == id(cb), rep
        assert not rep["dirty"] and not rep["armed"], rep
        hits = []
        gc.callbacks.append(lambda p, i: hits.append(p))
        gc.collect()
        assert hits == ["start", "stop"]
    """)


def test_reload_gc_keeps_the_program_list():
    run("""
        import importlib
        cb = gc.callbacks
        hits = []
        def a(p, i): hits.append(("a", p))
        cb.append(a)
        gc2 = importlib.reload(gc)
        late = gc2.callbacks
        def b(p, i): hits.append(("b", p))
        late.append(b)          # possibly the private list until the next collection
        gc.collect()
        assert gc.callbacks is cb, "program list not restored after reload"
        gc.collect()
        assert hits == [("a", "start"), ("b", "start"), ("a", "stop"), ("b", "stop")] * 2, hits
    """)


def test_disable_from_inside_a_callback():
    run("""
        order = []
        def a(p, i):
            order.append(("a", p))
            if p == "start" and R()["armed"]:
                rustique.disable()   # a collection is running: the swap back waits
        def b(p, i): order.append(("b", p))
        gc.callbacks.extend([a, b])
        gc.collect(0); gc.collect(0)
        assert order == [("a", "start"), ("b", "start"), ("a", "stop"), ("b", "stop")] * 2, order
        rep = R()
        assert not rep["armed"] and not rep["dirty"] and rep["forwarding"], rep
        rustique.enable(only=ONLY)
        rustique.disable()
        rep = R()
        assert not rep["forwarding"] and rep["callbacks_ptr"] == id(gc.callbacks), rep
    """)


def test_installs_without_importing_gc():
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    env["RUSTIQUE_TIERS"] = os.environ.get("RQ_GC_TIERS", "gc")
    src = textwrap.dedent("""
        import sys, json
        import rustique
        ONLY = __import__("os").environ.get("RQ_GC_ONLY", "gc").split(",")
        rustique.enable(only=ONLY)
        st = {t["name"]: t for t in rustique.status()["targets"]}["gc.adaptive"]
        if st["state"] != "installed":
            print(json.dumps({"refused": st.get("refusal"), "reason": st["reason"]})); sys.exit(77)
        assert "gc" not in sys.modules, "the tier imported gc"
        T = rustique._accel._t_gc
        assert T.report()["armed"], T.report()
        import gc                      # the hooks are in the static table every gc module uses
        gc.set_threshold(1000)
        rep = T.report()
        assert rep["stepped_aside"] == "gc.set_threshold" and gc.get_threshold() == (1000, 10, 10), rep
    """)
    r = subprocess.run([sys.executable, "-c", src], capture_output=True, text=True, env=env, timeout=120)
    if r.returncode == SKIP:
        info = json.loads(r.stdout.strip().splitlines()[-1])
        if info["refused"] in EXPECTED:
            pytest.skip(f"gc tier refused here: {info['reason']}")
    assert r.returncode == 0, r.stderr[-3000:]
