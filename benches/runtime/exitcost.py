#!/usr/bin/env python3
"""Exit-time cost of a workload process, and what exit policies would change.

    python benches/runtime/exitcost.py <family/name> [-p 3] [--modes plain,disable,freeze,osexit]
        (run with the workload's interpreter; spawns one child per mode and round)

Child: setup(), one run(), then the mode's action, then a normal interpreter exit:
  plain    nothing                       (CPython's Py_FinalizeEx: PyGC_Collect, module
                                           teardown + 2x _PyGC_CollectNoFail, interp clear)
  disable  gc.disable()                  (skips PyGC_Collect only: pylifecycle.c:2480 /
                                           gc.c:1794-1808; the shutdown collections still run)
  freeze   gc.freeze()                   (frozen objects are never collected, so cycles among
                                           them are neither traversed nor freed at exit)
  osexit   flush stdout/stderr, os._exit (lower bound: no finalization at all)
The child prints CLOCK_MONOTONIC at the end of its last statement and the duration of the
one collection at exit that invokes gc.callbacks (PyGC_Collect, reason MANUAL; the shutdown
collections skip callbacks, gc.c:1474/1647); the parent takes CLOCK_MONOTONIC when the child
has exited. exit_s = parent - child mark (includes process teardown by the kernel)."""

from __future__ import annotations

import argparse
import json
import os
import statistics as st
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def child(a):
    sys.path.insert(0, HERE)
    import gc

    import gcprobe

    mod = gcprobe.load_workload(a.workload)
    state = mod.setup()
    t0 = time.perf_counter()
    mod.run(state)
    run_s = time.perf_counter() - t0
    info = {"run_s": run_s, "tracked": len(gc.get_objects())}
    marks = []

    def cb(phase, i):
        marks.append((phase, time.monotonic(), i.get("collected", 0), i.get("candidates", 0)))
        if phase == "stop" and marks and marks[-2][0] == "start" and getattr(cb, "armed", False):
            sys.stderr.write(json.dumps({"exit_collect_s": marks[-1][1] - marks[-2][1],
                                         "exit_collected": i.get("collected", 0),
                                         "exit_candidates": i.get("candidates", 0)}) + "\n")
            sys.stderr.flush()

    gc.callbacks.append(cb)
    if a.mode == "disable":
        gc.disable()
    elif a.mode == "freeze":
        gc.freeze()
    cb.armed = True
    info["mark"] = time.monotonic()
    sys.stdout.write(json.dumps(info) + "\n")
    sys.stdout.flush()
    if a.mode == "osexit":
        sys.stderr.flush()
        os._exit(0)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workload")
    ap.add_argument("--child", action="store_true")
    ap.add_argument("--mode", default="plain")
    ap.add_argument("--modes", default="plain,disable,freeze,osexit")
    ap.add_argument("-p", type=int, default=3)
    ap.add_argument("-o")
    a = ap.parse_args()
    if a.child:
        return child(a)
    rows = {}
    for r in range(a.p):
        for m in a.modes.split(","):
            p = subprocess.Popen([sys.executable, os.path.abspath(__file__), a.workload, "--child", "--mode", m],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            out, err = p.communicate()
            t_end = time.monotonic()
            if p.returncode != 0:
                print(f"{m}: exit {p.returncode}\n{err[-800:]}", file=sys.stderr)
                continue
            d = json.loads(out.strip().splitlines()[-1])
            d["exit_s"] = t_end - d["mark"]
            for line in err.splitlines():
                if line.startswith("{\"exit_collect_s\""):
                    d.update(json.loads(line))
            rows.setdefault(m, []).append(d)
    summary = {}
    for m, ds in rows.items():
        summary[m] = {"exit_s": st.median(d["exit_s"] for d in ds), "run_s": st.median(d["run_s"] for d in ds),
                      "exit_collect_s": st.median(d.get("exit_collect_s", 0.0) for d in ds),
                      "exit_candidates": st.median(d.get("exit_candidates", 0) for d in ds),
                      "exit_collected": st.median(d.get("exit_collected", 0) for d in ds),
                      "tracked": st.median(d["tracked"] for d in ds)}
    res = {"workload": a.workload, "python": sys.version.split()[0], "summary": summary, "rows": rows}
    if a.o:
        with open(a.o, "w") as f:
            json.dump(res, f, indent=1)
    for m, s in summary.items():
        print(f"{a.workload:34s} {m:8s} exit {s['exit_s'] * 1e3:7.1f} ms  (PyGC_Collect {s['exit_collect_s'] * 1e3:6.1f} ms, "
              f"{s['exit_collected']:.0f}/{s['exit_candidates']:.0f})  run {s['run_s'] * 1e3:7.1f} ms  tracked {s['tracked']:.0f}")


if __name__ == "__main__":
    main()
