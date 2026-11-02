#!/usr/bin/env python3
"""Instruction counts of the measured run()s, policy A vs policy B (dev box: hardware counters
are immune to the box load that makes wall time noisy).

    python benches/runtime/tier_instr.py --pythons "3.15.1=/w/wl/3.15.1-{family}/bin/python,..." \
        --pair default,tier [--pair rustique,tier:all=1] [-p 3] [-n 5] [--cores 8,9] -o out.json SELECTORS...

Each cell is `gcprobe.py one <workload> --policy P -n N` under
`perf stat -e instructions:u --control fifo:...`: perf counts only between the probe's
enable/disable around the measured runs (setup, warm-up and exit excluded). Per workload and
pair: median over `p` processes of B / A, the digests (must match), the GC work ratio and peak
RSS ratio from the probe's own output.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import statistics as st
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import gcprobe  # noqa: E402


def cell(py: str, wl: str, pol: str, n: int, core: str | None) -> dict:
    with tempfile.TemporaryDirectory() as d:
        ctl, ack, out = os.path.join(d, "ctl"), os.path.join(d, "ack"), os.path.join(d, "o.json")
        os.mkfifo(ctl)
        os.mkfifo(ack)
        fam = wl.split("/")[0]
        cmd = (["taskset", "-c", core] if core else []) + [
            "perf", "stat", "-x,", "-e", "instructions:u", f"--control=fifo:{ctl},{ack}", "-D", "-1", "--"]
        cmd += shlex.split(py.replace("{family}", fam)) + [os.path.join(HERE, "gcprobe.py"), "one", wl, "--policy", pol,
                                                           "-n", str(n), "-o", out]
        env = {**os.environ, "RQ_PERF_CTL": ctl, "RQ_PERF_ACK": ack}
        r = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=3600)
        if r.returncode:
            return {"error": r.stderr[-1500:]}
        ins = None
        for line in r.stderr.splitlines():
            parts = line.split(",")
            if len(parts) > 2 and parts[2].startswith("instructions"):
                ins = int(parts[0]) if parts[0].isdigit() else None
        with open(out) as f:
            j = json.load(f)
        runs = j["runs"]
        work = st.median(sum(v["candidates"] for k, v in r_["gc"].items() if not k.startswith("x")) for r_ in runs)
        hwm = max(r_["hwm_kb"] for r_ in runs)
        return {"instructions": ins, "digest": j["digest"], "digest_ok": j["digest_ok"], "work": work, "hwm_kb": hwm,
                "wall": st.median(r_["wall_s"] for r_ in runs)}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("--pythons", required=True)
    ap.add_argument("--pair", action="append", required=True, help="A,B")
    ap.add_argument("-p", type=int, default=3)
    ap.add_argument("-n", type=int, default=5)
    ap.add_argument("--cores", default="")
    ap.add_argument("-o", required=True)
    a = ap.parse_args()
    pythons = dict(kv.split("=", 1) for kv in a.pythons.split(","))
    cores = [c for c in a.cores.split(",") if c] or [None]
    jobs = []
    for wl in gcprobe.select(a.workloads):
        for name, py in pythons.items():
            for pair in a.pair:
                for pol in pair.split(","):
                    for r in range(a.p):
                        jobs.append((name, py, wl, pol, r))
    free = list(cores)
    res: dict = {}

    def run(job):
        name, py, wl, pol, r = job
        core = free.pop(0)
        try:
            return job, cell(py, wl, pol, a.n, core)
        finally:
            free.append(core)

    with ThreadPoolExecutor(len(cores)) as ex:
        for (name, py, wl, pol, r), c in ex.map(run, jobs):
            res.setdefault(name, {}).setdefault(wl, {}).setdefault(pol, []).append(c)
            if "error" in c:
                print(f"FAILED {name} {wl} {pol}: {c['error'][-500:]}", flush=True)
    rows = []
    for name, wls in res.items():
        for wl, pols in sorted(wls.items()):
            for pair in a.pair:
                pa, pb = pair.split(",")
                A = [c for c in pols.get(pa, []) if "instructions" in c and c["instructions"]]
                B = [c for c in pols.get(pb, []) if "instructions" in c and c["instructions"]]
                if not A or not B:
                    continue
                ia, ib = st.median(c["instructions"] for c in A), st.median(c["instructions"] for c in B)
                wa, wb = st.median(c["work"] for c in A), st.median(c["work"] for c in B)
                ha, hb = st.median(c["hwm_kb"] for c in A), st.median(c["hwm_kb"] for c in B)
                digests = {c["digest"] for c in A + B}
                rows.append({"python": name, "workload": wl, "a": pa, "b": pb, "instr_ratio": ib / ia,
                             "work_ratio": (wb / wa) if wa else None, "rss_ratio": hb / ha,
                             "digests_equal": len(digests) == 1 and all(c["digest_ok"] for c in A + B),
                             "instr_a": ia, "instr_b": ib})
                print(f"{name} {wl:40s} {pb} / {pa}: instr {ib / ia:.3f}  work {(wb / wa) if wa else float('nan'):.2f}  "
                      f"RSS {hb / ha:.3f}  digests {'equal' if len(digests) == 1 else 'DIFFER'}", flush=True)
    with open(a.o, "w") as f:
        json.dump({"rows": rows, "raw": res}, f, indent=1)


if __name__ == "__main__":
    main()
