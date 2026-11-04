"""Attribute the free-threading penalty: compare steady-state perf profiles of one workload
on the GIL build and the free-threaded build, by symbol and by penalty category.

    python benches/ft/ftdiff.py <gil.data> <gil.times.json> <ft.data> <ft.times.json> [--top 30] [--json out.json]
    python benches/ft/ftdiff.py table <a.ftdiff.json>...      # one row per workload, delta per category

Inputs come from `benches/ft/perfprof.sh record` (same workload, `-n` runs, both builds).
Each sample is converted to milliseconds per `run()` (its share of the profile times the
median run time; samples include kernel time). Symbols are folded into categories that
match the sources of free-threading overhead: reference counting (biased/atomic, merges),
locks (critical sections, PyMutex, parking lot), QSBR, allocation (mimalloc vs pymalloc),
GC, the bytecode interpreter by opcode family (generic vs specialized), dict, type
lookup, kernel (syscalls, scheduler), and the rest. Validate on a known case first: the
`set.__reduce__` self-contention shows up as `locks` + `kernel` (sched_yield).
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import subprocess
from collections import defaultdict

CATS = [
    ("locks", r"_PyCriticalSection|PyMutex|_PyMutex|_PyParkingLot|_PySemaphore|_PyRecursiveMutex|_PyRWMutex|_PySeqLock|_PyOnceFlag|PyThread_acquire|PyThread_release|pthread_mutex|__lll_lock|futex|sched_yield|_Py_yield"),
    ("refcount", r"_Py_DecRef|Py_DecRef|_Py_IncRef|Py_IncRef|_Py_MergeZeroLocalRefcount|_Py_ExplicitMergeRefcount|_Py_brc_|brc_|_Py_TryIncref|_Py_IsOwnedByCurrentThread|merge_refcount|_PyObject_SetDeferredRefcount|Py_XDecRef|_Py_DecRefShared|_Py_DecRefSharedDebug"),
    ("qsbr", r"qsbr|_PyMem_FreeDelayed|free_delayed|_PyMem_ProcessDelayed"),
    ("gc", r"^gc_|_PyGC|visit_|update_refs|deduce_unreachable|move_unreachable|subtract_refs|_PyObject_GC_|PyObject_GC_|gc_collect|mark_heap|scan_heap|gc_mark|mi_heap_visit|_traverse$|traverse|validate_refcounts|clear_weakrefs|handle_resurrected|finalize_garbage|delete_garbage|merge_queued_objects|disable_deferred_refcounting|_PyObject_VisitManagedDict|visit_decref|visit_reachable|visit_clear"),
    ("alloc", r"_PyObject_MiMalloc|_PyObject_MiFree|_PyObject_MiRealloc|_PyMem_Mi|^mi_|^_mi_|_PyObject_Malloc|_PyObject_Free|_PyObject_Realloc|pymalloc|^PyObject_Malloc|^PyObject_Free|^PyObject_Realloc|^_PyMem_|^PyMem_|_Py_Dealloc|_PyObject_Calloc|^malloc$|^free$|^cfree|_int_malloc|_int_free|^realloc|tupledealloc|list_dealloc|dict_dealloc|subtype_dealloc|object_dealloc|_PyFreeList|freelist|_PyLong_New|PyFloat_FromDouble|_PyTuple_FromArray|PyTuple_New|tuple_alloc|PyType_GenericAlloc|_PyType_AllocNoTrack|_PyObject_New"),
    ("containers", r"^PyList_Append|_PyList_Append|^list_|_list_extend|^_PyList_|^PyList_|^set_|^_PySet_|^PySet_|frozenset_|^tuple_|^PyTuple_|_PyTuple_"),
    ("dict", r"dict|_PyDict|insertdict|unicodekeys_lookup|lookdict|insert_to_emptydict|PyDict_|dictkeys|new_keys_object|free_keys_object"),
    ("typelookup", r"_PyType_Lookup|find_name_in_mro|_PyType_LookupRef|type_cache|_PyObject_LookupSpecial|_PyType_LookupStackRef"),
    ("attr", r"_PyObject_StoreInstanceAttribute|PyMember_SetOne|PyMember_GetOne|_PyObject_TryGetInstanceAttribute|PyObject_GenericGetAttr|_PyObject_GenericGetAttrWithDict|PyObject_GetAttr|_PyObject_GetMethod|PyObject_SetAttr|_PyObject_GenericSetAttrWithDict|slot_tp_getattr|_PyObject_LookupAttr|PyObject_GetOptionalAttr"),
    ("interp_generic", r"^_TAIL_CALL_(LOAD_ATTR|STORE_ATTR|LOAD_GLOBAL|CALL|CALL_KW|BINARY_OP|COMPARE_OP|CONTAINS_OP|FOR_ITER|LOAD_SUPER_ATTR|TO_BOOL|STORE_SUBSCR|SEND|UNPACK_SEQUENCE|BINARY_SUBSCR)$"),
    ("interp_specialized", r"^_TAIL_CALL_"),
    ("interp_other", r"_PyEval_EvalFrameDefault|_PyEval_Vector|_PyFunction_Vectorcall|_PyObject_Vectorcall|PyObject_Vectorcall|_Py_Specialize|_PyFrame|_PyEval_|_PyStackRef|PyStackRef"),
    ("kernel", r"^\[k\]"),
]
CRX = [(n, re.compile(r)) for n, r in CATS]


def category(sym: str, kernel: bool) -> str:
    if kernel:
        return "kernel"
    for n, rx in CRX:
        if n != "kernel" and rx.search(sym):
            return n
    return "other"


def load(data: str, times: str) -> tuple[dict[str, float], float]:
    run_s = statistics.median(json.load(open(times))["times"])
    out = subprocess.run(["perf", "report", "-i", data, "--no-children", "--sort", "symbol", "--stdio", "-g", "none",
                          "--percent-limit", "0"], capture_output=True, text=True).stdout
    syms: dict[str, float] = defaultdict(float)
    for line in out.splitlines():
        m = re.match(r"\s*([\d.]+)%\s+\[([.k])\]\s+(.*?)\s*$", line)
        if m:
            name = re.sub(r"(\s+-)+$", "", m.group(3)).strip()
            if m.group(2) == "k":
                name = "[k] " + name
            syms[name] += float(m.group(1)) / 100 * run_s * 1e3
    return syms, run_s * 1e3


CAT_ORDER = ["locks", "kernel", "gc", "refcount", "dict", "containers", "attr", "interp_specialized",
             "interp_generic", "interp_other", "typelookup", "qsbr", "alloc", "other"]


def table(files: list[str]) -> None:
    """Markdown: one row per `--json` output, the t - GIL delta (ms per run) per category."""
    print("| workload | GIL ms | t ms | t/GIL | " + " | ".join(CAT_ORDER) + " |")
    print("|---|---:|---:|---:|" + "---:|" * len(CAT_ORDER))
    for f in files:
        d = json.load(open(f))
        g, t = d["cat_gil"], d["cat_ft"]
        name = f.rsplit("/", 1)[-1].removesuffix(".ftdiff.json")
        print(f"| {name} | {d['gil_ms']:.0f} | {d['ft_ms']:.0f} | {d['ft_ms'] / d['gil_ms']:.3f} | "
              + " | ".join(f"{t.get(c, 0) - g.get(c, 0):+.0f}" for c in CAT_ORDER) + " |")


def main() -> None:
    import sys

    if len(sys.argv) > 1 and sys.argv[1] == "table":
        table(sys.argv[2:])
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("gil_data")
    ap.add_argument("gil_times")
    ap.add_argument("ft_data")
    ap.add_argument("ft_times")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--json")
    a = ap.parse_args()
    g, gt = load(a.gil_data, a.gil_times)
    f, ft = load(a.ft_data, a.ft_times)
    cg: dict[str, float] = defaultdict(float)
    cf: dict[str, float] = defaultdict(float)
    for s, v in g.items():
        cg[category(s.removeprefix("[k] "), s.startswith("[k]"))] += v
    for s, v in f.items():
        cf[category(s.removeprefix("[k] "), s.startswith("[k]"))] += v
    print(f"run(): GIL {gt:.1f} ms, free-threaded {ft:.1f} ms, ratio {ft / gt:.3f}, delta {ft - gt:+.1f} ms")
    print()
    print("| category | GIL ms | t ms | delta ms | share of delta |")
    print("|---|---:|---:|---:|---:|")
    d = ft - gt
    for c in sorted(set(cg) | set(cf), key=lambda c: -(cf.get(c, 0) - cg.get(c, 0))):
        dd = cf.get(c, 0) - cg.get(c, 0)
        print(f"| {c} | {cg.get(c, 0):.1f} | {cf.get(c, 0):.1f} | {dd:+.1f} | {dd / d * 100 if d else 0:.0f}% |")
    print()
    print(f"top {a.top} symbols by delta (t - GIL, ms per run):")
    print()
    print("| symbol | category | GIL ms | t ms | delta |")
    print("|---|---|---:|---:|---:|")
    allk = set(g) | set(f)
    for s in sorted(allk, key=lambda s: -abs(f.get(s, 0) - g.get(s, 0)))[: a.top]:
        print(f"| `{s[:70]}` | {category(s.removeprefix('[k] '), s.startswith('[k]'))} | {g.get(s, 0):.2f} | {f.get(s, 0):.2f} | {f.get(s, 0) - g.get(s, 0):+.2f} |")
    if a.json:
        json.dump({"gil_ms": gt, "ft_ms": ft, "cat_gil": cg, "cat_ft": cf,
                   "top": {s: [g.get(s, 0), f.get(s, 0)] for s in sorted(allk, key=lambda s: -abs(f.get(s, 0) - g.get(s, 0)))[:200]}},
                  open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
