"""Where do the instructions of one accelerated call go? (w2-floors, docs/atlas/w2-floors.md §1)

Runs a cell like benches/targets/_callgrind.py (fresh process, warmed, statement unrolled
10x, region-instrumented, Ir(2n) - Ir(n) over n), but keeps callgrind's per-function costs
and splits the per-call instructions into components:

  kernel   functions of the Rustique extension (the Rust kernel and its helpers)
  alloc    object allocation/deallocation (pymalloc/mimalloc, *_New, *_dealloc, freelists)
  eval     the interpreter and call machinery (_PyEval_EvalFrameDefault, vectorcall
           dispatch, method descriptors, frames)
  libc     memcpy/memmove/memset/strlen...
  cpython  everything else in the interpreter (hashing, unicode helpers, dtoa, dict ops...)

    python benches/floors/cg_breakdown.py "stmt" --setup CODE [--enable g1,g2] [--top 12]
    python benches/floors/cg_breakdown.py --cells cells.json -o out.json    # many cells

`cells.json`: [{"name":..., "stmt":..., "setup":..., "enable": [..] | null}, ...].
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
sys.path.insert(0, os.path.join(HERE, "..", "targets"))
import _callgrind  # noqa: E402

ALLOC = re.compile(
    r"(PyObject_Malloc|PyObject_Free|PyObject_Realloc|PyObject_Calloc|PyMem_|_PyObject_Malloc|pymalloc|_PyMem_|"
    r"^_?mi_|^mi_|_mi_|^malloc$|^free$|^realloc$|^calloc$|_Py_Dealloc|dealloc|_PyObject_GC_Link|_PyObject_GC_New|"
    r"PyType_GenericAlloc|gc_alloc|_PyObject_New|PyUnicode_New|_PyLong_New|PyFloat_FromDouble|PyTuple_New|"
    r"PyList_New|tuple_alloc|_PyTuple_FromArray|freelist|_Py_NewReference|PyObject_GC_Del|_PyType_AllocNoTrack|"
    r"_PyObject_GC_TRACK|_PyObject_GC_UNTRACK|PyObject_GC_Track|PyObject_GC_UnTrack|_Py_FreeListAlloc|"
    r"_PyFreeList|maybe_freelist|_PyDict_NewPresized|new_keys_object|new_dict|free_keys_object|PyDict_New|"
    r"_PyLong_FromMedium|_PyLong_FromLarge|PyLong_From|_PyUnicode_FromASCII|unicode_dealloc|_PyObject_Free|"
    r"_PyObject_GC_Alloc|_PyObject_GC_NewVar|PyObject_Init|_Py_brc|_PyObject_MergeZeroLocalRefcount)")
EVAL = re.compile(
    r"(_TAIL_CALL_|_PyEval_|PyEval_|_PyFunction_Vectorcall|PyObject_Vectorcall|_PyObject_Vectorcall|_PyObject_Call|PyObject_Call|"
    r"cfunction_|method_vectorcall|methoddescr|vectorcall_|_PyObject_MakeTpCall|slot_tp_|slot_sq_|slot_nb_|"
    r"_PyFrame_|_PyThreadState_PushFrame|_PyThreadState_PopFrame|clear_thread_frame|_Py_Check|wrapper|"
    r"_PyStack_|_PyArg_|PySequence_Contains|_PySequence_IterSearch|PyObject_RichCompare|do_richcompare|"
    r"_Py_EnterRecursive|_Py_LeaveRecursive|_PyObject_GetMethod|PyObject_GetAttr|_PyObject_GenericGetAttr|"
    r"_PyType_Lookup|method_get|func_get|property_descr|_PyEval_EvalFrame|instrument|_Py_call_instrumentation)")


def _parse(fn: str) -> tuple[str | None, dict[tuple[str, str], int]]:
    """(dump name, {(object, function): exclusive Ir})."""
    name = None
    costs: dict[tuple[str, str], int] = collections.Counter()
    obs: dict[str, str] = {}
    fns: dict[str, str] = {}
    cur_ob = cur_fn = ""
    skip_next = False

    def resolve(table: dict[str, str], v: str) -> str:
        # compressed form "(id) name" or "(id)"
        m = re.match(r"\((\d+)\)(?: (.*))?$", v)
        if not m:
            return v
        if m.group(2) is not None:
            table[m.group(1)] = m.group(2)
            return m.group(2)
        return table.get(m.group(1), v)

    with open(fn) as f:
        for line in f:
            if line.startswith("desc: Trigger: Client Request: "):
                name = line.split("Client Request: ", 1)[1].strip()
            elif line.startswith("ob="):
                cur_ob = resolve(obs, line[3:].strip())
            elif line.startswith("fn="):
                cur_fn = resolve(fns, line[3:].strip())
            elif line.startswith(("cob=", "cfn=", "cfi=", "cfl=")):
                if line.startswith("cob="):
                    resolve(obs, line[4:].strip())
                elif line.startswith("cfn="):
                    resolve(fns, line[4:].strip())
            elif line.startswith("calls="):
                skip_next = True
            elif line[:1].isdigit() or line[:1] in "+-*":
                if skip_next:
                    skip_next = False
                    continue
                parts = line.split()
                if len(parts) >= 2:
                    costs[(cur_ob, cur_fn)] += int(parts[1])
    return name, costs


def category(ob: str, fn: str) -> str:
    if "_rustique" in ob:
        return "kernel"
    if "libc" in ob or "ld-linux" in ob:
        return "libc"
    if ALLOC.search(fn):
        return "alloc"
    if EVAL.search(fn):
        return "eval"
    return "cpython"


def run_cell(stmt: str, setup: str = "", enable: list[str] | None = None, n: int = 200,
             python: str | None = None) -> dict:
    spec = {"mode": "loop", "stmt": stmt, "setup": setup, "enable": enable, "n": n}
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        e = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "0"}
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", python or sys.executable,
               _callgrind.__file__, "_worker", json.dumps(spec)]
        r = subprocess.run(cmd, env=e, capture_output=True, text=True)
        if r.returncode:
            raise RuntimeError(f"callgrind cell failed: {r.stderr[-2000:]}")
        dumps = {}
        for fn in glob.glob(out + "*"):
            name, costs = _parse(fn)
            if name:
                dumps[name] = costs
    a, b = dumps["a"], dumps["b"]
    per = collections.Counter()
    for k in set(a) | set(b):
        v = (b.get(k, 0) - a.get(k, 0)) / (n * _callgrind.UNROLL)
        if abs(v) >= 0.05:
            per[k] = v
    comp = collections.Counter()
    for (ob, fn), v in per.items():
        comp[category(ob, fn)] += v
    top = sorted(per.items(), key=lambda kv: -kv[1])
    return {
        "total": sum(per.values()),
        "components": {k: round(v, 1) for k, v in sorted(comp.items(), key=lambda kv: -kv[1])},
        "top": [(fn, os.path.basename(ob), round(v, 1), category(ob, fn)) for (ob, fn), v in top[:40]],
    }


def fmt(name: str, r: dict, top: int) -> str:
    comps = "  ".join(f"{k} {v:,.0f}" for k, v in r["components"].items())
    lines = [f"{name}: {r['total']:,.0f} Ir/call   [{comps}]"]
    for fn, ob, v, c in r["top"][:top]:
        lines.append(f"    {v:9,.1f}  {c:7s} {fn[:90]}  ({ob})")
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("stmt", nargs="?")
    ap.add_argument("--setup", default="")
    ap.add_argument("--enable")
    ap.add_argument("--plain", action="store_true", help="also measure without enable")
    ap.add_argument("--cells", help="JSON list of cells")
    ap.add_argument("-o", "--out")
    ap.add_argument("-j", "--jobs", type=int, default=4)
    ap.add_argument("--top", type=int, default=12)
    a = ap.parse_args()
    if a.cells:
        cells = json.load(open(a.cells))
    else:
        en = a.enable.split(",") if a.enable else None
        cells = [{"name": a.stmt + (" [enabled]" if en else ""), "stmt": a.stmt, "setup": a.setup, "enable": en}]
        if a.plain and en:
            cells.append({"name": a.stmt + " [plain]", "stmt": a.stmt, "setup": a.setup, "enable": None})

    def one(c):
        try:
            return c["name"], run_cell(c["stmt"], c.get("setup", ""), c.get("enable"))
        except Exception as e:  # noqa: BLE001
            return c["name"], {"error": str(e)}

    results = {}
    with ThreadPoolExecutor(a.jobs) as ex:
        for name, r in ex.map(one, cells):
            results[name] = r
            print(fmt(name, r, a.top) if "error" not in r else f"{name}: ERROR {r['error'][-300:]}", flush=True)
    if a.out:
        with open(a.out, "w") as f:
            json.dump(results, f, indent=1)


if __name__ == "__main__":
    main()
