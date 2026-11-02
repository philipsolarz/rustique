"""Exact instruction count (callgrind, client requests) of one steady-state run() of a workload,
plain vs the interp tier (and/or Rustique's default groups; "noop" imports Rustique and enables
nothing, separating the tier from the import's own footprint): the arbiter when perf-stat
counts are within their noise (about ±0.5% per run).

    python benches/interp/wl_cg.py <family/name> [--variant plain --variant tier ...] [--venvs DIR]
"""
import argparse
import glob
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.normpath(os.path.join(HERE, "..", "workloads"))
WORKER = r'''
import sys
sys.path.insert(0, {here!r}); sys.path.insert(0, {targets!r})
import _callgrind
h = _callgrind.helper()
v = {variant!r}
if v != "plain":
    import _tier, rustique      # "noop": Rustique imported, nothing enabled
    if v == "default":
        rustique.enable()
    elif v in ("tier", "default+tier"):
        _tier.enable_tier(default=(v == "default+tier"))
import importlib.util, os
p = {wl!r}
sys.path.insert(0, os.path.dirname(p))
spec = importlib.util.spec_from_file_location("wl", p); m = importlib.util.module_from_spec(spec)
sys.modules["wl"] = m; spec.loader.exec_module(m)
st = m.setup(); m.run(st); m.run(st)
h.start(); r = m.run(st); h.dump("r"); h.stop()
print("DIGEST", m.digest(r))
'''


def one(py, wl, variant):
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "w.py")
        open(p, "w").write(WORKER.format(here=HERE, targets=os.path.join(HERE, "..", "targets"), variant=variant, wl=wl))
        out = os.path.join(d, "cg.out")
        r = subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "-q",
                            f"--callgrind-out-file={out}", py, p], capture_output=True, text=True,
                           env=dict(os.environ, PYTHONHASHSEED="0"))
        if r.returncode:
            raise RuntimeError(r.stderr[-2000:])
        dig = [ln for ln in r.stdout.splitlines() if ln.startswith("DIGEST")][-1]
        for fn in glob.glob(out + "*"):
            name = tot = None
            for line in open(fn):
                if line.startswith("desc: Trigger: Client Request: "):
                    name = line.split("Client Request: ", 1)[1].strip()
                elif line.startswith(("totals:", "summary:")):
                    tot = int(line.split()[1])
                    break
            if name == "r":
                return tot, dig


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("workloads", nargs="+")
    ap.add_argument("--variant", action="append")
    ap.add_argument("--venvs", default="/work/w2ws-interp/venvs")
    ap.add_argument("--py", default="3.15.1")
    ap.add_argument("-j", type=int, default=1)
    ap.add_argument("-o")
    a = ap.parse_args()
    variants = a.variant or ["plain", "tier"]
    sel = []
    for w in a.workloads:
        if "/" in w:
            sel.append(w)
        else:
            d = os.path.join(WL, w)
            sel += [f"{w}/{n[:-3]}" for n in sorted(os.listdir(d)) if n.endswith(".py") and not n.startswith(("_", "profile"))]
    import concurrent.futures as cf
    jobs = {}
    res = {}
    with cf.ThreadPoolExecutor(a.j) as ex:
        for w in sel:
            fam = w.split("/")[0]
            py = os.path.join(a.venvs, f"{fam}-{a.py}", "bin", "python")
            for v in variants:
                jobs[ex.submit(one, py, os.path.join(WL, w + ".py"), v)] = (w, v)
        for f in cf.as_completed(jobs):
            w, v = jobs[f]
            try:
                res.setdefault(w, {})[v] = f.result()
            except Exception as e:  # noqa: BLE001
                print(f"{w} {v}: FAILED {str(e)[-300:]}", flush=True)
                res.setdefault(w, {})[v] = None
            row = res[w]
            if len(row) == len(variants) and all(row.values()):
                base = row[variants[0]][0]
                digs = {x[1] for x in row.values()}
                print(f"{w:36s} " + " ".join(f"{vv}={row[vv][0] / base:.4f}" for vv in variants[1:])
                      + f"  plain={base:,}" + ("" if len(digs) == 1 else "  DIGEST MISMATCH"), flush=True)
    if a.o:
        json.dump({w: {v: (x[0] if x else None) for v, x in r.items()} for w, r in res.items()},
                  open(a.o, "w"), indent=1)


if __name__ == "__main__":
    main()
