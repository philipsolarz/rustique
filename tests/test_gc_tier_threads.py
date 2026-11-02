"""The `gc` tier under threads (both builds; the free-threaded collector runs callbacks
with the world restarted, so the observer, the program's callbacks and stepping aside race
for real there).

Each test runs in a fresh interpreter with the tier enabled and checks, at the end, that the
process is consistent: the program's callbacks were all called for every collection they were
registered for, the collector state holds CPython's value unless the policy is armed, and the
program's list is the one `gc.callbacks` shows.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap

import pytest

rustique = pytest.importorskip("rustique")

SKIP = 77
EXPECTED = ("version", "build", "jit", "platform", "symbol")

PRELUDE = """
import gc, json, sys, threading, time
import rustique
T = rustique._accel._t_gc
rustique.enable(only=["gc"])
_st = {t["name"]: t for t in rustique.status()["targets"]}["gc.adaptive"]
if _st["state"] != "installed":
    print(json.dumps({"refused": _st.get("refusal"), "reason": _st["reason"]}))
    sys.exit(77)

class Leaf:
    __slots__ = ("a", "b")
    def __init__(self, a): self.a = a; self.b = None

class Cyc:
    def __init__(self): self.me = self

def work(seconds, seed, base=60_000, cyclic=True):
    keep = [Leaf(i) for i in range(base)]
    end = time.monotonic() + seconds
    n = 0
    while time.monotonic() < end:
        tmp = [Leaf(i) for i in range(20_000)]
        if cyclic and n % 3 == seed % 3:
            for _ in range(2_000):
                Cyc()
        if n % 7 == seed % 7:
            gc.collect(n % 3)
        del tmp
        n += 1
    return n
"""


def run(body: str) -> dict:
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    env["RUSTIQUE_TIERS"] = "gc"
    r = subprocess.run([sys.executable, "-c", PRELUDE + textwrap.dedent(body)], capture_output=True, text=True,
                       env=env, timeout=600)
    if r.returncode == SKIP:
        info = json.loads(r.stdout.strip().splitlines()[-1])
        if info["refused"] in EXPECTED:
            pytest.skip(f"gc tier refused here: {info['reason']}")
        pytest.fail(f"gc tier refused unexpectedly: {info}")
    assert r.returncode == 0, r.stderr[-4000:]
    assert "Exception ignored" not in r.stderr, r.stderr[-4000:]
    return json.loads(r.stdout.strip().splitlines()[-1])


def test_allocating_threads_and_callback_mutation():
    out = run("""
        stop = threading.Event()
        counts = {}
        lock = threading.Lock()
        def make_cb(name):
            def cb(phase, info):
                with lock:
                    counts[(name, phase)] = counts.get((name, phase), 0) + 1
            return cb
        def mutator(k):
            cbs = [make_cb(f"m{k}.{j}") for j in range(4)]
            while not stop.is_set():
                for c in cbs:
                    gc.callbacks.append(c)
                for c in cbs:
                    gc.callbacks.remove(c)
        sentinel = make_cb("sentinel")
        gc.callbacks.append(sentinel)
        workers = [threading.Thread(target=work, args=(3.0, i)) for i in range(8)]
        muts = [threading.Thread(target=mutator, args=(k,)) for k in range(2)]
        for t in workers + muts: t.start()
        for t in workers: t.join()
        stop.set()
        for t in muts: t.join()
        gc.collect()
        rep = T.report()
        starts, stops = counts.get(("sentinel", "start"), 0), counts.get(("sentinel", "stop"), 0)
        print(json.dumps({"starts": starts, "stops": stops, "callbacks": len(gc.callbacks),
                          "first": gc.callbacks[0] is sentinel, "armed": rep["armed"],
                          "dirty": rep["dirty"], "llt": rep["long_lived_total"], "real": rep["real_total"],
                          "fulls": rep["fulls"], "forwarding": rep["forwarding"]}))
    """)
    assert out["starts"] == out["stops"] > 0, out
    assert out["callbacks"] == 1 and out["first"], out
    assert out["forwarding"] and out["armed"], out
    assert out["dirty"] == (out["llt"] != out["real"]), out


def test_step_aside_races_with_collections():
    out = run("""
        def tuner():
            time.sleep(0.5)
            gc.set_threshold(*gc.get_threshold())
        workers = [threading.Thread(target=work, args=(2.0, i, 200_000, False)) for i in range(6)]
        t = threading.Thread(target=tuner)
        for w in workers + [t]: w.start()
        for w in workers + [t]: w.join()
        rep = T.report()
        gc.collect()
        after = T.report()
        print(json.dumps({"stepped": rep["stepped_aside"], "dirty": rep["dirty"], "after_dirty": after["dirty"],
                          "armed": after["armed"], "thr": list(gc.get_threshold()),
                          "llt": after["long_lived_total"], "objs": len(gc.get_objects()),
                          "ft": after["ft"]}))
    """)
    assert out["stepped"] == "gc.set_threshold", out
    assert not out["dirty"] and not out["after_dirty"] and not out["armed"], out
    assert out["thr"] == [2000, 10, 10], out
    if not out["ft"]:
        # after a full collection CPython's long_lived_total is the number of survivors
        assert abs(out["llt"] - out["objs"]) < 1000, out


def test_enable_disable_under_load():
    out = run("""
        stop = threading.Event()
        def loop(i):
            while not stop.is_set():
                work(0.2, i)
        workers = [threading.Thread(target=loop, args=(i,)) for i in range(6)]
        for w in workers: w.start()
        states = []
        for k in range(40):
            rustique.disable()
            time.sleep(0.01)
            rustique.enable(only=["gc"])
            st = {t["name"]: t for t in rustique.status()["targets"]}["gc.adaptive"]
            states.append(st["state"])
            time.sleep(0.01)
        stop.set()
        for w in workers: w.join()
        hits = []
        gc.callbacks.append(lambda p, i: hits.append(p))
        gc.collect()
        rep = T.report()
        print(json.dumps({"installed": states.count("installed"), "n": len(states), "hits": hits,
                          "armed": rep["armed"], "forwarding": rep["forwarding"],
                          "dirty": rep["dirty"], "llt": rep["long_lived_total"], "real": rep["real_total"]}))
    """)
    assert out["installed"] == out["n"], out
    assert out["hits"] == ["start", "stop"], out
    assert out["armed"] and out["forwarding"], out
    assert out["dirty"] == (out["llt"] != out["real"]), out
