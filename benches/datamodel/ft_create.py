"""Free-threaded cost of dynamic sites (PyFunction_SetVectorcall stops the world on
3.15t): the main thread creates classes/decorated functions at run time while `--threads`
workers run Python code; reports creation time and worker throughput, plain vs enabled.

    python benches/datamodel/ft_create.py [--threads 8] [--n 2000] [--kind nt|dc|cm|sd]
"""
import argparse
import collections
import contextlib
import dataclasses
import functools
import json
import subprocess
import sys
import threading
import time


def child(kind, n, nthreads, on):
    if on:
        from rustique import _takeover as T

        T.enable(only=["generated", "contextlib", "functools"])
    stop = threading.Event()
    counts = [0] * nthreads

    def worker(i):
        c = 0
        x = 0
        while not stop.is_set():
            for j in range(1000):
                x += j
            c += 1
        counts[i] = c

    ts = [threading.Thread(target=worker, args=(i,)) for i in range(nthreads)]
    for t in ts:
        t.start()
    time.sleep(0.2)
    c0 = sum(counts)
    t0 = time.perf_counter()
    cpu0 = time.thread_time()
    for i in range(n):
        if kind == "nt":
            P = collections.namedtuple("P", "a b c")
            P(1, 2, 3)
        elif kind == "dc":
            dataclasses.make_dataclass("D", ["x", "y"], frozen=True)(1, 2)
        elif kind == "cm":

            @contextlib.contextmanager
            def cm():
                yield

            with cm():
                pass
        elif kind == "sd":

            @functools.singledispatch
            def f(x):
                return x

            f(1)
    dt = time.perf_counter() - t0
    cpu = time.thread_time() - cpu0
    stop.set()
    for t in ts:
        t.join()
    print(json.dumps({"wall_per": dt / n * 1e6, "cpu_per": cpu / n * 1e6, "worker_iters": sum(counts) - c0, "gil": sys._is_gil_enabled()}))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--n", type=int, default=2000)
    ap.add_argument("--kind", default="nt,dc,cm,sd")
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--child", nargs=4)
    a = ap.parse_args()
    if a.child:
        child(a.child[0], int(a.child[1]), int(a.child[2]), a.child[3] == "1")
        return
    for kind in a.kind.split(","):
        res = {}
        for on in ("0", "1"):
            rows = []
            for _ in range(a.reps):
                r = subprocess.run([sys.executable, __file__, "--child", kind, str(a.n), str(a.threads), on], capture_output=True, text=True, check=True)
                rows.append(json.loads(r.stdout.strip().splitlines()[-1]))
            res[on] = {k: sorted(x[k] for x in rows)[len(rows) // 2] for k in ("wall_per", "cpu_per")}
        print(f"{kind}: plain wall {res['0']['wall_per']:.1f} us cpu {res['0']['cpu_per']:.1f} us | on wall {res['1']['wall_per']:.1f} us cpu {res['1']['cpu_per']:.1f} us"
              f" | ratio wall {res['1']['wall_per'] / res['0']['wall_per']:.3f} cpu {res['1']['cpu_per'] / res['0']['cpu_per']:.3f}", flush=True)


if __name__ == "__main__":
    main()
