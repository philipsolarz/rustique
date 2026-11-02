"""The 8-thread rule for the `random` kernels (DECISIONS §10.9: CPU time per call at 8
threads ≤ 1.05× the original's on 3.15.1t), on the machinery of
benches/ft/kernel_scaling.py with the `random` cells (per-thread `random.Random(k)`
instances in `local` mode, one generator shared by all threads in `shared` mode, and the
module-level functions, which always share `random._inst`).

    python benches/targets/random_ft.py [-t 1,8] [-r 3] [--cells a,b] -o plain.json
    python benches/targets/random_ft.py --enable [...] -o enabled.json
    python benches/targets/random_ft.py table plain.json enabled.json
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ft"))
import kernel_scaling as ks  # noqa: E402

R5 = "import random; R = random.Random(5)"
W = "W = 'alpha beta gamma delta epsilon release deploy rollback latency throughput customer invoice backlog sprint incident outage database cache queue worker migration feature flag rollout canary regression benchmark memory leak timeout retry checkout payment search index'.split()"
CELLS = {
    "Random().expovariate": (R5, "R.expovariate(2.0)"),
    "Random().paretovariate": (R5, "R.paretovariate(2.0)"),
    "Random().normalvariate": (R5, "R.normalvariate(0.0, 1.0)"),
    "Random().lognormvariate": (R5, "R.lognormvariate(0.0, 0.5)"),
    "Random().gauss": (R5, "R.gauss(90, 60)"),
    "random.expovariate": ("import random", "random.expovariate(2.0)"),
    "random.gauss": ("import random", "random.gauss()"),
    "Random().choices weights": (R5 + "; T4 = ('low', 'medium', 'high', 'critical'); W4 = [50, 30, 15, 5]",
                                 "R.choices(T4, W4)"),
    "Random().choices k=40": (R5 + "; " + W, "R.choices(W, k=40)"),
    "Random().sample": (R5 + "; " + W, "R.sample(W, 5)"),
    "Random().sample(range)": (R5, "R.sample(range(1000000), 20)"),
    "Random().randint": (R5, "R.randint(1, 1000)"),
}

if __name__ == "__main__":
    ks.CELLS.clear()
    ks.CELLS.update(CELLS)
    ks.main()
