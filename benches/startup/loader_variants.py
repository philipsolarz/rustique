"""Whole-process cost of alternative `RUSTIQUE=1` startup lines (docs/atlas/w2-startup.md §2.1),
with the same installed extension.

    python benches/startup/loader_variants.py PLAIN_VENV RUSTIQUE_VENV [--wall-rounds 200] [-o out.json]

Each variant is a venv linking RUSTIQUE_VENV's `rustique` package with another
`_rustique_startup.pth` (plus extra top-level files). Cells: callgrind Ir and perf
cycles/faults/wall of `python -c pass` with RUSTIQUE unset and with RUSTIQUE=1, against the
plain venv. Variants:

    shipped     the wave-1 line: exec() of a loader string (compiled at every RUSTIQUE=1 start)
    lazyflags   shipped, with sys.setdlopenflags(RTLD_LAZY) around the load (lazy binding?)
    bootmod     gate imports a top-level `_rustique_boot.py` (pyc-cached) holding the loader
    toplevel    gate imports the extension as a top-level module `_rustique` (a link at the
                site-packages root), `startup()` in Rust does the env tests
    engpkg      gate imports `_rustique_engine._rustique`: the extension in an empty package
                (what maturin's mixed layout can build)
    shortexec   gate exec()s the shortest loader string (no env tests: Rust does them)
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from decompose import callgrind, env_for, perf, site_packages, wall  # noqa: E402

GATE = "import os; os.environ.get('RUSTIQUE', '0') != '0' and "

BOOT = '''\
import os, sys, _imp
p = os.path.join(os.path.dirname(__file__), 'rustique', '_rustique' + _imp.extension_suffixes()[0])
if not (os.environ['RUSTIQUE'].strip().lower() in ('', 'off', 'false', 'no') or sys.flags.ignore_environment):
    (sys.modules['_frozen_importlib']._load(sys.modules['_frozen_importlib_external'].spec_from_file_location('rustique._rustique', p))
     if os.path.isfile(p) else __import__('rustique._rustique')._rustique).startup()
'''


def variants(shipped: str, suffix: str) -> dict[str, tuple[str, dict[str, str]]]:
    loader = shipped.split('exec("', 1)[1].rsplit('")', 1)[0]
    lazy = loader.replace("(sys.modules['_frozen_importlib']._load(",
                          "(sys.setdlopenflags(os.RTLD_LAZY) or sys.modules['_frozen_importlib']._load(")
    assert lazy != loader
    return {
        "shipped": (shipped, {}),
        "lazyflags": (GATE + 'exec("' + lazy + '")\n', {}),
        "bootmod": (GATE + "__import__('_rustique_boot')\n", {"_rustique_boot.py": BOOT}),
        "toplevel": (GATE + "__import__('_rustique').startup()\n", {"@_rustique" + suffix: "rustique/_rustique" + suffix}),
        # maturin's mixed layout can ship this (module-name = "_rustique_engine._rustique",
        # python-packages = ["rustique"]): an empty package holding the extension
        "engpkg": (GATE + "__import__('_rustique_engine._rustique')._rustique.startup()\n",
                   {"_rustique_engine/__init__.py": "", "@_rustique_engine/_rustique" + suffix: "rustique/_rustique" + suffix}),
        "shortexec": (GATE + 'exec("sys.modules[\'_frozen_importlib\']._load(sys.modules[\'_frozen_importlib_external\'].spec_from_file_location(\'rustique._rustique\',sitedir+\'/rustique/_rustique\'+__import__(\'_imp\').extension_suffixes()[0])).startup()")\n', {}),
    }


def make(rq_venv: str, name: str, line: str, files: dict[str, str]) -> str:
    import subprocess
    path = f"{rq_venv.rstrip('/')}.lv-{name}"
    py = os.path.join(path, "bin", "python")
    if not os.path.exists(py):
        exe = os.path.realpath(os.path.join(rq_venv, "bin", "python"))
        subprocess.check_call([exe, "-m", "venv", "--without-pip", path])
    src, dst = site_packages(os.path.join(rq_venv, "bin", "python")), site_packages(py)
    for entry in os.listdir(src):
        if entry.startswith("rustique") and not os.path.lexists(os.path.join(dst, entry)):
            os.symlink(os.path.join(src, entry), os.path.join(dst, entry))
    with open(os.path.join(dst, "_rustique_startup.pth"), "w") as f:
        f.write(line)
    for fname, content in files.items():
        if fname.startswith("@"):  # a link to a file of the package
            target = os.path.join(dst, fname[1:])
            if not os.path.lexists(target):
                os.symlink(os.path.join(src, content), target)
        else:
            os.makedirs(os.path.dirname(os.path.join(dst, fname)), exist_ok=True)
            with open(os.path.join(dst, fname), "w") as f:
                f.write(content)
    for code in ("pass", "import rustique"):
        subprocess.check_call([py, "-c", code], env=env_for({}))
        subprocess.check_call([py, "-c", code], env=env_for({"RUSTIQUE": "1"}))
    return py


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("plain")
    ap.add_argument("rustique")
    ap.add_argument("--only", default="")
    ap.add_argument("--wall-rounds", type=int, default=200)
    ap.add_argument("--perf-runs", type=int, default=200)
    ap.add_argument("-o", "--output")
    args = ap.parse_args()
    import _imp
    rq = os.path.join(args.rustique, "bin", "python")
    shipped = open(os.path.join(site_packages(rq), "_rustique_startup.pth")).read()
    import subprocess
    suffix = subprocess.check_output([rq, "-c", "import _imp; print(_imp.extension_suffixes()[0])"], text=True).strip()
    del _imp
    vs = variants(shipped, suffix)
    if args.only:
        vs = {k: v for k, v in vs.items() if k in args.only.split(",") or k == "shipped"}
    cells: dict[str, tuple[str, dict[str, str]]] = {"plain": (os.path.join(args.plain, "bin", "python"), env_for({}))}
    for name, (line, files) in vs.items():
        py = make(args.rustique, name, line, files)
        cells[f"{name} unset"] = (py, env_for({}))
        cells[f"{name} on"] = (py, env_for({"RUSTIQUE": "1"}))
    ir = {k: callgrind(py, env) for k, (py, env) in cells.items()}
    pf = {k: perf(py, env, args.perf_runs) for k, (py, env) in cells.items()}
    wl = wall(cells, args.wall_rounds)
    print(f"{'cell':<18} {'Ir':>12} {'vs plain':>9} {'cycles':>12} {'faults':>7} {'wall ms':>8} {'vs plain':>9}")
    p = cells["plain"]
    for k in cells:
        cyc = pf[k].get("cycles:u", 0) + pf[k].get("cycles:k", 0)
        print(f"{k:<18} {ir[k]:>12,} {ir[k] / ir['plain']:>9.4f} {cyc:>12,.0f} {pf[k].get('page-faults', 0):>7.0f} "
              f"{wl[k]:>8.3f} {wl[k] / wl['plain']:>9.4f}")
    print()
    for name in vs:
        u, o = f"{name} unset", f"{name} on"
        print(f"{name:<10} on/unset: Ir {ir[o] / ir[u]:.4f} (+{ir[o] - ir[u]:,}), wall {wl[o] / wl[u]:.4f} "
              f"(+{wl[o] - wl[u]:.3f} ms)")
    if args.output:
        with open(args.output, "w") as f:
            json.dump({"ir": ir, "perf": pf, "wall_ms": wl}, f, indent=1)


if __name__ == "__main__":
    main()
