#!/usr/bin/env python3
"""Layer profiler (wave 2): where does a Python program's time go, by layer?

Attributes every CPU-cycle sample of a workload (benches/workloads/<family>/<name>.py) to the
innermost Python frame's *owner* x the leaf *native component*, and reports it in the six
layers of docs/atlas/w2-attribution.md: own / generated / language / stdlib / thirdparty /
runtime, plus the opcode split of interpreter time (CPython 3.15's tail-calling interpreter
compiles each opcode handler to its own function, `_TAIL_CALL_<OPNAME>`).

Two complementary recordings of the same steady state (setup(), one warm-up run(), then
run() in a loop for --seconds, perf gated by a control FIFO to the loop):

  plain   `perf record -e cycles --call-graph dwarf` WITHOUT -X perf: the interpreter runs
          exactly as in production (specialized Python-to-Python calls stay inlined). The
          owner of a sample is found from the bytecode pointer the interpreter keeps in a
          register (next_instr, r15 in the preserve_none convention) or spills to the stack:
          the driver dumps the address range of every code object's bytecode
          (id(code) + type(code).__basicsize__, len(co_code) bytes; the main thread's
          specialized bytecode lives there on both builds), and the first register / stack
          word (leaf upwards, up to the innermost interpreter frame) inside one of them
          names the code object. The frame-pointer chain (unwound from the dumped stack)
          names the leaf, the C entry point and the opcode handler. Primary numbers.
  xperf   `perf record -e cycles -g` with `python -X perf`: Python frames appear as
          `py::<qualname>:<file>` trampolines, giving the full Python stack (callers too).
          The trampoline is a PEP 523 frame evaluator, so the interpreter stops inlining
          Python-to-Python calls and despecializes CALL_PY_* / FOR_ITER_GEN / SEND_GEN /
          LOAD_ATTR_PROPERTY: its shares are distorted towards call overhead. Used for the
          caller -> callee entry tables (which stdlib functions / C entry points own and
          third-party code call) and to cross-check the plain owner attribution.

`--mode whole` profiles the whole process (startup, imports, setup, one run, finalization)
with frame-pointer callchains and CLOCK_MONOTONIC timestamps; the driver records the phase
boundaries.

Subcommands: profile, validate, rollup, ftdiff (see --help). Runs on Linux with perf and
hardware counters (the dev box); the analysis needs numpy.
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import pickle
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))

# =========================================================================================
# driver (runs inside the profiled interpreter; stdlib only)
# =========================================================================================


def _ctl(cmd):
    ctl = os.environ.get("RQ_PERF_CTL")
    if not ctl:
        return
    with open(ctl, "w") as f:
        f.write(cmd + "\n")
    with open(os.environ["RQ_PERF_ACK"]) as f:
        f.read(5)


def _load_workload(path):
    import importlib.util

    name = "workload_" + os.path.splitext(os.path.basename(path))[0]
    sys.path.insert(0, os.path.dirname(path))
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def _dist_files():
    """file path -> distribution name, from every installed distribution's RECORD."""
    out = {}
    try:
        import importlib.metadata as md
    except Exception:  # pragma: no cover
        return out
    for d in md.distributions():
        try:
            name = d.metadata["Name"]
            files = d.files or ()
        except Exception:
            continue
        for f in files:
            s = str(f)
            if not s.endswith((".py", ".so", ".pyx", ".pyd")) and ".so." not in s:
                continue
            try:
                out[os.path.realpath(str(d.locate_file(f)))] = name
            except Exception:
                pass
    return out


class _CodeDump:
    def __init__(self):
        self.seen = {}

    def _add(self, co, gname):
        import types

        k = id(co)
        if k in self.seen:
            return
        self.seen[k] = (co, gname)
        for c in co.co_consts:
            if isinstance(c, types.CodeType):
                self._add(c, gname)

    def collect(self):
        import gc
        import types

        def gname_of(g):
            if not isinstance(g, dict):
                return None
            n = g.get("__name__")
            if n is None and "environment" in g:
                return "<jinja2>"
            return n

        for o in gc.get_objects():
            t = type(o)
            try:
                if t is types.FunctionType:
                    self._add(o.__code__, gname_of(o.__globals__))
                elif t is types.GeneratorType:
                    self._add(o.gi_code, gname_of(o.gi_frame.f_globals) if o.gi_frame else None)
                elif t is types.CoroutineType:
                    self._add(o.cr_code, gname_of(o.cr_frame.f_globals) if o.cr_frame else None)
                elif t is types.AsyncGeneratorType:
                    self._add(o.ag_code, gname_of(o.ag_frame.f_globals) if o.ag_frame else None)
                elif t is types.CodeType:
                    self._add(o, None)
            except Exception:
                pass
        for f in sys._current_frames().values():
            while f is not None:
                self._add(f.f_code, gname_of(f.f_globals))
                f = f.f_back

    def rows(self):
        import types

        bs = types.CodeType.__basicsize__
        out = []
        for k, (co, g) in self.seen.items():
            try:
                adaptive = co._co_code_adaptive
            except Exception:
                adaptive = co.co_code
            out.append((k + bs, len(adaptive), co.co_filename, co.co_qualname,
                        co.co_firstlineno, g, adaptive))
        return out


def _opcode_meta():
    import dis
    import opcode

    spec = {k: list(v) for k, v in getattr(opcode, "_specializations", {}).items()}
    names = list(getattr(dis, "_all_opname", opcode.opname))
    caches = dict(getattr(opcode, "_inline_cache_entries", {}))
    return {"specializations": spec, "opnames": names, "caches": caches}


def _debug_offsets():
    """Addresses/offsets an external sampler needs to read the current Python frame of a
    thread: _PyRuntime._Py_DebugOffsets (3.13+), the layout of Include/internal/
    pycore_debug_offsets.h in 3.15 (uint64 fields)."""
    import ctypes
    import struct
    import threading

    rt = ctypes.addressof(ctypes.c_char.in_dll(ctypes.pythonapi, "_PyRuntime"))
    raw = ctypes.string_at(rt, 512)
    if raw[:8] != b"xdebugpy":
        raise RuntimeError("no _Py_DebugOffsets cookie")
    q = lambda o: struct.unpack_from("<Q", raw, o)[0]  # noqa: E731
    version = q(8)
    if version >> 16 != 0x30F:
        raise RuntimeError(f"debug offsets layout is for 3.15 (got {version:#x})")
    off = {"version": version, "free_threaded": q(16), "interpreters_head": q(40),
           "threads_head": q(72), "threads_main": q(80), "ts_next": q(192),
           "current_frame": q(208), "native_thread_id": q(248), "frame_previous": q(320),
           "frame_executable": q(328), "frame_instr_ptr": q(336)}
    off["code_filename"], off["code_qualname"] = q(384), q(400)
    probe = "rq_layers_ascii_probe_string"
    off["ascii_data"] = ctypes.string_at(id(probe), 256).find(probe.encode())
    c = _debug_offsets.__code__
    if struct.unpack("<Q", ctypes.string_at(id(c) + off["code_filename"], 8))[0] != \
            id(c.co_filename):
        raise RuntimeError("debug offsets self-check failed (code filename)")
    interp = struct.unpack("<Q", ctypes.string_at(rt + off["interpreters_head"], 8))[0]
    # self-check: our own current frame through these offsets
    ts = struct.unpack("<Q", ctypes.string_at(interp + off["threads_main"], 8))[0]
    tid = struct.unpack("<Q", ctypes.string_at(ts + off["native_thread_id"], 8))[0]
    if tid != threading.get_native_id():
        raise RuntimeError("debug offsets self-check failed (native thread id)")
    return {"pid": os.getpid(), "runtime": rt, "interp": interp, "offsets": off}


def _envinfo(workload, out_path):
    """Classification data for a whole-process profile, from a separate process of the same
    interpreter: paths, distribution files, opcode metadata, the workload module's name."""
    import sysconfig

    paths = sysconfig.get_paths()
    out = {"pid": -1, "codes": [], "env": {
        "python": sys.version.split()[0] + ("t" if not getattr(sys, "_is_gil_enabled",
                                                               lambda: True)() else ""),
        "executable": os.path.realpath(sys.executable), "stdlib": paths["stdlib"],
        "purelib": paths["purelib"], "platlib": paths["platlib"],
        "workload": os.path.abspath(workload),
        "workload_module": "workload_" + os.path.splitext(os.path.basename(workload))[0],
        "enabled": False, "tmp": tempfile.gettempdir(),
        "stdlib_modules": sorted(sys.stdlib_module_names)},
        "dist_files": _dist_files(), "opcode": _opcode_meta()}
    try:
        import importlib.metadata as md

        out["pkgdist"] = {k: v[0] for k, v in md.packages_distributions().items()}
    except Exception:
        out["pkgdist"] = {}
    with open(out_path, "wb") as f:
        pickle.dump(out, f, protocol=4)


