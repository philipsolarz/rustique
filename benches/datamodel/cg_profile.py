"""Function-level callgrind profile of one statement (enabled), for diagnosing a cell:
python benches/datamodel/cg_profile.py SETUP_FILE STMT"""
import os, subprocess, sys, tempfile
setup = open(sys.argv[1]).read()
stmt = sys.argv[2]
on = os.environ.get("DM_OFF") != "1"
code = (("from rustique import _takeover as _T\n_T.enable(only=['enum','generated','contextlib'])\n" if on else "")
        + setup + "\nimport sys\nsys.path.insert(0, %r)\nfrom _callgrind import helper\nh = helper()\n" % os.path.join(os.path.dirname(__file__), "..", "targets")
        + ("def _f():\n    for _ in range(2000):\n        %s\n_f()\nh.start()\n_f()\nh.stop()\n" % stmt
           if os.environ.get("DM_REGION") != "1" else "h.start()\nexec(%r)\nh.stop()\n" % stmt))
with tempfile.TemporaryDirectory() as d:
    p = os.path.join(d, "s.py"); open(p, "w").write(code)
    out = os.path.join(d, "cg.out")
    subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}", sys.executable, p], check=True)
    r = subprocess.run(["callgrind_annotate", "--threshold=99", out], capture_output=True, text=True)
    print("\n".join(r.stdout.splitlines()[:45]))
