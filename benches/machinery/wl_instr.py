#!/usr/bin/env python3
"""User-space instructions per run() of a workload, plain vs with prototype kernels.

    python benches/machinery/wl_instr.py <workload.py>... [--proto enum,dc,nt,ctx] [-r 3]

For each workload and side, runs fresh processes under `perf stat -e instructions:u` with
1 and K run() iterations after setup() + one warm-up run(); instructions per run =
(I_K - I_1) / (K - 1) (setup, imports and startup cancel). Median of `-r` repeats; the
digest of the last run must match between sides. `--proto` calls
`rustique._rustique.proto_enable(groups)` before the workload is imported (as
`RUSTIQUE=1` would), so the dataclass/namedtuple/contextmanager hooks see its classes.
Hardware counters: needs the dev box.
"""

import argparse
import json
import os
import statistics
import subprocess
import sys


def driver(path, iters, proto):
    if proto:
        import rustique._rustique as R

        R.proto_enable(proto)
    import importlib.util

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


def measure(py, path, iters, proto):
    cmd = ["perf", "stat", "-x,", "-e", "instructions:u", "--", py, __file__, "--driver", path,
           str(iters), proto or ""]
    r = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, PYTHONHASHSEED="0"))
    if r.returncode:
        raise RuntimeError(r.stderr[-2000:])
    dig = [ln for ln in r.stdout.splitlines() if ln.startswith("DIGEST")][-1]
    ins = [ln for ln in r.stderr.splitlines() if "instructions" in ln][-1]
    return int(ins.split(",")[0]), dig


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--driver":
        driver(sys.argv[2], int(sys.argv[3]), sys.argv[4] if len(sys.argv) > 4 else "")
        return
    ap = argparse.ArgumentParser()
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("--proto", default="all")
    ap.add_argument("-k", type=int, default=3)
    ap.add_argument("-r", type=int, default=3)
    ap.add_argument("-o")
    a = ap.parse_args()
    out = {}
    for w in a.workloads:
        side = {}
        for label, proto in (("plain", ""), ("proto", a.proto)):
            per = []
            digs = set()
            for _ in range(a.r):
                i1, d1 = measure(sys.executable, w, 1, proto)
                ik, dk = measure(sys.executable, w, a.k, proto)
                per.append((ik - i1) / (a.k - 1))
                digs |= {d1, dk}
            side[label] = (statistics.median(per), min(per), max(per), sorted(digs))
        ratio = side["proto"][0] / side["plain"][0]
        same = side["proto"][3] == side["plain"][3]
        name = os.path.basename(w)[:-3]
        out[name] = {"plain": side["plain"][:3], "proto": side["proto"][:3], "ratio": ratio,
                     "digest_match": same}
        print(f"{name:24} plain {side['plain'][0] / 1e6:10.1f} M  proto {side['proto'][0] / 1e6:10.1f} M"
              f"  ratio {ratio:.4f}  spread ±{(side['plain'][2] - side['plain'][1]) / side['plain'][0] / 2:.4f}"
              f"  digests {'ok' if same else 'MISMATCH'}", flush=True)
    if a.o:
        json.dump(out, open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