def driver_main(a):
    if a.mode == "envinfo":
        return _envinfo(a.workload, a.out)
    t_start = time.monotonic_ns()
    import sysconfig

    marks = {"driver_start": t_start}
    if a.enable:
        import rustique

        rustique.enable()
        marks["enabled"] = time.monotonic_ns()
    mod = _load_workload(a.workload)
    marks["imported"] = time.monotonic_ns()
    state = mod.setup()
    marks["setup"] = time.monotonic_ns()
    t0 = time.perf_counter()
    ref = mod.digest(mod.run(state))
    first_run = time.perf_counter() - t0
    marks["first_run"] = time.monotonic_ns()
    import types as _types

    out = {"pid": os.getpid(), "first_run_s": first_run, "digest": ref,
           "code_basicsize": _types.CodeType.__basicsize__}
    dump = _CodeDump()
    walls = []
    digest_ok = True
    if a.mode == "bpf":
        dump.collect()
        sync = os.environ["RQ_SYNC"].split(",")  # ready fifo (driver -> orchestrator), go fifo
        offs = _debug_offsets()
        with open(sync[0], "w") as f:
            f.write(json.dumps(offs) + "\n")
        with open(sync[1]) as f:
            f.readline()
        marks["gate_on"] = time.monotonic_ns()
        ru0 = __import__("resource").getrusage(0)
        t_loop = time.perf_counter()
        while True:
            t0 = time.perf_counter()
            res = mod.run(state)
            t1 = time.perf_counter()
            walls.append(t1 - t0)
            if t1 - t_loop >= a.seconds and len(walls) >= a.min_iters:
                break
        ru1 = __import__("resource").getrusage(0)
        out["cpu_s"] = (ru1.ru_utime - ru0.ru_utime) + (ru1.ru_stime - ru0.ru_stime)
        marks["gate_off"] = time.monotonic_ns()
        with open(sync[0], "w") as f:
            f.write("done\n")
        with open(sync[1]) as f:
            f.readline()
        digest_ok = mod.digest(res) == ref
        with open("/proc/self/maps") as f:
            out["proc_maps"] = f.read()
    if a.mode == "steady":
        dump.collect()
        _ctl("enable")
        marks["gate_on"] = time.monotonic_ns()
        t_loop = time.perf_counter()
        while True:
            t0 = time.perf_counter()
            res = mod.run(state)
            t1 = time.perf_counter()
            walls.append(t1 - t0)
            if t1 - t_loop >= a.seconds and len(walls) >= a.min_iters:
                break
        marks["gate_off"] = time.monotonic_ns()
        _ctl("disable")
        digest_ok = mod.digest(res) == ref
    dump.collect()
    paths = sysconfig.get_paths()
    out.update(
        walls=walls, digest_ok=digest_ok, marks=marks,
        env={"python": sys.version.split()[0] + ("t" if not getattr(sys, "_is_gil_enabled",
                                                                       lambda: True)() else ""),
             "executable": os.path.realpath(sys.executable), "stdlib": paths["stdlib"],
             "purelib": paths["purelib"], "platlib": paths["platlib"],
             "workload": os.path.abspath(a.workload), "workload_module": mod.__name__,
             "enabled": bool(a.enable), "tmp": tempfile.gettempdir(),
             "stdlib_modules": sorted(sys.stdlib_module_names)},
        dist_files=_dist_files(), opcode=_opcode_meta(), codes=dump.rows())
    try:
        import importlib.metadata as md

        out["pkgdist"] = {k: v[0] for k, v in md.packages_distributions().items()}
    except Exception:
        out["pkgdist"] = {}
    with open(a.out, "wb") as f:
        pickle.dump(out, f, protocol=4)
    marks["exit"] = time.monotonic_ns()
    if a.mode == "whole":  # rewrite with the final mark (finalization follows)
        with open(a.out, "wb") as f:
            pickle.dump(out, f, protocol=4)


# =========================================================================================
# owner classification (Python code objects)
# =========================================================================================

GROUP2 = {"xml", "email", "concurrent", "urllib", "http", "multiprocessing", "json",
          "logging", "asyncio", "importlib", "collections", "unittest", "encodings",
          "wsgiref", "sqlite3", "ctypes", "tomllib", "zoneinfo", "pathlib", "re", "string",
          "sysconfig", "compression", "profiling", "annotationlib"}
IMPORT_FILES = ("importlib/_bootstrap.py", "importlib/_bootstrap_external.py",
                "importlib/__init__.py", "importlib/machinery.py", "importlib/util.py",
                "zipimport.py", "importlib/_abc.py")
_TOTAL_ORDERING = re.compile(r"^_(gt|lt|ge|le)_from_(gt|lt|ge|le)$")


class Owners:
    def __init__(self, drv):
        env = drv["env"]
        self.wl = env["workload"]
        self.wl_dir = os.path.dirname(self.wl) + "/"
        self.wl_mod = env["workload_module"]
        self.stdlib = env["stdlib"].rstrip("/") + "/"
        self.tmp = env.get("tmp", "/tmp").rstrip("/") + "/"
        self.dist_files = drv.get("dist_files", {})
        self.pkgdist = drv.get("pkgdist", {})
        self.stdlib_mods = set(env.get("stdlib_modules", ()))
        self.driver = os.path.abspath(__file__)
        self.cache = {}
        # (qualname, filename) -> globals name, for -X perf names (no code identity)
        self.gname_by_name = {}
        for row in drv.get("codes", ()):
            k = (row[3], row[2])
            g = row[5]
            prev = self.gname_by_name.get(k, 0)
            self.gname_by_name[k] = g if prev in (0, g) else None

    def _mod_owner(self, gname):
        if not gname:
            return None
        if gname == self.wl_mod or gname == "__main__":
            return ("own", "")
        top = gname.split(".")[0]
        if top in self.stdlib_mods:
            return ("stdlib", top)
        if top in self.pkgdist:
            return ("thirdparty", self.pkgdist[top])
        return None

    def of(self, filename, qualname, gname=0):
        key = (filename, qualname, gname)
        r = self.cache.get(key)
        if r is None:
            if gname == 0:
                gname = self.gname_by_name.get((qualname, filename))
            r = self.cache[key] = self._of(filename, qualname, gname)
        return r

    def _of(self, fn, qual, gname):
        if not fn or not fn.isprintable() or (fn == "__init__" and qual == "__init__"):
            # a frame being torn down (garbage read) or the interpreter's __init__ shim frame
            # (CALL_ALLOC_AND_ENTER_INIT / EXIT_INIT_CHECK): imputed later by opcode
            return ("unresolved", "")
        if fn == self.wl or fn.startswith(self.wl_dir):
            return ("own", "")
        if fn == self.driver or fn.endswith("/benches/atlas/layers.py"):
            return ("harness", "")
        if fn.startswith("<frozen "):
            m = fn[8:-1]
            if m.startswith(("importlib", "zipimport")):
                return ("import", "importlib")
            return ("stdlib", m.split(".")[0])
        if fn.startswith("<"):
            if qual.startswith("__create_fn__"):
                return ("generated", "dataclasses")
            if gname and gname.startswith("namedtuple_"):
                return ("generated", "namedtuple")
            if fn.startswith("<attrs generated"):
                return ("generated", "attrs")
            if fn.startswith("<cattrs generated"):
                return ("generated", "cattrs")
            if fn == "<template>":
                return ("thirdparty", self.pkgdist.get("jinja2", "Jinja2"))
            if gname == "<jinja2>":
                return ("thirdparty", self.pkgdist.get("jinja2", "Jinja2"))
            mo = self._mod_owner(gname)
            if mo is not None:
                if mo[0] == "stdlib":
                    return ("generated", "exec:" + mo[1])
                return mo
            return ("other", fn[:40])
        if fn.startswith(self.stdlib) and "/site-packages/" not in fn:
            rel = fn[len(self.stdlib):]
            if rel.endswith(IMPORT_FILES) and rel in IMPORT_FILES:
                return ("import", "importlib")
            parts = rel.removesuffix(".py").split("/")
            if parts[-1] == "__init__":
                parts = parts[:-1]
            top = parts[0]
            if top == "collections" and qual.startswith("namedtuple.<locals>"):
                return ("generated", "namedtuple")
            if top == "functools" and _TOTAL_ORDERING.match(qual):
                return ("generated", "total_ordering")
            if top == "enum":
                return ("generated", "enum")
            if top == "dataclasses" and "<locals>" in qual:
                return ("generated", "dataclasses")
            if top in GROUP2 and len(parts) > 1:
                return ("stdlib", ".".join(parts[:2]))
            return ("stdlib", top)
        real = fn
        d = self.dist_files.get(real)
        if d is None and os.path.exists(fn):
            real = os.path.realpath(fn)
            d = self.dist_files.get(real)
        if d is not None:
            if d.lower() == "rustique":
                return ("rustique", "")
            return ("thirdparty", d)
        if "/site-packages/" in fn:
            top = fn.split("/site-packages/", 1)[1].split("/")[0].removesuffix(".py")
            if top == "rustique":
                return ("rustique", "")
            return ("thirdparty", self.pkgdist.get(top, top))
        if "/python/rustique/" in fn:
            return ("rustique", "")
        if fn.startswith(self.tmp) or fn.startswith("/tmp/"):
            return ("own", "")  # code the workload wrote (its app package, sphinx conf.py)
        if not fn.startswith("/"):
            mo = self._mod_owner(gname)
            if mo is not None:
                return mo
        return ("other", os.path.basename(fn))


# =========================================================================================
# analysis
# =========================================================================================


class CodeMap:
    def __init__(self, rows):
        import numpy as np

        rows = sorted(rows, key=lambda r: r[0])
        self.rows = rows
        self.starts = np.array([r[0] for r in rows], dtype=np.uint64)
        self.ends = np.array([r[0] + r[1] for r in rows], dtype=np.uint64)
        self.np = np
        self.instr_cache = {}

    def find_word(self, w):
        np = self.np
        i = int(np.searchsorted(self.starts, np.uint64(w), side="right")) - 1
        if i >= 0 and w <= int(self.ends[i]):
            return i
        return -1

    def find_in(self, words):
        """words: np.uint64 array -> (position, row index) of the first word in a bytecode."""
        np = self.np
        if len(words) == 0 or len(self.starts) == 0:
            return -1, -1
        idx = np.searchsorted(self.starts, words, side="right") - 1
        ok = idx >= 0
        idx2 = np.where(ok, idx, 0)
        ok &= words <= self.ends[idx2]
        pos = np.flatnonzero(ok)
        if len(pos) == 0:
            return -1, -1
        p = int(pos[0])
        return p, int(idx2[p])


