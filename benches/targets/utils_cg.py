"""Callgrind cell runner shared by the utils groups (random, textwrap, urllib, statistics,
ipaddress): per-call instruction counts (Ir), region-instrumented.

Each cell runs in its own process under callgrind: setup (imports, install), warm-up, then
Ir(run 2n) - Ir(run n) over n calls, so fixed costs cancel (statements unrolled 10x).
Method as in docs/atlas/mechanisms.md §1 (benches/mechanisms/overhead.py on p1/mechanisms).

A cell module defines ``COMMON`` (setup source run first), ``CELLS`` (name -> statement) and
optionally ``INSTALL`` (source that turns the kernels on). Every cell is measured twice:
``orig`` (plain CPython) and ``rq`` (kernels installed); the ratio rq/orig is reported.

    python benches/targets/utils_cg.py <cells-module> [--jit] [--only SUBSTR] [-o out.json]
"""

from __future__ import annotations

import glob
import importlib
import json
import os
import subprocess
import sys
import sysconfig
import tempfile

UNROLL = 10
HERE = os.path.dirname(os.path.abspath(__file__))

HELPER_C = r"""
#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <valgrind/callgrind.h>
static PyObject *h_start(PyObject *m, PyObject *a) { CALLGRIND_START_INSTRUMENTATION; CALLGRIND_ZERO_STATS; Py_RETURN_NONE; }
static PyObject *h_stop(PyObject *m, PyObject *a) { CALLGRIND_STOP_INSTRUMENTATION; Py_RETURN_NONE; }
static PyObject *h_dump(PyObject *m, PyObject *name) {
    const char *s = PyUnicode_AsUTF8(name);
    if (!s) return NULL;
    CALLGRIND_DUMP_STATS_AT(s);
    Py_RETURN_NONE;
}
static PyMethodDef methods[] = {
    {"start", h_start, METH_NOARGS, NULL}, {"stop", h_stop, METH_NOARGS, NULL},
    {"dump", h_dump, METH_O, NULL}, {NULL, NULL, 0, NULL},
};
static PyModuleDef_Slot slots[] = {
#ifdef Py_GIL_DISABLED
    {Py_mod_gil, Py_MOD_GIL_NOT_USED},
#endif
    {0, NULL}};
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_utils_cg_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__utils_cg_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper():
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-utils-cg")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_utils_cg_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "utils_cg.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", target, src])
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _utils_cg_v1  # type: ignore[import-not-found]

    return _utils_cg_v1


def load_cells(name):
    if HERE not in sys.path:
        sys.path.insert(0, HERE)
    return importlib.import_module(name)


def cell_list(mod, only):
    out = []
    for name, stmt in mod.CELLS.items():
        if only and only not in name:
            continue
        for variant in ("orig", "rq"):
            out.append((name, variant, stmt))
    return out


def worker(modname, idx, only):
    h = helper()
    mod = load_cells(modname)
    name, variant, stmt = cell_list(mod, only)[idx]
    ns: dict = {}
    exec(mod.COMMON, ns)
    if variant == "rq":
        exec(mod.INSTALL, ns)
    per_cell = getattr(mod, "SETUP", {}).get(name, "")
    exec(per_cell, ns)
    names = [k for k in ns if not k.startswith("__") and k.isidentifier()]
    binds = "".join(f"    {k} = _g[{k!r}]\n" for k in names)
    ns["_g"] = ns
    body = "".join(f"        {stmt}\n" for _ in range(UNROLL))
    exec(f"def fn(_n):\n{binds}    for _ in range(_n):\n{body}", ns)
    fn = ns["fn"]
    jit = getattr(sys, "_jit", None)
    jit_on = bool(jit and jit.is_enabled())
    n = getattr(mod, "N", {}).get(name, 100)
    fn(3000 if jit_on else 20)
    fn(n)
    h.start()
    fn(n)
    h.dump("a")
    h.stop()
    h.start()
    fn(2 * n)
    h.dump("b")
    h.stop()
    print(n)


def run_cell(modname, idx, only, env, annotate=False):
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable,
               os.path.abspath(__file__), "_worker", modname, str(idx), only or ""]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True)
        if r.returncode:
            return None, r.stderr[-2000:]
        n = int(r.stdout.strip().splitlines()[-1])
        tot = {}
        for fn in glob.glob(out + "*"):
            name = t = None
            with open(fn) as f:
                for line in f:
                    if line.startswith("desc: Trigger: Client Request: "):
                        name = line.split("Client Request: ", 1)[1].strip()
                    elif line.startswith(("totals:", "summary:")):
                        t = int(line.split()[1])
                        break
            if name:
                tot[name] = t
            if annotate and name == "b":
                subprocess.run(["callgrind_annotate", "--inclusive=no", "--threshold=99", fn])
        return (tot["b"] - tot["a"]) / (n * UNROLL), None


def main():
    if sys.argv[1] == "_worker":
        worker(sys.argv[2], int(sys.argv[3]), sys.argv[4] or None)
        return
    modname = sys.argv[1]
    jit = "--jit" in sys.argv
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
    out = sys.argv[sys.argv.index("-o") + 1] if "-o" in sys.argv else None
    env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "1" if jit else "0"}
    annotate = "--annotate" in sys.argv
    mod = load_cells(modname)
    rows: dict = {}
    for i, (name, variant, _stmt) in enumerate(cell_list(mod, only)):
        ir, err = run_cell(modname, i, only, env, annotate)
        rows.setdefault(name, {})[variant] = ir if err is None else f"ERR {err.strip().splitlines()[-1][:160]}"
        if variant == "rq":
            o, q = rows[name].get("orig"), rows[name]["rq"]
            ratio = f"{q / o:.3f}" if isinstance(o, float) and isinstance(q, float) else "-"
            fmt = lambda v: f"{v:.0f}" if isinstance(v, float) else str(v)  # noqa: E731
            print(f"{name:58s} {fmt(o):>10} {fmt(q):>10} {ratio:>7}", flush=True)
    if out:
        with open(out, "w") as f:
            json.dump({"python": sys.version, "jit": jit, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
