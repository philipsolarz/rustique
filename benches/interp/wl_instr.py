"""User-space instructions per steady-state run() of workloads under four variants:
plain CPython, Rustique's default groups, the interp tier alone, and both.

    python benches/interp/wl_instr.py <family/name|family|all>... [--venvs DIR] [-k 3] [-j 6] [-o out.json]

Each (workload, variant) runs in fresh processes under `perf stat -e instructions:u`, once with
1 and once with K run() iterations after setup() and one warm-up run(); instructions per run
= (I_K - I_1) / (K - 1) (startup, imports, setup and enable cancel). The digest of the last
run must be identical across variants (a mismatch fails the run). Each family runs in its own
venv (`--venvs DIR/<family>-<py>`, requirements of benches/workloads/<family>), where this
build of rustique must be installed. Hardware counters: the dev box.

Caveat (M, 3.15.1, 2026-11-02): on the shared dev box the (I_K - I_1) difference of single
processes moved by up to ±0.5% on steady workloads and far more on workloads whose runs are
not steady (pip_resolve 1.47, coverage_pyflakes_tests 1.077 for the interp tier alone, whose
exact count is 0.975): the interp tier's whole-run claims use wl_cg.py (callgrind, exact Ir of
one steady-state run) instead.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import os
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.normpath(os.path.join(HERE, "..", "workloads"))
VARIANTS = ("plain", "default", "tier", "default+tier")


def driver(path: str, iters: int, variant: str) -> None:
    sys.path.insert(0, HERE)
    if variant != "plain":
        import _tier
        import rustique

        if variant == "default":
            rustique.enable()
        elif variant == "tier":
            _tier.enable_tier()
        else:
            _tier.enable_tier(default=True)
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
    if variant in ("tier", "default+tier"):
        print("GROUPS", json.dumps(_tier.groups()))


def measure(py: str, path: str, iters: int, variant: str) -> tuple[int, str, str]:
    cmd = ["perf", "stat", "-x,", "-e", "instructions:u", "--", py, __file__, "--driver", path, str(iters), variant]
    r = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, PYTHONHASHSEED="0"))
    if r.returncode:
        raise RuntimeError(f"{path} {variant}: {r.stderr[-1500:]}")
    dig = [ln for ln in r.stdout.splitlines() if ln.startswith("DIGEST")][-1]
    groups = ([ln for ln in r.stdout.splitlines() if ln.startswith("GROUPS")] or [""])[-1]
    ins = [ln for ln in r.stderr.splitlines() if "instructions" in ln][-1]
    return int(ins.split(",")[0]), dig, groups


def discover(sel: list[str]) -> list[str]:
    fams = sorted(d for d in os.listdir(WL) if os.path.isdir(os.path.join(WL, d)) and not d.startswith("_"))
    allw = [f"{f}/{n[:-3]}" for f in fams for n in sorted(os.listdir(os.path.join(WL, f)))
            if n.endswith(".py") and not n.startswith(("_", "profile"))]
    out: list[str] = []
    for s in sel:
        if s == "all":
            out += allw
        elif s in fams:
            out += [w for w in allw if w.startswith(s + "/")]
        else:
            out.append(s)
    return list(dict.fromkeys(out))


def one(w: str, venvs: str, py_tag: str, k: int, variants: tuple[str, ...]) -> dict:
    fam = w.split("/")[0]
    py = os.path.join(venvs, f"{fam}-{py_tag}", "bin", "python")
    path = os.path.join(WL, w + ".py")
    row: dict = {"workload": w}
    digs = set()
    for v in variants:
        i1, d1, _ = measure(py, path, 1, v)
        ik, dk, g = measure(py, path, k, v)
        row[v] = (ik - i1) / (k - 1)
        digs |= {d1, dk}
        if g:
            row[v + ":groups"] = json.loads(g[len("GROUPS "):])
    row["same_digest"] = len(digs) == 1
    return row


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--driver":
        driver(sys.argv[2], int(sys.argv[3]), sys.argv[4])
        return
    ap = argparse.ArgumentParser()
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("--venvs", default="/work/w2ws-interp/venvs")
    ap.add_argument("--py", default="3.15.1")
    ap.add_argument("--variants", default=",".join(VARIANTS))
    ap.add_argument("-k", type=int, default=3)
    ap.add_argument("-j", type=int, default=6)
    ap.add_argument("-o")
    a = ap.parse_args()
    variants = tuple(a.variants.split(","))
    rows = []
    with cf.ThreadPoolExecutor(a.j) as ex:
        futs = {ex.submit(one, w, a.venvs, a.py, a.k, variants): w for w in discover(a.workloads)}
        for f in cf.as_completed(futs):
            try:
                r = f.result()
            except Exception as e:  # noqa: BLE001
                print(f"{futs[f]}: FAILED {e}", flush=True)
                continue
            rows.append(r)
            base = r["plain"]
            print(f"{r['workload']:36s} " + " ".join(f"{v}={r[v] / base:.4f}" for v in variants[1:])
                  + ("" if r["same_digest"] else "  DIGEST MISMATCH"), flush=True)
    rows.sort(key=lambda r: r["workload"])
    fams: dict[str, list[dict]] = {}
    for r in rows:
        fams.setdefault(r["workload"].split("/")[0], []).append(r)
    summary = {}
    for fam, rs in sorted(fams.items()) + [("all", rows)]:
        summary[fam] = {v: statistics.geometric_mean(r[v] / r["plain"] for r in rs) for v in variants[1:]}
        if "default" in variants and "default+tier" in variants:
            summary[fam]["tier|default"] = statistics.geometric_mean(r["default+tier"] / r["default"] for r in rs)
        print(f"{fam:12s} n={len(rs):2d} " + " ".join(f"{k}={v:.4f}" for k, v in summary[fam].items()))
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "k": a.k, "rows": rows, "summary": summary}, f, indent=1)
    sys.exit(0 if all(r["same_digest"] for r in rows) else 1)


if __name__ == "__main__":
    main()
