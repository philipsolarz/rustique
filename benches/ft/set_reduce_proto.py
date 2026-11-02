"""Evidence for the `set.__reduce__` prototype (crates/rustique-py/src/proto/set_reduce.rs).

    python benches/ft/set_reduce_proto.py check            # differential: plain vs enable(only=set), fresh processes
    python benches/ft/set_reduce_proto.py cells [-o x.json]   # callgrind Ir per call, plain vs enabled (GIL build)
    python benches/ft/set_reduce_proto.py time  [-o x.json]   # wall ns per call, plain vs enabled (t build)

`check` prints one digest per side over 2,000 generated sets/frozensets/subclasses: the
reduce tuple's repr, the type, the state, `sys.getsizeof` of the list (capacity), and the
sequence of attribute lookups a logging `__getattribute__` sees; any difference is a bug.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "targets"))

CHECK = r'''
import hashlib, random, sys, copy, pickle
if sys.argv[1] == "on":
    import rustique; rustique.enable(only=["set"])
    assert any(t["name"] == "set.__reduce__" and t["state"] == "installed" for t in rustique.status()["targets"])
log = []
class S(set): pass
class F(frozenset): pass
class Slots(set):
    __slots__ = ("a", "b")
class GS(set):
    def __getstate__(self): log.append("getstate"); return {"custom": len(self)}
class It(set):
    def __iter__(self): log.append("iter"); return super().__iter__()
class Spy(set):
    def __getattribute__(self, name): log.append(name); return super().__getattribute__(name)
class Boom(set):
    def __getstate__(self): raise ValueError("no state")
r = random.Random(7)
h = hashlib.sha256()
def val():
    k = r.randrange(5)
    return [r.randrange(-50, 50), str(r.randrange(100)), r.random(), (1, "t"), None][k]
for i in range(2000):
    n = r.choice([0, 1, 2, 5, 8, 9, 40, 100])
    items = [val() for _ in range(n)]
    cls = r.choice([set, frozenset, S, F, Slots, GS, It, Spy, Boom])
    o = cls(items)
    if cls is S and i % 2: o.x = i; o.y = [i]
    if cls is Slots and i % 3: o.a = i
    log.clear()
    try:
        red = o.__reduce__()
        out = (repr(red), red[0].__name__, sys.getsizeof(red[1][0]), tuple(log))
    except Exception as e:
        out = ("raised", type(e).__name__, str(e), tuple(log))
    h.update(repr(out).encode())
    # callers: deepcopy, copy of subclasses, pickle protocol 2 (all through __reduce_ex__)
    if cls is not Boom:
        h.update(repr(sorted(map(repr, copy.deepcopy(o)))).encode())
        h.update(repr(type(copy.copy(o))).encode())
        h.update(pickle.dumps(o, protocol=2) if cls in (set, frozenset, S, F) else b"")
print(sys.argv[1], h.hexdigest())
'''

CELLS = {
    "set().__reduce__()": "x = set()",
    "{1..5}.__reduce__()": "x = {1, 2, 3, 4, 5}",
    "set(range(100)).__reduce__()": "x = set(range(100))",
    "frozenset(5 str).__reduce__()": "x = frozenset('abcde')",
    "subclass+__dict__.__reduce__()": "class S(set): pass\nx = S([1, 2, 3]); x.k = 1",
}
CALLERS = {
    "copy.deepcopy(frozenset 5)": ("import copy; x = frozenset('abcde')", "copy.deepcopy(x)"),
    "copy.deepcopy({1..5})": ("import copy; x = {1, 2, 3, 4, 5}", "copy.deepcopy(x)"),
    "pickle.dumps(set 5, protocol=2)": ("import pickle; x = {1, 2, 3, 4, 5}", "pickle.dumps(x, protocol=2)"),
}


def cmd_check(a) -> None:
    env = dict(os.environ, PYTHONHASHSEED="0")  # set order of str elements must match across processes
    outs = [subprocess.run([sys.executable, "-c", CHECK, side], capture_output=True, text=True, env=env)
            for side in ("off", "on")]
    for o in outs:
        if o.returncode:
            sys.exit(o.stderr[-2000:])
        print(o.stdout.strip())
    d = [o.stdout.split()[-1] for o in outs]
    print("IDENTICAL" if d[0] == d[1] else "DIFFERENT")
    sys.exit(0 if d[0] == d[1] else 1)


def cells():
    for name, setup in CELLS.items():
        yield name, setup, "x.__reduce__()"
    for name, (setup, stmt) in CALLERS.items():
        yield name, setup, stmt


def cmd_cells(a) -> None:
    from _callgrind import per_call

    res = {}
    for name, setup, stmt in cells():
        p = per_call(stmt, setup=setup)
        e = per_call(stmt, setup=setup, enable=["set"])
        res[name] = {"plain": p, "enabled": e, "ratio": e / p}
        print(f"{name:34s} plain {p:9,.0f} Ir  enabled {e:9,.0f} Ir  ratio {e / p:.3f}", flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)


TIME = r'''
import sys, timeit, json
if sys.argv[1] == "on":
    import rustique; rustique.enable(only=["set"])
setup, stmt = sys.argv[2], sys.argv[3]
ns = {}
exec(setup, ns)
n = 2000
best = min(timeit.repeat(stmt, globals=ns, number=n, repeat=7)) / n * 1e9
print(json.dumps(best))
'''


def cmd_time(a) -> None:
    res = {}
    for name, setup, stmt in cells():
        v = {}
        for side in ("off", "on"):
            r = subprocess.run([sys.executable, "-c", TIME, side, setup, stmt], capture_output=True, text=True)
            if r.returncode:
                sys.exit(r.stderr[-2000:])
            v[side] = json.loads(r.stdout)
        res[name] = {"plain_ns": v["off"], "enabled_ns": v["on"], "ratio": v["on"] / v["off"]}
        print(f"{name:34s} plain {v['off']:9,.0f} ns  enabled {v['on']:9,.0f} ns  ratio {v['on'] / v['off']:.3f}", flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"), indent=1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=["check", "cells", "time"])
    ap.add_argument("-o")
    a = ap.parse_args()
    {"check": cmd_check, "cells": cmd_cells, "time": cmd_time}[a.cmd](a)


if __name__ == "__main__":
    main()
