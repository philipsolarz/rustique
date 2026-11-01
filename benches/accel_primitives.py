"""Per-primitive cost of the acceleration framework at install time (callgrind regions,
fresh process each, 3.15.1): fingerprints, layout checks, a target's enable with its
sites and pins, group enables. `python benches/accel_primitives.py`."""

import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "targets"))
from _callgrind import region  # noqa: E402

d = tempfile.mkdtemp()
with open(os.path.join(d, "_rustique_selftest.py"), "w") as f:
    f.write("DEP = object()\nSEP = '/'\nclass K:\n    attr = 1000\n    def __repr__(self):\n        return 'orig'\n"
            "class Obj:\n    shadow = 'c'\nOBJ = Obj()\nOBJ.a = 1\n"
            "def ident(x):\n    return ('orig', x)\ndef ident2(x):\n    return ('orig', x)\ndef same(x):\n    return x\n")
S = f"import sys; sys.path.insert(0, {d!r}); import rustique._rustique as r; A = r._accel; import posixpath, json\n"
N = 100
cells = {
    "fingerprint_code(posixpath.join)": (S, f"A._bench('fingerprint', posixpath.join, {N})", N),
    "fingerprint_code(json.dumps)": (S, f"A._bench('fingerprint', json.dumps, {N})", N),
    "fingerprint_c(list.sort)": (S, f"A._bench('fingerprint_c', list.sort, {N})", N),
    "code layout check (uncached)": (S, f"A._bench('layout', posixpath.join, {N})", N),
    "enable: 1 vectorcall + 1 pin (_selftest.same), first": (S + "import _rustique_selftest\n", "A.enable(['_selftest.same'], quiet=True)", 1),
    "enable: same target again after disable": (S + "import _rustique_selftest\nA.enable(['_selftest.same'], quiet=True); A.disable(['_selftest'], quiet=True)\n", "A.enable(['_selftest.same'], quiet=True)", 1),
    "enable: vectorcall + 5 pins (pin, absent, global, attr)": (S + "import _rustique_selftest\nA.enable(['_selftest.same'], quiet=True); A.disable(['_selftest'], quiet=True)\n", "A.enable(['_selftest.vectorcall'], quiet=True)", 1),
    "enable: slot site on a heap type": (S + "import _rustique_selftest\nA.enable(['_selftest.same'], quiet=True); A.disable(['_selftest'], quiet=True)\n", "A.enable(['_selftest.slot'], quiet=True)", 1),
    "enable: group containers (7 targets)": (S, "A.enable(['containers'], quiet=True)", 1),
    "enable: group builtins (6 targets)": (S, "A.enable(['builtins'], quiet=True)", 1),
    "enable: pending module target (no import)": (S, "A.enable(['_selftest.same'], quiet=True)", 1),
    "lazy install on import (_selftest.same)": (S + "A.enable(['_selftest.same'], quiet=True)\n", "import _rustique_selftest", 1),
    "import of the same module, nothing pending": (S, "import _rustique_selftest", 1),
    "status() (13 targets installed)": (S + "A.enable(quiet=True)\n", "A.status()", 1),
}
for k, (setup, stmt, n) in cells.items():
    print(f"{k:58s} {region(stmt, setup) / n:>12,.0f} Ir", flush=True)
