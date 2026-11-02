"""Swap the named core opcode handlers for an _rqeval build's and run a small program.

    python benches/interp/rqeval/swaptest.py MOD OP[,OP...]|ALL [--dir /work/w2-interp/rqeval]
"""
import argparse, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rqpatch

ap = argparse.ArgumentParser()
ap.add_argument("mod")
ap.add_argument("ops")
ap.add_argument("--dir", default="/work/w2-interp/rqeval")
a = ap.parse_args()
if a.ops == "ALL":
    mod, table, done = rqpatch.swap_all(a.dir, a.mod)
else:
    mod, table, done = rqpatch.swap(a.dir, a.mod, a.ops.split(","))
print("swapped", len(done), flush=True)


def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)


class P:
    def __init__(self, a):
        self.a = a


def work():
    s = 0
    for i in range(1000):
        s += P(i).a
    d = {str(i): i for i in range(100)}
    try:
        d["x"]
    except KeyError:
        s += 1
    return s, fib(15), sorted(d)[:3], [x * x for x in range(5)], sum(x for x in range(10))


print(work(), flush=True)
