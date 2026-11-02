"""The 8-thread rule (DECISIONS §10.9): CPU time per call at N threads, enabled vs plain,
for each data-model kernel's hot operation (free-threaded builds; also runs on GIL builds).

    python benches/datamodel/ft_scaling.py [--threads 8] [--iters 200000] [--reps 3]
"""
import argparse
import json
import subprocess
import sys
import threading
import time

SETUP = """
import collections, contextlib, dataclasses, enum, functools
class Color(enum.Enum):
    RED = 1
    GREEN = 2
class Perm(enum.Flag):
    R = 4
    W = 2
NT = collections.namedtuple("NT", "a b c")
@dataclasses.dataclass(frozen=True)
class F:
    x: int
    y: int
@contextlib.contextmanager
def cm():
    yield
@functools.singledispatch
def fmt(x):
    return 0
@fmt.register(int)
def _(x):
    return 1
class H:
    @functools.singledispatchmethod
    def handle(self, x):
        return 0
h = H()
m = Color.GREEN
t = (1, 2, 3)
"""
OPS = {
    "m.value": "m.value",
    "Color(2)": "Color(2)",
    "Perm.R | Perm.W": "Perm.R | Perm.W",
    "hash(m)": "hash(m)",
    "NT(1, 2, 3)": "NT(1, 2, 3)",
    "NT._make(t)": "NT._make(t)",
    "F(1, 2)": "F(1, 2)",
    "F(x=1, y=2)": "F(x=1, y=2)",
    "cm()": "cm()",
    "fmt(1)": "fmt(1)",
    "h.handle(1)": "h.handle(1)",
}


def child(op, nthreads, iters, on):
    if on:
        from rustique import _takeover as T

        T.enable(only=["enum", "generated", "contextlib", "functools"])
    ns = {}
    exec(SETUP, ns)
    body = "def run(n):\n    for _ in range(n):\n" + "".join(f"        {OPS[op]}\n" for _ in range(10))
    exec(body, ns)
    run = ns["run"]
    run(1000)
    cpu = [0.0] * nthreads
    barrier = threading.Barrier(nthreads)

    def worker(i):
        barrier.wait()
        c0 = time.thread_time()
        run(iters // 10)
        cpu[i] = time.thread_time() - c0

    ts = [threading.Thread(target=worker, args=(i,)) for i in range(nthreads)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    print(json.dumps({"cpu_per_call_ns": sum(cpu) / (nthreads * iters) * 1e9}))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--iters", type=int, default=200000)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--only", default="")
    ap.add_argument("--child", nargs=4)
    a = ap.parse_args()
    if a.child:
        child(a.child[0], int(a.child[1]), int(a.child[2]), a.child[3] == "1")
        return
    worst = 0
    for op in OPS:
        if a.only not in op:
            continue
        # Plain and enabled interleaved; the minimum over repeats (a shared, loaded box
        # only ever adds time).
        vals = {"0": [], "1": []}
        for _ in range(a.reps):
            for on in ("0", "1"):
                p = subprocess.run([sys.executable, __file__, "--child", op, str(a.threads), str(a.iters), on], capture_output=True, text=True, check=True)
                vals[on].append(json.loads(p.stdout.strip().splitlines()[-1])["cpu_per_call_ns"])
        r = {k: min(v) for k, v in vals.items()}
        ratio = r["1"] / r["0"]
        worst = max(worst, ratio)
        print(f"{op:16s} plain {r['0']:8.1f} ns  on {r['1']:8.1f} ns  ({ratio:.3f})", flush=True)
    print(f"threads={a.threads} gil={getattr(sys, '_is_gil_enabled', lambda: True)()} worst {worst:.3f}")


if __name__ == "__main__":
    main()
