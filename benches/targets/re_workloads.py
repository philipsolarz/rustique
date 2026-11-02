"""Steady-state instructions of workloads with and without the `re` group (docs/targets/re.md):
`perf stat -e instructions:u` of `benches/workloads/run.py one WL -n N` at two run counts; the
difference over the extra runs is the per-run steady state (setup, imports and warm-up cancel).
Variants: plain; `re` only; every group; every group except `re` (RUSTIQUE_EXCLUDE=re). Digests
of every variant must match plain's.

    python benches/targets/re_workloads.py web/email_mime data/log_analytics ... [-r 3] [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
RUN = os.path.join(HERE, "..", "workloads", "run.py")
VARIANTS = {
    "plain": ([], {}),
    "re-only": (["--enable", "--only", "re"], {}),
    "all": (["--enable"], {}),
    "all-but-re": (["--enable"], {"RUSTIQUE_EXCLUDE": "re"}),
}


def instructions(wl: str, n: int, args: list[str], env: dict[str, str]) -> tuple[int, str]:
    e = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    e.update(env, PYTHONHASHSEED="0")
    p = subprocess.run(["perf", "stat", "-x,", "-e", "instructions:u", sys.executable, RUN, "one", wl, "-n", str(n),
                        *args], capture_output=True, text=True, env=e, check=False)
    ir = None
    for line in p.stderr.splitlines():
        parts = line.split(",")
        if len(parts) > 2 and parts[2].startswith("instructions"):
            ir = int(parts[0])
    digest = ""
    for line in p.stdout.splitlines():
        if line.startswith("{"):
            digest = json.loads(line).get("digest", "")
    if ir is None:
        raise RuntimeError(f"{wl} {args}: {p.stderr[-1500:]}")
    return ir, digest


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("-n", type=int, default=5, help="extra runs measured")
    ap.add_argument("-r", type=int, default=3, help="repetitions (median)")
    ap.add_argument("-o")
    a = ap.parse_args()
    res = {}
    for wl in a.workloads:
        row = {}
        digests = set()
        for v, (args, env) in VARIANTS.items():
            per = []
            for _ in range(a.r):
                lo, d1 = instructions(wl, 1, args, env)
                hi, d2 = instructions(wl, 1 + a.n, args, env)
                digests.update({d1, d2})
                per.append((hi - lo) / a.n)
            row[v] = statistics.median(per)
        row["digests_match"] = len(digests) == 1
        row["re-only/plain"] = row["re-only"] / row["plain"]
        row["all/all-but-re"] = row["all"] / row["all-but-re"]
        res[wl] = row
        print(f"{wl:28s} plain={row['plain']:,.0f} re-only={row['re-only/plain']:.4f} "
              f"all/all-but-re={row['all/all-but-re']:.4f} digests={'ok' if row['digests_match'] else 'DIFF'}",
              flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump(res, f, indent=1)


if __name__ == "__main__":
    main()
