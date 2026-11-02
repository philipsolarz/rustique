"""Where does install work go? Per-group callgrind profile of `enable()` and lazy installs
(w2-floors §4; budget: docs/ACCELERATION.md §3.12).

Each cell: a fresh process under callgrind, only the region instrumented (client requests
of benches/targets/_callgrind.py). The region's per-function costs are attributed to
categories (inclusive, a category's calls into another category are left to that one):

  fp_code    code fingerprints (accel/fingerprint.rs: canonical code-object hash; includes
             PyCode_GetCode's de-optimised bytecode copy)
  fp_c       C fingerprints (ml_name/ml_flags/ml_doc)
  self_test  behavioural self-tests at install (Prepare::self_test, targets' selftest code)
  pins       dependency pins (Prepare::pin*, dict/type watchers)
  intern     string interning (prepare::interned, PyUnicode_Intern*)
  symbols    run-time symbol lookups (Prepare::symbol, dlsym)
  sites      site declaration and swap (Prepare::ml_meth/slot/vectorcall, accel/site.rs)
  status     status building (registry::status)
  jit        sys._clear_internal_caches after ml_meth installs
  prepare    the targets' own prepare code (targets::*), other than the above
  registry   the registry (activate, bookkeeping, version gate), other than the above
  python     CPython work not under a Rustique function (the import itself, Python code)

    python benches/floors/install/install_profile.py [cell ...] [-o out.json] [-j 4]
"""
from __future__ import annotations

import argparse
import collections
import glob
import json
import os
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "targets"))
import _callgrind  # noqa: E402

A = "import rustique._rustique as r, gc; "
# One vectorcall target of another group first: the process's one-time costs (monitoring
# calibration, registry creation) are not charged to the measured group (as copy_install.py).
WARM = ("import sys, types; m = types.ModuleType('_rustique_selftest'); "
        "exec('SEP = 1\\ndef same_a(x):\\n    return x\\n', m.__dict__); "
        "sys.modules['_rustique_selftest'] = m; r._accel.enable(['_selftest.shared_a']); ")
POP = "[sys.modules.pop(k) for k in list(sys.modules) if k == {m!r} or k.startswith({m!r} + '.')]; "
GC = "gc.collect()"

def enable_cell(group, pre):
    return {"setup": A + WARM + pre + "; " + GC, "stmt": f"r._accel.enable([{group!r}])"}

def import_cells(mod, groups, pre=""):
    base = A + WARM + "import sys; " + POP.format(m=mod) + (pre + "; " if pre else "")
    return ({"setup": base + GC, "stmt": f"import {mod}"},
            {"setup": base + f"r._accel.enable({groups!r}); " + GC, "stmt": f"import {mod}"})

CELLS = {
    "enable json (imported)": enable_cell("json", "import json"),
    "enable copy (imported)": enable_cell("copy", "import copy"),
    "enable dataclasses (imported)": enable_cell("dataclasses", "import copy, dataclasses"),
    "enable logging (imported)": enable_cell("logging", "import logging"),
    "enable random (imported)": enable_cell("random", "import random"),
    "enable posixpath (imported)": enable_cell("posixpath", "import posixpath"),
    "enable fnmatch (imported)": enable_cell("fnmatch", "import fnmatch"),
    "enable pathlib (imported)": enable_cell("pathlib", "import pathlib"),
    "enable datetime (imported)": enable_cell("datetime", "import datetime, _strptime"),
    "enable textwrap (imported)": enable_cell("textwrap", "import textwrap"),
    "enable urllib (imported)": enable_cell("urllib", "import urllib.parse"),
    "enable statistics (imported)": enable_cell("statistics", "import statistics"),
    "enable float": enable_cell("float", "pass"),
    "enable containers": enable_cell("containers", "pass"),
    "enable builtins": enable_cell("builtins", "pass"),
    "public rustique.enable(only=['json']) (imported)": {
        "setup": A + WARM + "import rustique, json; " + GC, "stmt": "rustique.enable(only=['json'])"},
}
# Re-enable after disable (Phase 2/3's "warm" for logging: process-wide caches, interned
# keys and code-fingerprint cache already filled).
for g, pre in [("json", "import json"), ("logging", "import logging"), ("pathlib", "import pathlib"),
               ("copy", "import copy"), ("random", "import random")]:
    CELLS[f"re-enable {g} (after enable+disable)"] = {
        "setup": A + WARM + pre + f"; r._accel.enable([{g!r}]); r._accel.disable([{g!r}]); " + GC,
        "stmt": f"r._accel.enable([{g!r}])"}
