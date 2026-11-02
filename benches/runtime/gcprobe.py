#!/usr/bin/env python3
"""GC / allocation probe for one workload (benches/workloads/<family>/<name>.py).

    python benches/runtime/gcprobe.py one <family/name> [--policy SPEC] [-n 5] [-o out.json]
    python benches/runtime/gcprobe.py sweep <selectors...> --policies default,thr:20000 ...
           [--pythons 3.15.1=/path/py,...] [-p 3] [-n 5] [--cores 8,10,12,14] --out DIR

`one` runs in this process: the policy (benches/runtime/gcpolicy.py) is installed first,
before the workload module is imported (as a launch-time switch would), then setup(), one
warm-up run(), then `n` measured run()s. Per measured run it records:

  wall_s, cpu_s           perf_counter / process_time around run()
  collections             one row per collection: generation, start (s from run start),
                          duration (C-measured info['duration'] on 3.15, else the
                          callback-to-callback interval), collected, uncollectable,
                          candidates (objects examined)
  hwm_kb, rss_kb          peak RSS during the run (VmHWM, reset through
                          /proc/self/clear_refs before each run) and RSS after it

and per process: tracked objects after warm-up (len(gc.get_objects()) + get_freeze_count()),
`pending_garbage` (what an explicit gc.collect() finds after the runs) and `frozen_garbage`
(what it finds after gc.unfreeze(): cyclic garbage the policy kept in the permanent
generation), the process peak RSS (ru_maxrss), the policy's own report.

The probe's callback stores numbers in array('d') rows so it allocates no GC-tracked
objects. With RQ_PERF_CTL/RQ_PERF_ACK set (see benches/workloads/data/_profiler.py) it
enables perf around the measured runs only.

`sweep` runs `one` in fresh processes: for every workload, `p` rounds, each round runs every
(python, policy) cell once in a rotating order; jobs go to a pool of workers, one per core
of --cores (taskset), in that order, so the cells of one round run at about the same time
and see the same box load. Results: DIR/<python>/<family>__<name>/<policy>.<round>.json.
"""

from __future__ import annotations

import argparse
import gc
import importlib.util
import json
import os
import shlex
import sys
import time
from array import array

HERE = os.path.dirname(os.path.abspath(__file__))
# RQ_WL_ROOT: another workloads root (e.g. the wave-2 families of other branches)
WL = os.environ.get("RQ_WL_ROOT") or os.path.join(os.path.dirname(HERE), "workloads")
sys.path.insert(0, HERE)


def _status_kb(field):
    try:
        with open("/proc/self/status") as f:
            for line in f:
                if line.startswith(field):
                    return int(line.split()[1])
    except OSError:
        pass
    return -1


def _reset_hwm():
    try:
        with open("/proc/self/clear_refs", "w") as f:
            f.write("5")
        return True
    except OSError:
        return False


def _ctl(cmd):
    ctl = os.environ.get("RQ_PERF_CTL")
    if not ctl:
        return
    with open(ctl, "w") as f:
        f.write(cmd + "\n")
    with open(os.environ["RQ_PERF_ACK"]) as f:
        f.read(5)


def load_workload(name):
    path = os.path.join(WL, name + ".py")
    if not os.path.exists(path):
        path = name if name.endswith(".py") else name + ".py"
    spec = importlib.util.spec_from_file_location("workload_" + os.path.basename(path)[:-3], path)
    mod = importlib.util.module_from_spec(spec)
    sys.path.insert(0, os.path.dirname(path))
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    return mod


class Recorder:
    """gc.callbacks recorder; rows are (gen, t_start, dur, collected, uncollectable, cand,
    explicit). `explicit` is 1 for collections run by a gc.collect() call from Python code
    (the probe wraps gc.collect: the policy modules keep the original), 0 for automatic."""

    NF = 7

    def __init__(self):
        self.rows = array("d")
        self.t0 = time.perf_counter()
        self._start = 0.0
        self.on = False
        self.explicit = 0

    def __call__(self, phase, info):
        if phase == "start":
            self._start = time.perf_counter()
            return
        if not self.on:
            return
        end = time.perf_counter()
        dur = info.get("duration", -1.0)
        if dur is None or dur < 0:
            dur = end - self._start
        self.rows.extend((info["generation"], self._start - self.t0, dur, info["collected"],
                          info["uncollectable"], info.get("candidates", -1),
                          1 if self.explicit else 0))

    def take(self):
        r = self.rows
        self.rows = array("d")
        n = self.NF
        return [list(r[i:i + n]) for i in range(0, len(r), n)]


