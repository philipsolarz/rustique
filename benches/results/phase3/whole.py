"""One workload of benches/workloads per process: `setup()` + one `run()`, nothing else.

    python whole.py data/state_snapshot        (cwd = the repository root)

For whole-process timing under `pyperf command` (run_phase3.sh `whole`): the timed process
includes interpreter start, the `.pth` startup switch (`RUSTIQUE=1`), imports, setup and
one run. With `$WHOLE_DIGEST` set, appends `<digest> <rustique loaded?>` to that file, so
that the plain and enabled processes' results can be compared afterwards.
"""

import importlib.util
import os
import sys

name = sys.argv[1]
path = os.path.join(os.getcwd(), "benches", "workloads", name + ".py")
spec = importlib.util.spec_from_file_location("workload_" + name.replace("/", "_"), path)
mod = importlib.util.module_from_spec(spec)
sys.path.insert(0, os.path.dirname(path))
spec.loader.exec_module(mod)
result = mod.run(mod.setup())
out = os.environ.get("WHOLE_DIGEST")
if out:
    with open(out, "a") as f:
        f.write(f"{mod.digest(result)} rustique={'rustique._rustique' in sys.modules}\n")