class Analyzer:
    def __init__(self, drv, comps, opmeta):
        from _taxonomy import Opcodes

        self.drv = drv
        self.owners = Owners(drv)
        self.comps = comps
        self.ops = Opcodes(opmeta.get("specializations", {}))
        self.opnames = opmeta.get("opnames", [])
        self.caches = opmeta.get("caches", {})
        self.codemap = CodeMap(drv["codes"]) if drv.get("codes") else None
        self.base_of = {}
        for base, specs in opmeta.get("specializations", {}).items():
            for s in specs:
                self.base_of[s] = base

    # ---- instruction decoding (plain mode: which instruction was executing) ----
    def _instrs(self, i):
        r = self.codemap.instr_cache.get(i)
        if r is None:
            raw = self.codemap.rows[i][6]
            starts = {}
            ends = {}
            off = 0
            n = len(raw)
            names = self.opnames
            while off + 1 < n:
                op = raw[off]
                name = names[op] if op < len(names) else f"<{op}>"
                base = self.base_of.get(name, name)
                ln = 2 * (1 + self.caches.get(base, 0))
                starts[off] = name
                ends[off + ln] = (off, name)
                off += ln
            r = self.codemap.instr_cache[i] = (starts, ends)
        return r

    def op_state(self, row_i, word, handler_op):
        """handler_op: base-stripped handler name. -> 'specialized' | 'unspecialized' |
        'deopt' | '?' (instruction not identified)."""
        fam, spec = self.ops.family(handler_op)
        if spec:
            return "specialized"
        start = self.codemap.rows[row_i][0]
        off = word - start
        starts, ends = self._instrs(row_i)
        name = starts.get(off)
        cand = []
        if name is not None:
            cand.append(name)
        e = ends.get(off)
        if e is not None:
            cand.append(e[1])
        for name in cand:
            b = self.base_of.get(name, name)
            if b == fam or name.startswith("INSTRUMENTED_") and name[13:] == fam:
                return "unspecialized" if name == fam else "deopt"
        return "?"

    def _dist_of_ext(self, comp, dso):
        d = self.owners.dist_files.get(dso)
        if d:
            return d
        top = comp[4:]
        return self.owners.pkgdist.get(top, top)

    # ---- classification of one sample ----
    def classify(self, frames, owner, kernel, word=None, row_i=None):
        """frames: [(sym, dso, comp)] leaf first, native frames up to (excluding) the
        innermost Python frame. owner: (group, name, code_label). -> key tuple."""
        from _taxonomy import (CALL_WRAPPERS, DEALLOC_CHAIN, GC_FRAMES, IMPORT_FRAMES, TAIL,
                               TRANSPARENT_COMPONENTS, base_name)

        group, oname, clabel = owner
        k = None
        op = ""
        for i, (s, _d, c) in enumerate(frames):
            if c in ("interp:op", "interp:eval", "perf-trampoline"):
                k = i
                if c == "interp:op":
                    op = base_name(s)[len(TAIL):]
                else:
                    op = "(eval)"
                break
        chain = frames if k is None else frames[:k]
        # effective component: skip transparent leaves
        eff = None
        for s, _d, c in frames:
            if c not in TRANSPARENT_COMPONENTS:
                eff = c
                break
        if eff is None:
            eff = frames[0][2] if frames else "unknown"
        leaf = base_name(frames[0][0]) if frames and frames[0][0] else (
            frames[0][1] if frames else "?")
        # entry: outermost non-wrapper native frame above the interpreter frame
        entry = ""
        entry_comp = ""
        if k is not None and k > 0:
            j = k - 1
            while j > 0 and base_name(chain[j][0] or "").startswith(CALL_WRAPPERS):
                j -= 1
            entry = base_name(chain[j][0] or "?")
            entry_comp = chain[j][2]
        elif k is None and frames:
            entry = base_name(frames[-1][0] or "?")
            entry_comp = frames[-1][2]
        where = "inline" if k == 0 or (k is None and eff.startswith("interp")) else "callee"
        bases = [base_name(s or "") for s, _d, _c in chain]
        comps = [c for _s, _d, c in chain]
        state = ""
        if op and k == 0 and word is not None and row_i is not None and op != "(eval)":
            state = self.op_state(row_i, word, op)
        elif op and op != "(eval)" and word is not None and row_i is not None:
            state = self.op_state(row_i, word, op)

        # ---- layer ----
        if "rt:gc" in comps or any(b.startswith(GC_FRAMES) for b in bases):
            layer = ("runtime", "gc")
        elif group == "import" or any(b in IMPORT_FRAMES for b in bases) or op in (
                "IMPORT_NAME", "IMPORT_FROM"):
            layer = ("runtime", "import")
        elif eff in ("rt:alloc", "rt:dealloc", "rt:ft", "rt:locks", "rt:startup", "rt:import"):
            layer = ("runtime", eff[3:])
        elif any(b in DEALLOC_CHAIN or b.endswith("dealloc") for b in bases):
            layer = ("runtime", "dealloc")
        elif group == "generated":
            layer = ("generated", oname)
        elif group == "stdlib":
            layer = ("stdlib", oname)
        elif group == "rustique":
            layer = ("stdlib", "rustique-glue")
        elif group == "child":
            layer = ("runtime", "startup")
        elif group in ("own", "thirdparty", "other", "unresolved", "harness", "none"):
            layer = None
            # the C entry point decides for code called from Python bytecode; without a
            # Python frame the outer frames are process entry / script runner: leaf decides
            scan = range(len(chain) - 1, -1, -1) if group not in ("none", "harness") else ()
            for idx in scan:
                s, d, c = chain[idx]
                b = bases[idx]
                if b.startswith("tuplegetter"):
                    layer = ("generated", "namedtuple")
                    break
                if c.startswith("cmod:"):
                    layer = ("stdlib", c[5:])
                    break
                if c.startswith("ext:"):
                    layer = ("thirdparty", self._dist_of_ext(c, d))
                    break
                if c == "rustique":
                    layer = ("stdlib", "rustique:" + rustique_module(s or ""))
                    break
            if layer is None:
                if eff.startswith("cmod:"):
                    layer = ("stdlib", eff[5:])
                elif eff.startswith("ext:"):
                    layer = ("thirdparty", self._dist_of_ext(eff, frames[0][1]))
                elif eff == "rustique":
                    layer = ("stdlib", "rustique:" + rustique_module(frames[0][0] or ""))
                elif group in ("unresolved", "none", "harness", "other"):
                    layer = ("other", group if group != "other" else "other:" + oname)
                elif eff.startswith(("interp", "perf-trampoline")):
                    layer = ("own", "") if group == "own" else ("thirdparty", oname)
                elif eff.startswith("lang:"):
                    layer = ("language", eff[5:])
                else:
                    layer = ("language", "other:" + eff)
        else:
            layer = ("other", group)
        return (layer[0], layer[1], group, oname, clabel, op, state, where, entry, entry_comp,
                leaf, eff, "k" if kernel else "")


KEY_FIELDS = ("layer", "sub", "group", "owner", "code", "op", "state", "where", "entry",
              "entry_comp", "leaf", "comp", "kernel")


def rustique_module(sym):
    s = sym.lower()
    for m in ("json", "deepcopy", "copy", "datetime", "strptime", "strftime", "posixpath",
              "pathlib", "fnmatch", "paths", "logging", "random", "textwrap", "urllib",
              "statistics", "float", "dtoa", "list", "tuple", "builtins", "dataclass", "sum",
              "sorted", "minmax"):
        if m in s:
            return m
    return "other"


