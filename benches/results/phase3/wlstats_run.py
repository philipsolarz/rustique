"""Per-target counters of ONE `run()` of a workload (setup and warm-up excluded).

    python wlstats_run.py data/state_snapshot OUT.json      (cwd = the repository root)

`rustique.enable()` before the workload module is imported (as benches/workloads/run.py
does), counters on, `setup()`, one warm-up `run()`, counters reset, one `run()`, then
`rustique.stats()` and the target states are written as JSON (same record shape as a
`RUSTIQUE_REPORT=*.json` record: pid, argv, status, stats, counters).
"""

import importlib.util
import json
import os
import sys

import rustique

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
result = mod.run(state)
rec = {"pid": os.getpid(), "argv": sys.argv, "status": rustique.status(), "stats": rustique.stats(),
       "counters": True, "digest": mod.digest(result)}
with open(out, "w") as f:
    json.dump(rec, f, default=list)
    f.write("\n")
