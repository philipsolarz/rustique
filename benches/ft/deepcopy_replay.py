"""Replay the `copy.deepcopy` calls of a workload, plain then with Rustique's `copy` group
(the check of `docs/atlas/w2-dx.md` §5.2: sqlglot's calls measured 4.4x slower enabled on
3.15.1t, 0.57 on 3.15.1).

    python benches/ft/deepcopy_replay.py devtools/sqlglot_transpile [-r 5] [--only copy]

One process: the workload's `setup()` and one `run()` with `copy.deepcopy` wrapped (the
wrapper is installed before the workload imports anything, so `from copy import deepcopy`
captures it) record every outermost call's arguments; the wrapper is removed; the recorded
calls are replayed `-r` times plain (best total CPU time, `time.thread_time_ns`), then
after `rustique.enable(only=...)` the same. Prints per-call CPU ns and the ratio, and the
target's counters.
"""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.join(HERE, "..", "workloads")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("workload")
    ap.add_argument("-r", type=int, default=5)
    ap.add_argument("--only", default="copy")
    ap.add_argument("--limit", type=int, default=0, help="replay only the first N calls")
    ap.add_argument("--nogc", action="store_true", help="gc.disable() around the replays")
    ap.add_argument("--cg", action="store_true", help="under callgrind: dump one replay each (plain, enabled)")
    a = ap.parse_args()

    orig = copy.deepcopy
    calls: list[tuple[object, object]] = []
    depth = 0

    def wrapper(x, memo=None, _nil=[]):  # noqa: B006 - mirrors copy.deepcopy's signature
        nonlocal depth
        if depth == 0:
            calls.append((x, memo))
        depth += 1
        try:
            return orig(x, memo)
        finally:
            depth -= 1

    copy.deepcopy = wrapper
    fam, name = a.workload.split("/")
    spec = importlib.util.spec_from_file_location(name, os.path.join(WL, fam, name + ".py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    state = mod.setup()
    mod.run(state)
    copy.deepcopy = orig
    # `from copy import deepcopy` in the workload's libraries captured the wrapper: put the
    # original back wherever a module global holds it.
    for m in list(sys.modules.values()):
        d = getattr(m, "__dict__", None)
        if isinstance(d, dict):
            for k, v in list(d.items()):
                if v is wrapper:
                    d[k] = orig
    # Keep only calls without a caller memo (a caller's memo holds state of that run).
    calls = [(x, m) for x, m in calls if m is None]
    if a.limit:
        calls = calls[: a.limit]

    import gc
    import resource

    usage: dict[str, list[float]] = {}

    def replay(tag: str = "") -> int:
        best = None
        dc = copy.deepcopy
        for _ in range(a.r):
            r0 = resource.getrusage(resource.RUSAGE_THREAD)
            t0 = time.thread_time_ns()
            for x, _m in calls:
                dc(x)
            t = time.thread_time_ns() - t0
            r1 = resource.getrusage(resource.RUSAGE_THREAD)
            if tag:
                u = usage.setdefault(tag, [0.0, 0.0, 0, 0, 0])
                u[0] += r1.ru_utime - r0.ru_utime
                u[1] += r1.ru_stime - r0.ru_stime
                u[2] += r1.ru_minflt - r0.ru_minflt
                u[3] += r1.ru_nvcsw - r0.ru_nvcsw
                u[4] += r1.ru_nivcsw - r0.ru_nivcsw
            best = t if best is None else min(best, t)
        return best

    if a.nogc:
        gc.disable()

    if a.cg:
        if a.nogc:
            import gc

            gc.disable()
        sys.path.insert(0, os.path.join(HERE, "..", "targets"))
        from _callgrind import helper

        h = helper()
        for x, _m in calls:
            copy.deepcopy(x)
        h.start()
        for x, _m in calls:
            copy.deepcopy(x)
        h.stop()
        h.dump("plain")
        import rustique

        rustique.enable(only=a.only.split(","))
        for x, _m in calls:
            copy.deepcopy(x)
        h.start()
        for x, _m in calls:
            copy.deepcopy(x)
        h.stop()
        h.dump("enabled")
        return
    replay()  # warm
    plain = replay("plain")
    import rustique

    rustique.enable(only=a.only.split(","))
    rustique.stats(True)
    replay()
    on = replay("enabled")
    st = {k: v for k, v in rustique.stats().items() if k.startswith("copy.")}
    n = len(calls)
    print(json.dumps({"workload": a.workload, "build": sys.version.split()[0] + ("t" if not sys._is_gil_enabled() else ""),
                      "calls": n, "plain_ns": plain / n, "enabled_ns": on / n, "ratio": on / plain,
                      "rusage(user s, sys s, minflt, vcsw, ivcsw)": usage, "stats": st}))


if __name__ == "__main__":
    main()
