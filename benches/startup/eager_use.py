"""Which eagerly installed targets does a short process actually call? (docs/atlas/w2-startup.md §2.4)

    RUSTIQUE=1 RUSTIQUE_STATS=1 python benches/startup/eager_use.py [IMPORT ...]

For each import (default: the `import_startup` workload's 21), a fresh process with
RUSTIQUE=1 and counters on imports it, then reports which targets installed at startup
(state `installed` before the import ran: modules imported by the interpreter itself) got
any call (hit or fallback) during the whole process. Eager targets that no process calls are
validation work that a first-call (deferred) install would never do.
"""
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "workloads", "devtools"))
from import_startup import IMPORTS  # noqa: E402

CHILD = r"""
import sys
{imp}
import rustique
st = rustique.status()
eager = [t["name"] for t in st["targets"] if t["state"] == "installed" and t.get("module") in (None, "posixpath", "time")]
c = rustique.stats()
used = {{n: c[n]["hits"] + sum(c[n]["fallback"].values()) for n in eager if n in c}}
sys.stdout.write("\n@@" + repr(used))
"""


def main():
    imps = sys.argv[1:] or list(IMPORTS)
    env = {**os.environ, "RUSTIQUE": "1", "RUSTIQUE_STATS": "1"}
    calls = {}
    for mod in imps:
        code = CHILD.format(imp=f"import {mod}" if mod else "")
        out = subprocess.run([sys.executable, "-c", code], env=env, capture_output=True, text=True)
        if "@@" not in out.stdout:
            print(mod, "failed", out.stderr[-300:])
            continue
        used = eval(out.stdout.rsplit("@@", 1)[1])
        calls[mod or "<bare>"] = used
        print(f"{mod or '<bare>':<26} called {sum(1 for v in used.values() if v)}/{len(used)}: "
              + ", ".join(f"{k}={v}" for k, v in sorted(used.items()) if v))
    names = sorted({n for u in calls.values() for n in u})
    print("\nper target: processes that called it")
    for n in names:
        print(f"  {n:<22} {sum(1 for u in calls.values() if u.get(n))}/{len(calls)}")
    json.dump(calls, sys.stderr)


if __name__ == "__main__":
    main()
