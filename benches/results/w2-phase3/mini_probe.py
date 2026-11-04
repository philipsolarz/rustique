"""One workload in fresh processes per mode, modes interleaved (rotated each round); each process is
`run.py one <w> -n 5` (setup, warm-up, 5 timed run()s). Per mode: the median of all timed runs,
ratio to plain, digests checked. Run from the build clone with the family venv's python.
    python mini_probe.py WORKLOAD ROUNDS MODE...
    MODE: plain | import (RUSTIQUE unset, `import rustique` only) | enabled | excl=G1,G2 (enabled, RUSTIQUE_EXCLUDE)"""
import json, os, statistics, subprocess, sys

RUN = os.path.abspath("benches/workloads/run.py")
w, rounds, modes = sys.argv[1], int(sys.argv[2]), sys.argv[3:]


def cmd(mode):
    env = {k: v for k, v in os.environ.items() if k not in ("RUSTIQUE", "RUSTIQUE_EXCLUDE", "RUSTIQUE_TIERS", "RUSTIQUE_ONLY")}
    one = [RUN, "one", w, "-n", "5"]
    if mode == "plain":
        return [sys.executable, *one], env
    if mode == "import":
        code = ("import sys, runpy, rustique; sys.path.insert(0, %r); sys.argv = %r; runpy.run_path(%r, run_name='__main__')"
                % (os.path.dirname(RUN), one, RUN))
        return [sys.executable, "-c", code], env
    if mode == "enabled":
        return [sys.executable, *one, "--enable"], env
    if mode.startswith("excl="):
        env["RUSTIQUE_EXCLUDE"] = mode[5:]
        return [sys.executable, *one, "--enable"], env
    raise SystemExit(f"bad mode {mode}")


res = {m: [] for m in modes}
digests = {m: set() for m in modes}
for r in range(rounds):
    for m in modes[r % len(modes):] + modes[: r % len(modes)]:
        c, env = cmd(m)
        out = subprocess.run(c, env=env, capture_output=True, text=True)
        if out.returncode:
            print(m, "FAILED", out.stderr[-800:], flush=True)
            continue
        j = json.loads(out.stdout.strip().splitlines()[-1])
        res[m].append(j["times"])
        digests[m].add(j["digest"])
base = statistics.median(t for ts in res["plain"] for t in ts)
alld = set().union(*digests.values())
print(f"# {w}: {rounds} rounds, modes rotated; per mode the median of all timed runs; digests {'all equal' if len(alld) == 1 else 'DIFFER: ' + str(digests)}")
for m in modes:
    v = statistics.median(t for ts in res[m] for t in ts)
    print(f"{m:130s} {v * 1e3:8.1f} ms  ratio {v / base:.3f}  (per process: {', '.join(f'{statistics.median(ts) * 1e3:.0f}' for ts in res[m])})", flush=True)
