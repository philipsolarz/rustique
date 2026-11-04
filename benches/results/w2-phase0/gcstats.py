"""Garbage-collector baseline of ONE steady-state `run()` of a workload (plain CPython).

    python gcstats.py data/state_snapshot OUT.json      (cwd = the repository root)

As benches/workloads/run.py `one` (plain): load the workload, `setup()`, one warm-up
`run()`, then one measured `run()` with a `gc.callbacks` hook that timestamps every
collection's start and stop (`time.perf_counter_ns`). Nothing else changes: no
`gc.collect()` before the run, thresholds as the interpreter sets them.

Records (one JSON object, appended to OUT.json): collections per generation, total and
per-generation time inside collections, objects collected / uncollectable per generation,
the run's wall time and the share of it spent in collections, the `gc.get_stats()` deltas
(a cross-check of the callback counts), peak RSS of the process (`ru_maxrss`, KiB; after
setup and after the run) and of its waited-for children, and the result digest.
"""

import gc
import importlib.util
import json
import os
import resource
import sys
import sysconfig
import threading
import time

name, out = sys.argv[1], sys.argv[2]
path = os.path.join(os.getcwd(), "benches", "workloads", name + ".py")
spec = importlib.util.spec_from_file_location("workload_" + name.replace("/", "_"), path)
mod = importlib.util.module_from_spec(spec)
sys.path.insert(0, os.path.dirname(path))
spec.loader.exec_module(mod)

state = mod.setup()
rss_setup = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
mod.run(state)  # warm-up
rss_warm = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss

events = []          # (generation, duration_ns, collected, uncollectable, extra info)
_start = {}          # thread id -> start ns (a collection runs in one thread at a time)
_lock = threading.Lock()


def _cb(phase, info):
    now = time.perf_counter_ns()
    tid = threading.get_ident()
    if phase == "start":
        _start[tid] = now
    elif phase == "stop":
        t0 = _start.pop(tid, None)
        extra = {k: v for k, v in info.items() if k not in ("generation", "collected", "uncollectable")}
        with _lock:
            events.append((info.get("generation"), (now - t0) if t0 is not None else None,
                           info.get("collected", 0), info.get("uncollectable", 0), extra))


before = gc.get_stats()
gc.callbacks.append(_cb)
t0 = time.perf_counter_ns()
result = mod.run(state)
t1 = time.perf_counter_ns()
gc.callbacks.remove(_cb)
after = gc.get_stats()

per_gen: dict[str, dict] = {}
for g, dur, col, unc, extra in events:
    d = per_gen.setdefault(str(g), {"collections": 0, "time_ns": 0, "collected": 0, "uncollectable": 0,
                                    "untimed": 0, "candidates": 0, "reported_duration_s": 0.0})
    d["collections"] += 1
    d["candidates"] += extra.get("candidates") or 0          # 3.14+: objects the collection examined
    d["reported_duration_s"] += extra.get("duration") or 0.0  # 3.14+: CPython's own timing of it
    d["collected"] += col or 0
    d["uncollectable"] += unc or 0
    if dur is None:
        d["untimed"] += 1
    else:
        d["time_ns"] += dur
gc_ns = sum(d["time_ns"] for d in per_gen.values())
extras = sorted({k for *_, e in events for k in e})
rec = {
    "workload": name,
    "python": sys.version,
    "free_threaded": bool(sysconfig.get_config_var("Py_GIL_DISABLED")),
    "gc_enabled": gc.isenabled(),
    "threshold": gc.get_threshold(),
    "run_s": (t1 - t0) / 1e9,
    "collections": sum(d["collections"] for d in per_gen.values()),
    "gc_s": gc_ns / 1e9,
    "gc_share": gc_ns / (t1 - t0) if t1 > t0 else None,
    "collected": sum(d["collected"] for d in per_gen.values()),
    "uncollectable": sum(d["uncollectable"] for d in per_gen.values()),
    "candidates": sum(d["candidates"] for d in per_gen.values()),
    "per_generation": per_gen,
    "callback_info_extra_keys": extras,
    "get_stats_delta": [{k: a.get(k, 0) - b.get(k, 0) for k in a} for a, b in zip(after, before)],
    "maxrss_kib_after_setup": rss_setup,
    "maxrss_kib_after_warmup": rss_warm,
    "maxrss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    "children_maxrss_kib": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,
    "digest": mod.digest(result),
}
with open(out, "a") as f:
    f.write(json.dumps(rec) + "\n")
print(f"{name:40s} run {rec['run_s'] * 1e3:8.1f} ms  gc {rec['collections']:5d} coll "
      f"{rec['gc_s'] * 1e3:8.2f} ms ({(rec['gc_share'] or 0) * 100:5.1f}%)  collected {rec['collected']:9d}  "
      f"maxrss {rec['maxrss_kib'] / 1024:7.1f} MiB  gens {json.dumps({g: d['collections'] for g, d in per_gen.items()})}")
