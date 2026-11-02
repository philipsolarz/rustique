"""Wall-time decomposition of the RUSTIQUE=1 startup path (docs/atlas/w2-startup.md §1.3).

    python benches/startup/wall_steps.py VENV [-n 200] [--cores 16-23] [--toplevel]

Runs `python -c CODE` in VENV with RUSTIQUE unset (so the `.pth` gate stops), where CODE
replays the startup path step by step with `time.perf_counter_ns()` between steps (user +
kernel time of each step, page faults included), and prints the median per step over `-n`
fresh processes pinned with taskset. Steps:

    find      locate the extension: `spec_from_file_location` on the known path (shipped
              line), or the full `sys.meta_path` search for a top-level `_rustique`
              (`--toplevel`; VENV must have the `_rustique<suffix>` link at the site-packages
              root, as made by loader_variants.py)
    dlopen    `_imp.create_dynamic(spec)`: mmap, relocations, symbol binding, module object
    exec      `_imp.exec_dynamic(m)`
    enable    `m.startup()` with RUSTIQUE=1 set in-process (os.environ, before the call)
Also: the whole process (`-c pass` vs `-c CODE`) and the getrusage minor-fault count after
each step. The difference between the process total and the steps is interpreter
startup/teardown, including unmapping the extension at exit.
"""

from __future__ import annotations

import argparse
import os
import statistics
import subprocess
import time

CODE = r"""
import time, sys, os, resource
c = time.perf_counter_ns
f = lambda: resource.getrusage(resource.RUSAGE_SELF).ru_minflt
r = []; t = c(); r.append(('start', t, f()))
import _imp
if {toplevel}:
    spec = sys.modules['_frozen_importlib']._find_spec('_rustique', None)
else:
    sp = [p for p in sys.path if p.endswith('site-packages')][-1]
    spec = sys.modules['_frozen_importlib_external'].spec_from_file_location(
        'rustique._rustique', sp + '/rustique/_rustique' + _imp.extension_suffixes()[0])
r.append(('find', c(), f()))
m = _imp.create_dynamic(spec)
r.append(('dlopen', c(), f()))
_imp.exec_dynamic(m)
sys.modules[spec.name] = m
r.append(('exec', c(), f()))
os.environ['RUSTIQUE'] = '1'
m.startup()
r.append(('enable', c(), f()))
print(' '.join(f'{{n}}:{{t - r[0][1]}}:{{fl - r[0][2]}}' for n, t, fl in r[1:]))
"""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("venv")
    ap.add_argument("-n", type=int, default=200)
    ap.add_argument("--cores", default="16-23")
    ap.add_argument("--toplevel", action="store_true")
    a = ap.parse_args()
    py = os.path.join(a.venv, "bin", "python")
    code = CODE.format(toplevel=a.toplevel)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "PYTHON", "VIRTUAL_ENV"))}
    env["PYTHONHASHSEED"] = "0"
    pre = ["taskset", "-c", a.cores] if a.cores else []
    steps: dict[str, list[float]] = {}
    faults: dict[str, list[int]] = {}
    whole = {"pass": [], "steps": []}
    for _ in range(a.n):
        for kind, c in (("pass", "pass"), ("steps", code)):
            t0 = time.perf_counter_ns()
            out = subprocess.run(pre + [py, "-c", c], env=env, capture_output=True, text=True)
            whole[kind].append((time.perf_counter_ns() - t0) / 1e6)
            if out.returncode:
                raise SystemExit(out.stderr)
            if kind == "steps":
                prev_t, prev_f = 0, 0
                for item in out.stdout.split():
                    n, t, fl = item.split(":")
                    steps.setdefault(n, []).append((int(t) - prev_t) / 1e3)
                    faults.setdefault(n, []).append(int(fl) - prev_f)
                    prev_t, prev_f = int(t), int(fl)
    print(f"{a.venv} ({'top-level search' if a.toplevel else 'known path'}), n={a.n}, cores {a.cores}")
    total = 0.0
    for n, v in steps.items():
        med = statistics.median(v)
        total += med
        print(f"  {n:<8} {med:>8.1f} us  (p10 {sorted(v)[len(v) // 10]:.1f}, p90 {sorted(v)[9 * len(v) // 10]:.1f})"
              f"  minor faults {statistics.median(faults[n]):.0f}")
    print(f"  {'sum':<8} {total:>8.1f} us")
    p, s = statistics.median(whole["pass"]), statistics.median(whole["steps"])
    print(f"  process: -c pass {p:.3f} ms, -c steps {s:.3f} ms, difference {1000 * (s - p):.0f} us")


if __name__ == "__main__":
    main()
