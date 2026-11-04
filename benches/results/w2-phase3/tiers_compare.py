"""Opt-in tiers on the workload set: plain, the default tier and the default tier plus opt-in
tiers, interleaved in fresh processes (benches/workloads/run.py `compare`, more than two sides).

    python tiers_compare.py <family|family/name|all ...> --sides default,interp [-n 5] [-p 5] [-o out.json]
        (cwd = the repository root, in the family's venv)

Sides: `plain` (always measured, first in the list), `default` (`rustique.enable()`), and any
RUSTIQUE_TIERS value with `+` for `,` (`interp`, `gc`, `gc+interp`): `rustique.enable()` with
that RUSTIQUE_TIERS in the child's environment. Per workload, `p` rounds; each round runs
every side once in a fresh process (`run.py one`: enable before the workload is imported,
setup(), 1 warm-up, `n` timed run()s), the order rotating by one side per round. Each side's
time is the median of its p x n timed runs. Ratios: every side against plain and every tier
side against default. A digest mismatch between any two processes is reported and makes the
exit status non-zero, as in run.py.

After its timed runs, each enabled child records the state of the opt-in tiers' targets
(installed / suspended / refused ...) so that a tier that refused or switched itself off
cannot pass silently; that happens outside the timed region.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import time

REPO = os.getcwd()
RUN = os.path.join(REPO, "benches", "workloads", "run.py")
sys.path.insert(0, os.path.dirname(RUN))
import run as runner  # noqa: E402

CHILD = r"""
import json, os, sys, time
if sys.path and sys.path[0] == "":
    del sys.path[0]  # as `run.py one`: the workloads directory first, not the cwd
sys.path.insert(0, sys.argv[1])
enable = sys.argv[3] == "1"
if enable:
    import rustique
    rustique.enable()
import run as runner
mod = runner.load(sys.argv[2])
state = mod.setup()
mod.run(state)
times, result = [], None
for _ in range(int(sys.argv[4])):
    t0 = time.perf_counter()
    result = mod.run(state)
    times.append(time.perf_counter() - t0)
tiers = None
if enable:
    st = rustique.status()
    tiers = {}
    for t in st["targets"]:
        if t.get("tier"):
            k = f'{t["tier"]}:{t["state"]}' + (f'/{t.get("refusal")}' if t["state"] == "refused" else "")
            tiers[k] = tiers.get(k, 0) + 1
print(json.dumps({"times": times, "digest": mod.digest(result), "tiers": tiers}))
"""


def side_env(side: str) -> tuple[bool, dict[str, str]]:
    env = {k: v for k, v in os.environ.items() if k not in ("RUSTIQUE", "RUSTIQUE_TIERS")}
    if side == "plain":
        return False, env
    if side != "default":
        env["RUSTIQUE_TIERS"] = side.replace("+", ",")
    return True, env


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("--sides", required=True, help="comma-separated: default,interp,gc,gc+interp (plain is implied)")
    ap.add_argument("-n", type=int, default=5)
    ap.add_argument("-p", type=int, default=5)
    ap.add_argument("-o")
    a = ap.parse_args()
    sides = ["plain"] + [s for s in a.sides.split(",") if s and s != "plain"]
    rows, bad = [], False
    for w in runner.select(a.workloads):
        times = {s: [] for s in sides}
        digests, tiers, failed = set(), {s: {} for s in sides}, False
        t_start = time.time()
        for r in range(a.p):
            order = sides[r % len(sides):] + sides[: r % len(sides)]
            for s in order:
                en, env = side_env(s)
                cmd = [sys.executable, "-c", CHILD, os.path.dirname(RUN), w, "1" if en else "0", str(a.n)]
                p = subprocess.run(cmd, capture_output=True, text=True, env=env)
                if p.returncode != 0:
                    print(f"{w} {s}: FAILED\n{p.stderr[-2000:]}", file=sys.stderr, flush=True)
                    failed = bad = True
                    break
                d = json.loads(p.stdout.strip().splitlines()[-1])
                times[s] += d["times"]
                digests.add(d["digest"])
                for k, v in (d["tiers"] or {}).items():
                    tiers[s][k] = tiers[s].get(k, 0) + v
            if failed:
                break
        if failed:
            continue
        med = {s: statistics.median(times[s]) for s in sides}
        same = len(digests) == 1
        bad |= not same
        row = {"workload": w, "median_s": med, "times": times, "same_result": same,
               "vs_plain": {s: med[s] / med["plain"] for s in sides if s != "plain"},
               "vs_default": {s: med[s] / med["default"] for s in sides if s not in ("plain", "default")}
               if "default" in med else {},
               "tiers": {s: {k: v // a.p for k, v in tiers[s].items()} for s in sides if s != "plain"},
               "elapsed_s": time.time() - t_start}
        rows.append(row)
        parts = [f"{s} {med[s] * 1e3:8.1f} ms" for s in sides]
        rat = [f"{s}/plain {row['vs_plain'][s]:.3f}" for s in sides if s != "plain"]
        rat += [f"{s}/default {row['vs_default'][s]:.3f}" for s in row["vs_default"]]
        print(f"{w:36s} " + "  ".join(parts) + "  | " + "  ".join(rat) +
              f"  {'same' if same else 'DIGEST MISMATCH'}", flush=True)
        for s in sides[1:]:
            if s != "default":
                print(f"{'':36s}   {s} tier targets per process: {row['tiers'][s]}", flush=True)
    if rows:
        for s in sides[1:]:
            gm = statistics.geometric_mean(r["vs_plain"][s] for r in rows)
            print(f"{'geometric mean':36s} {s}/plain {gm:.3f}")
        for s in sides[1:]:
            if rows[0]["vs_default"].get(s) is not None:
                gm = statistics.geometric_mean(r["vs_default"][s] for r in rows)
                print(f"{'geometric mean':36s} {s}/default {gm:.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "sides": sides, "n": a.n, "p": a.p, "rows": rows}, f, indent=1)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
