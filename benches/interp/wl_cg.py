"""Exact instruction count (callgrind, client requests) of one steady-state run() of a
workload, plain or with an eval-loop variant (varstat.py's variant syntax).

    python benches/interp/wl_cg.py <workload.py> --variant V [--variant V ...]
"""
import argparse, json, os, subprocess, sys, tempfile, glob
HERE = os.path.dirname(os.path.abspath(__file__))

WORKER = r'''
import sys
sys.path.insert(0, {here!r}); sys.path.insert(0, {targets!r})
import varstat, _callgrind
from opprof import load_workload
h = _callgrind.helper()
varstat.apply_variant({variant!r})
w = load_workload({wl!r}); st = w.setup(); w.run(st); w.run(st)
h.start(); w.run(st); h.dump("r"); h.stop()
'''


def one(wl, variant):
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "w.py")
        open(p, "w").write(WORKER.format(here=HERE, targets=os.path.join(HERE, "..", "targets"),
                                         variant=variant, wl=os.path.abspath(wl)))
        out = os.path.join(d, "cg.out")
        subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no",
                        "-q", f"--callgrind-out-file={out}", sys.executable, p],
                       check=True, env=dict(os.environ, PYTHONHASHSEED="0"),
                       stdout=subprocess.DEVNULL)
        for fn in glob.glob(out + "*"):
            name = tot = None
            for line in open(fn):
                if line.startswith("desc: Trigger: Client Request: "):
                    name = line.split("Client Request: ", 1)[1].strip()
                elif line.startswith(("totals:", "summary:")):
                    tot = int(line.split()[1]); break
            if name == "r":
                return tot


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("workload"); ap.add_argument("--variant", action="append", required=True)
    ap.add_argument("-o")
    a = ap.parse_args()
    res = {}
    for v in a.variant:
        res[v] = one(a.workload, v)
        b = res[a.variant[0]]
        print(f"{os.path.basename(a.workload)[:-3]:20s} {v:50s} {res[v]:>15,} Ir ({res[v] / b:.4f})", flush=True)
    if a.o:
        json.dump(res, open(a.o, "w"))


if __name__ == "__main__":
    main()