def _resolve_word_plain(an, sample, unwound, k_frame):
    """Find the bytecode pointer: registers first (r15 = next_instr), then stack words from
    sp up to the innermost interpreter frame's own frame record. -> (row_i, word, how)."""
    np = an.codemap.np
    regs = sample.regs
    ri = sample.attr.reg_index
    order = ["r15", "r13", "r12", "r14", "bx", "ax", "cx", "dx", "si", "di", "r8", "r9", "r10",
             "r11", "bp"]
    for r in order:
        j = ri.get(r)
        if j is None:
            continue
        w = regs[j]
        i = an.codemap.find_word(w)
        if i >= 0:
            return i, w, "reg"
    stack = sample.stack
    if not stack:
        return -1, None, "none"
    sp = regs[ri["sp"]]
    nwords = len(stack) // 8
    how = "stack"
    if k_frame is not None:
        # own frame record of frame k is unwound[k+1][1]
        if k_frame + 1 < len(unwound) and unwound[k_frame + 1][1] is not None:
            lim = unwound[k_frame + 1][1] + 16 - sp
        elif unwound[k_frame][1] is not None:
            lim = unwound[k_frame][1] + 1024 - sp
        else:
            lim = 4096
        lim = max(0, min(nwords, lim // 8 + 1))
    else:
        lim = nwords
        how = "deep"
    words = np.frombuffer(stack, dtype="<u8", count=nwords)[:lim]
    p, i = an.codemap.find_in(words)
    if i >= 0:
        return i, int(words[p]), how
    if k_frame is not None and lim < nwords:
        words2 = np.frombuffer(stack, dtype="<u8", count=nwords)[lim:]
        p, i = an.codemap.find_in(words2)
        if i >= 0:
            return i, int(words2[p]), "deep"
    return -1, None, "none"


def analyze_plain(data_path, drv, comps, an, stats):
    from _perfdata import (KERNEL_START, PERF_CONTEXT_MAX, REC_MMAP, REC_MMAP2, REC_SAMPLE,
                           Maps, PerfData, Symbolizer, fp_unwind)

    pd = PerfData(data_path)
    maps = Maps()
    samples = []
    for rtype, start, stop in pd.records():
        if rtype in (REC_MMAP, REC_MMAP2):
            pid, addr, length, pgoff, name = pd.mmap_record(rtype, start)
            maps.add(pid, addr, length, pgoff, name)
        elif rtype == REC_SAMPLE:
            samples.append((start, stop))
    maps.finalize()
    sym = Symbolizer(maps)
    counter = Counter()
    res = Counter()
    main_pid = drv["pid"]
    for start, stop in samples:
        s = pd.sample(start, stop)
        period = s.period
        if s.pid != main_pid:
            res["other-process"] += period
            counter[("other", "child-process", "none", "", "", "", "", "", "", "", "", "", "")] \
                += period
            continue
        kernel = s.ip >= KERNEL_START
        unwound = fp_unwind(s)
        if not unwound:
            res["no-regs"] += period
            continue
        frames = []
        for n, (ip, _bp) in enumerate(unwound):
            name, dso = sym.sym(s.pid, ip, n > 0)
            frames.append((name, dso, comps.of(name, dso)))
        k = None
        for i, (_n, _d, c) in enumerate(frames):
            if c in ("interp:op", "interp:eval"):
                k = i
                break
        row_i, word, how = _resolve_word_plain(an, s, unwound, k)
        res[how] += period
        if row_i >= 0:
            row = an.codemap.rows[row_i]
            g, nm = an.owners.of(row[2], row[3], row[5])
            owner = (g, nm, _code_label(row[2], row[3], an.owners))
        else:
            owner = ("unresolved", "", "")
        chain = frames if k is None else frames[:k + 1]
        key = an.classify(chain, owner, kernel, word, row_i if row_i >= 0 else None)
        counter[key] += period
    pd.close()
    stats["resolution"] = dict(res)
    stats["samples"] = len(samples)
    return counter


def fixup_counter(counter):
    """Re-label owners that later versions of Owners classify differently (garbage names of
    frames being torn down, the __init__ shim frame, cattrs-generated code), then impute."""
    F = {f: i for i, f in enumerate(KEY_FIELDS)}
    out = Counter()
    for k, v in counter.items():
        if k[F["leaf"]] == "[unknown]" and k[F["comp"]] == "unknown" and k[F["layer"]] != "runtime":
            # the sampler got no user stack (bpf_get_stack failed): native part unknown
            nk = list(k)
            nk[F["layer"]], nk[F["sub"]] = "other", "no-native-stack"
            out[tuple(nk)] += v
            continue
        if k[F["group"]] == "other":
            o = k[F["owner"]]
            code = k[F["code"]]
            nk = list(k)
            if not o or not o.isprintable() or o == "__init__":
                nk[F["group"]], nk[F["owner"]] = "unresolved", ""
                if nk[F["layer"]] == "other":
                    nk[F["sub"]] = "unresolved"
                k = tuple(nk)
            elif o.startswith("<cattrs generated") or code.startswith("<cattrs generated"):
                nk[F["group"]], nk[F["owner"]] = "generated", "cattrs"
                if nk[F["layer"]] == "other":
                    nk[F["layer"]], nk[F["sub"]] = "generated", "cattrs"
                k = tuple(nk)
        out[k] += v
    return impute_unresolved(out)[0]


def impute_unresolved(counter):
    """Samples whose bytecode pointer was not found (mostly RETURN_VALUE / EXIT_INIT_CHECK
    between two frames) get the owner distribution of the resolved samples of the same opcode
    (else of all resolved interpreter samples). Owner-independent layers (runtime, C modules,
    extensions) were already assigned and are kept. Imputed keys carry code '(imputed)'."""
    F = {f: i for i, f in enumerate(KEY_FIELDS)}
    by_op = defaultdict(Counter)
    overall = Counter()
    for k, v in counter.items():
        g = k[F["group"]]
        if g in ("own", "thirdparty", "stdlib", "generated", "import", "rustique"):
            by_op[k[F["op"]]][(g, k[F["owner"]])] += v
            overall[(g, k[F["owner"]])] += v
    out = Counter()
    imputed = 0
    for k, v in counter.items():
        if k[F["group"]] != "unresolved" or k[F["layer"]] != "other":
            out[k] += v
            continue
        dist = by_op.get(k[F["op"]]) or overall
        tot = sum(dist.values())
        if not tot:
            out[k] += v
            continue
        imputed += v
        eff = k[F["comp"]]
        for (g, o), w in dist.items():
            if g == "own":
                lay = ("own", "") if eff.startswith(("interp", "perf")) else (
                    "language", eff[5:] if eff.startswith("lang:") else "other:" + eff)
            elif g == "thirdparty":
                lay = ("thirdparty", o) if eff.startswith(("interp", "perf")) else (
                    "language", eff[5:] if eff.startswith("lang:") else "other:" + eff)
            elif g == "stdlib":
                lay = ("stdlib", o)
            elif g == "generated":
                lay = ("generated", o)
            elif g == "rustique":
                lay = ("stdlib", "rustique-glue")
            else:
                lay = ("runtime", "import")
            nk = list(k)
            nk[F["layer"]], nk[F["sub"]] = lay
            nk[F["group"]], nk[F["owner"]], nk[F["code"]] = g, o, "(imputed)"
            out[tuple(nk)] += v * w / tot
    return out, imputed


def _code_label(fn, qual, owners):
    if fn.startswith(owners.stdlib):
        fn = fn[len(owners.stdlib):]
    elif "/site-packages/" in fn:
        fn = fn.split("/site-packages/", 1)[1]
    elif fn == owners.wl:
        fn = os.path.basename(fn)
    elif fn.startswith("/"):
        fn = "/".join(fn.split("/")[-2:])
    return f"{fn}:{qual}"


def _parse_py(name):
    s = name[4:]
    qual, _, fn = s.partition(":")
    return qual, fn


def analyze_xperf(data_path, map_path, drv, comps, an, stats):
    from _perfdata import (KERNEL_START, PERF_CONTEXT_MAX, REC_MMAP, REC_MMAP2, REC_SAMPLE,
                           Maps, PerfData, PerfMap, Symbolizer)

    pd = PerfData(data_path)
    maps = Maps()
    samples = []
    for rtype, start, stop in pd.records():
        if rtype in (REC_MMAP, REC_MMAP2):
            pid, addr, length, pgoff, name = pd.mmap_record(rtype, start)
            maps.add(pid, addr, length, pgoff, name)
        elif rtype == REC_SAMPLE:
            samples.append((start, stop))
    maps.finalize()
    pmaps = {}
    if map_path and os.path.exists(map_path):
        pmaps[drv["pid"]] = PerfMap(map_path)
    child_maps = set()
    sym = Symbolizer(maps, pmaps)
    counter = Counter()
    entries = Counter()
    pystack_res = Counter()
    main_pid = drv["pid"]
    for start, stop in samples:
        s = pd.sample(start, stop)
        period = s.period
        if not s.callchain:
            counter[("other", "no-chain", "none", "", "", "", "", "", "", "", "", "", "")] \
                += period
            continue
        child = s.pid != main_pid
        if child and s.pid not in pmaps:
            cm = f"/tmp/perf-{s.pid}.map"
            pmaps[s.pid] = PerfMap(cm) if os.path.exists(cm) else None
            child_maps.add(cm)
        ips = [ip for ip in s.callchain if ip < PERF_CONTEXT_MAX]
        kernel = bool(ips) and ips[0] >= KERNEL_START
        if kernel:
            ips = [ip for ip in ips if ip < KERNEL_START]
        frames = []
        py = []  # (index in frames, qual, file)
        for n, ip in enumerate(ips):
            name, dso = sym.sym(s.pid, ip, n > 0)
            if name and name.startswith("py::"):
                q, f = _parse_py(name)
                py.append((len(frames), q, f))
                frames.append((name, dso, "perf-trampoline"))
            else:
                frames.append((name, dso, comps.of(name, dso)))
        if py:
            i0, q, f = py[0]
            g, nm = an.owners.of(f, q)
            owner = (g, nm, _code_label(f, q, an.owners))
            chain = frames[:i0 + 1]
        else:
            owner = ("child" if child else "none", "", "")
            chain = frames
        key = an.classify(chain, owner, kernel)
        counter[key] += period
        pystack_res[("child-" if child else "") + ("py" if py else "nopy")] += period
        if child:
            continue
        # caller -> callee entry: innermost own/thirdparty frame and what it called
        groups = [an.owners.of(f, q) for _i, q, f in py]
        for j, (gg, nn) in enumerate(groups):
            if gg in ("own", "thirdparty"):
                caller = (gg, nn)
                if j == 0:
                    callee = ("bytecode" if key[7] == "inline" else "native",
                              key[8] if key[7] != "inline" else "", key[0], key[1], key[5])
                else:
                    cg, cn = groups[j - 1]
                    q = py[j - 1][1]
                    callee = ("python", f"{cn}:{q}" if cn else q, cg, cn, "")
                entries[(caller, callee)] += period
                break
    pd.close()
    for cm in child_maps:
        try:
            os.unlink(cm)
        except OSError:
            pass
    stats["samples"] = len(samples)
    stats["pystack"] = dict(pystack_res)
    return counter, entries


BPF_MAP_KEYS = 262144  # distinct (thread, frame, caller frames, user stack) keys per run
BPF_PROG = r"""
hardware:cycles:{period}
/pid == {pid}/
{{
  $ts = *(uint64 *)uptr({interp} + {threads_head});
  $found = (uint64)0;
  $i = 0;
  while ($i < 64) {{
    if ($ts == 0) {{ break; }}
    if (*(uint64 *)uptr($ts + {native_tid}) == (uint64)tid) {{ $found = $ts; break; }}
    $ts = *(uint64 *)uptr($ts + {ts_next});
    $i++;
  }}
  $code = (uint64)0;
  $ip = (uint64)0;
  $p = (uint64)0;
  $c1 = (uint64)0; $c2 = (uint64)0; $c3 = (uint64)0; $c4 = (uint64)0; $c5 = (uint64)0;
  $c6 = (uint64)0;
  if ($found != 0) {{
    $fr = *(uint64 *)uptr($found + {cur_frame});
    if ($fr != 0) {{
      $code = *(uint64 *)uptr($fr + {executable});
      $code = $code - ($code & 3);
      $ip = *(uint64 *)uptr($fr + {instr_ptr});
      $p = *(uint64 *)uptr($fr + {previous});
    if ($p != 0) {{ $c1 = *(uint64 *)uptr($p + {executable}); $p = *(uint64 *)uptr($p + {previous}); }}
    if ($p != 0) {{ $c2 = *(uint64 *)uptr($p + {executable}); $p = *(uint64 *)uptr($p + {previous}); }}
    if ($p != 0) {{ $c3 = *(uint64 *)uptr($p + {executable}); $p = *(uint64 *)uptr($p + {previous}); }}
    if ($p != 0) {{ $c4 = *(uint64 *)uptr($p + {executable}); $p = *(uint64 *)uptr($p + {previous}); }}
    if ($p != 0) {{ $c5 = *(uint64 *)uptr($p + {executable}); $p = *(uint64 *)uptr($p + {previous}); }}
    if ($p != 0) {{ $c6 = *(uint64 *)uptr($p + {executable}); $p = *(uint64 *)uptr($p + {previous}); }}
    }}
  }}
  $k = (uint64)0;
  if ((reg("ip") >> 47) > 0) {{ $k = (uint64)1; }}
  @s[tid, $code, $ip, $k, $c1, $c2, $c3, $c4, $c5, $c6, ustack(raw, 48)] = count();
  if ($code != 0) {{
    $fn = *(uint64 *)uptr($code + {code_filename});
    $qn = *(uint64 *)uptr($code + {code_qualname});
    @f[$code] = str(uptr($fn + {ascii_data}), 160);
    @q[$code] = str(uptr($qn + {ascii_data}), 96);
  }}
}}
"""


def _record_bpf(py, workload, tmpdir, tag, seconds, enable, period, cpus=None,
                system_wide=False):
    """Steady state sampled by an eBPF program on the hardware cycles event: per sample, the
    user stack (frame pointers) and the thread's current Python frame read through
    _Py_DebugOffsets (tstate->current_frame->f_executable / instr_ptr). No PEP 523 hook:
    the interpreter runs exactly as in production."""
    ready = os.path.join(tmpdir, f"ready-{tag}")
    go = os.path.join(tmpdir, f"go-{tag}")
    for p_ in (ready, go):
        if os.path.exists(p_):
            os.unlink(p_)
        os.mkfifo(p_)
    out = os.path.join(tmpdir, f"{tag}.drv")
    bt_out = os.path.join(tmpdir, f"{tag}.bt")
    env = dict(os.environ, RQ_SYNC=f"{ready},{go}")
    if os.environ.get("RQ_EXTRA_PYTHONPATH"):
        env["PYTHONPATH"] = os.environ["RQ_EXTRA_PYTHONPATH"]
    cmd = [py, os.path.abspath(__file__), "driver", workload, "--mode", "bpf", "--out", out,
           "--seconds", str(seconds)]
    if enable:
        cmd.append("--enable")
    if cpus:
        cmd = ["taskset", "-c", cpus] + cmd
    drv_err = open(os.path.join(tmpdir, f"{tag}.drv.err"), "w")
    proc = subprocess.Popen(cmd, env=env, stdout=drv_err, stderr=subprocess.STDOUT)
    bt = None
    try:
        with open(ready) as f:
            line = f.readline()
        if not line:
            try:
                proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                pass
            drv_err.flush()
            raise RuntimeError("driver died before ready: " +
                               open(drv_err.name).read()[-3000:])
        info = json.loads(line)
        o = info["offsets"]
        prog = BPF_PROG.format(period=period, pid=info["pid"], interp=info["interp"],
                               threads_head=o["threads_head"], native_tid=o["native_thread_id"],
                               ts_next=o["ts_next"], cur_frame=o["current_frame"],
                               executable=o["frame_executable"], instr_ptr=o["frame_instr_ptr"],
                               previous=o["frame_previous"], code_filename=o["code_filename"],
                               code_qualname=o["code_qualname"], ascii_data=o["ascii_data"])
        prog_path = os.path.join(tmpdir, f"{tag}.bt.prog")
        with open(prog_path, "w") as f:
            f.write(prog)
        bt_cmd = ["sudo", "-n", "env", f"BPFTRACE_MAX_MAP_KEYS={BPF_MAP_KEYS}",
                  "BPFTRACE_MAX_STRLEN=160", "bpftrace", "-o", bt_out]
        if not system_wide:  # task-bound counters (main thread only); else every CPU
            bt_cmd += ["-p", str(info["pid"])]
        bt = subprocess.Popen(bt_cmd + [prog_path],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        t0 = time.time()
        while True:  # wait for the probes to attach
            if os.path.exists(bt_out) and "Attaching" in open(bt_out, errors="replace").read():
                break
            if bt.poll() is not None:
                raise RuntimeError("bpftrace failed: " + bt.stderr.read()[-3000:])
            if time.time() - t0 > 60:
                raise RuntimeError("bpftrace did not attach")
            time.sleep(0.05)
        time.sleep(0.2)
        with open(go, "w") as f:
            f.write("go\n")
        with open(ready) as f:
            f.readline()  # "done"
        subprocess.run(["sudo", "-n", "kill", "-INT", str(bt.pid)])
        # sudo relays SIGINT to bpftrace, which prints its maps and exits
        try:
            bt.wait(timeout=120)
        except subprocess.TimeoutExpired:
            subprocess.run(["sudo", "-n", "pkill", "-INT", "-f", prog_path])
            bt.wait(timeout=60)
        with open(go, "w") as f:
            f.write("fin\n")
        proc.wait(timeout=600)
    finally:
        if proc.poll() is None:
            proc.kill()
        if bt is not None and bt.poll() is None:
            subprocess.run(["sudo", "-n", "pkill", "-KILL", "-f", os.path.join(tmpdir, tag)])
        drv_err.close()
    if proc.returncode != 0 or not os.path.exists(out):
        raise RuntimeError("driver failed: " + open(drv_err.name).read()[-3000:])
    with open(out, "rb") as f:
        drv = pickle.load(f)
    return bt_out, drv, info


NKEY = 10  # tid, code, instr_ptr, kernel, 6 caller code objects
_NAME_RE = re.compile(r"^@([fq])\[(\d+)\]: (.*)$")


def parse_bpf_names(path):
    """@f/@q[code] = co_filename / co_qualname as last seen at that address (ASCII only)."""
    fq = {"f": {}, "q": {}}
    with open(path, errors="replace") as f:
        for line in f:
            if line.startswith(("@f[", "@q[")):
                m = _NAME_RE.match(line.rstrip("\n"))
                if m:
                    fq[m.group(1)][int(m.group(2))] = m.group(3)
    return {c: (fn, fq["q"].get(c, "?")) for c, fn in fq["f"].items()}


def parse_bpftrace(path):
    """-> list of [tid, code, ip, kernel, c1..c6, [stack addresses leaf first], count]."""
    out = []
    cur = None
    with open(path, errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("@s["):
                head = line[3:]
                tail_cnt = None
                if "]:" in head:
                    head, _, c = head.rpartition("]:")
                    tail_cnt = int(c.strip())
                parts = [x.strip() for x in head.split(",")]
                nf = NKEY if len(parts) > NKEY else 4
                nums = [int(x) for x in parts[:nf] if x]
                cur = (nums, [])
                for x in parts[nf:]:
                    for t in x.split():
                        try:
                            cur[1].append(int(t, 16))
                        except ValueError:
                            pass
                if tail_cnt is not None:
                    out.append((cur[0] + [0] * NKEY)[:NKEY] + [cur[1], tail_cnt])
                    cur = None
                continue
            if cur is None:
                continue
            if line.startswith("]:"):
                cnt = int(line[2:].strip())
                out.append((cur[0] + [0] * NKEY)[:NKEY] + [cur[1], cnt])
                cur = None
                continue
            t = line.strip()
            if t:
                try:
                    cur[1].append(int(t, 16))
                except ValueError:
                    pass
    return out


def maps_from_proc(text, pid):
    from _perfdata import Maps

    m = Maps()
    for line in text.splitlines():
        p_ = line.split(None, 5)
        if len(p_) < 6 or not p_[5].startswith("/"):
            continue
        a, b = (int(x, 16) for x in p_[0].split("-"))
        m.add(pid, a, b - a, int(p_[2], 16), p_[5].strip())
    m.finalize()
    return m


def analyze_bpf(bt_out, drv, comps, an, stats, period):
    from _perfdata import Symbolizer

    entries = parse_bpftrace(bt_out)
    names = parse_bpf_names(bt_out)
    pid = drv["pid"]
    sym = Symbolizer(maps_from_proc(drv.get("proc_maps", ""), pid))
    bs = drv.get("code_basicsize", 208)
    by_id = {}
    if an.codemap is not None:
        for i, row in enumerate(an.codemap.rows):
            by_id[row[0] - bs] = i
    counter = Counter()
    ent = Counter()
    res = Counter()
    nsamp = 0
    for e in entries:
        tid, code, ip, kernel = e[:4]
        callers = e[4:NKEY]
        stack, cnt = e[NKEY], e[NKEY + 1]
        v = cnt * period
        nsamp += cnt
        frames = []
        for n, addr in enumerate(stack):
            if addr >= 0xFFFF800000000000:
                continue
            name, dso = sym.sym(pid, addr, n > 0)
            frames.append((name, dso, comps.of(name, dso)))
        if not frames:
            frames = [(None, "[unknown]", "unknown")]
        k = None
        for i, (_n, _d, c) in enumerate(frames):
            if c in ("interp:op", "interp:eval"):
                k = i
                break
        row_i = by_id.get(code, -1) if code else -1
        if row_i >= 0:
            row = an.codemap.rows[row_i]
            g, nm = an.owners.of(row[2], row[3], row[5])
            owner = (g, nm, _code_label(row[2], row[3], an.owners))
            res["code"] += v
            word = ip if row[0] <= ip <= row[0] + row[1] else None
            if word is None:
                res["ip-outside"] += v
        elif code and code in names:
            fn_, qn_ = names[code]
            g, nm = an.owners.of(fn_, qn_)
            owner = (g, nm, _code_label(fn_, qn_, an.owners))
            res["code-by-name"] += v
            word = None
        else:
            owner = ("unresolved" if code else "none", "", "")
            res["unknown-code" if code else "no-frame"] += v
            word = None
        if k is None and not code:
            owner = ("none", "", "")
        chain = frames if k is None else frames[:k + 1]
        key = an.classify(chain, owner, bool(kernel), word, row_i if word is not None else None)
        counter[key] += v
        # caller -> callee entry from the Python stack (innermost own/thirdparty frame)
        pys = [(owner[0], owner[1], row_i)]
        for c in callers:
            c = c - (c & 3)
            ri = by_id.get(c, -1) if c else -1
            if ri >= 0:
                r2 = an.codemap.rows[ri]
                pys.append(an.owners.of(r2[2], r2[3], r2[5]) + (ri,))
            elif c in names:
                pys.append(an.owners.of(*names[c]) + (-1,))
            elif c == 0:
                break
        for j, (gg, nn, ri) in enumerate(pys):
            if gg in ("own", "thirdparty"):
                if j == 0:
                    callee = ("bytecode" if key[7] == "inline" else "native",
                              key[8] if key[7] != "inline" else "", key[0], key[1], key[5])
                else:
                    cg, cn, cri = pys[j - 1]
                    q = an.codemap.rows[cri][3] if cri is not None and cri >= 0 else "?"
                    if cri == -1:
                        cc = callers[j - 2] if j >= 2 else code
                        q = names.get(cc - (cc & 3), ("", "?"))[1]
                    callee = ("python", f"{cn}:{q}" if cn else q, cg, cn, "")
                ent[((gg, nn), callee)] += v
                break
    stats["resolution"] = dict(res)
    stats["samples"] = nsamp
    stats["period"] = period
    stats["keys"] = len(entries)
    if len(entries) >= 0.95 * BPF_MAP_KEYS:
        stats["warning"] = "bpftrace map nearly full: samples lost"
    tot = sum(counter.values())
    if drv.get("cpu_s"):
        # sampled cycles per CPU-second of the loop: ~ the clock (3.3-3.7 GHz) when no
        # sample was lost; lower means lost samples (full maps, BPF recursion guard)
        stats["sampled_ghz"] = round(tot / drv["cpu_s"] / 1e9, 3)
    return counter, ent


def analyze_whole(data_path, map_path, drv, comps, an, stats):
    """Whole-process profile (fp callchains, -X perf or not): phases from the driver's
    CLOCK_MONOTONIC marks; layer per sample as in steady state, plus phase."""
    from _perfdata import (KERNEL_START, PERF_CONTEXT_MAX, REC_MMAP, REC_MMAP2, REC_SAMPLE,
                           Maps, PerfData, PerfMap, Symbolizer)
    from _taxonomy import FINALIZE_FRAMES, STARTUP_FRAMES, base_name

    pd = PerfData(data_path)
    maps = Maps()
    samples = []
    for rtype, start, stop in pd.records():
        if rtype in (REC_MMAP, REC_MMAP2):
            pid, addr, length, pgoff, name = pd.mmap_record(rtype, start)
            maps.add(pid, addr, length, pgoff, name)
        elif rtype == REC_SAMPLE:
            samples.append((start, stop))
    maps.finalize()
    pmaps = {}
    if map_path and os.path.exists(map_path):
        pmaps[drv["pid"]] = PerfMap(map_path)
    sym = Symbolizer(maps, pmaps)
    marks = drv["marks"]
    bounds = [("startup", marks["driver_start"]), ("import", marks["imported"]),
              ("setup", marks["setup"]), ("run", marks["first_run"])]
    counter = Counter()
    main_pid = drv["pid"]
    for start, stop in samples:
        s = pd.sample(start, stop)
        period = s.period
        if s.pid != main_pid or not s.callchain:
            continue
        ips = [ip for ip in s.callchain if ip < PERF_CONTEXT_MAX]
        kernel = bool(ips) and ips[0] >= KERNEL_START
        if kernel:
            ips = [ip for ip in ips if ip < KERNEL_START]
        frames = []
        py = []
        for n, ip in enumerate(ips):
            name, dso = sym.sym(s.pid, ip, n > 0)
            if name and name.startswith("py::"):
                q, f = _parse_py(name)
                py.append((len(frames), q, f))
                frames.append((name, dso, "perf-trampoline"))
            else:
                frames.append((name, dso, comps.of(name, dso)))
        phase = "finalize"
        for ph, t in bounds:
            if s.time < t:
                phase = ph
                break
        bases = [base_name(n or "") for n, _d, _c in frames]
        if any(b.startswith(FINALIZE_FRAMES) for b in bases):
            phase = "finalize"
        elif phase == "startup" and any(b.startswith(STARTUP_FRAMES) for b in bases):
            phase = "startup"
        if py:
            i0, q, f = py[0]
            g, nm = an.owners.of(f, q)
            owner = (g, nm, _code_label(f, q, an.owners))
            chain = frames[:i0 + 1]
        else:
            owner = ("none", "", "")
            chain = frames
        key = an.classify(chain, owner, kernel)
        counter[(phase,) + key] += period
    pd.close()
    stats["samples"] = len(samples)
    return counter


# =========================================================================================
# summaries
# =========================================================================================

LAYERS = ("own", "generated", "language", "stdlib", "thirdparty", "runtime", "other")


def summarize(counter, top=40):
    tot = sum(counter.values()) or 1
    sh = lambda v: round(v / tot, 5)  # noqa: E731
    lay = Counter()
    sub = Counter()
    grp = Counter()
    owner_layer = Counter()
    matrix = Counter()
    ops = defaultdict(Counter)       # group -> (op) -> cycles (inline + callee)
    op_inline = defaultdict(Counter)
    op_callee_layer = defaultdict(Counter)   # (group, op) -> layer:sub of callee time
    op_state = defaultdict(Counter)  # group -> state
    lang_entry = Counter()
    lang_by_group = Counter()
    third_dist = Counter()
    third_inclusive = defaultdict(Counter)   # dist -> layer:sub
    code = Counter()
    leaf = Counter()
    entries = Counter()
    kernel = 0
    for key, v in counter.items():
        k = dict(zip(KEY_FIELDS, key))
        lay[k["layer"]] += v
        sub[(k["layer"], k["sub"])] += v
        grp[k["group"]] += v
        owner_layer[(k["group"], k["layer"])] += v
        matrix[(k["group"] + (":" + k["owner"] if k["owner"] else ""), k["comp"])] += v
        if k["kernel"]:
            kernel += v
        if k["op"] and k["group"] in ("own", "thirdparty", "stdlib", "generated"):
            g = k["group"]
            ops[g][k["op"]] += v
            if k["where"] == "inline":
                op_inline[g][k["op"]] += v
            else:
                op_callee_layer[(g, k["op"])][k["layer"] + ":" + k["sub"]] += v
            if k["state"]:
                op_state[g][(k["op"], k["state"])] += v
        if k["layer"] == "language":
            lang_entry[(k["sub"], k["entry"] or k["leaf"], k["op"])] += v
            lang_by_group[(k["group"], k["sub"])] += v
        if k["group"] == "thirdparty":
            third_inclusive[k["owner"]][k["layer"] + ":" + k["sub"]] += v
        if k["layer"] == "thirdparty":
            third_dist[k["sub"]] += v
        if k["code"]:
            code[(k["layer"], k["code"])] += v
        leaf[(k["leaf"], k["comp"])] += v
        if k["entry"]:
            entries[(k["entry"], k["entry_comp"], k["layer"])] += v
    out = {
        "layers": {l: sh(lay.get(l, 0)) for l in LAYERS},
        "sublayers": {f"{a}:{b}": sh(v) for (a, b), v in sub.most_common()},
        "owner_groups": {g: sh(v) for g, v in grp.most_common()},
        "owner_x_layer": {f"{g}->{l}": sh(v) for (g, l), v in owner_layer.most_common()},
        "matrix": [{"owner": o, "component": c, "share": sh(v)}
                   for (o, c), v in matrix.most_common(80)],
        "kernel_share": sh(kernel),
        "opcodes": {},
        "language_entries": [{"category": a, "entry": b, "op": c, "share": sh(v)}
                             for (a, b, c), v in lang_entry.most_common(top)],
        "language_by_owner": {f"{g}:{c}": sh(v) for (g, c), v in lang_by_group.most_common()},
        "thirdparty_dists": {d: sh(v) for d, v in third_dist.most_common()},
        "thirdparty_inclusive": {d: {"total": sh(sum(c.values())),
                                     "by_layer": {x: sh(y) for x, y in c.most_common(12)}}
                                 for d, c in sorted(third_inclusive.items(),
                                                    key=lambda kv: -sum(kv[1].values()))},
        "top_code": [{"layer": a, "code": b, "share": sh(v)} for (a, b), v in code.most_common(top)],
        "top_leaf": [{"symbol": a, "component": b, "share": sh(v)}
                     for (a, b), v in leaf.most_common(top)],
        "top_entries": [{"entry": a, "component": b, "layer": c, "share": sh(v)}
                        for (a, b, c), v in entries.most_common(top)],
    }
    for g in ops:
        rows = []
        for op, v in ops[g].most_common():
            rows.append({"op": op, "share": sh(v), "inline": sh(op_inline[g].get(op, 0)),
                         "callee": {x: sh(y) for x, y in op_callee_layer[(g, op)].most_common(6)}})
        st = Counter()
        for (op, state), v in op_state[g].items():
            st[state] += v
        out["opcodes"][g] = {"total": sh(sum(ops[g].values())), "rows": rows[:60],
                             "state": {s: sh(v) for s, v in st.most_common()},
                             "state_by_op": [{"op": o, "state": s, "share": sh(v)}
                                             for (o, s), v in op_state[g].most_common(40)]}
    return out


def summarize_entries(entries, tot, top=60):
    sh = lambda v: round(v / tot, 5)  # noqa: E731
    by_caller = defaultdict(Counter)
    for (caller, callee), v in entries.items():
        by_caller[caller][callee] += v
    out = {}
    for caller, c in sorted(by_caller.items(), key=lambda kv: -sum(kv[1].values())):
        name = caller[0] + (":" + caller[1] if caller[1] else "")
        out[name] = {"total": sh(sum(c.values())),
                     "callees": [{"kind": a, "callee": b, "layer": f"{cl}:{cs}", "op": op,
                                  "share": sh(v)}
                                 for (a, b, cl, cs, op), v in c.most_common(top)]}
    return out


def counter_to_rows(counter):
    return [[list(k), v] for k, v in counter.items()]


def rows_to_counter(rows):
    return Counter({tuple(k): v for k, v in rows})


# =========================================================================================
# orchestration
# =========================================================================================


def _driver_cmd(py, workload, mode, out, seconds, enable, perf_x):
    cmd = [py] + (["-X", "perf"] if perf_x else []) + [os.path.abspath(__file__), "driver",
                                                       workload, "--mode", mode, "--out", out,
                                                       "--seconds", str(seconds)]
    if enable:
        cmd.append("--enable")
    return cmd


def _record(py, workload, tmpdir, tag, mode, seconds, enable, perf_x, callgraph, freq,
            cpus=None):
    ctl = os.path.join(tmpdir, f"ctl-{tag}")
    ack = os.path.join(tmpdir, f"ack-{tag}")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    data = os.path.join(tmpdir, f"{tag}.data")
    out = os.path.join(tmpdir, f"{tag}.drv")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    if os.environ.get("RQ_EXTRA_PYTHONPATH"):
        env["PYTHONPATH"] = os.environ["RQ_EXTRA_PYTHONPATH"]
    if perf_x:
        env["PYTHONPERFSUPPORT"] = "1"  # child Python processes write perf maps too
    if mode == "whole":
        env.pop("RQ_PERF_CTL")
        env.pop("RQ_PERF_ACK")
    cmd = ["perf", "record", "-q", "-e", "cycles", "-F", str(freq), "-k", "CLOCK_MONOTONIC",
           "-B", "-N", "-o", data] + callgraph
    if mode == "steady":
        cmd += ["-D", "-1", "--control", f"fifo:{ctl},{ack}"]
    if mode == "whole":
        marks_path = os.path.join(tmpdir, f"{tag}.marks")
        cmd += ["--", py] + (["-X", "perf"] if perf_x else []) + [
            os.path.join(HERE, "_whole.py"), workload, marks_path]
    else:
        cmd += ["--"] + _driver_cmd(py, workload, mode, out, seconds, enable, perf_x)
    if cpus:
        cmd = ["taskset", "-c", cpus] + cmd
    r = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if mode == "whole":
        if r.returncode != 0 or not os.path.exists(marks_path):
            raise RuntimeError(f"perf record failed ({tag}): {r.stderr[-3000:]}")
        subprocess.run([py, os.path.abspath(__file__), "driver", workload, "--mode", "envinfo",
                        "--out", out], check=True, env=env)
        with open(out, "rb") as f:
            drv = pickle.load(f)
        pid, t0, t1, t2, t3 = (int(x) for x in open(marks_path).read().split())
        drv["pid"] = pid
        drv["marks"] = {"driver_start": t0, "imported": t1, "setup": t2, "first_run": t3}
        drv["first_run_s"] = (t3 - t2) / 1e9
    elif r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f"perf record failed ({tag}): {r.stderr[-3000:]}")
    else:
        with open(out, "rb") as f:
            drv = pickle.load(f)
    pmap = f"/tmp/perf-{drv['pid']}.map"
    keep_map = None
    if os.path.exists(pmap):
        keep_map = os.path.join(tmpdir, f"{tag}.map")
        shutil.move(pmap, keep_map)
    return data, drv, keep_map


def _components(a):
    sys.path.insert(0, HERE)
    from _taxonomy import Components

    return Components(a.cpython_src, os.path.join(a.cache, "cpython_src_index.json"))


def profile_main(a):
    sys.path.insert(0, HERE)
    wl = os.path.abspath(a.workload)
    name = os.path.splitext(os.path.basename(wl))[0]
    family = os.path.basename(os.path.dirname(wl))
    tmpdir = tempfile.mkdtemp(prefix=f"rqlayers-{name}-", dir=a.tmp)
    t_all = time.time()

    def log(*x):
        print(f"[{family}/{name} {time.time() - t_all:6.1f}s]", *x, file=sys.stderr, flush=True)

    comps = _components(a)
    result = {"workload": f"{family}/{name}", "python_arg": a.python, "enabled": a.enable,
              "method": {}}
    modes = a.modes.split(",")
    try:
        if "bpf" in modes:
            bt_out, drv, info = _record_bpf(a.python, wl, tmpdir, "bpf", a.seconds, a.enable,
                                            a.period, a.cpus, a.system_wide)
            an = Analyzer(drv, comps, drv["opcode"])
            stats = {}
            cnt, ent = analyze_bpf(bt_out, drv, comps, an, stats, a.period)
            tot = sum(cnt.values())
            cnt, imputed = impute_unresolved(cnt)
            stats["imputed_share"] = round(imputed / (tot or 1), 5)
            result["python"] = drv["env"]["python"]
            result["runs"] = len(drv["walls"])
            result["wall_median_s"] = round(statistics.median(drv["walls"]), 5)
            result["cycles_per_run"] = round(tot / max(1, len(drv["walls"])))
            result["digest_ok"] = drv["digest_ok"]
            result["bpf"] = {"stats": stats, "summary": summarize(cnt),
                             "counter": counter_to_rows(cnt), "codes": len(drv["codes"]),
                             "offsets": info["offsets"], "runs": len(drv["walls"]),
                             "cpu_s_per_run": (drv.get("cpu_s") or 0) / max(1, len(drv["walls"])),
                             "entries": summarize_entries(ent, tot or 1),
                             "entry_counter": [[list(k[0]), list(k[1]), v]
                                               for k, v in ent.items()],
                             "wall_median_s": round(statistics.median(drv["walls"]), 5),
                             "cycles_per_run": round(tot / max(1, len(drv["walls"])))}
            log("bpf", stats, f"{tot / 1e9:.2f} Gcycles")
            if a.keep:
                os.makedirs(a.keep, exist_ok=True)
                shutil.copy(bt_out, os.path.join(a.keep, f"{name}.bt"))
        if "plain" in modes:
            data, drv, _ = _record(a.python, wl, tmpdir, "plain", "steady", a.seconds, a.enable,
                                   False, ["--call-graph", f"dwarf,{a.stack}"], a.freq, a.cpus)
            an = Analyzer(drv, comps, drv["opcode"])
            stats = {}
            cnt = analyze_plain(data, drv, comps, an, stats)
            tot = sum(cnt.values())
            cnt, imputed = impute_unresolved(cnt)
            stats["imputed_share"] = round(imputed / (tot or 1), 5)
            result.setdefault("python", drv["env"]["python"])
            result.setdefault("runs", len(drv["walls"]))
            result.setdefault("wall_median_s", round(statistics.median(drv["walls"]), 5))
            result.setdefault("cycles_per_run", round(tot / max(1, len(drv["walls"]))))
            result.setdefault("digest_ok", drv["digest_ok"])
            result["plain"] = {"stats": stats, "summary": summarize(cnt),
                               "counter": counter_to_rows(cnt), "codes": len(drv["codes"]),
                               "runs": len(drv["walls"]),
                               "wall_median_s": round(statistics.median(drv["walls"]), 5),
                               "cycles_per_run": round(tot / max(1, len(drv["walls"])))}
            log("plain", stats, f"{tot / 1e9:.2f} Gcycles")
            if a.keep:
                os.makedirs(a.keep, exist_ok=True)
                shutil.copy(data, os.path.join(a.keep, f"{name}.plain.data"))
            os.unlink(data)
        if "xperf" in modes:
            data, drv, pmap = _record(a.python, wl, tmpdir, "xperf", "steady", a.seconds,
                                      a.enable, True, ["-g"], a.freq, a.cpus)
            an = Analyzer(drv, comps, drv["opcode"])
            stats = {}
            cnt, ent = analyze_xperf(data, pmap, drv, comps, an, stats)
            tot = sum(cnt.values())
            result.setdefault("python", drv["env"]["python"])
            result["xperf"] = {"stats": stats, "summary": summarize(cnt),
                               "entries": summarize_entries(ent, tot or 1),
                               "counter": counter_to_rows(cnt),
                               "entry_counter": [[list(k[0]), list(k[1]), v]
                                                 for k, v in ent.items()],
                               "runs": len(drv["walls"]),
                               "wall_median_s": round(statistics.median(drv["walls"]), 5),
                               "cycles_per_run": round(tot / max(1, len(drv["walls"])))}
            log("xperf", stats)
            if a.keep:
                shutil.copy(data, os.path.join(a.keep, f"{name}.xperf.data"))
                if pmap:
                    shutil.copy(pmap, os.path.join(a.keep, f"{name}.xperf.map"))
            os.unlink(data)
        for wmode in [m for m in ("whole", "wholeplain") if m in modes]:
            data, drv, pmap = _record(a.python, wl, tmpdir, wmode, "whole", 0, a.enable,
                                      wmode == "whole", ["-g"], a.whole_freq, a.cpus)
            an = Analyzer(drv, comps, drv["opcode"])
            stats = {}
            cnt = analyze_whole(data, pmap, drv, comps, an, stats)
            tot = sum(cnt.values())
            phases = Counter()
            for k, v in cnt.items():
                phases[k[0]] += v
            per_phase = {}
            for ph in phases:
                sub = Counter({k[1:]: v for k, v in cnt.items() if k[0] == ph})
                per_phase[ph] = summarize(sub, top=15)
            result[wmode] = {"stats": stats, "cycles": tot,
                               "phases": {p: round(v / tot, 5) for p, v in phases.most_common()},
                               "phase_cycles": dict(phases),
                               "per_phase": per_phase,
                               "summary": summarize(Counter({k[1:]: v for k, v in cnt.items()})),
                               "counter": [[list(k), v] for k, v in cnt.items()],
                               "first_run_s": drv["first_run_s"]}
            log(wmode, stats, {p: round(v / tot, 3) for p, v in phases.items()})
            os.unlink(data)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
    result["method"] = {
        "event": f"cycles (user+kernel), -F {a.freq}",
        "plain": f"--call-graph dwarf,{a.stack}; owner from the bytecode pointer in registers / "
                 "the dumped stack; frame-pointer unwind of the dumped stack",
        "xperf": "-g (frame pointers) with python -X perf; owner = innermost py:: trampoline",
        "whole": f"-g, -X perf, -F {a.whole_freq}, whole process, phases from CLOCK_MONOTONIC marks",
    }
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    opener = gzip.open if a.out.endswith(".gz") else open
    with opener(a.out, "wt") as f:
        json.dump(result, f)
    log("->", a.out)


def load_profile(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        return json.load(f)


# =========================================================================================
# validation: known cases
# =========================================================================================

CASES = os.path.join(HERE, "cases")


def validate_main(a):
    """Profile the known cases and check the expected attribution."""
    results = {}
    ok_all = True
    for case, check in (("json_dumps", _check_json), ("dataclass_init", _check_dataclass),
                        ("gc_heavy", _check_gc), ("calls_attrs", _check_calls)):
        out = os.path.join(a.outdir, f"case_{case}.json.gz")
        args = argparse.Namespace(**vars(a))
        args.workload = os.path.join(CASES, case + ".py")
        args.out = out
        profile_main(args)
        prof = load_profile(out)
        ok, msg = check(prof)
        ok_all &= ok
        results[case] = {"ok": ok, "detail": msg}
        print(f"{case:16s} {'OK ' if ok else 'BAD'} {msg}", flush=True)
    with open(os.path.join(a.outdir, "validation.json"), "w") as f:
        json.dump(results, f, indent=1)
    sys.exit(0 if ok_all else 1)


def _incl(prof, mode, pred):
    cnt = rows_to_counter(prof[mode]["counter"])
    tot = sum(cnt.values()) or 1
    return sum(v for k, v in cnt.items() if pred(dict(zip(KEY_FIELDS, k)))) / tot


def _check_json(prof):
    # >95% of samples have an encoder_* frame between the leaf and the interpreter, i.e. the
    # sample is under _json's encoder (entry encoder_call / leaf encoder_*), owner stdlib json
    msgs = []
    ok = True
    for mode in ("bpf", "plain", "xperf"):
        if mode not in prof:
            continue
        under = _incl(prof, mode, lambda k: k["entry"].startswith("encoder_") or
                      k["leaf"].startswith(("encoder_", "py_encode_basestring")) or
                      (k["layer"] == "stdlib" and k["sub"] in ("json", "json.encoder")))
        ok &= under > 0.95
        msgs.append(f"{mode}: under encoder/json {under:.3f}")
    return ok, "; ".join(msgs)


def _check_dataclass(prof):
    msgs = []
    ok = True
    for mode in ("bpf", "plain", "xperf"):
        if mode not in prof:
            continue
        gen = _incl(prof, mode, lambda k: k["group"] == "generated" and k["owner"] == "dataclasses")
        lay = prof[mode]["summary"]["layers"]
        if mode == "bpf":  # frame-exact: the generated __init__ owns >40%; the allocating
            # CALL_ALLOC_AND_ENTER_INIT runs in the caller's frame (own)
            alloc_own = _incl(prof, mode, lambda k: k["op"] == "CALL_ALLOC_AND_ENTER_INIT"
                              and k["group"] == "own")
            alloc_all = _incl(prof, mode, lambda k: k["op"] == "CALL_ALLOC_AND_ENTER_INIT")
            ok &= gen > 0.4 and alloc_own > 0.9 * alloc_all
            msgs.append(f"CALL_ALLOC_AND_ENTER_INIT own {alloc_own:.3f}/{alloc_all:.3f}")
        msgs.append(f"{mode}: owner generated:dataclasses {gen:.3f} layers {lay}")
    return ok, "; ".join(msgs)


def _check_gc(prof):
    msgs = []
    ok = True
    for mode in ("bpf", "plain", "xperf"):
        if mode not in prof:
            continue
        gc_ = _incl(prof, mode, lambda k: k["layer"] == "runtime" and k["sub"] == "gc")
        rt = prof[mode]["summary"]["layers"]["runtime"]
        ok &= rt > 0.5 and gc_ > 0.3
        msgs.append(f"{mode}: runtime {rt:.3f} (gc {gc_:.3f})")
    return ok, "; ".join(msgs)


def _check_calls(prof):
    # plain mode must see specialized Python calls (CALL_PY_EXACT_ARGS etc.), -X perf must not
    msgs = []
    ok = True
    for mode in ("bpf", "plain", "xperf"):
        if mode not in prof:
            continue
        spec = _incl(prof, mode, lambda k: k["op"] in ("CALL_PY_EXACT_ARGS",
                                                       "CALL_BOUND_METHOD_EXACT_ARGS"))
        own = prof[mode]["summary"]["layers"]["own"]
        unres = prof[mode].get("stats", {}).get("resolution", {})
        msgs.append(f"{mode}: own {own:.3f} spec-call {spec:.3f} res {unres}")
        if mode in ("bpf", "plain"):
            ok &= spec > 0.01 and own > 0.5
    return ok, "; ".join(msgs)


def headline(prof):
    """The distortion-free summary of a profile: plain, unless the steady state runs in child
    processes (then -X perf with child-process attribution)."""
    xp = prof.get("xperf")
    xcyc = sum(v for _k, v in xp["counter"]) if xp else 0
    for mode in ("bpf", "plain"):
        pl = prof.get(mode)
        if not pl:
            continue
        cyc = sum(v for _k, v in pl["counter"])
        # the main process only waits for child processes (bpf samples the main process)
        if xcyc and cyc < 0.2 * xcyc:
            continue
        if pl["summary"]["sublayers"].get("other:child-process", 0) < 0.5 and \
                pl["summary"]["layers"].get("other", 0) < 0.5:
            return mode, pl
    return "xperf", xp


def _wname(p):
    return p["workload"]


def _pct(x, d=1):
    return f"{100 * x:.{d}f}"


def rollup_main(a):
    profs = [load_profile(p) for p in a.profiles]
    profs.sort(key=_wname)
    fams = defaultdict(list)
    for p in profs:
        fams[p["workload"].split("/")[0]].append(p)
    md = []
    js = {"workloads": {}, "families": {}, "overall": {}}

    def layers_of(p):
        return headline(p)[1]["summary"]["layers"]

    def subs_of(p):
        return headline(p)[1]["summary"]["sublayers"]

    def mean(vals):
        vals = list(vals)
        return sum(vals) / len(vals) if vals else 0.0

    # ---- 1. per-workload layer table ----
    md.append("| workload | own | generated | language | stdlib | thirdparty | runtime | other "
              "| largest stdlib / thirdparty / runtime parts |")
    md.append("|---|---:|---:|---:|---:|---:|---:|---:|---|")
    for fam in sorted(fams):
        for p in fams[fam]:
            L = layers_of(p)
            S = subs_of(p)
            mode = headline(p)[0]
            parts = []
            for lay in ("stdlib", "thirdparty", "runtime", "generated"):
                best = sorted(((v, k) for k, v in S.items() if k.startswith(lay + ":")),
                              reverse=True)[:2 if lay != "generated" else 1]
                parts += [f"{k.split(':', 1)[1] or lay} {_pct(v)}" for v, k in best if v >= 0.02]
            md.append(f"| {p['workload']}{' (x)' if mode == 'xperf' else ''} | "
                      + " | ".join(_pct(L[l]) for l in LAYERS) + " | " + ", ".join(parts) + " |")
            js["workloads"][p["workload"]] = {"mode": mode, "layers": L,
                                              "sublayers": dict(list(S.items())[:25]),
                                              "wall_median_s": p.get("wall_median_s"),
                                              "cycles_per_run": p.get("cycles_per_run")}
    md.append("")
    # ---- 2. family and overall means ----
    md.append("| family | n | own | generated | language | stdlib | thirdparty | runtime | other |")
    md.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    groups = [(f, fams[f]) for f in sorted(fams)] + [("all", profs)]
    for fam, ps in groups:
        m = {l: mean(layers_of(p)[l] for p in ps) for l in LAYERS}
        js["families" if fam != "all" else "overall"][fam] = m
        md.append(f"| {fam} | {len(ps)} | " + " | ".join(_pct(m[l]) for l in LAYERS) + " |")
    md.append("")
    out_md = "\n".join(md)
    # ---- 3. sublayer means (overall), per layer ----
    sub_mean = Counter()
    sub_max = {}
    for p in profs:
        for k, v in subs_of(p).items():
            sub_mean[k] += v / len(profs)
            if v > sub_max.get(k, (0, ""))[0]:
                sub_max[k] = (v, p["workload"])
    js["sublayer_mean"] = {k: round(v, 5) for k, v in sub_mean.most_common()}
    js["sublayer_max"] = {k: [round(v[0], 4), v[1]] for k, v in sub_max.items()}
    md2 = ["| layer:part | mean | max (workload) |", "|---|---:|---|"]
    for k, v in sub_mean.most_common(a.top):
        mx = sub_max[k]
        md2.append(f"| {k} | {_pct(v, 2)} | {_pct(mx[0])} ({mx[1]}) |")
    out_md += "\n" + "\n".join(md2) + "\n"
    with open(a.out, "w") as f:
        json.dump(js, f, indent=1)
    if a.md:
        with open(a.md, "w") as f:
            f.write(out_md)
    print(out_md)


def batch_main(a):
    import concurrent.futures as cf
    import queue

    todo = []
    for w in a.workloads:
        if "/" in w:
            todo.append(w)
        else:
            d = os.path.join(a.workloads_dir, w)
            for f in sorted(os.listdir(d)):
                if f.endswith(".py") and not f.startswith(("_", "profile")):
                    todo.append(f"{w}/{f[:-3]}")
    os.makedirs(a.outdir, exist_ok=True)
    cpu_q = queue.Queue()
    for c in a.cpu_sets.split(","):
        cpu_q.put(c)
    tag = a.build + ("-rq" if a.enable else "") + a.suffix

    def one(w):
        fam, name = w.split("/")
        out = os.path.join(a.outdir, f"{fam}__{name}__{tag}.json.gz")
        if os.path.exists(out) and not a.force:
            return w, "cached", 0
        py = os.path.join(a.venvs, f"{fam}-{a.build}", "bin", "python")
        cpus = cpu_q.get()
        t0 = time.time()
        try:
            cmd = [sys.executable, os.path.abspath(__file__), "profile",
                   os.path.join(a.workloads_dir, fam, name + ".py"), "--out", out, "--python", py,
                   "--modes", a.modes, "--seconds", str(a.seconds), "--cpus", cpus]
            if a.enable:
                cmd.append("--enable")
            if a.extra:
                cmd += a.extra.split()
            env = dict(os.environ)
            if a.pythonpath:
                env["RQ_EXTRA_PYTHONPATH"] = a.pythonpath.replace("{build}", a.build)
            r = subprocess.run(cmd, capture_output=True, text=True, env=env)
            log = os.path.join(a.outdir, f"{fam}__{name}__{tag}.log")
            with open(log, "w") as f:
                f.write(r.stdout + r.stderr)
            return w, ("ok" if r.returncode == 0 else f"FAIL rc={r.returncode}"), time.time() - t0
        finally:
            cpu_q.put(cpus)

    with cf.ThreadPoolExecutor(a.jobs) as ex:
        for w, st, dt in ex.map(one, todo):
            print(f"{w:45s} {st} {dt:6.1f}s", flush=True)


# =========================================================================================
# CLI
# =========================================================================================


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("driver")
    d.add_argument("workload")
    d.add_argument("--mode", choices=["steady", "whole", "bpf", "envinfo"], default="steady")
    d.add_argument("--out", required=True)
    d.add_argument("--seconds", type=float, default=8.0)
    d.add_argument("--min-iters", type=int, default=2)
    d.add_argument("--enable", action="store_true")

    def common(p):
        p.add_argument("--python", required=True)
        p.add_argument("--enable", action="store_true", help="rustique.enable() first")
        p.add_argument("--modes", default="bpf,xperf")
        p.add_argument("--seconds", type=float, default=8.0)
        p.add_argument("--freq", type=int, default=1999)
        p.add_argument("--period", type=int, default=2000003,
                       help="bpf mode: cycles between samples (prime, ~1.8 kHz at 3.7 GHz)")
        p.add_argument("--whole-freq", type=int, default=4999)
        p.add_argument("--stack", type=int, default=16384)
        p.add_argument("--cpus", default=None, help="taskset CPU list")
        p.add_argument("--main-thread-only", dest="system_wide", action="store_false",
                       help="bpf mode: task-bound counters on the main thread (default: "
                            "all-CPU counters filtered by pid, i.e. every thread)")
        p.add_argument("--cpython-src", default=os.environ.get(
            "CPYTHON_SRC", os.path.expanduser("~/src/cpython-3.15.1")))
        p.add_argument("--cache", default=os.path.expanduser("~/.cache/rq-layers"))
        p.add_argument("--tmp", default=None)
        p.add_argument("--keep", default=None, help="directory to keep raw perf data in")

    p = sub.add_parser("profile")
    p.add_argument("workload")
    p.add_argument("--out", required=True)
    common(p)
    v = sub.add_parser("validate")
    v.add_argument("--outdir", required=True)
    common(v)
    b = sub.add_parser("batch", help="profile many workloads in parallel, one family venv each")
    b.add_argument("workloads", nargs="+", help="family/name or family")
    b.add_argument("--workloads-dir", default=os.path.join(REPO, "benches", "workloads"))
    b.add_argument("--venvs", required=True, help="root of <family>-<build> venvs")
    b.add_argument("--build", default="3.15.1")
    b.add_argument("--outdir", required=True)
    b.add_argument("--jobs", type=int, default=6)
    b.add_argument("--cpu-sets", default="0-1,2-3,4-5,6-7,8-9,10-11,12-13,14-15")
    b.add_argument("--suffix", default="")
    b.add_argument("--force", action="store_true")
    for x in ("--enable",):
        b.add_argument(x, action="store_true")
    b.add_argument("--modes", default="bpf,xperf")
    b.add_argument("--seconds", type=float, default=8.0)
    b.add_argument("--extra", default="", help="extra args for profile")
    b.add_argument("--pythonpath", default="",
                   help="PYTHONPATH for the profiled interpreter (e.g. an unpacked rustique "
                        "wheel for --enable runs; {build} is substituted)")
    r = sub.add_parser("rollup")
    r.add_argument("profiles", nargs="+")
    r.add_argument("--out", required=True)
    r.add_argument("--md")
    r.add_argument("--top", type=int, default=50)
    a = ap.parse_args(argv)
    if a.cmd == "batch":
        return batch_main(a)
    if a.cmd == "rollup":
        return rollup_main(a)
    if a.cmd == "driver":
        driver_main(a)
    elif a.cmd == "profile":
        profile_main(a)
    elif a.cmd == "validate":
        os.makedirs(a.outdir, exist_ok=True)
        validate_main(a)


if __name__ == "__main__":
    main()