def summarize(cols):
    """Per generation; explicit gc.collect() collections are keyed 'x<gen>'."""
    by = {}
    for g, _t, d, c, u, cand, *x in cols:
        key = f"x{int(g)}" if x and x[0] else int(g)
        s = by.setdefault(key, [0, 0.0, 0.0, 0, 0, 0])
        s[0] += 1
        s[1] += d
        s[2] = max(s[2], d)
        s[3] += int(c)
        s[4] += int(u)
        s[5] += int(cand)
    return {str(g): {"n": v[0], "time_s": v[1], "max_s": v[2], "collected": v[3],
                     "uncollectable": v[4], "candidates": v[5]} for g, v in sorted(by.items(), key=str)}


def cmd_one(a):
    import gcpolicy

    rec = Recorder()
    gc.callbacks.append(rec)
    pol = gcpolicy.make(a.policy)
    if a.enable or os.environ.get("RQ_ENABLE") == "1":
        import rustique

        rustique.enable()
    real_collect = gc.collect

    def collect(*args, **kw):  # measurement only: marks explicit collections
        rec.explicit += 1
        try:
            return real_collect(*args, **kw)
        finally:
            rec.explicit -= 1

    gc.collect = collect
    pol.install()
    t_imp = time.perf_counter()
    mod = load_workload(a.workload)
    state = mod.setup()
    pol.after_setup()
    t_setup = time.perf_counter() - t_imp
    rec.on = True
    rec.t0 = time.perf_counter()
    ref = mod.digest(mod.run(state))  # warm-up
    warm_cols = rec.take()
    pol.after_warmup()
    tracked = len(gc.get_objects())
    frozen0 = gc.get_freeze_count()
    runs = []
    _ctl("enable")
    for _ in range(a.n):
        hwm_ok = _reset_hwm()
        rec.t0 = time.perf_counter()
        c0 = time.process_time()
        t0 = time.perf_counter()
        res = mod.run(state)
        t1 = time.perf_counter()
        c1 = time.process_time()
        cols = rec.take()
        extra = None
        last = getattr(mod, "LAST", None)
        if last:
            lags = last.get("lags") or []
            extra = {k: v for k, v in last.items() if k != "lags"}
            if lags:
                extra.update(lag_n=len(lags), lag_p50=lags[len(lags) // 2],
                             lag_p90=lags[int(len(lags) * 0.9)], lag_max=lags[-1],
                             lag_mean=sum(lags) / len(lags))
        runs.append({"extra": extra, "wall_s": t1 - t0, "cpu_s": c1 - c0, "hwm_kb": _status_kb("VmHWM:") if hwm_ok else -1,
                     "rss_kb": _status_kb("VmRSS:"), "gc": summarize(cols),
                     "collections": cols if a.keep_cols else None})
    _ctl("disable")
    ok = mod.digest(res) == ref
    rec.on = False
    polrep = pol.report()
    # what is still pending: an explicit full collection (the policy steps aside for it)
    pol.uninstall()
    gc.collect = real_collect
    pending = gc.collect()
    frozen = gc.get_freeze_count()
    gc.unfreeze()
    frozen_garbage = gc.collect()
    import resource

    out = {
        "workload": a.workload, "policy": a.policy, "enabled": bool(a.enable or os.environ.get("RQ_ENABLE") == "1"),
        "python": sys.version.split()[0] + ("t" if not getattr(sys, "_is_gil_enabled", lambda: True)() else ""),
        "executable": sys.executable, "pid": os.getpid(),
        "setup_s": t_setup, "digest": ref, "digest_ok": ok,
        "warmup_gc": summarize(warm_cols), "tracked_after_warmup": tracked,
        "frozen_after_warmup": frozen0, "frozen_at_end": frozen,
        "pending_garbage": pending, "frozen_garbage": frozen_garbage,
        "maxrss_kb": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
        "threshold": gc.get_threshold(), "policy_report": polrep, "runs": runs,
    }
    s = json.dumps(out)
    if a.o:
        with open(a.o, "w") as f:
            f.write(s)
    else:
        print(s)


# ---------------------------------------------------------------------------------------
# sweep


def discover():
    out = []
    for fam in sorted(os.listdir(WL)):
        d = os.path.join(WL, fam)
        if not os.path.isdir(d) or fam.startswith((".", "_")):
            continue
        for f in sorted(os.listdir(d)):
            if f.endswith(".py") and not f.startswith(("_", "profile")):
                out.append(f"{fam}/{f[:-3]}")
    return out


def select(sel):
    allw = discover()
    out = []
    for s in sel:
        if s == "all":
            out += allw
        elif "/" not in s:
            out += [w for w in allw if w.startswith(s + "/")]
        else:
            out.append(s)
    return list(dict.fromkeys(out))


def cmd_sweep(a):
    import subprocess

    pythons = {}
    for kv in a.pythons.split(","):
        k, _, v = kv.partition("=")
        pythons[k] = v
    policies = a.policies.split(";")
    cores = [c for c in a.cores.split(",") if c] or [None]
    cells = [(py, pol) for py in pythons for pol in policies]
    jobs = []
    for w in select(a.workloads):
        fam = w.split("/")[0]
        for r in range(a.p):
            k = r % len(cells)
            for py, pol in cells[k:] + cells[:k]:
                d = os.path.join(a.out, py, w.replace("/", "__"))
                outp = os.path.join(d, f"{pol.replace('/', '_')}.{r}.json")
                if os.path.exists(outp) and not a.force:
                    continue
                os.makedirs(d, exist_ok=True)
                cmd = shlex.split(pythons[py].replace("{family}", fam)) + [os.path.abspath(__file__), "one", w,
                       "--policy", pol, "-n", str(a.n), "-o", outp]
                if a.keep_cols:
                    cmd.append("--keep-cols")
                jobs.append((w, py, pol, r, cmd))
    print(f"{len(jobs)} jobs on {len(cores)} cores", flush=True)
    free = list(cores)
    running = []
    t_all = time.time()
    done = 0
    while jobs or running:
        while jobs and free:
            w, py, pol, r, cmd = jobs.pop(0)
            core = free.pop(0)
            full = (["taskset", "-c", core] if core is not None else []) + cmd
            running.append((core, w, py, pol, r, subprocess.Popen(
                full, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)))
        time.sleep(0.2)
        still = []
        for item in running:
            core, w, py, pol, r, p = item
            if p.poll() is None:
                still.append(item)
                continue
            err = p.stderr.read()
            free.append(core)
            done += 1
            if p.returncode != 0:
                print(f"FAILED {py} {w} {pol} round {r}: {err[-1500:]}", flush=True)
            elif done % 10 == 0:
                print(f"{done} done, {len(jobs)} queued ({time.time() - t_all:.0f}s)", flush=True)
        running = still
    print(f"all done ({time.time() - t_all:.0f}s)", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    one = sub.add_parser("one")
    one.add_argument("workload")
    one.add_argument("--policy", default="default")
    one.add_argument("-n", type=int, default=5)
    one.add_argument("-o")
    one.add_argument("--keep-cols", action="store_true")
    one.add_argument("--enable", action="store_true", help="rustique.enable() before the workload import")
    sw = sub.add_parser("sweep")
    sw.add_argument("workloads", nargs="+")
    sw.add_argument("--pythons", required=True, help="name=exe,... ({family} expands)")
    sw.add_argument("--policies", default="default", help="';'-separated policy specs")
    sw.add_argument("-p", type=int, default=3)
    sw.add_argument("-n", type=int, default=5)
    sw.add_argument("--cores", default="")
    sw.add_argument("--out", required=True)
    sw.add_argument("--force", action="store_true")
    sw.add_argument("--keep-cols", action="store_true")
    a = ap.parse_args()
    if a.cmd == "one":
        cmd_one(a)
    else:
        cmd_sweep(a)


if __name__ == "__main__":
    main()
