"""Function-level callgrind profile of one statement's loop (for explaining a cell).

    python benches/interp/cg_annotate_cell.py "stmt" --setup CODE [--n 2000] [--top 25] [--jit]
Prints callgrind_annotate's inclusive=no top functions for n executions (unrolled 10x).
"""
import argparse, json, os, subprocess, sys, tempfile
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets"))
import _callgrind

WORKER = r'''
import sys, json
sys.path.insert(0, {tdir!r})
import _callgrind
h = _callgrind.helper()
ns = {{"__name__": "__bench__"}}
exec({setup!r}, ns)
names = [k for k in ns if not k.startswith("__") and k.isidentifier()]
binds = "".join(f"    {{k}} = _g[{{k!r}}]\n" for k in names)
ns["_g"] = ns
body = "".join(f"        {stmt}\n" for _ in range(10))
exec(f"def _fn(_n):\n{{binds}}    for _ in range(_n):\n{{body}}", ns)
fn = ns["_fn"]
fn({warm})
h.start(); fn({n}); h.dump("a"); h.stop()
'''

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stmt")
    ap.add_argument("--setup", default="")
    ap.add_argument("--n", type=int, default=2000)
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--jit", action="store_true")
    a = ap.parse_args()
    tdir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets")
    with tempfile.TemporaryDirectory() as d:
        w = os.path.join(d, "w.py")
        open(w, "w").write(WORKER.format(tdir=tdir, setup=a.setup, stmt=a.stmt, n=a.n,
                                         warm=30000 if a.jit else 100))
        out = os.path.join(d, "cg.out")
        env = dict(os.environ, PYTHONHASHSEED="0", PYTHON_JIT="1" if a.jit else "0")
        subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q",
                        f"--callgrind-out-file={out}", sys.executable, w], env=env, check=True)
        import glob
        f = [x for x in glob.glob(out + "*") if open(x).read(4000).find("Client Request: a") >= 0][0]
        r = subprocess.run(["callgrind_annotate", "--inclusive=no", f"--threshold=99", f],
                           capture_output=True, text=True)
        lines = r.stdout.splitlines()
        tot = next((l for l in lines if "PROGRAM TOTALS" in l), "")
        print(tot)
        i = next(i for i, l in enumerate(lines) if "file:function" in l)
        for l in lines[i + 2:i + 2 + a.top]:
            print(l)

if __name__ == "__main__":
    main()
