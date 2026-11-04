"""Adversarial programs for GC policies (gcprobe-compatible: setup/run/digest).

Each run() first builds a long-lived structure that produces no cyclic garbage (so a
yield-driven policy backs off as far as it can), then switches phase and produces cyclic
garbage that lives long enough to reach the oldest generation before it dies (so only a
full collection can free it). A sample of the garbage cycles carries weakref callbacks: the
callback runs inside the collection that frees the cycle, so `lag` = time from the moment a
cycle became unreachable to the moment it was collected (gcprobe reads MODULE.LAST).

Select the scenario with the RQ_ADV environment variable:
  late_cycles    build 600k-object tree, then 400k self-referencing nodes kept in a FIFO
                 window of 60k (they survive young and middle collections) and dropped
  burst          as late_cycles but the garbage comes in one burst: a 300k-node cyclic
                 graph built, used, dropped, three times
  steady         no build phase: a 40k-node cyclic window over 1M nodes (GC-heavy steady
                 state from the start; the policy must stay at CPython's rule)
"""

from __future__ import annotations

import collections
import hashlib
import os
import time
import weakref

SCEN = os.environ.get("RQ_ADV", "late_cycles")
LAST = {}


class Node:
    __slots__ = ("me", "payload", "__weakref__")

    def __init__(self, payload):
        self.payload = payload
        self.me = self  # a cycle: only the cyclic GC frees it


class Leaf:
    __slots__ = ("a", "b")

    def __init__(self, a, b):
        self.a = a
        self.b = b


def setup():
    return {"scen": SCEN}


def _build(n):
    # long-lived, acyclic, GC-tracked: lists of Leaf objects inside a dict
    return {i: [Leaf(i, j) for j in range(5)] for i in range(n // 6)}


def run(state):
    lags = []
    dropped = {}

    def cb(ref, _dropped=dropped, _lags=lags):
        t = _dropped.pop(id(ref), None)
        if t is not None:
            _lags.append(time.perf_counter() - t)

    refs = set()
    t0 = time.perf_counter()
    keep = None
    total = 0
    if state["scen"] in ("late_cycles", "burst"):
        keep = _build(600_000)
    t_phase = time.perf_counter()
    if state["scen"] == "late_cycles" or state["scen"] == "steady":
        window = collections.deque()
        n, w = (400_000, 60_000) if state["scen"] == "late_cycles" else (1_000_000, 40_000)
        for i in range(n):
            node = Node(i)
            if i % 97 == 0:
                r = weakref.ref(node, cb)
                refs.add(r)
                dropped[id(r)] = None
                window.append((node, r))
            else:
                window.append((node, None))
            if len(window) > w:
                old, r = window.popleft()
                total += old.payload
                if r is not None:
                    dropped[id(r)] = time.perf_counter()
                del old
        window.clear()
        t_end_phase = time.perf_counter()
        for k, v in list(dropped.items()):
            if v is None:
                dropped[k] = t_end_phase
    elif state["scen"] == "burst":
        for _ in range(3):
            nodes = [Node(i) for i in range(300_000)]
            for i in range(0, len(nodes), 97):
                r = weakref.ref(nodes[i], cb)
                refs.add(r)
                dropped[id(r)] = None
            total += sum(nd.payload for nd in nodes)
            # a few allocations so the nodes get promoted before they die
            pad = [Leaf(i, i) for i in range(50_000)]
            del pad
            t_drop = time.perf_counter()
            for k in dropped:
                if dropped[k] is None:
                    dropped[k] = t_drop
            del nodes
            # keep allocating acyclic objects (an application keeps working)
            work = [Leaf(i, i) for i in range(200_000)]
            del work
    t_done = time.perf_counter()
    pending = sum(1 for r in refs if r() is not None)
    LAST.clear()
    LAST.update(scen=state["scen"], lags=sorted(lags), uncollected_at_end=pending,
                sampled=len(refs), phase_s=t_done - t_phase, build_s=t_phase - t0)
    del keep
    return total


def digest(result):
    return hashlib.sha256(str(result).encode()).hexdigest()
