"""Where `import rustique; rustique.enable()` spends its instructions (callgrind regions,
fresh process each): loading the extension (dlopen + PyInit), executing it (type objects,
submodules), the Python package (`rustique/__init__.py`, `_takeover`), and `enable()`.

Usage: python benches/startup_breakdown.py
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "targets"))
from _callgrind import region  # noqa: E402

FIND = (
    "import importlib.util, importlib.machinery, os, sys\n"
    "import rustique as _probe_pkg\n"
)
# Locate the .so without importing the package (spec search only).
SPEC = (
    "import importlib.util, sys\n"
    "_s = importlib.util.find_spec('rustique')\n"
    "import os\n"
    "_d = os.path.dirname(_s.origin)\n"
    "import importlib.machinery as _m\n"
    "_so = [os.path.join(_d, f) for f in os.listdir(_d) if f.startswith('_rustique.') and f.endswith(_m.EXTENSION_SUFFIXES[0])][0]\n"
    "_es = importlib.util.spec_from_file_location('rustique._rustique', _so)\n"
)

cells = {
    "dlopen + PyInit (create)": (SPEC, "_mod = importlib.util.module_from_spec(_es)"),
    "module exec (types, submodules)": (SPEC + "_mod = importlib.util.module_from_spec(_es)\n", "_es.loader.exec_module(_mod)"),
    "import rustique (everything)": ("import importlib.util", "import rustique"),
    "import rustique (ext preloaded)": (SPEC + "_mod = importlib.util.module_from_spec(_es); _es.loader.exec_module(_mod); sys.modules['rustique._rustique'] = _mod\n", "import rustique"),
    "rustique.enable()": ("import rustique", "rustique.enable()"),
    "rustique.enable() 2nd call": ("import rustique; rustique.enable()", "rustique.enable()"),
    "rustique.status()": ("import rustique; rustique.enable()", "rustique.status()"),
    "_accel.enable(quiet=True)": ("import rustique", "rustique._rustique._accel.enable(quiet=True)"),
}
for name, (setup, stmt) in cells.items():
    print(f"{name:36s} {region(stmt, setup):>12,d} Ir", flush=True)
