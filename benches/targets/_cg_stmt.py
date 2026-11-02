"""Function-level callgrind profile of one statement (exclusive Ir), for kernel tuning.

    python benches/targets/_cg_stmt.py "sorted(d)" --setup "d = [f'k{i:05d}' for i in range(1000)]" [--enable builtins,containers] [-n 20] [--top 30]
"""
import argparse, os, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER = r'''
import sys
sys.path.insert(0, {here!r})
from _callgrind import helper
h = helper()
{setup}
if {enable!r}:
    import rustique
    rustique.enable(only={enable!r}.split(","))
for _ in range(3):
    {stmt}
h.start()
for _ in range({n}):
    {stmt}
h.stop()
h.dump("cell")
'''

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stmt"); ap.add_argument("--setup", default=""); ap.add_argument("--enable", default="")
    ap.add_argument("-n", type=int, default=20); ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--inclusive", action="store_true")
    a = ap.parse_args()
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "w.py")
        with open(src, "w") as f:
            f.write(WORKER.format(here=HERE, setup=a.setup, enable=a.enable, stmt=a.stmt, n=a.n))
        out = os.path.join(d, "cg.out")
        subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}", sys.executable, src], check=True)
        files = sorted(f for f in os.listdir(d) if f.startswith("cg.out."))
        target = os.path.join(d, files[0])
        args = ["callgrind_annotate", "--threshold=99.9"] + (["--inclusive=yes"] if a.inclusive else []) + [target]
        lines = subprocess.run(args, capture_output=True, text=True).stdout.splitlines()
        shown = 0
        for l in lines:
            if "PROGRAM TOTALS" in l:
                print(l)
            elif "%)" in l and shown < a.top:
                print(l[:190]); shown += 1

if __name__ == "__main__":
    main()
