"""Diff two function-level callgrind profiles (plain vs enabled) of one region:
python benches/datamodel/cg_diff.py SETUP_FILE STMT"""
import os, re, subprocess, sys, tempfile
from collections import Counter
setup = open(sys.argv[1]).read()
stmt = sys.argv[2]
helper_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "targets")
def prof(on):
    code = (("from rustique import _takeover as _T\n_T.enable(only=['enum','generated','contextlib'])\n" if on else "import rustique\n")
            + setup + "\nimport sys\nsys.path.insert(0, %r)\nfrom _callgrind import helper\nh = helper()\n" % helper_dir
            + "h.start()\nexec(%r)\nh.stop()\n" % stmt)
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "s.py"); open(p, "w").write(code)
        out = os.path.join(d, "cg.out")
        subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}", sys.executable, p], check=True, env={**os.environ, "PYTHONHASHSEED": "0"})
        r = subprocess.run(["callgrind_annotate", "--threshold=100", out], capture_output=True, text=True)
    c = Counter()
    for line in r.stdout.splitlines():
        m = re.match(r"\s*([\d,]+) \([^)]*\)\s+\S*:(\S+)", line)
        if m:
            c[m.group(2)] += int(m.group(1).replace(",", ""))
    return c
a, b = prof(False), prof(True)
print("total plain", sum(a.values()), "on", sum(b.values()))
d = Counter({k: b[k] - a[k] for k in set(a) | set(b)})
for k, v in sorted(d.items(), key=lambda kv: -abs(kv[1]))[:25]:
    print(f"{v:>10,}  {k}")
