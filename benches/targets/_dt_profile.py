"""Profile one statement under callgrind (inclusive per-function costs). Dev helper.

python benches/targets/_dt_profile.py "time.strftime(ISOF, tup)" [n]
"""
import os, subprocess, sys, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import datetime_cg as cg

if len(sys.argv) > 1 and sys.argv[1] == "_w":
    h = cg.helper()
    ns = {}
    exec(cg.COMMON, ns)
    if not os.environ.get('NO_RQ'):
        exec(cg.RQ, ns)
    stmt = sys.argv[2]
    if ";;" in stmt:
        setup, stmt = stmt.split(";;", 1)
        exec(setup, ns)
    exec(f"def fn(_n):\n    for _ in range(_n):\n        {stmt}\n", ns)
    ns["fn"](50)
    h.start()
    ns["fn"](int(sys.argv[3]))
    h.dump("a")
    h.stop()
    sys.exit()
stmt = sys.argv[1]
n = sys.argv[2] if len(sys.argv) > 2 else "1000"
with tempfile.TemporaryDirectory() as d:
    out = os.path.join(d, "cg.out")
    subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}",
                    sys.executable, __file__, "_w", stmt, n], check=True, env={**os.environ, "TZ": "UTC"})
    import glob
    f = [x for x in glob.glob(out + "*")][-1]
    r = subprocess.run(["callgrind_annotate", "--threshold=99", f], capture_output=True, text=True)
    print("\n".join(r.stdout.splitlines()[:int(os.environ.get("LINES", "45"))]))
    if os.environ.get("TREE"):
        r = subprocess.run(["callgrind_annotate", "--tree=caller", "--threshold=100", f], capture_output=True, text=True)
        lines = r.stdout.splitlines()
        for i, l in enumerate(lines):
            if os.environ["TREE"] in l and "*" in l:
                print("\n".join(lines[max(0, i - 6):i + 1]))
        sys.exit()
    r = subprocess.run(["callgrind_annotate", "--inclusive=yes", "--threshold=100", f], capture_output=True, text=True)
    print("\n".join(l for l in r.stdout.splitlines() if "rustique" in l or "PyUnicode" in l or "Py" in l and "TAIL" not in l)[:6000])