CELLS["enable posixpath+fnmatch+pathlib (imported, one call)"] = enable_cell(
    "pathlib", "import pathlib, fnmatch")
CELLS["enable posixpath+fnmatch+pathlib (imported, one call)"]["stmt"] = "r._accel.enable(['posixpath', 'fnmatch', 'pathlib'])"
CELLS["cold enable json (first group of the process)"] = {"setup": A + "import json; " + GC,
                                                          "stmt": "r._accel.enable(['json'])"}
for name, (mod, groups, pre) in {
    "json": ("json", ["json"], ""),
    "copy": ("copy", ["copy"], ""),
    "logging": ("logging", ["logging"], ""),
    "random": ("random", ["random"], ""),
    "pathlib": ("pathlib", ["pathlib", "posixpath", "fnmatch"], "import posixpath, fnmatch"),
    "posixpath+fnmatch via glob": ("glob", ["posixpath", "fnmatch"], ""),
    "textwrap": ("textwrap", ["textwrap"], ""),
    "statistics": ("statistics", ["statistics"], ""),
}.items():
    plain, lazy = import_cells(mod, groups, pre)
    CELLS[f"import {name} plain"] = plain
    CELLS[f"import {name} lazy"] = lazy

CATS = [
    ("fp_c", r"accel::fingerprint::(method_def|c_object)"),
    ("fp_code", r"accel::fingerprint::|^PyCode_GetCode$|_PyCode_GetCode|deopt_code|"
                r"HashMap<usize, \(pyo3::instance::Py<pyo3::types::any::PyAny>, alloc::string::String\)|"
                r"RawTable<\(usize, \(pyo3::instance::Py<pyo3::types::any::PyAny>, alloc::string::String\)\)>"),
    ("self_test", r"self_test|selftest"),
    ("pins", r"Prepare.*::pin|accel::guard::|PyDict_Watch|PyType_Watch|PyDict_AddWatcher|PyType_AddWatcher"),
    ("intern", r"prepare::interned|PyUnicode_Intern|_PyUnicode_Intern|intern_common|"
               r"HashMap<alloc::string::String, pyo3::instance::Py<pyo3::types::string::PyString>|"
               r"RawTable<\(alloc::string::String, pyo3::instance::Py<pyo3::types::string::PyString>\)>"),
    ("symbols", r"Prepare.*::symbol|dlsym|_dl_lookup|do_lookup_x|_dl_sym"),
    ("sites", r"Prepare.*::(ml_meth|slot|vectorcall|push_site|original_vectorcall)|accel::site::|PyFunction_SetVectorcall"),
    ("status", r"accel::registry::(status|static_status|versions_of)"),
    ("jit", r"clear_internal_caches|_PyExecutor|_Py_Executors_Invalidate"),
    ("prepare", r"rustique_py::targets::|rustique_py::takeover::|rustique_py::builtins::|rustique_py::accel::prepare::|accel::version::"),
    ("registry", r"rustique_py::accel::|_rustique"),
]
CAT_RE = [(c, re.compile(r)) for c, r in CATS]

def cat_of(ob, fn):
    for c, rx in CAT_RE:
        if rx.search(fn):
            return c
    if "_rustique" in ob:
        return "registry"
    return None

