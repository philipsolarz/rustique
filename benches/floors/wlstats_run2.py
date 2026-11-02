"""benches/results/phase3/wlstats_run.py plus the w2-floors prototype trace (if the build has
`_rustique._proto_trace`): counters of ONE steady-state `run()`.

    python benches/floors/wlstats_run2.py data/state_snapshot OUT.json   (cwd = repository root)
"""
import importlib.util, json, os, sys
import rustique
from rustique import _rustique
rustique.enable()
rustique.stats(True)
name, out = sys.argv[1], sys.argv[2]
path = os.path.join(os.getcwd(), "benches", "workloads", name + ".py")
spec = importlib.util.spec_from_file_location("workload_" + name.replace("/", "_"), path)
mod = importlib.util.module_from_spec(spec)
sys.path.insert(0, os.path.dirname(path))
spec.loader.exec_module(mod)
state = mod.setup()
mod.run(state)
rustique.stats(reset=True)
t0 = len(_rustique._proto_trace()) if hasattr(_rustique, "_proto_trace") else 0
base = {(a, b): n for a, b, n in _rustique._proto_trace()} if hasattr(_rustique, "_proto_trace") else {}
result = mod.run(state)
rec = {"pid": os.getpid(), "argv": sys.argv, "status": rustique.status(), "stats": rustique.stats(),
       "counters": True, "digest": mod.digest(result)}
if hasattr(_rustique, "_proto_trace"):
    rec["proto_trace"] = sorted(([a, b, n - base.get((a, b), 0)] for a, b, n in _rustique._proto_trace()
                                 if n - base.get((a, b), 0) > 0), key=lambda r: -r[2])
with open(out, "w") as f:
    json.dump(rec, f, default=list)
    f.write("\n")
