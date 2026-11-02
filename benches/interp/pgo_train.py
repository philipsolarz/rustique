"""PGO training run for the interp tier's handler sets (crates/rustique-interp/tools/pgo.sh):
the tier on, the stdlib-only custom workloads for a few seconds each, then micro cells that
reach every override's fast path and fallbacks, and the specialized LOAD_ATTR forms."""
import importlib.util
import os
import sys
import time

import rustique

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.join(HERE, "..", "workloads")
sys.path.insert(0, HERE)
import _tier  # noqa: E402

_tier.enable_tier()
SECS = float(os.environ.get("RQ_PGO_SECS", "2"))


def load(path):
    spec = importlib.util.spec_from_file_location("w_" + os.path.basename(path)[:-3], path)
    mod = importlib.util.module_from_spec(spec)
    sys.path.insert(0, os.path.dirname(path))
    spec.loader.exec_module(mod)
    return mod


names = os.environ.get("RQ_PGO_WORKLOADS", "custom")
paths = []
for sel in names.split(","):
    d = os.path.join(WL, sel)
    if os.path.isdir(d):
        paths += [os.path.join(d, f) for f in sorted(os.listdir(d)) if f.endswith(".py") and not f.startswith("_")]
    else:
        paths.append(os.path.join(WL, sel + ".py"))
for p in paths:
    try:
        m = load(p)
        st = m.setup()
    except Exception as e:  # a family whose requirements are not installed
        print("skip", p, e)
        continue
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < SECS:
        m.run(st)
    print("trained", os.path.basename(p), flush=True)

import _cells  # noqa: E402

for _ in range(200):
    for f in _cells.ALL:
        f()
rustique.disable()
print("done", _tier.groups())
