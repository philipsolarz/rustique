import sys, os, subprocess, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import helper
if len(sys.argv) > 1 and sys.argv[1] == "_w":
    h = helper()
    import rustique, posixpath, fnmatch
    from rustique._rustique import _accel as A
    A.enable(["_selftest"], [])  # warm the registry (first-enable work is not the group's)
    A.disable(["_selftest"])
    g = sys.argv[2]
    if g == "pathlib-import":
        A.enable(["pathlib"], [])
        h.start(); import pathlib; h.dump("r"); h.stop()
    elif g == "none":
        h.start(); A.enable(["_selftest"], []); h.dump("r"); h.stop()
    else:
        if g == "pathlib":
            import pathlib
        h.start(); A.enable([g], []); h.dump("r"); h.stop()
    sys.exit(0)
for g in sys.argv[1:]:
    d = tempfile.mkdtemp()
    out = os.path.join(d, "cg.out")
    subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}",
                    sys.executable, os.path.abspath(__file__), "_w", g], check=True)
    f = [x for x in os.listdir(d) if x.startswith("cg.out")]
    f = os.path.join(d, sorted(f)[-1])
    r = subprocess.run(["callgrind_annotate", "--threshold=99", "--inclusive=yes" if os.environ.get("INCL") else "--inclusive=no", f], capture_output=True, text=True)
    print(f"===== {g}")
    print("\n".join(r.stdout.splitlines()[:70]))
