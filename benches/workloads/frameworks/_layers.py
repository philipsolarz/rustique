#!/usr/bin/env python3
"""Layer profile of one workload of the frameworks/libs/services families, with third-party
attribution: the six layers of the wave-2 brief, the top third-party distributions, and the
CPython-provided entry points each distribution (and the program's own code) spends time in.

Not a workload (leading underscore). Reuses the wave-1 driver, symbolizer and classifier of
``benches/workloads/data/_profiler.py`` and the method of
``benches/workloads/custom/_profile_custom.py`` (w2-wl-custom), so layer numbers are
comparable across slices:

  python benches/workloads/frameworks/_layers.py <workload.py> --out <profile.json>
        [--seconds 6] [--steps perf,xperf,cprofile,stat] [--stat-enable]

Steps (fresh processes of the running interpreter; setup(), one warm-up run(), then only
steady-state run() calls are sampled, gated with perf's control FIFO; all threads):

  perf     plain ``perf record -e cpu-clock -F 1999 -g`` (frame pointers; kernel samples
           included on the dev box, perf_event_paranoid=-1): the undistorted split by leaf
           native component. ``-X perf`` disables inlined Python-to-Python calls and
           inflates interpreter/call shares (up to 1.27-1.43x, w2-interp), so it is used
           for *who* (owners), never for *what* (components).
  xperf    the same with ``-X perf``: each sample's Python stack. Samples are re-weighted
           per leaf component so the component marginal equals the plain profile's
           (trampoline samples folded into "interpreter" first).
  cprofile one run() under cProfile, read per code object (``Profile.getstats()``; pstats
           merges equal (file, line, name) labels and loses exec-generated methods), with
           caller -> callee edges: which CPython-provided functions each distribution calls,
           how often, and their inclusive time (cProfile-inflated; ranks, not shares).
  stat     ``perf stat -e instructions:u`` over the gated run() loop, plain and (with
           ``--stat-enable``) after ``rustique.enable()``: the noise-robust indicative ratio.

Owners (innermost Python frame, by code-object provenance): own (the workload file),
gen:<kind> (dataclass ``__create_fn__``/namedtuple ``<string>`` code, ``enum.py``,
``functools`` total_ordering, attrs/cattrs ``<... generated ...>`` code, Jinja2
``<template>`` code), stdlib:<module>, thirdparty:<distribution>.

Layers: runtime = allocation/deallocation (``memory/gc`` leaf), cyclic GC (a GC frame on
the stack), import, thread/lock machinery (GIL, parking lot, QSBR), any owner;
generated = everything else under gen:* frames; stdlib = everything else under stdlib
frames, plus C stdlib modules (``_json``, ``_sre``, ``_pickle``, ``_socket``, ``select``,
``_asyncio``, sqlite, ...) entered directly from own/third-party bytecode; thirdparty =
interpreter time under third-party frames plus third-party native extensions (any owner);
language = native object-protocol/builtin components entered from own or third-party
bytecode; own = interpreter time under the workload's own frames. Kernel samples (syscalls)
go to the layer of the C entry that made them (socket/select/io -> stdlib, uvloop ->
thirdparty, page faults under malloc -> runtime) and are also reported separately.

Per *caller* (the innermost own or third-party frame on the stack), every sample is
labelled with the CPython-provided entry point it is in: the outermost stdlib function
the caller called (``stdlib:json/__init__.py:dumps``), the C entry point reached from its
bytecode (``C:builtin_isinstance``, ``C:_PyType_Lookup``...), the generated function, or
runtime work -- the table the atlas uses for "which entry points do libraries spend time
in".
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "data"))
import _profiler as P  # noqa: E402  (wave-1 profiler: driver, symbolizer, classifier)

LANGUAGE = {"attr/type", "calls", "str", "bytes", "int", "float", "dict", "list/tuple/set",
            "iter/range/slice", "builtins-module", "abstract", "other-objects", "generators",
            "exceptions", "libc", "libm", "other"}
RUNTIME = {"memory/gc", "import", "threads/locks"}
INTERP = {"interpreter", "perf-trampoline"}
TOTAL_ORDERING = re.compile(r"^_(gt|lt|ge|le)_from_(gt|lt|ge|le)$")
LIBC_ALLOC = ("malloc", "free", "calloc", "realloc", "_int_malloc", "_int_free", "cfree",
              "malloc_consolidate", "_int_realloc", "unlink_chunk", "tcache_", "sysmalloc",
              "__libc_malloc", "__libc_free", "__libc_calloc", "__libc_realloc", "mmap", "munmap",
              "__mmap", "__munmap", "brk", "__brk", "sbrk")
THREAD_FILES = ("Python/ceval_gil.c", "Python/lock.c", "Python/parking_lot.c", "Python/qsbr.c",
                "Python/critical_section.c", "Python/pystate.c", "Python/thread",
                "Python/brc.c", "Modules/_threadmodule.c")


# --------------------------------------------------------------------------------------
# classification
# --------------------------------------------------------------------------------------


def make_owner(env: dict, workload: str):
    stdlib = env["stdlib"].rstrip("/") + "/"
    pkgdist = env.get("pkgdist", {})
    wl = os.path.abspath(workload)

    def owner(qual: str, fname: str) -> str:
        if fname == wl:
            return "own"
        if fname.startswith("<"):
            if fname == "<string>":
                if qual.startswith("__create_fn__"):
                    return "gen:dataclass"
                if qual == "<lambda>" or qual == "__new__":
                    return "gen:namedtuple"
                return "gen:exec"
            if fname.startswith("<attrs generated"):
                return "gen:attrs"
            if fname.startswith("<cattrs generated") or fname.startswith("<cattrs"):
                return "gen:cattrs"
            if fname == "<template>":
                return "gen:jinja2"
            if not fname.startswith("<frozen "):
                return "gen:exec"
        if fname.startswith("<frozen "):
            rel = fname[8:-1].replace(".", "/") + ".py"
        elif fname.startswith(stdlib) and "/site-packages/" not in fname:
            rel = fname[len(stdlib):]
        elif fname.endswith(("_profiler.py", "_layers.py")):
            return "<driver>"
        elif "/site-packages/" in fname:
            top = fname.split("/site-packages/", 1)[1].split("/")[0].removesuffix(".py")
            return "thirdparty:" + pkgdist.get(top, top)
        else:
            return "other:" + os.path.basename(fname)
        if rel == "enum.py":
            return "gen:enum"
        if rel == "functools.py" and TOTAL_ORDERING.match(qual):
            return "gen:total_ordering"
        if rel == "collections/__init__.py" and qual.startswith("namedtuple.<locals>"):
            return "gen:namedtuple"
        if rel == "reprlib.py" and "recursive_repr" in qual:
            return "gen:dataclass"
        parts = rel.removesuffix(".py").split("/")
        if parts[-1] == "__init__":
            parts = parts[:-1]
        if parts[0].startswith("importlib") or parts[0] in ("_bootstrap", "_bootstrap_external"):
            return "stdlib:importlib"
        return "stdlib:" + (".".join(parts[:2]) if parts[0] in P.GROUP2 and len(parts) > 1 else parts[0])

    def pyname(qual: str, fname: str) -> str:
        if fname == wl:
            return f"own:{qual}"
        if fname.startswith(stdlib):
            return f"{fname[len(stdlib):]}:{qual}"
        if "/site-packages/" in fname:
            return fname.split("/site-packages/", 1)[1] + ":" + qual
        return f"{os.path.basename(fname)}:{qual}"

    def extdist(dso: str) -> str:
        if "/site-packages/" in dso:
            top = dso.split("/site-packages/", 1)[1].split("/")[0]
            top = top.removesuffix(".libs")
            return pkgdist.get(top, pkgdist.get(top.split(".")[0], top))
        return os.path.basename(dso).split(".")[0]

    return owner, pyname, extdist


class Classify:
    def __init__(self, cls: P.Classifier, extdist):
        self.cls = cls
        self.extdist = extdist
        self.cache = {}

    def __call__(self, sym, dso):
        key = (sym, dso)
        r = self.cache.get(key)
        if r is None:
            r = self.cache[key] = self._classify(sym, dso)
        return r

    def _classify(self, sym, dso):
        comp = self.cls.component(sym, dso)
        b = P.base_name(sym) if sym else ""
        if dso and "/lib-dynload/" in dso:
            return "cmod:" + os.path.basename(dso).split(".")[0].lstrip("_")
        if comp.startswith("ext:") or (dso and "/site-packages/" in dso and comp != "kernel"):
            return "ext:" + self.extdist(dso or "")
        if comp == "libc" and b.startswith(LIBC_ALLOC):
            return "memory/gc"
        if comp.startswith("lib:"):
            if "sqlite" in comp:
                return "cmod:sqlite3"
            if "ssl" in comp or "crypto" in comp:
                return "cmod:ssl/hashlib"
            if "ffi" in comp:
                return "cmod:ctypes"
            return "libc"
        if comp in ("kernel", "libc", "libm", "libstdc++"):
            return comp
        rel = self.cls.index.get(b, "")
        if rel.startswith(THREAD_FILES):
            return "threads/locks"
        if b.startswith("sqlite3") or comp in ("cursor", "connection", "statement", "row", "module",
                                                "blob", "microprotocols", "prepare_protocol"):
            return "cmod:sqlite3"
        if rel.startswith("Python/context.c") or rel.startswith("Python/hamt.c"):
            return "cmod:contextvars"
        if comp.startswith("other:Python/structmember") or b.startswith("PyMember_"):
            return "attr/type"
        if comp in ("runtime",):
            return "other-objects"
        if comp.startswith("other:"):
            return "other"
        if comp in LANGUAGE or comp in RUNTIME or comp in INTERP:
            return comp
        return "cmod:" + comp  # a C stdlib module (json-accel, re-engine, pickle, datetime, ...)


def walk(frames, classify):
    """-> leaf component, leaf symbol, index of the innermost Python frame, in_gc, entry
    (the C entry point reached from bytecode: outermost non-wrapper native frame below the
    innermost interpreter frame) and its component."""
    kind, s, dso = frames[0]
    if kind == "py":
        leafc, leafsym = "perf-trampoline", None
    else:
        leafc, leafsym = classify(s, dso), s
    pyidx = None
    boundary = None
    for i, (k, s2, d2) in enumerate(frames):
        if k == "py":
            pyidx = i
            if boundary is None:
                boundary = i
            break
        if boundary is None and s2 is not None and classify(s2, d2) in INTERP:
            boundary = i
    if boundary is None:
        boundary = len(frames)
    in_gc = any(k == "n" and s2 is not None and P.base_name(s2).startswith(P.GC_FRAMES)
                for k, s2, _ in frames[:boundary])
    entry, entryc = None, None
    for j in range(boundary - 1, -1, -1):
        k, s3, d3 = frames[j]
        if k == "py" or s3 is None:
            continue
        c3 = classify(s3, d3)
        b3 = P.base_name(s3)
        if c3 in INTERP or b3.startswith(P.CALL_WRAPPERS):
            continue
        entry, entryc = b3, c3
        break
    return leafc, leafsym, pyidx, in_gc, entry, entryc


def layer_of(owner: str, comp: str, in_gc: bool, entryc: str | None) -> str:
    if in_gc or comp in RUNTIME:
        return "runtime"
    if owner == "stdlib:importlib":
        return "runtime"
    if comp.startswith("ext:"):
        return "thirdparty"
    if owner.startswith("gen:"):
        return "generated"
    if owner.startswith("stdlib:"):
        return "stdlib"
    ec = entryc or ""
    if ec.startswith("cmod:"):  # a C stdlib module entered from bytecode, and what it calls
        return "stdlib"
    if ec.startswith("ext:"):  # a third-party extension entered from bytecode
        return "thirdparty"
    if comp == "kernel" or comp == "libc":
        if ec in RUNTIME:
            return "runtime"
        if owner in ("own",) or owner.startswith("thirdparty:"):
            return "language"
        return "runtime"
    if comp.startswith("cmod:"):
        return "stdlib"
    if owner == "own":
        return "own" if comp in INTERP else "language"
    if owner.startswith("thirdparty:"):
        return "thirdparty" if comp in INTERP else "language"
    return "runtime"  # no Python frame in the gated loop (thread start-up, interpreter-level)


# --------------------------------------------------------------------------------------
# perf
# --------------------------------------------------------------------------------------


class Parser(P.PerfParser):
    """perf >= 7 right-aligns the tid column: sample headers start with spaces, frame lines
    with a tab (the wave-1 parser took every indented line for a frame; fix by w2-wl-custom)."""

    def parse(self, data_file):
        proc = subprocess.Popen(["perf", "script", "-i", data_file, "-F", "tid,period,ip,sym,dso,dsoff"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                errors="replace")
        samples, cur = [], None
        for line in proc.stdout:
            line = line.rstrip("\n")
            if not line.strip():
                if cur is not None and cur[1]:
                    samples.append(cur)
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
            _ip, sym, dsopart = m.groups()
            if sym.startswith("py::"):
                cur[1].append(("py", sym[4:], None))
                continue
            dso, _, off = dsopart.rpartition("+0x")
            if not dso:
                dso, off = dsopart, "0"
            leaf = not cur[1]
            o = int(off, 16)
            cur[1].append(("n", self.sym(dso, o if leaf else max(o - 1, 0)), dso))
        if cur is not None and cur[1]:
            samples.append(cur)
        proc.wait()
        return samples


def run_perf(py, wl, tmp, seconds, perf_x):
    ctl, ack = os.path.join(tmp, "ctl"), os.path.join(tmp, "ack")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    data = os.path.join(tmp, "perf-x.data" if perf_x else "perf.data")
    out = os.path.join(tmp, "drv.json")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    cmd = (["perf", "record", "-q", "-e", "cpu-clock", "-F", "1999", "-g", "-D", "-1",
            "--control", f"fifo:{ctl},{ack}", "-o", data, "--"]
           + P._run_driver(py, wl, "perf", out, seconds, 3, perf_x))
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(out) as f:
        return data, json.load(f)


def plain_profile(samples, classify) -> dict:
    total = sum(p for p, _ in samples) or 1
    comp, entry, top = Counter(), Counter(), Counter()
    gc = 0
    for period, frames in samples:
        leafc, leafsym, _pyidx, in_gc, ent, entc = walk(frames, classify)
        comp[leafc] += period
        gc += period if in_gc else 0
        top[(P.base_name(leafsym) if leafsym else "?", leafc)] += period
        entry[(ent or "(interpreter)", entc or "interpreter")] += period
    sh = lambda v: round(v / total, 5)
    return {"samples": len(samples), "component": {k: sh(v) for k, v in comp.most_common()},
            "gc_share": sh(gc), "top_native": [[n, c, sh(v)] for (n, c), v in top.most_common(50)],
            "top_entry": [[n, c, sh(v)] for (n, c), v in entry.most_common(50)]}


def stack_info(frames, pyidx, owner_fn, pyname_fn):
    """innermost owner/function, the caller (innermost own/third-party frame) and the
    CPython-provided Python function the caller entered (outermost stdlib/gen frame below it)."""
    if pyidx is None:
        return "<no-python-frame>", "<none>", "<no-python-frame>", None
    inner_owner = inner_func = None
    caller = "<stdlib-root>"
    entered = None
    for k, s2, _d in frames[pyidx:]:
        if k != "py":
            continue
        qual, _, fname = s2.partition(":")
        o = owner_fn(qual, fname)
        if o == "<driver>":
            o = "own"
        if inner_owner is None:
            inner_owner, inner_func = o, pyname_fn(qual, fname)
        if o == "own" or o.startswith("thirdparty:") or o.startswith("other:"):
            caller = o
            break
        entered = (o, pyname_fn(qual, fname))
    return inner_owner, inner_func, caller, entered


def xperf_profile(samples, classify, owner_fn, pyname_fn, plain_comp: dict) -> dict:
    rows = []  # (period, owner, comp, in_gc, layer, caller, label, func, entry)
    xcomp = Counter()
    for period, frames in samples:
        leafc, leafsym, pyidx, in_gc, ent, entc = walk(frames, classify)
        comp = "interpreter" if leafc == "perf-trampoline" else leafc
        own, func, caller, entered = stack_info(frames, pyidx, owner_fn, pyname_fn)
        lay = layer_of(own, comp, in_gc, entc)
        if lay == "runtime":
            label = ("gc" if in_gc else "import" if (own == "stdlib:importlib" or comp == "import")
                     else "threads/locks" if comp == "threads/locks" else "alloc/dealloc")
        elif lay == "generated":
            label = f"{own}:{func.split(':', 1)[-1]}" if entered is None else f"{entered[0]}:{entered[1].split(':', 1)[-1]}"
        elif lay == "stdlib":
            if own.startswith("stdlib:") and entered is not None:
                label = f"py:{entered[1]}"
            elif own.startswith("stdlib:"):
                label = f"py:{func}"
            else:
                mod = entc if (entc or "").startswith("cmod:") else comp
                label = f"C:{ent or P.base_name(leafsym or '?')} [{mod.removeprefix('cmod:')}]"
        elif lay == "language":
            label = f"C:{ent or P.base_name(leafsym or '?')} [{entc or comp}]"
        elif lay == "thirdparty":
            label = ("ext:" + comp[4:]) if comp.startswith("ext:") else (
                "ext:" + entc[4:] if (entc or "").startswith("ext:") else "bytecode")
        else:
            label = "bytecode"
        xcomp[comp] += period
        rows.append((period, own, comp, in_gc, lay, caller, label, func, ent, entc,
                     comp == "kernel"))
    xtot = sum(xcomp.values()) or 1
    w = {c: (plain_comp.get(c, 0.0) / (v / xtot)) if v else 0.0 for c, v in xcomp.items()}
    layers, owners, stdlib_mod, third, gen, lang_comp, lang_entry, runtime = (Counter() for _ in range(8))
    kernel_by_layer = Counter()
    lang_sites = Counter()  # (C entry, innermost Python function) for the language layer
    per_caller = defaultdict(Counter)
    caller_tot = Counter()
    pyself = Counter()
    raw_layers = Counter()
    rtot = sum(r[0] * w.get(r[2], 0.0) for r in rows) or 1
    for period, own, comp, in_gc, lay, caller, label, func, ent, entc, kern in rows:
        v = period * w.get(comp, 0.0) / rtot
        raw_layers[lay] += period / sum(xcomp.values())
        layers[lay] += v
        owners[own] += v
        per_caller[caller][(lay, label)] += v
        caller_tot[caller] += v
        pyself[(func, own)] += v
        if kern:
            kernel_by_layer[lay] += v
        if lay == "stdlib":
            mod = own if own.startswith("stdlib:") else "C:" + (
                entc if (entc or "").startswith("cmod:") else comp).removeprefix("cmod:")
            stdlib_mod[mod.removeprefix("stdlib:")] += v
        elif lay == "thirdparty":
            dist = comp[4:] if comp.startswith("ext:") else (
                entc[4:] if (entc or "").startswith("ext:") else own.removeprefix("thirdparty:"))
            third[dist + (" (native)" if comp.startswith("ext:") or (entc or "").startswith("ext:") else "")] += v
        elif lay == "generated":
            gen[own] += v
        elif lay == "language":
            lang_comp[comp] += v
            lang_entry[f"{ent or '?'} [{entc or comp}]"] += v
            lang_sites[(ent or "?", func)] += v
        elif lay == "runtime":
            runtime[label] += v
    # third-party distributions: everything under their frames (any layer), by distribution
    dist_total = Counter()
    for c, v in caller_tot.items():
        if c.startswith("thirdparty:"):
            dist_total[c[11:]] += v
    r5 = lambda x: round(x, 5)
    return {
        "samples": len(samples),
        "layers": {k: r5(v) for k, v in layers.most_common()},
        "layers_raw_xperf": {k: r5(v) for k, v in raw_layers.most_common()},
        "owners": {k: r5(v) for k, v in owners.most_common(40)},
        "stdlib_modules": {k: r5(v) for k, v in stdlib_mod.most_common(30)},
        "thirdparty_layer": {k: r5(v) for k, v in third.most_common(25)},
        "generated": {k: r5(v) for k, v in gen.most_common(12)},
        "language_components": {k: r5(v) for k, v in lang_comp.most_common(20)},
        "language_entries": {k: r5(v) for k, v in lang_entry.most_common(40)},
        "runtime": {k: r5(v) for k, v in runtime.most_common()},
        "language_sites": [[e, f, r5(v)] for (e, f), v in lang_sites.most_common(40)],
        "kernel_by_layer": {k: r5(v) for k, v in kernel_by_layer.most_common()},
        "caller_total": {k: r5(v) for k, v in caller_tot.most_common(25)},
        "dist_total": {k: r5(v) for k, v in dist_total.most_common(20)},
        "per_caller": {c: {"total": r5(caller_tot[c]),
                           "by_layer": {lay: r5(sum(v for (l2, _), v in cnt.items() if l2 == lay))
                                        for lay in ("own", "thirdparty", "language", "stdlib", "generated", "runtime")},
                           "top": [[lay, lab, r5(v)] for (lay, lab), v in cnt.most_common(30)]}
                       for c, cnt in sorted(per_caller.items(), key=lambda kv: -caller_tot[kv[0]])[:14]},
        "top_python_self": [[f, o, r5(v)] for (f, o), v in pyself.most_common(50)],
        "reweight": {c: round(v, 4) for c, v in sorted(w.items())},
    }


def run_stat(py, wl, tmp, seconds, enable=False):
    ctl, ack = os.path.join(tmp, "sctl"), os.path.join(tmp, "sack")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    out = os.path.join(tmp, "drv-stat.json")
    csv = os.path.join(tmp, "stat.csv")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    drv = P._run_driver(py, wl, "perf", out, seconds, 3, False)
    if enable:
        drv = [py, "-c", "import rustique, runpy, sys; rustique.enable(); "
               "sys.argv = sys.argv[1:]; runpy.run_path(sys.argv[0], run_name='__main__')"] + drv[1:]
    cmd = (["perf", "stat", "-x", ",", "-e", "instructions:u,cycles:u", "-D", "-1",
            "--control", f"fifo:{ctl},{ack}", "-o", csv, "--"] + drv)
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(out) as f:
        d = json.load(f)
    vals = {}
    for line in open(csv):
        parts = line.strip().split(",")
        if len(parts) > 2 and parts[0].replace(".", "").isdigit():
            vals[parts[2].split(":")[0]] = float(parts[0])
    n = len(d["walls"])
    return {"runs": n, "instructions_per_run": vals.get("instructions", 0) / n,
            "cycles_per_run": vals.get("cycles", 0) / n, "wall_median_s": statistics.median(d["walls"]),
            "utime_per_run_s": statistics.median(d["utimes"]), "stime_per_run_s": statistics.median(d["stimes"]),
            "digest": d["digest"], "digest_ok": d["digest_ok"]}


# --------------------------------------------------------------------------------------
# cProfile per code object, with caller -> callee edges
# --------------------------------------------------------------------------------------


def cprofile_driver(wl: str, out: str) -> None:
    import cProfile

    mod = P._load_workload(wl)
    state = mod.setup()
    mod.run(state)
    prof = cProfile.Profile()
    prof.enable()
    mod.run(state)
    prof.disable()

    def key(code):
        if isinstance(code, str):
            return ("~", code)
        return (code.co_filename, code.co_qualname)

    nodes = defaultdict(lambda: [0, 0.0, 0.0])
    edges = defaultdict(lambda: [0, 0.0, 0.0])
    for e in prof.getstats():
        k = key(e.code)
        nodes[k][0] += e.callcount
        nodes[k][1] += e.inlinetime
        nodes[k][2] += e.totaltime
        for sub in e.calls or ():
            ek = (k, key(sub.code))
            edges[ek][0] += sub.callcount
            edges[ek][1] += sub.totaltime
            edges[ek][2] += sub.inlinetime
    with open(out, "w") as f:
        json.dump({"env": P._env_info(),
                   "nodes": [[f_, n, c, it, tt] for (f_, n), (c, it, tt) in nodes.items()],
                   "edges": [[a[0], a[1], b[0], b[1], c, t, it] for (a, b), (c, t, it) in edges.items()]}, f)


def run_cprofile(py, wl, tmp, owner_fn, pyname_fn) -> dict:
    out = os.path.join(tmp, "drv-cprofile.json")
    subprocess.run([py, os.path.abspath(__file__), wl, "--out", out, "--cprofile-driver"], check=True)
    with open(out) as f:
        cp = json.load(f)

    def own_of(fname, name):
        if fname == "~":
            return "<builtin>"
        o = owner_fn(name, fname)
        return "own" if o == "<driver>" else o

    def label(fname, name):
        if fname == "~":
            return name
        return pyname_fn(name, fname)

    tot = sum(n[3] for n in cp["nodes"]) or 1.0
    by_owner_tt, by_owner_calls = Counter(), Counter()
    for f_, n, c, it, _tt in cp["nodes"]:
        o = own_of(f_, n)
        by_owner_tt[o] += it
        by_owner_calls[o] += c
    # edges from own/third-party callers into CPython-provided callees
    edge_t = defaultdict(Counter)  # builtins: self time; Python stdlib/generated: inclusive time
    edge_c = defaultdict(Counter)
    for cf, cn, ef, en, c, t, it in cp["edges"]:
        co = own_of(cf, cn)
        if not (co == "own" or co.startswith("thirdparty:")):
            continue
        eo = own_of(ef, en)
        if eo == "<builtin>" and en.startswith(("<built-in", "<method")):
            lab = label(ef, en)
            edge_t[co][lab] += it
            edge_c[co][lab] += c
        elif eo.startswith("stdlib:") or eo.startswith("gen:"):
            lab = label(ef, en)
            edge_t[co][lab] += t
            edge_c[co][lab] += c
    calls_into = {co: [[lab, edge_c[co][lab], round(t / tot, 5)] for lab, t in cnt.most_common(25)]
                  for co, cnt in edge_t.items()}
    calls_rank = {co: [[lab, c] for lab, c in cnt.most_common(15)] for co, cnt in edge_c.items()}
    return {"total_calls": sum(n[2] for n in cp["nodes"]),
            "owner_tottime_share": {k: round(v / tot, 5) for k, v in by_owner_tt.most_common(30)},
            "owner_calls": dict(by_owner_calls.most_common(30)),
            "calls_into_cpython_by_time": calls_into,
            "calls_into_cpython_by_count": calls_rank,
            "top_builtins": sorted([[n, c, round(it / tot, 5)] for f_, n, c, it, _ in cp["nodes"] if f_ == "~"],
                                   key=lambda r: -r[1])[:30]}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("workload")
    ap.add_argument("--out", required=True)
    ap.add_argument("--seconds", type=float, default=6.0)
    ap.add_argument("--steps", default="perf,xperf,cprofile,stat")
    ap.add_argument("--stat-enable", action="store_true")
    ap.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC", os.path.expanduser("~/src/cpython-3.15.1")))
    ap.add_argument("--cache", default=os.path.join(tempfile.gettempdir(), "rq-layers-cache"))
    ap.add_argument("--cprofile-driver", action="store_true", help=argparse.SUPPRESS)
    a = ap.parse_args()
    if a.cprofile_driver:
        return cprofile_driver(os.path.abspath(a.workload), a.out)
    os.makedirs(a.cache, exist_ok=True)
    py = sys.executable
    wl = os.path.abspath(a.workload)
    steps = a.steps.split(",")
    result = {"workload": os.path.basename(wl)[:-3]}
    if os.path.exists(a.out):
        with open(a.out) as f:
            result = json.load(f)
    tmp = tempfile.mkdtemp(prefix="rqlayers-")
    env = P._env_info()
    result["python"] = env["python"]
    owner_fn, pyname_fn, extdist = make_owner(env, wl)
    classify = Classify(P.Classifier(a.cpython_src, a.cache), extdist)
    parser = Parser(classify.cls, None, None)
    name = result["workload"]
    if "perf" in steps:
        data, drv = run_perf(py, wl, tmp, a.seconds, False)
        result["plain"] = plain_profile(parser.parse(data), classify)
        result["plain"]["wall_median_s"] = round(statistics.median(drv["walls"]), 4)
        result["plain"]["runs"] = len(drv["walls"])
        print(f"[{name}] perf plain {result['plain']['samples']}", file=sys.stderr, flush=True)
    if "xperf" in steps:
        data, drv = run_perf(py, wl, tmp, a.seconds, True)
        samples = parser.parse(data)
        unresolved = sum(1 for _p, fr in samples for k, s, d in fr
                         if k == "n" and s is None and d and "perf-" in d and d.endswith(".map"))
        P._rm_perf_map(drv)
        result["xperf"] = xperf_profile(samples, classify, owner_fn, pyname_fn, result["plain"]["component"])
        result["xperf"]["wall_median_s"] = round(statistics.median(drv["walls"]), 4)
        result["xperf"]["unresolved_python_frames"] = unresolved
        print(f"[{name}] perf -X {result['xperf']['samples']} (unresolved py frames {unresolved})",
              file=sys.stderr, flush=True)
    if "cprofile" in steps:
        result["cprofile"] = run_cprofile(py, wl, tmp, owner_fn, pyname_fn)
        print(f"[{name}] cprofile {result['cprofile']['total_calls']} calls", file=sys.stderr, flush=True)
    if "stat" in steps:
        result["stat"] = run_stat(py, wl, tmp, a.seconds)
        if a.stat_enable:
            result["stat_enabled"] = run_stat(py, wl, tmp, a.seconds, enable=True)
            result["instr_ratio_enabled"] = round(result["stat_enabled"]["instructions_per_run"]
                                                  / result["stat"]["instructions_per_run"], 4)
        print(f"[{name}] stat {result['stat']['instructions_per_run']:.3e} instr/run"
              + (f", enabled ratio {result.get('instr_ratio_enabled')}" if a.stat_enable else ""),
              file=sys.stderr, flush=True)
    result["method"] = __doc__.split("\n\n", 2)[1]
    with open(a.out, "w") as f:
        json.dump(result, f, indent=1)


if __name__ == "__main__":
    main()
