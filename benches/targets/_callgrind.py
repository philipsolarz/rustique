"""Region-instrumented callgrind measurements, shared by the target benchmarks
(docs/ACCELERATION.md §7.4; method of docs/atlas/mechanisms.md: instructions are counted
only inside the measured region, each cell in its own fresh process).

    from _callgrind import per_call, region, table

    # Ir per execution of `stmt` (Ir(2n) - Ir(n) over n, statement unrolled 10x, warmed):
    plain = per_call("json.dumps(d)", setup="import json; d = {'a': 1}")
    fast = per_call("json.dumps(d)", setup="import json; d = {'a': 1}", enable=["json"])
    # Ir of one execution of a one-shot region (fresh process, e.g. startup work):
    ir = region("import json", setup="import rustique")

`enable=[...]` calls `rustique._takeover.enable(only=...)` at the end of the setup (so a
module imported by the setup is patched at once). `jit=True` runs with PYTHON_JIT=1 and
warms 30 000 iterations so traces exist. `env` adds environment variables.

CLI:
    python benches/targets/_callgrind.py "stmt" [--setup CODE] [--enable json,copy] [--jit]
    python benches/targets/_callgrind.py --region "import json" [--setup CODE]

Needs valgrind (with headers) and a C compiler; the client-request helper is compiled once
per interpreter into ~/.cache/rustique-callgrind.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import subprocess
import sys
import sysconfig
import tempfile
from typing import Any

UNROLL = 10

HELPER_C = r"""
#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <valgrind/callgrind.h>
static PyObject *h_start(PyObject *m, PyObject *a) { CALLGRIND_ZERO_STATS; CALLGRIND_START_INSTRUMENTATION; Py_RETURN_NONE; }
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
    {Py_mod_multiple_interpreters, Py_MOD_PER_INTERPRETER_GIL_SUPPORTED},
    {0, NULL}};
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_rq_callgrind_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__rq_callgrind_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper() -> Any:
    """The client-request module (start/stop/dump), compiled on first use."""
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-callgrind")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_rq_callgrind_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "rq_callgrind.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", target, src])
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _rq_callgrind_v1  # type: ignore[import-not-found]

    return _rq_callgrind_v1


def _setup_ns(setup: str, enable: list[str] | None) -> dict[str, Any]:
    ns: dict[str, Any] = {"__name__": "__bench__"}
    exec(setup, ns)
    if enable is not None:
        from rustique import _rustique, _takeover

        _rustique._accel._set_validation(os.environ.get("RUSTIQUE_VALIDATE", "eager"))
        _takeover.enable(only=enable)
    return ns


def _worker(spec: dict[str, Any]) -> None:
    h = helper()
    ns = _setup_ns(spec["setup"], spec["enable"])
    if spec["mode"] == "region":
        code = compile(spec["stmt"], "<region>", "exec")
        h.start()
        exec(code, ns)
        h.dump("r")
        h.stop()
        return
    names = [k for k in ns if not k.startswith("__") and k.isidentifier()]
    binds = "".join(f"    {k} = _g[{k!r}]\n" for k in names)
    ns["_g"] = ns
    body = "".join(f"        {spec['stmt']}\n" for _ in range(UNROLL))
    exec(f"def _fn(_n):\n{binds}    for _ in range(_n):\n{body}", ns)
    fn = ns["_fn"]
    jit = getattr(sys, "_jit", None)
    fn(30000 if (jit and jit.is_enabled()) else 50)
    n = spec["n"]
    fn(n)
    h.start()
    fn(n)
    h.dump("a")
    h.stop()
    h.start()
    fn(2 * n)
    h.dump("b")
    h.stop()


def _run(spec: dict[str, Any], env: dict[str, str] | None, python: str | None) -> dict[str, int]:
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        # Targets validate at enable (not at their first call, DECISIONS §10.5 item 4), so
        # a cell's warm-up never includes a validation and states read after enable are final.
        e = {**os.environ, "PYTHONHASHSEED": "0", **(env or {})}
        e.setdefault("RUSTIQUE_VALIDATE", "eager")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", python or sys.executable,
               os.path.abspath(__file__), "_worker", json.dumps(spec)]
        r = subprocess.run(cmd, env=e, capture_output=True, text=True)
        if r.returncode:
            raise RuntimeError(f"callgrind cell failed: {r.stderr[-2000:]}")
        totals: dict[str, int] = {}
        for fn in glob.glob(out + "*"):
            name = total = None
            with open(fn) as f:
                for line in f:
                    if line.startswith("desc: Trigger: Client Request: "):
                        name = line.split("Client Request: ", 1)[1].strip()
                    elif line.startswith(("totals:", "summary:")):
                        total = int(line.split()[1])
                        break
            if name and total is not None:
                totals[name] = total
        return totals


def per_call(stmt: str, setup: str = "", *, enable: list[str] | None = None, jit: bool = False, n: int = 200,
             env: dict[str, str] | None = None, python: str | None = None) -> float:
    """Instructions per execution of `stmt` (fresh process, warmed, region-instrumented)."""
    spec = {"mode": "loop", "stmt": stmt, "setup": setup, "enable": enable, "n": n}
    t = _run(spec, {**(env or {}), "PYTHON_JIT": "1" if jit else "0"}, python)
    return (t["b"] - t["a"]) / (n * UNROLL)


def region(stmt: str, setup: str = "", *, enable: list[str] | None = None, env: dict[str, str] | None = None,
           python: str | None = None) -> int:
    """Instructions of one execution of `stmt` after `setup` (fresh process)."""
    spec = {"mode": "region", "stmt": stmt, "setup": setup, "enable": enable, "n": 0}
    return _run(spec, env, python)["r"]


def table(rows: dict[str, dict[str, float]], base: str = "plain") -> str:
    """Format `{cell: {variant: Ir}}` with ratios against `base`."""
    out = []
    for cell, variants in rows.items():
        b = variants.get(base)
        parts = []
        for v, ir in variants.items():
            ratio = f" ({ir / b:.3f})" if b and v != base else ""
            parts.append(f"{v}={ir:,.0f}{ratio}")
        out.append(f"{cell:48s} " + "  ".join(parts))
    return "\n".join(out)


def main() -> None:
    if len(sys.argv) > 2 and sys.argv[1] == "_worker":
        _worker(json.loads(sys.argv[2]))
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("stmt", nargs="?")
    ap.add_argument("--region")
    ap.add_argument("--setup", default="")
    ap.add_argument("--enable")
    ap.add_argument("--jit", action="store_true")
    a = ap.parse_args()
    enable = a.enable.split(",") if a.enable else None
    if a.region:
        print(region(a.region, a.setup, enable=enable))
    else:
        plain = per_call(a.stmt, a.setup, jit=a.jit)
        res = {"plain": plain}
        if enable:
            res["enabled"] = per_call(a.stmt, a.setup, enable=enable, jit=a.jit)
        print(table({a.stmt: res}))


if __name__ == "__main__":
    main()
