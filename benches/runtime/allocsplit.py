#!/usr/bin/env python3
"""Split a workload's steady-state CPU time into allocation / deallocation / GC buckets.

    python benches/runtime/allocsplit.py <family/name> --python <interp> --out <json>
           [--policy SPEC] [-n 5] [--event cpu-clock] [--freq 1999] [--keep DIR]

Runs `gcprobe.py one` under `perf record -g`, gated by a perf control FIFO to the measured
run()s only (no setup, no warm-up), symbolizes every native frame with the wave-1
symbolizer (benches/workloads/data/_profiler.py: DSO offset -> PT_LOAD -> nm, which also
handles python-build-standalone's BOLT layout), and classifies each sample by the native
frames between the leaf and the innermost interpreter frame (_TAIL_CALL_* / _PyEval_*):

  gc:visit        inside a collection; leaf is a tp_traverse or a visit callback
                  (visit_decref, visit_reachable, FT mark/scan visitors, mimalloc heap walk)
  gc:passes       inside a collection; leaf is the collection function itself: with LTO,
                  update_refs, subtract_refs' and move_unreachable's list walks and the
                  gc_list_* splicing are inlined into gc_collect_main (GIL build)
  gc:free         inside a collection; leaf in deallocation/free (deleting garbage)
  gc:other        inside a collection; everything else (list splicing, untrack_tuples,
                  weakref/finalizer handling, freelist clearing, callbacks, stop-the-world)
  dealloc         outside collections, a deallocation frame (_Py_Dealloc, *_dealloc,
                  PyObject_GC_Del, *Free*, mi_free) is on the native stack
  alloc           outside collections, an allocation frame (_PyObject_Malloc, gc_alloc,
                  _PyObject_GC_New*, PyType_GenericAlloc, pymalloc/mimalloc internals,
                  _PyObject_GC_Link, _Py_NewReference) is on the native stack
  gc:track        PyObject_GC_Track/UnTrack, gc list maintenance outside collections
  other           the rest (interpreter, object protocol, C code)

Freelist pushes/pops are inlined into their callers (float_add, tuple_alloc, ...) and land
in `other`; mimalloc/pymalloc fast paths inlined into _PyObject_Malloc land in `alloc`.
The split is therefore a lower bound for alloc/dealloc (validated on micro-cases, see
docs/atlas/w2-runtime.md).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "workloads", "data"))
import _profiler as P  # noqa: E402

GC_FRAMES = ("gc_collect_main", "_Py_RunGC", "_PyGC_Collect", "gc_collect_internal",
             "gc_collect_young", "gc_collect_full", "gc_collect_increment", "gc_collect_region",
             "_PyGC_CollectNoFail", "PyGC_Collect", "gc_collect_impl")
TRAVERSE = re.compile(r"(_traverse$|^visit_|^update_refs|^subtract_refs|^move_unreachable|"
                      r"^deduce_unreachable|^gc_visit_heaps|^mark_heap_visitor|^scan_heap_visitor|"
                      r"^gc_mark_|^gc_propagate_alive|^mark_reachable|^gc_prime_|^_mi_heap_visit|"
                      r"^mi_heap_visit|^_mi_heap_area_visit|^mi_heap_area_visit|^_PyGC_Visit|"
                      r"^gc_visit_|^merge_queued_objects|^gc_maybe_init_refs|^restore_refs|"
                      r"^op_from_block|^visit_decref|^visit_reachable|^gc_decref|^gc_restore)")
DEALLOC = re.compile(r"(_dealloc$|^_Py_Dealloc|^PyObject_GC_Del|^_PyObject_Free|^PyObject_Free|"
                     r"^pymalloc_free|^_PyMem_MiFree|^_PyObject_MiFree|^mi_free|^_mi_free|"
                     r"^mi_page_free|^_mi_page_free|^_PyMem_RawFree|^PyMem_Free|^PyMem_RawFree|"
                     r"^free$|^cfree|^_int_free|^_PyMem_FreeDelayed|^free_delayed|^_Py_Dealloc|"
                     r"^subtype_clear|^_PyObject_ClearManagedDict|^insert_to_emptydict|"
                     r"^free_keys_object|^_PyDict_Clear|^clear_freelist|^_PyUnicode_ExactDealloc|"
                     r"^_PyMem_ArenaFree|^munmap|^_PyFloat_ExactDealloc|^_PyLong_ExactDealloc)")
ALLOC = re.compile(r"(^_PyObject_Malloc|^_PyObject_Calloc|^_PyObject_Realloc|^PyObject_Malloc|"
                   r"^PyObject_Calloc|^PyObject_Realloc|^pymalloc_|^allocate_from_new_pool|"
                   r"^new_arena|^_PyMem_ArenaAlloc|^_PyObject_MiMalloc|^_PyObject_MiCalloc|"
                   r"^_PyObject_MiRealloc|^_PyMem_MiMalloc|^_PyMem_MiCalloc|^_PyMem_MiRealloc|"
                   r"^mi_|^_mi_|^gc_alloc|^_PyObject_GC_New|^_PyObject_GC_NewVar|"
                   r"^PyType_GenericAlloc|^_PyType_AllocNoTrack|^_PyObject_GC_Link|"
                   r"^_Py_NewReference|^PyObject_Init|^_PyObject_New|^PyMem_Malloc|^PyMem_Calloc|"
                   r"^PyMem_Realloc|^PyMem_RawMalloc|^_PyMem_RawMalloc|^_PyMem_RawCalloc|"
                   r"^_PyMem_RawRealloc|^malloc$|^calloc$|^realloc$|^_int_malloc|^mmap|"
                   r"^PyUnstable_Object_GC_NewWithExtraData|^_PyObject_GC_Resize|^record_allocation)")
TRACK = re.compile(r"^(PyObject_GC_Track|PyObject_GC_UnTrack|_PyObject_GC_TRACK|"
                   r"_PyObject_GC_UNTRACK|gc_list_)")
INTERP = re.compile(r"^(_TAIL_CALL_|_PyEval_|PyEval_|_Py_call_instrumentation|_PyFrame_|"
                    r"py_trampoline|_PyJIT)")


def classify(stack):
    """stack: leaf-first list of base symbol names (native frames only)."""
    names = [P.base_name(s) if s else "?" for s in stack]
    if any(n.startswith(GC_FRAMES) for n in names):
        leaf = names[0]
        if TRAVERSE.search(leaf):
            return "gc:visit"
        if leaf.startswith(GC_FRAMES):
            # LTO inlines update_refs / subtract_refs' list walk / move_unreachable /
            # gc_list_* into gc_collect_main: its self time is the list passes
            return "gc:passes"
        for n in names:
            if n.startswith(GC_FRAMES):
                break
            if DEALLOC.search(n):
                return "gc:free"
        return "gc:other"
    region = []
    for n in names:
        if INTERP.search(n):
            break
        region.append(n)
    if not region:
        return "other"
    for n in region:
        if DEALLOC.search(n):
            return "dealloc"
    for n in region:
        if ALLOC.search(n):
            return "alloc"
    if TRACK.search(region[0]):
        return "gc:track"
    return "other"


def parse(data_file):
    elfs, cache = {}, {}

    def sym(dso, off):
        k = (dso, off)
        s = cache.get(k, 0)
        if s == 0:
            e = elfs.get(dso)
            if e is None:
                e = elfs[dso] = P.ElfSyms(dso) if os.path.isfile(dso) else None
            s = e.lookup_off(off) if e is not None else None
            cache[k] = s
        return s

    proc = subprocess.Popen(["perf", "script", "-i", data_file, "-F", "tid,period,ip,sym,dso,dsoff"],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                            errors="replace")
    cats = Counter()
    leafs = {}
    cur = None
    total = 0

    def flush(c):
        nonlocal total
        if c is None or not c[1]:
            return
        cat = classify(c[1])
        cats[cat] += c[0]
        total += c[0]
        leaf = P.base_name(c[1][0]) if c[1][0] else "?"
        leafs.setdefault(cat, Counter())[leaf] += c[0]

    for line in proc.stdout:
        line = line.rstrip("\n")
        if not line.strip():
            flush(cur)
            cur = None
            continue
        if not line.startswith("\t"):
            p = line.split()
            period = int(p[1]) if len(p) > 1 and p[1].isdigit() else 1
            cur = (period, [])
            continue
        m = P._LINE_RE.match(line)
        if not m or cur is None:
            continue
        _ip, s, dsopart = m.groups()
        if s.startswith("py::"):
            continue
        dso, _, off = dsopart.rpartition("+0x")
        if not dso:
            dso, off = dsopart, "0"
        if s and s != "[unknown]" and not s.startswith("0x"):
            cur[1].append(s.split("+0x")[0])
            continue
        o = int(off, 16)
        cur[1].append(sym(dso, o if not cur[1] else max(o - 1, 0)))
    flush(cur)
    proc.wait()
    return total, cats, leafs


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workload")
    ap.add_argument("--python", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--policy", default="default")
    ap.add_argument("-n", type=int, default=5)
    ap.add_argument("--event", default="cpu-clock")
    ap.add_argument("--freq", default="1999")
    ap.add_argument("--keep")
    ap.add_argument("--env", action="append", default=[], help="K=V for the profiled process")
    a = ap.parse_args()
    tmp = a.keep or tempfile.mkdtemp(prefix="allocsplit-")
    os.makedirs(tmp, exist_ok=True)
    ctl, ack = os.path.join(tmp, "ctl"), os.path.join(tmp, "ack")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    data = os.path.join(tmp, "perf.data")
    probe_out = os.path.join(tmp, "probe.json")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    for kv in a.env:
        k, _, v = kv.partition("=")
        env[k] = v
    cmd = (["perf", "record", "-q", "-e", a.event, "-F", a.freq, "-g", "-D", "-1",
            "--control", f"fifo:{ctl},{ack}", "-o", data, "--",
            a.python, os.path.join(HERE, "gcprobe.py"), "one", a.workload, "--policy", a.policy,
            "-n", str(a.n), "-o", probe_out])
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    total, cats, leafs = parse(data)
    with open(probe_out) as f:
        probe = json.load(f)
    out = {
        "workload": a.workload, "python": probe["python"], "policy": a.policy,
        "allocator_env": env.get("PYTHONMALLOC", ""),
        "samples": total, "shares": {k: v / total for k, v in cats.most_common()},
        "top_leafs": {k: [(n, c / total) for n, c in v.most_common(12)] for k, v in leafs.items()},
        "probe_gc_share": sum(sum(g["time_s"] for g in r["gc"].values()) for r in probe["runs"])
                          / sum(r["wall_s"] for r in probe["runs"]),
        "walls": [r["wall_s"] for r in probe["runs"]],
    }
    with open(a.out, "w") as f:
        json.dump(out, f, indent=1)
    if not a.keep:
        for p in (data, ctl, ack, probe_out):
            try:
                os.unlink(p)
            except OSError:
                pass
        os.rmdir(tmp)
    print(json.dumps({k: round(v, 4) for k, v in out["shares"].items()}))


if __name__ == "__main__":
    main()
