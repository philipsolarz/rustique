"""Lazy-install cells in a minimal fresh process (w2-floors §4), the method of w2-startup's
cells: GC disabled, cold (the measured import is the process's first activation), callgrind
region around `import X` only. Three variants per module: plain (extension imported,
nothing enabled), `only=[group]`, and every group enabled (what `RUSTIQUE=1` does, which
also installs the groups of target modules that X imports on the way).

    python benches/floors/install/lazy_cells.py [module ...]
"""
import json, os, subprocess, sys, sysconfig, tempfile, glob

CACHE = os.path.join(os.path.expanduser("~"), ".cache", "rustique-callgrind")
GROUP = {"json": "json", "copy": "copy", "logging": "logging", "random": "random", "pathlib": "pathlib",
         "textwrap": "textwrap", "urllib.parse": "urllib", "statistics": "statistics", "dataclasses": "dataclasses"}
WORKER = r'''
import sys, gc
sys.path.insert(0, {cache!r})
import _rq_callgrind_v1 as h
import rustique._rustique as r
gc.disable()
mode = {mode!r}
if mode == "only":
    r._accel.enable([{group!r}])
elif mode == "all":
    r._accel.enable()
before = set(r._accel.installed() if mode != "plain" else [])
h.start()
import {mod}
h.dump("r")
h.stop()
after = set(r._accel.installed() if mode != "plain" else [])
print("INSTALLED", sorted({{n.split(".")[0] if not n.startswith(("datetime", "time.")) else n for n in after - before}}))
'''

def cell(mod, mode):
    code = WORKER.format(cache=CACHE, mode=mode, group=GROUP[mod], mod=mod)
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        r = subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}",
                            sys.executable, "-c", code], capture_output=True, text=True,
                           env={**os.environ, "PYTHONHASHSEED": "0"})
        if r.returncode:
            raise RuntimeError(r.stderr[-1500:])
        inst = [l for l in r.stdout.splitlines() if l.startswith("INSTALLED")]
        for fn in glob.glob(out + "*"):
            tag = None
            for line in open(fn):
                if line.startswith("desc: Trigger: Client Request: "):
                    tag = line.split("Client Request: ", 1)[1].strip()
                elif line.startswith(("totals:", "summary:")) and tag == "r":
                    return int(line.split()[1]), (inst[0][10:] if inst else "")
    raise RuntimeError("no region")

for mod in sys.argv[1:] or ["json", "copy", "logging", "random", "pathlib", "textwrap", "urllib.parse", "statistics"]:
    p, _ = cell(mod, "plain")
    o, io = cell(mod, "only")
    a, ia = cell(mod, "all")
    print(json.dumps({"module": mod, "plain": p, "only": o, "only_over": o - p, "only_pct": round(100 * (o / p - 1), 2),
                      "all": a, "all_over": a - p, "all_pct": round(100 * (a / p - 1), 2), "installed_only": io,
                      "installed_all": ia}), flush=True)
