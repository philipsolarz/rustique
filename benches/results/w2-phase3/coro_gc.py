"""bm_coroutines' body at module level in a pyperformance-like process (sitecustomize enables
Rustique when RUSTIQUE_ENABLE=1): tracked objects, collections, and time with the GC on / off."""
import gc
import statistics
import sys
import time

import pyperf  # noqa: F401  (as the benchmark process)


async def fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return await fibonacci(n - 1) + await fibonacci(n - 2)


def run(loops):
    t0 = time.perf_counter()
    for _ in range(loops):
        coro = fibonacci(25)
        try:
            while True:
                coro.send(None)
        except StopIteration:
            pass
    return time.perf_counter() - t0


run(3)
gc.collect()
objs = len(gc.get_objects())
c0 = [s["collections"] for s in gc.get_stats()]
on = statistics.median(run(5) for _ in range(9))
c1 = [s["collections"] for s in gc.get_stats()]
gc.disable()
off = statistics.median(run(5) for _ in range(9))
print(f"{sys.argv[1]:8s} tracked {objs:7d}  collections during 45 loops {[b - a for a, b in zip(c0, c1)]}  "
      f"gc on {on / 5 * 1e3:6.2f} ms/loop  gc off {off / 5 * 1e3:6.2f} ms/loop  rustique {'rustique' in sys.modules}")
