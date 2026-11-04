"""Ir of compiling (and running) candidate `RUSTIQUE=1` loader strings, one fresh process per
cell (callgrind regions, benches/targets/_callgrind.py). The shipped `.pth` line exec()s a
string literal when RUSTIQUE is set, so its compile is paid by every RUSTIQUE=1 process.

    python benches/startup/loader_compile.py
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "targets"))
from _callgrind import region  # noqa: E402

PTH = open(os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
                        "python", "_rustique_startup.pth")).read()
SHIPPED = PTH.split('exec("', 1)[1].rsplit('")', 1)[0]
CANDIDATES = {
    "shipped loader": SHIPPED,
    "no OFF/-E tests": "import sys,_imp;p=sitedir+'/rustique/_rustique'+_imp.extension_suffixes()[0];(sys.modules['_frozen_importlib']._load(sys.modules['_frozen_importlib_external'].spec_from_file_location('rustique._rustique',p)) if os.path.isfile(p) else __import__('rustique._rustique')._rustique)._accel.startup()",
    "create_dynamic + namespace": "import _imp;m=_imp.create_dynamic(type(sys.implementation)(name='rustique._rustique',origin=sitedir+'/rustique/_rustique'+_imp.extension_suffixes()[0]))",
    "one call": "f(a,b)",
    "pass": "pass",
}
for name, s in CANDIDATES.items():
    ir = region("compile(S, '<pth>', 'exec')", setup=f"S = {s!r}")
    print(f"{name:<32} {len(s):>4} chars {ir:>10,} Ir", flush=True)
