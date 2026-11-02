"""Cost of one stop-the-world on a free-threaded build next to N busy threads, measured
with CPython alone: `f.__defaults__ = ...` stops the world on 3.15t exactly like
PyFunction_SetVectorcall (funcobject.c), whereas `f.__doc__ = ...` does not.

    python benches/datamodel/ft_stw.py [--threads 8] [--n 300]
"""
import argparse
import threading
import time


def run(nthreads, n, attr):
    stop = threading.Event()

    def worker():
        x = 0
        while not stop.is_set():
            for j in range(1000):
                x += j

    ts = [threading.Thread(target=worker) for _ in range(nthreads)]
    for t in ts:
        t.start()
    time.sleep(0.1)

    def f(a=0):
        return a

    t0 = time.perf_counter()
    for i in range(n):
        setattr(f, attr, (i,) if attr == "__defaults__" else str(i))
    dt = (time.perf_counter() - t0) / n
    stop.set()
    for t in ts:
        t.join()
    return dt * 1e6


ap = argparse.ArgumentParser()
ap.add_argument("--threads", type=int, default=8)
ap.add_argument("--n", type=int, default=300)
a = ap.parse_args()
for nt in (0, a.threads):
    print(f"threads={nt:2d}  __doc__ (no STW) {run(nt, a.n, '__doc__'):9.2f} us   __defaults__ (STW) {run(nt, a.n, '__defaults__'):9.2f} us")
