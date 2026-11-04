"""Steady-state driver for profiling one workload with `perf` (setup and warm-up excluded).

    perf record -F 1999 -g --control fifo:CTL,ACK -D -1 -o out.data -- \
        python -X perf benches/ft/perfdrive.py data/state_snapshot -n 5 [--enable] --ctl CTL,ACK
    perf stat -e cycles,instructions,... --control fifo:CTL,ACK -D -1 -- python benches/ft/perfdrive.py ...

`setup()` and one warm-up `run()` happen before recording starts: the driver writes
`enable` to perf's control FIFO, runs `n` timed `run()`s, writes `disable`, and prints the
times. `--enable` calls `rustique.enable()` before the workload is imported. Without
`--ctl` it just runs (and prints the times), for a timing-only comparison. The control
FIFOs are created by `benches/ft/perfprof.sh`.
"""

import argparse
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "workloads"))


def ctl(paths, word):
    if not paths:
        return
    c, a = paths
    with open(c, "w") as f:
        f.write(word + "\n")
    with open(a) as f:
        f.readline()  # "ack"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("workload")
    ap.add_argument("-n", type=int, default=5)
    ap.add_argument("--enable", action="store_true")
    ap.add_argument("--ctl", help="CTL_FIFO,ACK_FIFO")
    a = ap.parse_args()
    if a.enable:
        import rustique

        rustique.enable()
    import run as wl_run

    mod = wl_run.load(a.workload)
    state = mod.setup()
    mod.run(state)
    paths = a.ctl.split(",") if a.ctl else None
    ctl(paths, "enable")
    times = []
    for _ in range(a.n):
        t0 = time.perf_counter()
        r = mod.run(state)
        times.append(time.perf_counter() - t0)
    ctl(paths, "disable")
    print(json.dumps({"workload": a.workload, "enabled": a.enable, "times": times, "digest": mod.digest(r)}))


if __name__ == "__main__":
    main()
