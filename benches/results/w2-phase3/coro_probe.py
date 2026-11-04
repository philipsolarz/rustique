"""pyperformance's bm_coroutines body, timed in this process after an optional enable().
    python coro_probe.py run MODE      MODE: plain | import | all | only=G1,G2 | exclude=G1,G2
    python coro_probe.py sweep MODES... [-p 5]   (fresh processes, modes interleaved)"""
import os
import statistics
import subprocess
import sys
import time


def child(mode):
    if mode != "plain":
        import rustique
        if mode == "all":
            rustique.enable()
        elif mode.startswith("only="):
            rustique.enable(only=mode[5:].split(","))
        elif mode.startswith("exclude="):
            rustique.enable(exclude=mode[8:].split(","))

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

    run(2)
    ts = [run(3) for _ in range(9)]
    print(statistics.median(ts))


if sys.argv[1] == "run":
    child(sys.argv[2])
else:
    modes = [m for m in sys.argv[2:] if not m.startswith("-p")]
    p = 5
    res = {m: [] for m in modes}
    for r in range(p):
        for m in modes[r % len(modes):] + modes[: r % len(modes)]:
            out = subprocess.run([sys.executable, __file__, "run", m], capture_output=True, text=True)
            if out.returncode:
                print(m, "FAILED", out.stderr[-500:]); continue
            res[m].append(float(out.stdout.strip().splitlines()[-1]))
    base = statistics.median(res["plain"])
    for m in modes:
        v = statistics.median(res[m])
        print(f"{m:40s} {v * 1e3:8.1f} ms  ratio {v / base:.3f}  ({', '.join(f'{x * 1e3:.0f}' for x in res[m])})", flush=True)
