"""Profile one logging_cg cell (variant) under callgrind and print inclusive costs.
Usage: python benches/targets/_prof_cell.py CELL VARIANT"""
import os, subprocess, sys, tempfile
here = os.path.dirname(os.path.abspath(__file__))
cell, variant = sys.argv[1], sys.argv[2]
with tempfile.TemporaryDirectory() as d:
    out = os.path.join(d, "cg.out")
    subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}",
                    sys.executable, os.path.join(here, "logging_cg.py"), "_worker", cell, variant], check=True)
    files = sorted(f for f in os.listdir(d) if f.startswith("cg.out"))
    # the "b" dump (2n calls)
    target = [os.path.join(d, f) for f in files][-2] if len(files) > 2 else os.path.join(d, files[-1])
    r = subprocess.run(["callgrind_annotate", "--inclusive=yes", "--threshold=99.5", target], capture_output=True, text=True)
    lines = r.stdout.splitlines()
    for l in lines[:12]:
        print(l)
    n = 0
    for l in lines[12:]:
        if "TAIL_CALL" in l or "_PyEval_EvalFrame" in l:
            continue
        print(l[:170])
        n += 1
        if n > int(sys.argv[3] if len(sys.argv) > 3 else 45):
            break
