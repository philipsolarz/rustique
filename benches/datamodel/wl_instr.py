#!/usr/bin/env python3
"""User-space instructions per `run()` of workloads: plain, everything enabled ("all"), and
everything except the data-model groups ("base"), so `all/base` is this workstream's share.

    python benches/datamodel/wl_instr.py custom/rules_engine frameworks [-k 3] [-r 3] [-o out.json]

For each workload and side: fresh processes under `perf stat -e instructions:u` with 1 and
K `run()`s after `setup()` and one warm-up `run()`; per run = (I_K - I_1) / (K - 1) (startup,
imports and setup cancel). `rustique.enable(...)` runs before the workload module is
imported (as `RUSTIQUE=1` would). Median of `-r` repeats; every digest must match plain.
Needs hardware counters (the dev box). Selectors as in benches/workloads/run.py.
"""

import argparse
import importlib.util
import json
import os
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.join(HERE, "..", "workloads")
MINE = ["enum", "generated", "contextlib", "functools"]


def driver(path, iters, side):
    if side != "plain":
        import rustique

        groups = {t["group"] for t in rustique.status()["targets"]}
        rustique.enable(exclude=[g for g in MINE if g in groups] if side == "base" else [])
    sys.path.insert(0, os.path.dirname(path))
    spec = importlib.util.spec_from_file_location("wl_" + os.path.basename(path)[:-3], path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    st = mod.setup()
    res = mod.run(st)
    for _ in range(iters):
        res = mod.run(st)
    print("DIGEST", mod.digest(res))


def measure(path, iters, side):
    cmd = ["perf", "stat", "-x,", "-e", "instructions:u", "--", sys.executable, __file__, "--driver", path, str(iters), side]
    r = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, PYTHONHASHSEED="0"))
    if r.returncode:
        raise RuntimeError(r.stderr[-3000:])
    dig = [ln for ln in r.stdout.splitlines() if ln.startswith("DIGEST")][-1]
    ins = [ln for ln in r.stderr.splitlines() if "instructions" in ln][-1]
    return int(ins.split(",")[0]), dig


def select(sel):
    sys.path.insert(0, WL)
    import run as runmod  # benches/workloads/run.py

    return [os.path.join(WL, w + ".py") for w in runmod.select(sel)]


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--driver":
        driver(sys.argv[2], int(sys.argv[3]), sys.argv[4])
        return
    ap = argparse.ArgumentParser()
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("-k", type=int, default=3)
    ap.add_argument("-r", type=int, default=3)
    ap.add_argument("--sides", default="plain,base,all")
    ap.add_argument("-o")
    a = ap.parse_args()
    out = {}
    sides = a.sides.split(",")
    for w in select(a.workloads):
        res = {}
        for side in sides:
            per, digs = [], set()
            for _ in range(a.r):
                i1, d1 = measure(w, 1, side)
                ik, dk = measure(w, a.k, side)
                per.append((ik - i1) / (a.k - 1))
                digs |= {d1, dk}
            res[side] = {"median": statistics.median(per), "min": min(per), "max": max(per), "digests": sorted(digs)}
        name = os.path.relpath(w, WL)[:-3]
        p = res["plain"]["median"]
        row = {s: res[s]["median"] / p for s in sides if s != "plain"}
        same = all(res[s]["digests"] == res["plain"]["digests"] for s in sides)
        if "all" in res and "base" in res:
            row["all/base"] = res["all"]["median"] / res["base"]["median"]
        out[name] = {"sides": res, "ratios": row, "digest_match": same}
        spread = (res["plain"]["max"] - res["plain"]["min"]) / p / 2
        print(f"{name:32} plain {p / 1e6:9.1f} M  " + "  ".join(f"{k} {v:.4f}" for k, v in row.items())
              + f"  ±{spread:.4f}  digests {'ok' if same else 'MISMATCH'}", flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
