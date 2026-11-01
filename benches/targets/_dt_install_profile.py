"""Dev helper: inclusive callgrind profile of one region (enable(), an import)."""
import glob, os, subprocess, sys, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
if len(sys.argv) > 1 and sys.argv[1] == "_w":
    import _callgrind
    h = _callgrind.helper()
    exec(sys.argv[2], {})
    ns = {}
    exec(sys.argv[3].split(";;")[0], ns)
    h.start()
    exec(sys.argv[3].split(";;")[1], ns)
    h.dump("a")
    h.stop()
    sys.exit()
setup, stmt = sys.argv[1], sys.argv[2]
with tempfile.TemporaryDirectory() as d:
    out = os.path.join(d, "cg.out")
    subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}",
                    sys.executable, __file__, "_w", "pass", f"{setup};;{stmt}"], check=True)
    f = sorted(glob.glob(out + "*"))[-1]
    r = subprocess.run(["callgrind_annotate", "--inclusive=yes", "--threshold=99.9", f], capture_output=True, text=True)
    lines = [l for l in r.stdout.splitlines() if "rustique" in l or "TOTALS" in l or "Py" in l and "TAIL" not in l]
    print("\n".join(lines[: int(os.environ.get("LINES", "60"))]))
