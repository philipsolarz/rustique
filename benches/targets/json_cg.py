"""Instruction counts (callgrind, region-instrumented) of the `float` and `json` targets.

Every (cell, variant) runs in its own process under callgrind: setup, warm-up, then
Ir(2n calls) - Ir(n calls) over n, statements unrolled 10x, so fixed costs cancel (noise
about +-15 Ir per call). Variants: `plain` (nothing installed) and `on` (the target
groups installed; `float` and `json` together unless the cell says otherwise). The ratio
on/plain is the never-slower gate (<= 1.05 on every cell).

    python benches/targets/json_cg.py [--only SUBSTR] [-j 4] [--jit] [-o out.json]
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
from concurrent.futures import ThreadPoolExecutor

UNROLL = 10
N = 200

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
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_rq_cg_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__rq_cg_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper():
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-cg", sysconfig.get_config_var("SOABI"))
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_rq_cg_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "rq_cg.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", target, src])
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _rq_cg_v1  # type: ignore[import-not-found]

    return _rq_cg_v1


COMMON = r"""
import json, math, io, decimal, datetime, collections, enum
from json import dumps, loads
x = 1.23
xs = 123.456789
big = 1.2345678901234567e+300
tiny = 2.5e-07
inf = math.inf
class F(float):
    pass
fsub = F(1.5)
record = {"id": 12345, "name": "Ada Lovelace", "email": "ada@example.com", "active": True,
          "score": 98.5, "tags": ["admin", "math"], "age": 36, "ratio": 0.3333333333333333,
          "manager": None, "city": "London"}
small_list = [1, 2, 3, "a", "b", None, True, 4.5]
floats = {"lat": 51.507351, "lon": -0.127758, "alt": 35.2, "speed": 12.75, "heading": 270.0,
          "acc": 4.8, "t": 1730419200.125, "temp": 18.4}
unicode_rec = {"name": "Zoë Grün", "city": "München", "emoji": "\U0001F600", "jp": "東京"}
nested = {"user": record, "items": [{"sku": f"A{i}", "qty": i, "price": i * 1.5} for i in range(5)], "page": 1}
records_1000 = [dict(record, id=i, score=i / 7) for i in range(1000)]
rec_text = json.dumps(record)
rec_bytes = rec_text.encode()
small_list_text = json.dumps(small_list)
floats_text = json.dumps(floats)
nested_text = json.dumps(nested)
unicode_text = json.dumps(unicode_rec, ensure_ascii=False)
records_text = json.dumps(records_1000)
class Color(enum.IntEnum):
    RED = 1
od = collections.OrderedDict(record)
with_decimal = dict(record, price=decimal.Decimal("9.99"))
with_date = dict(record, when=datetime.date(2026, 11, 1))
def default(o):
    if isinstance(o, decimal.Decimal):
        return str(o)
    if isinstance(o, datetime.date):
        return o.isoformat()
    raise TypeError(o)
