"""Run the workload set (benches/workloads/<family>/<name>.py) plain or with Rustique.

    python benches/workloads/run.py list
    python benches/workloads/run.py one data/state_snapshot [--enable] [--only json,copy] [-n 5]
    python benches/workloads/run.py compare data/state_snapshot web [-n 5] [-p 3] [--only ...] [-o out.json]

`one` runs in this process: optional `rustique.enable()` first (before the workload module is
imported, as a user would), `setup()`, one warm-up `run()`, then `n` timed `run()`s; prints a
JSON line with the times and the digest. `compare` runs `one` in fresh processes, `p` pairs,
plain and enabled interleaved, and reports per workload: median steady-state time of each
side, the ratio (enabled / plain), and whether every digest matched (a mismatch is a bug:
the run exits non-zero). Selectors: `family/name`, `family`, or `all`.

Workloads need their family's requirements: `pip install -r benches/workloads/<family>/requirements.txt`.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
FAMILIES = ("web", "data", "devtools")


def discover() -> list[str]:
    out = []
    for fam in FAMILIES:
        d = os.path.join(HERE, fam)
        if not os.path.isdir(d):
            continue
        for f in sorted(os.listdir(d)):
            if f.endswith(".py") and not f.startswith("_") and not f.startswith("profile"):
                out.append(f"{fam}/{f[:-3]}")
    return out


def select(sel: list[str]) -> list[str]:
    allw = discover()
    out: list[str] = []
    for s in sel:
        if s == "all":
            out += allw
        elif s in FAMILIES:
            out += [w for w in allw if w.startswith(s + "/")]
        elif s in allw:
            out.append(s)
        else:
            sys.exit(f"unknown workload {s!r}; see `run.py list`")
    return list(dict.fromkeys(out))


def load(name: str):
    path = os.path.join(HERE, name + ".py")
    spec = importlib.util.spec_from_file_location("workload_" + name.replace("/", "_"), path)
    mod = importlib.util.module_from_spec(spec)
    sys.path.insert(0, os.path.dirname(path))
    spec.loader.exec_module(mod)
    return mod


def cmd_one(a: argparse.Namespace) -> None:
    if a.enable:
        import rustique

        kw = {}
        if a.only:
            kw["only"] = a.only.split(",")
        rustique.enable(**kw)
    mod = load(a.workload)
    state = mod.setup()
    mod.run(state)  # warm-up
    times = []
    result = None
    for _ in range(a.n):
        t0 = time.perf_counter()
        result = mod.run(state)
        times.append(time.perf_counter() - t0)
    print(json.dumps({"workload": a.workload, "enabled": a.enable, "times": times,
                      "digest": mod.digest(result)}))


def cmd_compare(a: argparse.Namespace) -> None:
    rows = []
    bad = False
    for w in select(a.workloads):
        side: dict[bool, list[float]] = {False: [], True: []}
        digests = set()
        for _ in range(a.p):
            for en in (False, True):
                cmd = [sys.executable, __file__, "one", w, "-n", str(a.n)]
                if en:
                    cmd.append("--enable")
                    if a.only:
                        cmd += ["--only", a.only]
                r = subprocess.run(cmd, capture_output=True, text=True)
                if r.returncode != 0:
                    print(f"{w} enabled={en}: FAILED\n{r.stderr[-2000:]}", file=sys.stderr)
                    bad = True
                    break
                d = json.loads(r.stdout.strip().splitlines()[-1])
                side[en] += d["times"]
                digests.add(d["digest"])
        if not side[False] or not side[True]:
            continue
        p, e = statistics.median(side[False]), statistics.median(side[True])
        same = len(digests) == 1
        bad |= not same
        rows.append({"workload": w, "plain_s": p, "enabled_s": e, "ratio": e / p, "same_result": same})
        print(f"{w:40s} plain {p * 1e3:9.1f} ms  enabled {e * 1e3:9.1f} ms  ratio {e / p:6.3f}  "
              f"{'same' if same else 'DIGEST MISMATCH'}", flush=True)
    if rows:
        gm = statistics.geometric_mean(r["ratio"] for r in rows)
        print(f"{'geometric mean':40s} {'':40s} ratio {gm:6.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "rows": rows}, f, indent=1)
    sys.exit(1 if bad else 0)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    one = sub.add_parser("one")
    one.add_argument("workload")
    one.add_argument("--enable", action="store_true")
    one.add_argument("--only")
    one.add_argument("-n", type=int, default=5)
    cmp_ = sub.add_parser("compare")
    cmp_.add_argument("workloads", nargs="+")
    cmp_.add_argument("--only")
    cmp_.add_argument("-n", type=int, default=5)
    cmp_.add_argument("-p", type=int, default=3)
    cmp_.add_argument("-o")
    a = ap.parse_args()
    if a.cmd == "list":
        print("\n".join(discover()))
    elif a.cmd == "one":
        cmd_one(a)
    else:
        cmd_compare(a)


if __name__ == "__main__":
    main()
