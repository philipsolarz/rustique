"""Threads run Python code while the main thread swaps and restores core opcode handlers.

    python benches/interp/rqeval/ft_race.py MOD OPS|ALL [--threads 4] [--cycles 500] [--dir D]

Each worker checks its own results against values computed before any swap; any mismatch,
exception or crash fails the run. On 3.15.1t this races table writes (atomic release stores of
aligned pointers) against dispatch in running threads.
"""
import argparse, os, sys, threading, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rqpatch

ap = argparse.ArgumentParser()
ap.add_argument("mod"); ap.add_argument("ops")
ap.add_argument("--threads", type=int, default=4)
ap.add_argument("--cycles", type=int, default=500)
ap.add_argument("--dir", default="/work/w2-interp/rqeval")
a = ap.parse_args()


class A:
    def __init__(self, x): self.x = x
    def m(self): return self.x
class B(A): pass
class Other: pass
class Meta(type): pass
class MA(metaclass=Meta): pass


def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)


def work():
    objs = [A(1), B(2), A(3)]
    s = 0
    for i in range(200):
        o = objs[i % 3]
        s += o.x + o.m() + isinstance(o, A) + isinstance(o, Other) + isinstance(o, MA)
        try:
            {}[i]
        except KeyError:
            s += 1
    return s, fib(12), [x * x for x in range(10)], sum(x for x in range(20))


expected = work()
stop = threading.Event()
errors = []
counts = [0] * a.threads


def worker(k):
    try:
        while not stop.is_set():
            r = work()
            if r != expected:
                errors.append((k, r))
                return
            counts[k] += 1
    except BaseException as e:  # noqa
        errors.append((k, repr(e)))


ts = [threading.Thread(target=worker, args=(k,)) for k in range(a.threads)]
for t in ts:
    t.start()
t0 = time.perf_counter()
for c in range(a.cycles):
    if a.ops == "ALL":
        mod, table, done = rqpatch.swap_all(a.dir, a.mod)
    else:
        mod, table, done = rqpatch.swap(a.dir, a.mod, a.ops.split(","))
    time.sleep(0.001)
    rqpatch.unswap(mod, table, done)
stop.set()
for t in ts:
    t.join()
gil = getattr(sys, "_is_gil_enabled", lambda: True)()
print(f"gil={gil} cycles={a.cycles} threads={a.threads} work-iterations={sum(counts)} "
      f"errors={len(errors)} {errors[:3]} wall={time.perf_counter() - t0:.1f}s")
sys.exit(1 if errors else 0)