def parse(path):
    """(dump name, self costs {key: Ir}, calls {(caller, callee): inclusive Ir}, ob of key)."""
    name = None
    self_ = collections.Counter(); calls = collections.Counter(); obof = {}
    obs, fns = {}, {}
    cur = ("", ""); cur_ob = ""; callee = None; pending = False
    def res(table, v):
        m = re.match(r"\((\d+)\)(?: (.*))?$", v)
        if not m:
            return v
        if m.group(2) is not None:
            table[m.group(1)] = m.group(2); return m.group(2)
        return table.get(m.group(1), v)
    cob = ""
    with open(path) as f:
        for line in f:
            if line.startswith("desc: Trigger: Client Request: "):
                name = line.split("Client Request: ", 1)[1].strip()
            elif line.startswith("ob="):
                cur_ob = res(obs, line[3:].strip())
            elif line.startswith("fn="):
                cur = res(fns, line[3:].strip()); obof[cur] = cur_ob; cob = cur_ob
            elif line.startswith("cob="):
                cob = res(obs, line[4:].strip())
            elif line.startswith("cfn="):
                callee = res(fns, line[4:].strip()); obof.setdefault(callee, cob)
            elif line.startswith("calls="):
                pending = True
            elif line[:1].isdigit() or line[:1] in "+-*":
                p = line.split()
                if len(p) < 2:
                    continue
                if pending:
                    calls[(cur, callee)] += int(p[1]); pending = False; cob = cur_ob
                else:
                    self_[cur] += int(p[1])
    return name, self_, calls, obof

def attribute(self_, calls, obof):
    cat = {fn: cat_of(obof.get(fn, ""), fn) for fn in set(self_) | {c for _, c in calls} | {c for c, _ in calls}}
    # inclusive of uncategorised functions reached from a category: their self + their calls
    incl = collections.Counter(self_)
    for (a, b), v in calls.items():
        incl[a] += v
    out = collections.Counter()
    for fn, v in self_.items():
        c = cat.get(fn)
        if c:
            out[c] += v
    for (a, b), v in calls.items():
        ca, cb = cat.get(a), cat.get(b)
        if ca and not cb:
            out[ca] += v  # an uncategorised callee's whole subtree counts for the caller's category
    total = sum(self_.values())
    out["python"] = total - sum(out.values())
    return total, out

def run(cell):
    spec = {"mode": "region", "stmt": cell["stmt"], "setup": cell["setup"], "enable": None, "n": 0}
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable,
               _callgrind.__file__, "_worker", json.dumps(spec)]
        r = subprocess.run(cmd, env={**os.environ, "PYTHONHASHSEED": "0"}, capture_output=True, text=True)
        if r.returncode:
            return {"error": r.stderr[-1500:]}
        for fn in glob.glob(out + "*"):
            name, self_, calls, obof = parse(fn)
            if name == "r":
                total, cats = attribute(self_, calls, obof)
                top = sorted(self_.items(), key=lambda kv: -kv[1])[:25]
                incl = collections.Counter({f: v for f, v in self_.items() if "_rustique" in obof.get(f, "")})
                for (x, y), v in calls.items():
                    if "_rustique" in obof.get(x, ""):
                        incl[x] += v
                return {"total": total, "cats": dict(cats.most_common()),
                        "top": [(f[:110], v, cat_of(obof.get(f, ""), f)) for f, v in top],
                        "rust_incl": [(f[:140], v) for f, v in incl.most_common(40)]}
    return {"error": "no region"}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cells", nargs="*")
    ap.add_argument("-o", "--out")
    ap.add_argument("-j", "--jobs", type=int, default=4)
    ap.add_argument("--top", type=int, default=0)
    ap.add_argument("--incl", type=int, default=0)
    a = ap.parse_args()
    names = [n for n in CELLS if not a.cells or any(s in n for s in a.cells)]
    res = {}
    with ThreadPoolExecutor(a.jobs) as ex:
        for n, r in zip(names, ex.map(lambda n: run(CELLS[n]), names)):
            res[n] = r
            if "error" in r:
                print(f"{n}: ERROR {r['error'][-400:]}", flush=True); continue
            cats = "  ".join(f"{k} {v/1e3:,.1f}k" for k, v in r["cats"].items() if abs(v) >= 500)
            print(f"{n:52s} {r['total']:>11,d}  | {cats}", flush=True)
            for f, v, c in r["top"][:a.top]:
                print(f"      {v:>9,d} {c or '-':9s} {f}")
            for f, v in r["rust_incl"][:a.incl]:
                print(f"      incl {v:>9,d} {f}")
    if a.out:
        json.dump(res, open(a.out, "w"), indent=1)

if __name__ == "__main__":
    main()