circular = [1, 2]
circular.append(circular)
bad_json = '{"id": 1, "name": "x",}'
"""

# name -> (stmt, groups for the `on` variant)
CELLS: dict[str, tuple[str, str]] = {
    # --- float.repr ----------------------------------------------------------------------
    "float repr(1.23)": ("repr(x)", "float"),
    "float repr(123.456789)": ("repr(xs)", "float"),
    "float repr(1.2345678901234567e300)": ("repr(big)", "float"),
    "float str(2.5e-07)": ("str(tiny)", "float"),
    "float f'{x}'": ("f'{x}'", "float"),
    "float f'{x:.2f}' [not reached]": ("f'{x:.2f}'", "float"),
    "float f'v={x} t={xs}ms'": ("f'v={x} t={xs}ms'", "float"),
    "float repr(inf) [fallback]": ("repr(inf)", "float"),
    "float repr(F(1.5)) [subclass]": ("repr(fsub)", "float"),
    "float '%s' % x": ("'%s' % (x,)", "float"),
    # --- json.dumps ----------------------------------------------------------------------
    "dumps record": ("dumps(record)", "float,json"),
    "dumps small list": ("dumps(small_list)", "float,json"),
    "dumps floats": ("dumps(floats)", "float,json"),
    "dumps nested": ("dumps(nested)", "float,json"),
    "dumps unicode": ("dumps(unicode_rec)", "float,json"),
    "dumps unicode ensure_ascii=False": ("dumps(unicode_rec, ensure_ascii=False)", "float,json"),
    "dumps str": ("dumps('hello')", "float,json"),
    "dumps int": ("dumps(7)", "float,json"),
    "dumps record indent=2": ("dumps(record, indent=2)", "float,json"),
    "dumps record sort_keys": ("dumps(record, sort_keys=True)", "float,json"),
    "dumps record separators": ("dumps(record, separators=(',', ':'))", "float,json"),
    "dumps record [json only]": ("dumps(record)", "json"),
    "dumps record default= [fallback]": ("dumps(record, default=str)", "float,json"),
    "dumps with Decimal default= [fallback]": ("dumps(with_decimal, default=default)", "float,json"),
    "dumps OrderedDict [fallback]": ("dumps(od)", "float,json"),
    "dumps IntEnum": ("dumps([Color.RED])", "float,json"),
    "dumps cls=JSONEncoder [fallback]": ("dumps(record, cls=json.JSONEncoder)", "float,json"),
    "JSONEncoder().encode(record)": ("enc.encode(record)", "float,json"),
    # --- json.loads ----------------------------------------------------------------------
    "loads record": ("loads(rec_text)", "float,json"),
    "loads record bytes": ("loads(rec_bytes)", "float,json"),
    "loads small list": ("loads(small_list_text)", "float,json"),
    "loads floats": ("loads(floats_text)", "float,json"),
    "loads nested": ("loads(nested_text)", "float,json"),
    "loads unicode": ("loads(unicode_text)", "float,json"),
    "loads object_hook [fallback]": ("loads(rec_text, object_hook=dict)", "float,json"),
}

ERROR_CELLS: dict[str, tuple[str, str]] = {
    # Statements that raise: measured inside try/except.
    "dumps Decimal no default [error fallback]": ("dumps(with_decimal)", "float,json"),
    "dumps circular [error fallback]": ("dumps(circular)", "float,json"),
    "dumps circular, kernel declines at once [floor]": ("dumps(circular, skipkeys=[])", "float,json"),
    "dumps Decimal, kernel declines at once [floor]": ("dumps(with_decimal, skipkeys=[])", "float,json"),
    "loads invalid [error fallback]": ("loads(bad_json)", "float,json"),
}


def all_cells():
    return {**CELLS, **ERROR_CELLS}


def worker(name: str, variant: str) -> None:
    h = helper()
    stmt, groups = all_cells()[name]
    ns: dict = {}
    exec(COMMON, ns)
    exec("enc = json.JSONEncoder()", ns)
    if variant == "on":
        import rustique
        from rustique import _takeover as _tk

        rustique.enable(only=groups.split(","))
        for d in _tk.status()["targets"]:
            if d["group"] in groups.split(",") and d["state"] != "installed":
                raise SystemExit(f"{d['name']} not installed: {d['state']} {d['reason']}")
    names = [k for k in ns if not k.startswith("__") and k.isidentifier()]
    binds = "".join(f"    {k} = _g[{k!r}]\n" for k in names)
    ns["_g"] = ns
    if name in ERROR_CELLS:
        line = f"        try:\n            {stmt}\n        except Exception:\n            pass\n"
    else:
        line = f"        {stmt}\n"
    body = line * UNROLL
    exec(f"def fn(_n):\n{binds}    for _ in range(_n):\n{body}", ns)
    fn = ns["fn"]
    jit = getattr(sys, "_jit", None)
    fn(30000 if (jit and jit.is_enabled()) else 50)
    fn(N)
    h.start()
    fn(N)
    h.dump("a")
    h.stop()
    h.start()
    fn(2 * N)
    h.dump("b")
    h.stop()


def run_cell(name: str, variant: str, env: dict) -> tuple[float | None, str | None]:
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable,
               os.path.abspath(__file__), "_worker", name, variant]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True)
        if r.returncode:
            return None, r.stderr[-2000:]
        tot = {}
        for fn in glob.glob(out + "*"):
            tag = t = None
            with open(fn) as f:
                for line in f:
                    if line.startswith("desc: Trigger: Client Request: "):
                        tag = line.split("Client Request: ", 1)[1].strip()
                    elif line.startswith(("totals:", "summary:")):
                        t = int(line.split()[1])
                        break
            if tag:
                tot[tag] = t
        return (tot["b"] - tot["a"]) / (N * UNROLL), None


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "_worker":
        worker(sys.argv[2], sys.argv[3])
        return
    p = argparse.ArgumentParser()
    p.add_argument("--only", default="")
    p.add_argument("-j", type=int, default=4)
    p.add_argument("--jit", action="store_true")
    p.add_argument("-o")
    a = p.parse_args()
    env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "1" if a.jit else "0"}
    names = [n for n in all_cells() if not a.only or any(s in n for s in a.only.split("|"))]
    jobs = [(n, v) for n in names for v in ("plain", "on")]
    with ThreadPoolExecutor(a.j) as ex:
        res = dict(zip(jobs, ex.map(lambda j: run_cell(j[0], j[1], env), jobs)))
    rows = {}
    worst = 0.0
    for n in names:
        (p_ir, p_err), (o_ir, o_err) = res[(n, "plain")], res[(n, "on")]
        if p_err or o_err:
            print(f"{n:48s} ERROR {(p_err or o_err).strip().splitlines()[-1][:200]}")
            rows[n] = {"error": (p_err or o_err)[-500:]}
            continue
        ratio = o_ir / p_ir
        worst = max(worst, ratio)
        rows[n] = {"plain": round(p_ir, 1), "on": round(o_ir, 1), "ratio": round(ratio, 3)}
        print(f"{n:48s} {p_ir:10.0f} {o_ir:10.0f} {ratio:7.3f}", flush=True)
    build = f"{sys.version.split()[0]}{'t' if sysconfig.get_config_var('Py_GIL_DISABLED') else ''}"
    print(f"# {build} jit={a.jit}: worst ratio {worst:.3f}")
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"build": build, "jit": a.jit, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
