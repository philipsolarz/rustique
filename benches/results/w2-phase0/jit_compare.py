"""CPython's own JIT on the workload set: plain without the JIT vs plain with `PYTHON_JIT=1`.

    python jit_compare.py compare data [web ...] [-n 5] [-p 3] [-o out.json]   (cwd = repo root)

The protocol of benches/workloads/run.py `compare`, with the two sides changed: no
Rustique on either side; side "nojit" runs with `PYTHON_JIT` removed from the environment
(the default of a `--enable-experimental-jit=yes-off` build: JIT off), side "jit" with
`PYTHON_JIT=1`. Per workload `p` interleaved pairs of fresh processes, each process
`setup()` + one warm-up `run()` + `n` timed `run()`s; median of the timed runs per side,
ratio = jit / nojit; digests must match. Every child reports `sys._jit.is_enabled()`, so a
side that did not get the JIT it asked for is visible (and counted as a failure).
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

HERE = os.path.abspath(__file__)
RUN_PY = os.path.join(os.getcwd(), "benches", "workloads", "run.py")


def _jit_state() -> dict:
    j = getattr(sys, "_jit", None)
    return {"available": bool(j and j.is_available()), "enabled": bool(j and j.is_enabled())}


def cmd_one(a: argparse.Namespace) -> None:
    path = os.path.join(os.getcwd(), "benches", "workloads", a.workload + ".py")
    spec = importlib.util.spec_from_file_location("workload_" + a.workload.replace("/", "_"), path)
    mod = importlib.util.module_from_spec(spec)
    sys.path.insert(0, os.path.dirname(path))
    spec.loader.exec_module(mod)
    state = mod.setup()
    mod.run(state)  # warm-up
    times = []
    result = None
    for _ in range(a.n):
        t0 = time.perf_counter()
        result = mod.run(state)
        times.append(time.perf_counter() - t0)
    print(json.dumps({"workload": a.workload, "jit": _jit_state(), "times": times, "digest": mod.digest(result)}))


def select(fams: list[str]) -> list[str]:
    out = subprocess.run([sys.executable, RUN_PY, "list"], capture_output=True, text=True, check=True).stdout.split()
    return [w for w in out if any(w == f or w.startswith(f + "/") for f in fams)]


def cmd_compare(a: argparse.Namespace) -> None:
    rows, bad = [], False
    for w in select(a.workloads):
        side: dict[str, list[float]] = {"nojit": [], "jit": []}
        digests, jit_seen = set(), {"nojit": set(), "jit": set()}
        for _ in range(a.p):
            for s in ("nojit", "jit"):
                env = {k: v for k, v in os.environ.items() if k != "PYTHON_JIT"}
                if s == "jit":
                    env["PYTHON_JIT"] = "1"
                r = subprocess.run([sys.executable, HERE, "one", w, "-n", str(a.n)], capture_output=True, text=True, env=env)
                if r.returncode != 0:
                    print(f"{w} {s}: FAILED\n{r.stderr[-2000:]}", file=sys.stderr)
                    bad = True
                    break
                d = json.loads(r.stdout.strip().splitlines()[-1])
                side[s] += d["times"]
                digests.add(d["digest"])
                jit_seen[s].add(d["jit"]["enabled"])
        if not side["nojit"] or not side["jit"]:
            continue
        p, j = statistics.median(side["nojit"]), statistics.median(side["jit"])
        same = len(digests) == 1
        jit_ok = jit_seen["nojit"] == {False} and jit_seen["jit"] == {True}
        bad |= not same or not jit_ok
        rows.append({"workload": w, "nojit_s": p, "jit_s": j, "ratio": j / p, "same_result": same,
                     "jit_enabled": {k: sorted(v) for k, v in jit_seen.items()}})
        print(f"{w:40s} nojit {p * 1e3:9.1f} ms  jit {j * 1e3:9.1f} ms  ratio {j / p:6.3f}  "
              f"{'same' if same else 'DIGEST MISMATCH'}{'' if jit_ok else '  JIT STATE ' + str(jit_seen)}", flush=True)
    if rows:
        print(f"{'geometric mean':40s} {'':36s} ratio {statistics.geometric_mean(r['ratio'] for r in rows):6.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "jit": _jit_state(), "rows": rows}, f, indent=1)
    sys.exit(1 if bad else 0)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    one = sub.add_parser("one")
    one.add_argument("workload")
    one.add_argument("-n", type=int, default=5)
    c = sub.add_parser("compare")
    c.add_argument("workloads", nargs="+")
    c.add_argument("-n", type=int, default=5)
    c.add_argument("-p", type=int, default=3)
    c.add_argument("-o")
    a = ap.parse_args()
    cmd_one(a) if a.cmd == "one" else cmd_compare(a)


if __name__ == "__main__":
    main()
