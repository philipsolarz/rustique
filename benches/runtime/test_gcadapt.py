"""Eligibility tests (a) and (d) for the prototype policy, each in a fresh interpreter.

    python -m pytest benches/runtime/test_gcadapt.py -q

(a1) configuration round-trip: get_threshold() reports the program-visible thresholds while
     the policy holds threshold2 (GIL) / threshold0 (FT) elsewhere;
(a2) inventory: the policy never freezes, so get_objects()/get_referrers() see every object;
(a3) explicit gc.collect(g) is CPython's: runs the requested generation, runs while the
     policy defers, and leaves no cyclic garbage (a second collect() returns 0);
(d)  set_threshold / enable / disable / freeze / unfreeze / set_debug end the policy: real
     thresholds become exactly what the program set, the callback is removed.
"""

import os
import subprocess
import sys
import textwrap

HERE = os.path.dirname(os.path.abspath(__file__))

PRELUDE = f"""
import gc, sys
sys.path.insert(0, {HERE!r})
import gcadapt
pol = gcadapt.install()
FT = gcadapt.FT

class Leaf:
    __slots__ = ("a",)
    def __init__(self, a): self.a = a

def grow(n):  # long-lived, acyclic: full collections find nothing -> the policy backs off
    return [[Leaf(i) for i in range(4)] for _ in range(n // 5)]

class Cyc:
    def __init__(self): self.me = self
"""


def run(body):
    src = PRELUDE + textwrap.dedent(body)
    r = subprocess.run([sys.executable, "-c", src], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr[-3000:]
    return r.stdout


def test_threshold_virtualized_while_deferring():
    out = run("""
        keep = [grow(200_000) for _ in range(8)]
        real = gcadapt._real["get_threshold"]()
        print(pol.active, pol.k, gc.get_threshold(), real)
        assert gc.get_threshold() == (2000, 10, 10)
        if pol.k and not FT:
            assert real[2] != 10 or not pol.blocked
    """)
    assert out.startswith("True")


def test_explicit_collect_is_cpythons():
    run("""
        keep = [grow(200_000) for _ in range(6)]
        assert pol.active
        junk = [Cyc() for _ in range(5000)]
        del junk
        n = gc.collect()
        assert n >= 5000, n
        assert gc.collect() == 0
        for g in (0, 1, 2):
            before = gc.get_stats()[g]["collections"]
            gc.collect(g)
            assert gc.get_stats()[g]["collections"] == before + 1
    """)


def test_no_freeze_inventory_exact():
    run("""
        keep = [grow(200_000) for _ in range(6)]
        assert gc.get_freeze_count() == 0
        x = Leaf(1); holder = [x]
        assert any(o is holder for o in gc.get_referrers(x))
        assert any(o is holder for o in gc.get_objects())
    """)


def test_step_aside_on_each_tuning_call():
    for call, expect in (("gc.set_threshold(5000)", (5000, 10, 10)),
                         ("gc.set_threshold(700, 20, 30)", (700, 20, 30)),
                         ("gc.disable(); gc.enable()", (2000, 10, 10)),
                         ("gc.freeze(); gc.unfreeze()", (2000, 10, 10)),
                         ("gc.set_debug(0)", (2000, 10, 10))):
        out = run(f"""
            keep = [grow(200_000) for _ in range(6)]
            {call}
            real = tuple(gcadapt._real["get_threshold"]())
            assert not pol.active, pol.report()
            assert pol._cb not in gc.callbacks
            assert real == {expect!r} == tuple(gc.get_threshold()), (real, gc.get_threshold())
            print(pol.stepped_aside)
        """)
        assert out.strip()


def test_set_threshold_errors_are_cpythons():
    run("""
        for bad in ((), (1, 2, 3, 4), ("x",)):
            try:
                gc.set_threshold(*bad)
            except TypeError:
                pass
            else:
                raise AssertionError(bad)
    """)


def test_not_installed_when_disabled_at_launch():
    src = "import gc, sys; gc.disable(); sys.path.insert(0, %r); import gcadapt; p = gcadapt.install(); assert not p.active" % HERE
    subprocess.run([sys.executable, "-c", src], check=True)
