"""Whole-process run of one workload for benches/atlas/layers.py (--modes whole,wholeplain).

    python _whole.py <workload.py> <marks-out>

Nothing but the workload: interpreter startup, import of the workload module, setup(), one
run(), interpreter finalization. CLOCK_MONOTONIC marks (ns) go to <marks-out>, written after
run() returns (finalization follows)."""

import os
import sys
import time

t_main = time.monotonic_ns()
wl, out = sys.argv[1], sys.argv[2]
sys.path.insert(0, os.path.dirname(wl))
import importlib.util  # noqa: E402

name = "workload_" + os.path.splitext(os.path.basename(wl))[0]
spec = importlib.util.spec_from_file_location(name, wl)
mod = importlib.util.module_from_spec(spec)
sys.modules[name] = mod
spec.loader.exec_module(mod)
t_imported = time.monotonic_ns()
state = mod.setup()
t_setup = time.monotonic_ns()
mod.run(state)
t_run = time.monotonic_ns()
with open(out, "w") as f:
    f.write(f"{os.getpid()} {t_main} {t_imported} {t_setup} {t_run}\n")
