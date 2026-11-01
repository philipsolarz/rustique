"""Profile one workload module (shared workload format) into the shared profile JSON schema.

    python3.15 profile_devtools.py profile <workload.py> --python VENV/bin/python \
        --out PROFILES_DIR/<name>.json [--seconds 5] [--scratch DIR]
    python3.15 profile_devtools.py rollup PROFILES_DIR/*.json          # family tables (markdown)

`profile` runs three separate child processes of the target interpreter, each after
`setup()` and one warm-up `run()`:

1. perf: `perf record -e cpu-clock -F 1999 -g` with events enabled only around repeated
   steady-state `run()` calls (`--control` fifo), the child under `-X perf`
   (PYTHONPERFSUPPORT=1, inherited by subprocesses, so whole-process workloads that spawn
   children are covered too). Each sample's leaf native symbol -> component, innermost
   Python frame -> owner, plus the native *entry* (first non-dispatch native frame called
   by the interpreter, and which opcode called it) and the *stdlib entry* (outermost
   stdlib Python frame of the innermost stdlib run).
2. cProfile over repeated steady-state `run()` calls.
3. arg sizes: `sys.monitoring` (CALL for C callables, PY_START for Python functions) on the
   top builtin/stdlib hotspots found by cProfile, during one `run()`.

BOLT note (PBS 3.15 build, perf 6.18): sample IPs reported by `perf script -F ip` are
*virtual addresses* for this binary - no file-offset correction. Validated on `json.dumps`
of a large list: 100 % of the 5,995 samples under `JSONEncoder.iterencode` have
`encoder_call`/`encoder_listencode_*` on the stack, top leaves `_Py_dg_dtoa`,
`encoder_listencode_obj`; adding (0x1800000 - 0x1569000) instead yields nonsense
(`_PyInterpreterConfig_AsDict`, `sqlite3WindowCodeStep`). perf itself cannot name the BOLTed
code, so IPs are symbolized here with `nm -nS` over every executable section:
.text/.text.warm/.text.cold hold the BOLT-rewritten hot code, and `.bolt.org.text` still
holds every function BOLT did not move (they execute from there, e.g.
`py_trampoline_evaluator`). Unsymbolized leaves are counted in `perf.unsymbolized_leaf`
(<= 7 per profile of ~10k).
"""

from __future__ import annotations

import argparse
import bisect
import collections
import json
import os
import re
import resource
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))

# --------------------------------------------------------------------------------------
# Child side (runs inside the target interpreter)
# --------------------------------------------------------------------------------------


def _load(path):
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "devtools_wl_" + os.path.basename(path)[:-3], path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    return mod


def _rusage():
    s = resource.getrusage(resource.RUSAGE_SELF)
    c = resource.getrusage(resource.RUSAGE_CHILDREN)
    return s.ru_utime + c.ru_utime, s.ru_stime + c.ru_stime


def child_perf(args):
    mod = _load(args.workload)
    state = mod.setup()
    ref = mod.digest(mod.run(state))
    ctl = open(args.ctl, "w")
    ack = open(args.ack, "r")

    def command(cmd):
        ctl.write(cmd + "\n")
        ctl.flush()
        ack.readline()

    walls, digests = [], set()
    u0, s0 = _rusage()
    command("enable")
    t_start = time.perf_counter()
    while True:
        t = time.perf_counter()
        r = mod.run(state)
        walls.append(time.perf_counter() - t)
        if time.perf_counter() - t_start >= args.seconds and len(walls) >= args.min_runs:
            break
        digests.add(mod.digest(r))
    command("disable")
    u1, s1 = _rusage()
    digests.add(mod.digest(r))
    with open(args.meta, "w") as f:
        json.dump({"walls": walls, "utime": u1 - u0, "stime": s1 - s0,
                   "deterministic": digests == {ref}, "pid": os.getpid()}, f)


def child_time(args):
    mod = _load(args.workload)
    state = mod.setup()
    mod.run(state)
    walls = []
    u0, s0 = _rusage()
    for _ in range(args.runs):
        t = time.perf_counter()
        mod.run(state)
        walls.append(time.perf_counter() - t)
    u1, s1 = _rusage()
    with open(args.meta, "w") as f:
        json.dump({"walls": walls, "utime": u1 - u0, "stime": s1 - s0}, f)


def child_importtime(args):
    """For workloads that expose IMPORTS: `-X importtime` per import, 3 repeats, medians."""
    import sysconfig

    mod = _load(args.workload)
    stdlib = sysconfig.get_path("stdlib")
    per = collections.defaultdict(lambda: collections.defaultdict(list))
    for _ in range(3):
        for name in mod.IMPORTS:
            code = (f"import {name}" if name else "pass")
            p = subprocess.run([sys.executable, "-X", "importtime", "-c", code],
                               capture_output=True, text=True)
            for line in p.stderr.splitlines():
                m = re.match(r"import time:\s+(\d+) \|\s+(\d+) \| ( *)(\S+)", line)
                if m:
                    per[name or "<bare>"][m.group(4)].append(int(m.group(1)))
    out = {}
    for name, mods in per.items():
        selfs = {m: statistics.median(v) for m, v in mods.items()}
        out[name] = selfs
    with open(args.meta, "w") as f:
        json.dump({"importtime_self_us": out, "stdlib": stdlib}, f)


def _prelude_cprofile(outdir):
    """Child prelude for whole-process workloads: profile the child until exit."""
    return (f"import cProfile as _dcp, atexit as _dax, os as _dos; _dpr=_dcp.Profile(); "
            f"_dax.register(lambda: (_dpr.disable(), _dpr.dump_stats({outdir!r}+'/%d.prof' % _dos.getpid()))); "
            f"_dpr.enable(); ")


def child_cprofile(args):
    import cProfile

    mod = _load(args.workload)
    state = mod.setup()
    if args.whole_process:
        state["DEVTOOLS_CHILD_PRELUDE"] = _prelude_cprofile(args.outdir)
    mod.run(state)  # warm-up
    if args.whole_process:
        for f in os.listdir(args.outdir):
            os.unlink(os.path.join(args.outdir, f))
    prof = cProfile.Profile()
    t0 = time.perf_counter()
    runs = 0
    while True:
        prof.enable()
        mod.run(state)
        prof.disable()
        runs += 1
        if time.perf_counter() - t0 >= args.seconds:
            break
    prof.dump_stats(os.path.join(args.outdir, "parent.prof"))
    with open(args.meta, "w") as f:
        json.dump({"runs": runs}, f)


# ---- arg-size capture via sys.monitoring -------------------------------------------------

_SIMPLE_LOADS = {"LOAD_FAST", "LOAD_FAST_BORROW", "LOAD_FAST_CHECK", "LOAD_DEREF", "LOAD_NAME",
                 "LOAD_GLOBAL", "LOAD_CONST", "LOAD_SMALL_INT", "LOAD_FAST_AND_CLEAR"}
_DOUBLE_LOADS = {"LOAD_FAST_LOAD_FAST", "LOAD_FAST_BORROW_LOAD_FAST_BORROW"}
_SKIP = {"NOP", "NOT_TAKEN", "CACHE", "EXTENDED_ARG"}


def _type_name(tp):
    mod = getattr(tp, "__module__", "builtins")
    return tp.__qualname__ if mod == "builtins" else f"{mod}.{tp.__qualname__}"


def _callable_key(c):
    """(owner, name, bound) in cProfile's naming: owner is a type name or 'module:<m>'."""
    import types

    tc = type(c)
    if tc in (types.MethodDescriptorType, types.WrapperDescriptorType,
              types.ClassMethodDescriptorType):
        return _type_name(c.__objclass__), c.__name__, False
    if tc is types.BuiltinFunctionType or tc is _BUILTIN_METHOD_TYPE:
        s = c.__self__
        if s is None or isinstance(s, types.ModuleType):
            return "module:" + (s.__name__ if s is not None else (c.__module__ or "?")), c.__name__, False
        if isinstance(s, type):
            return _type_name(s), c.__name__, True
        return _type_name(type(s)), c.__name__, True
    if tc is types.MethodWrapperType:
        return _type_name(type(c.__self__)), c.__name__, True
    return None


def _builtin_key_from_cprofile(name):
    m = re.match(r"<method '([^']+)' of '([^']+)' objects>", name)
    if m:
        return m.group(2), m.group(1)
    m = re.match(r"<built-in method (\S+) of (\S+) object at", name)
    if m:
        return m.group(2), m.group(1)
    m = re.match(r"<built-in method ([\w.]+)\.(\w+)>", name)
    if m:
        return "module:" + m.group(1), m.group(2)
    m = re.match(r"<built-in method (\w+)>", name)
    if m:
        return "module:builtins", m.group(1)
    return None


_SIZED_TYPES = (str, bytes, bytearray, memoryview, list, tuple, dict, set, frozenset)


def _sized(v):
    # exact builtin containers only: never run user __len__ from a monitoring callback
    if type(v) in _SIZED_TYPES or isinstance(v, (str, bytes, list, tuple, dict)):
        try:
            return len(v)
        except Exception:
            return None
    return None


# 3.15: bound methods of C types are `builtin_method`, not builtin_function_or_method
_BUILTIN_METHOD_TYPE = type(re.compile("").match)  # PyCMethodObject ("builtin_method")


def _c_callable_types():
    import types

    return frozenset({types.MethodDescriptorType, types.WrapperDescriptorType,
                      types.ClassMethodDescriptorType, types.BuiltinFunctionType,
                      types.MethodWrapperType, _BUILTIN_METHOD_TYPE})


_C_CALLABLE_TYPES = _c_callable_types()


class _ArgRecorder:
    TOOL = 4  # sys.monitoring tool id (PROFILER_ID=2 may be taken by coverage? it uses 1/3/5)

    def __init__(self, builtin_keys, py_targets):
        self.builtin_keys = set(builtin_keys)
        self.py_targets = py_targets  # code -> label
        self.sizes = collections.defaultdict(list)
        self.types = collections.defaultdict(collections.Counter)
        self.calls = collections.Counter()
        self.recipes = {}
        self.instrs = {}

    # backward parse of the argument expressions feeding a CALL
    def _recipe(self, code, offset):
        key = (code, offset)
        if key in self.recipes:
            return self.recipes[key]
        import dis

        if code not in self.instrs:
            ins = list(dis.get_instructions(code))
            self.instrs[code] = (ins, {i.offset: n for n, i in enumerate(ins)})
        ins, index = self.instrs[code]
        recipe = None
        n = index.get(offset)
        if n is not None:
            call = ins[n]
            argc = call.arg or 0
            skip = 1 if call.opname == "CALL_KW" else 0
            items = self._parse(ins, n - 1, argc + skip)
            if items is not None and len(items) >= argc + skip and argc >= 1:
                recipe = items[skip + argc - 1]  # deepest = first explicit argument
        self.recipes[key] = recipe
        return recipe

    def _parse(self, ins, j, count):
        items = []
        while len(items) < count:
            if j < 0:
                return None
            i = ins[j]
            op = i.opname
            if op in _SKIP:
                j -= 1
                continue
            if op in _SIMPLE_LOADS:
                if op == "LOAD_GLOBAL" and (i.arg & 1):
                    return None
                items.append(("const", i.argval) if op in ("LOAD_CONST", "LOAD_SMALL_INT")
                             else ("global", i.argval) if op in ("LOAD_GLOBAL", "LOAD_NAME")
                             else ("local", i.argval))
                j -= 1
            elif op in _DOUBLE_LOADS:
                a, b = i.argval
                items.append(("local", b))
                items.append(("local", a))
                j -= 1
            elif op == "LOAD_ATTR" and not (i.arg & 1):
                inner = self._parse(ins, j - 1, 1)
                if not inner or len(inner) != 1:
                    return None
                # find where the inner expression started
                k = j - 1
                depth = self._span(ins, j - 1)
                if depth is None:
                    return None
                items.append(("attr", inner[0], i.argval))
                j = depth
            else:
                return None
        return items

    def _span(self, ins, j):
        """Index just before the single simple expression ending at j."""
        while j >= 0 and ins[j].opname in _SKIP:
            j -= 1
        if j < 0:
            return None
        op = ins[j].opname
        if op in _SIMPLE_LOADS:
            return j - 1
        if op == "LOAD_ATTR" and not (ins[j].arg & 1):
            return self._span(ins, j - 1)
        return None

    @staticmethod
    def _eval(recipe, frame):
        kind = recipe[0]
        if kind == "const":
            return recipe[1]
        if kind == "local":
            return frame.f_locals[recipe[1]]
        if kind == "global":
            g = frame.f_globals
            if recipe[1] in g:
                return g[recipe[1]]
            return frame.f_builtins[recipe[1]]
        if kind == "attr":
            obj = _ArgRecorder._eval(recipe[1], frame)
            name = recipe[2]
            for klass in type(obj).__mro__:
                d = klass.__dict__.get(name)
                if d is not None and hasattr(type(d), "__get__") and not callable(d):
                    raise LookupError("descriptor")  # property etc.: don't re-run user code
            d = getattr(obj, "__dict__", None)
            if d is not None and name in d:
                return d[name]
            return getattr(obj, name)
        raise LookupError(kind)

    def record(self, label, value):
        self.calls[label] += 1
        self.types[label][type(value).__name__] += 1
        n = _sized(value)
        if n is not None:
            self.sizes[label].append(n)

    def on_call(self, code, offset, func, arg0):
        # Never DISABLE: generic call sites (`f(*args)`, visitor dispatch) are polymorphic,
        # and disabling them on a first non-target callable would undercount targets.
        if type(func) not in _C_CALLABLE_TYPES:
            return None
        try:
            key = _callable_key(func)
        except Exception:
            key = None
        if key is None or (key[0], key[1]) not in self.builtin_keys:
            return None
        owner, name, bound = key
        label = f"{owner}.{name}" if not owner.startswith("module:") else f"{owner[7:]}.{name}"
        subject = arg0
        if not bound and not owner.startswith("module:"):
            # descriptor call: arg0 is self. For re patterns and str.join, the first explicit
            # argument is the interesting input.
            if owner in ("re.Pattern",) or (owner, name) in (("str", "join"), ("bytes", "join")):
                recipe = self._recipe(code, offset)
                if recipe is None:
                    self.calls[label + " (arg unresolved)"] += 1
                    return None
                try:
                    subject = self._eval(recipe, sys._getframe(1))
                except Exception:
                    self.calls[label + " (arg unresolved)"] += 1
                    return None
        if subject is sys.monitoring.MISSING:
            self.calls[label] += 1
            return None
        self.record(label, subject)
        return None

    def on_start(self, code, offset, frame=None):
        label = self.py_targets.get(code)
        if label is None:
            return None
        loc = (frame or sys._getframe(1)).f_locals
        nargs = code.co_argcount + code.co_kwonlyargcount + bool(code.co_flags & 0x04)  # CO_VARARGS
        names = code.co_varnames[:nargs]
        values = [loc.get(nm) for nm in names if nm not in ("self", "cls")]
        subject = next((v for v in values if _sized(v) is not None), values[0] if values else None)
        self.record(label, subject)
        return None

    def start(self):
        mon = sys.monitoring
        mon.use_tool_id(self.TOOL, "devtools-argsizes")
        mon.register_callback(self.TOOL, mon.events.CALL, self.on_call)
        mon.register_callback(self.TOOL, mon.events.PY_START, self.on_start)
        if self.builtin_keys:
            mon.set_events(self.TOOL, mon.events.CALL)
        for code in self.py_targets:
            mon.set_local_events(self.TOOL, code, mon.events.PY_START)

    def stop(self):
        mon = sys.monitoring
        mon.set_events(self.TOOL, 0)
        for code in self.py_targets:
            mon.set_local_events(self.TOOL, code, 0)
        mon.free_tool_id(self.TOOL)

    def result(self):
        out = {}
        for label in sorted(set(self.calls) | set(self.sizes)):
            xs = sorted(self.sizes.get(label, []))
            entry = {"calls": self.calls[label], "types": dict(self.types[label].most_common(4))}
            if xs:
                q = lambda p: xs[min(len(xs) - 1, int(p * len(xs)))]
                hist = collections.Counter(0 if x == 0 else 1 << (x.bit_length() - 1) for x in xs)
                entry.update({"p50": q(0.5), "p90": q(0.9), "p99": q(0.99), "max": xs[-1],
                              "mean": round(sum(xs) / len(xs), 1),
                              "histogram": {f"<{2 * b}" if b else "0": c for b, c in sorted(hist.items())}})
            out[label] = entry
        return out


def _find_code_objects(targets):
    """targets: list of (filename, firstlineno, funcname) -> {code: label}"""
    import gc

    want = {(f, l, n): f"{os.path.basename(f)}:{n}" for f, l, n in targets}
    found = {}

    def visit(code):
        k = (code.co_filename, code.co_firstlineno, code.co_name)
        if k in want and code not in found:
            found[code] = want[k]
        for c in code.co_consts:
            if hasattr(c, "co_code"):
                visit(c)

    import types

    # only real function objects: getattr() on arbitrary objects can run user code
    # (astroid proxies infer on __getattr__)
    for obj in gc.get_objects():
        if type(obj) is types.FunctionType:
            visit(obj.__code__)
    return found


def child_argsizes(args):
    with open(args.targets) as f:
        targets = json.load(f)
    mod = _load(args.workload)
    state = mod.setup()
    if args.whole_process:
        state["DEVTOOLS_CHILD_PRELUDE"] = (
            f"import sys as _dsys; _dsys.path.insert(0, {HERE!r}); import profile_devtools as _dpd; "
            f"_dpd.argsizes_in_child({args.targets!r}, {args.outdir!r}); ")
        mod.run(state)
        with open(args.meta, "w") as f:
            json.dump({}, f)
        return
    mod.run(state)  # warm-up
    keys = [tuple(k) for k in targets["builtins"]]
    py = _find_code_objects([tuple(t) for t in targets["python"]])
    rec = _ArgRecorder(keys, py)
    rec.start()
    try:
        mod.run(state)
    finally:
        rec.stop()
    with open(args.meta, "w") as f:
        json.dump({"arg_sizes": rec.result(),
                   "python_targets_found": sorted(set(py.values()))}, f)


def argsizes_in_child(targets_path, outdir):
    """Installed by the prelude in whole-process children; dumps at exit."""
    import atexit

    with open(targets_path) as f:
        targets = json.load(f)
    keys = [tuple(k) for k in targets["builtins"]]
    rec = _ArgRecorder(keys, {})
    # Python targets are found lazily: code objects appear as modules import.
    want = {(f, l, n): f"{os.path.basename(f)}:{n}" for f, l, n in map(tuple, targets["python"])}
    mon = sys.monitoring
    rec.start()

    def on_start_any(code, offset):
        k = (code.co_filename, code.co_firstlineno, code.co_name)
        label = want.get(k)
        if label is None:
            return mon.DISABLE
        rec.py_targets[code] = label
        return rec.on_start(code, offset, sys._getframe(1))

    mon.register_callback(rec.TOOL, mon.events.PY_START, on_start_any)
    mon.set_events(rec.TOOL, mon.events.CALL | mon.events.PY_START)

    def dump():
        mon.set_events(rec.TOOL, 0)
        with open(os.path.join(outdir, f"{os.getpid()}.json"), "w") as f:
            json.dump({"calls": rec.calls, "sizes": rec.sizes,
                       "types": {k: dict(v) for k, v in rec.types.items()}}, f)

    atexit.register(dump)


# --------------------------------------------------------------------------------------
# Parent side: symbolization and classification
# --------------------------------------------------------------------------------------

_SUFFIX = re.compile(r"\.(cold|warm|lto_priv|constprop|isra|part|llvm|localalias|org|\d+)(\.\d+)*.*$")


class Symbolizer:
    def __init__(self, binary, cache_dir):
        self.binary = binary
        os.makedirs(cache_dir, exist_ok=True)
        cache = os.path.join(cache_dir, os.path.basename(binary) + f".{os.stat(binary).st_size}.nm")
        if not os.path.exists(cache):
            out = subprocess.run(["nm", "-nS", binary], capture_output=True, text=True).stdout
            with open(cache, "w") as f:
                f.write(out)
        secs = subprocess.run(["readelf", "-SW", binary], capture_output=True, text=True).stdout
        self.ranges = []
        # every executable PROGBITS section: .text/.text.warm/.text.cold (BOLT-rewritten hot
        # code) *and* .bolt.org.text (functions BOLT left in place still execute from there)
        for m in re.finditer(r"\]\s+(\S+)\s+PROGBITS\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)\s+\S+\s+(\S*X\S*)", secs):
            a, size = int(m.group(2), 16), int(m.group(3), 16)
            self.ranges.append((a, a + size, m.group(1)))
        starts, ends, names = [], [], []
        with open(cache) as f:
            for line in f:
                p = line.split()
                if len(p) != 4 or p[2] not in "tTwWiI":
                    continue
                a = int(p[0], 16)
                if not any(lo <= a < hi for lo, hi, _ in self.ranges):
                    continue
                starts.append(a)
                ends.append(a + int(p[1], 16))
                names.append(p[3])
        self.starts, self.ends, self.names = starts, ends, names
        self.cache = {}

    def __call__(self, ip):
        r = self.cache.get(ip)
        if r is not None:
            return r
        i = bisect.bisect_right(self.starts, ip) - 1
        if i >= 0 and ip < self.ends[i]:
            r = _SUFFIX.sub("", self.names[i])
        else:
            r = None
        self.cache[ip] = r
        return r


# Leaf symbol -> component. First match wins; patterns are anchored prefixes unless noted.
_RULES = [
    ("profiler-artifact", r"py_trampoline|PyUnstable_Code_GetExtra|_PyCode_GetExtra|compile_trampoline|"
                          r"perf_map|_PyPerfTrampoline|perf_trampoline"),
    ("monitoring", r"_Py_call_instrumentation|_Py_Instrument|_PyMonitoring|monitoring_|sys_monitoring|"
                   r"call_instrumentation|instrument_|_PyLegacyTracing|sys_trace|sys_profile|call_trace|"
                   r"_PyEval_SetProfile|_PyEval_SetTrace|profile_trampoline|call_trampoline|"
                   r"_Py_GetBaseCodeUnit|de_instrument|get_tools_for_instruction|remove_tools|"
                   r"_Py_Instrumentation|global_get_tools|instrument_lock"),
    ("cprofile", r"_lsprof|profiler_|ptrace_|pending_|_lsprof_|getEntry|newProfilerEntry|"
                 r"initContext|Stop$|getSubEntry|call_timer|flush_unmatched|profile_callback"),
    ("memory/gc", r"gc_|_PyObject_GC_|PyObject_GC_|_PyMem|PyMem_|mi_|_mi_|pymalloc|_PyObject_Malloc|"
                  r"_PyObject_Free|_PyObject_Calloc|_PyObject_Realloc|PyObject_Malloc|PyObject_Free|"
                  r"PyObject_Calloc|PyObject_Realloc|_Py_Dealloc|_Py_DecRef|Py_DecRef|_Py_IncRef|"
                  r"_PyGC_|visit_|deduce_unreachable|update_refs|subtract_refs|move_unreachable|"
                  r"move_legacy_finalizers|PyObject_Init|_PyObject_New|PyType_GenericAlloc|"
                  r"_PyType_AllocNoTrack|_Py_freelist|_PyFreeList|_Py_MergeZeroLocalRefcount|"
                  r"_Py_ExplicitMergeRefcount|_PyObject_GC_Link|_Py_ScheduleGC|_Py_RunGC|"
                  r"PyObject_CallFinalizer|_PyObject_GC_New|free_|_PyObject_NewVar|"
                  r"_PyObject_GetState|untrack_|gc_collect|collect$|validate_list|"
                  r"_PyTrash_|_PyObject_AssertFailed|PyObject_GC_Del|_Py_Arena|new_arena|"
                  r"allocate_from_new_pool|insert_to_usedpool|insert_to_freepool|pymalloc_|"
                  r"_PyObject_ClearFreeLists|clear_freelist|.*_dealloc$|.*_traverse$|subtype_dealloc"),
    ("weakref", r"weakref|PyWeakref_|_PyWeakref|PyObject_ClearWeakRefs|proxy_|handle_callbacks|"
                r"clear_weakref|_PyStaticType_ClearWeakRefs"),
    ("interpreter", r"_TAIL_CALL_|_PyEval_EvalFrame|_PyEval_|_PyFrame_|_Py_Specialize|_PyEvalFrame|"
                    r"clear_thread_frame|clear_gen_frame|PyEval_|_Py_HandlePending|_Py_EnterRecursive|"
                    r"_Py_CheckRecursiveCall|_Py_LeaveRecursive|py_trampoline|_PyPerf|_PyCode_Quicken|"
                    r"_PyThreadState_|PyThreadState_|_PyInterpreterState_|PyFrame_|frame_|"
                    r"framelocalsproxy|_PyFrameLocals|_Py_uop|_PyOptimizer|_PyJIT|_Py_ExecutorDetach|"
                    r"_PyInterpreterFrame|unspecialize|specialize|_Py_GatherStats|_PyUop|"
                    r"_Py_CheckFunctionResult|_Py_EnsureFuncTstateNotNULL|_PyStackRef|"
                    r"_Py_BuildMap_StackRef|_PyList_FromStackRefStealOnSuccess|_PyTuple_FromStackRefStealOnSuccess|"
                    r"_PyEval_FrameClearAndPop|_PyCode_CheckLineNumber|PyCode_Addr2Line|"
                    r"_Py_Instructions|_PyFunction_SetVersion|_PyFunction_LookupByVersion|_PyFunction_GetVersionForCurrentState|"
                    r"_PyType_LookupStackRefAndVersion|"
                    r"_PyObject_GetMethodStackRef|_Py_GetConfig|_PyCompactLong_"),
    ("calls", r"_PyObject_Vectorcall|PyObject_Vectorcall|_PyFunction_Vectorcall|method_vectorcall|"
              r"cfunction_|_PyObject_MakeTpCall|PyObject_Call|_PyObject_Call|vectorcall_|_PyStack_|"
              r"PyCFunction_|_PyCFunction_|wrapperdescr_call|wrapper_call|_PyObject_FastCall|object_vacall|"
              r"callmethod|_PyVectorcall|PyVectorcall|_Py_VectorcallMethod|PyObject_VectorcallMethod|"
              r"func_vectorcall|_PyFunction_|method_vectorcall_|_PyArg_|PyArg_|vgetargs|vgetargskeywords|"
              r"convertitem|convertsimple|_PyArg_UnpackKeywords|find_keyword|seterror|"
              r"_Py_set_function_type_params|bound_vectorcall|meth_vectorcall|method_call|"
              r"_PyObject_VectorcallTstate|_PyObject_VectorcallDictTstate|_PyObject_Call_Prepend"),
    ("re-engine", r"sre_|_sre|SRE_|pattern_|match_|scanner_|state_init|state_fini|state_reset|getstring|"
                  r"_validate|template_|compile_template|expand_template|sre_lower|sre_upper|"
                  r"sre_category|charset|_sre_|pattern_subx|pattern_new_match|getslice|"
                  r"general_mark|sre_search|sre_match"),
    ("json-accel", r"encoder_|scanstring|scan_once|py_encode_basestring|_parse_object|_parse_array|"
                   r"_match_number|raise_errmsg|write_escaped|ascii_escape|escape_unicode|_json_"),
    ("marshal", r"r_object|r_ref|r_string|r_long|r_byte|r_short|r_float|r_PyLong|w_object|w_|"
                r"PyMarshal_|marshal_|read_object|_PyMarshal|r_bytes"),
    ("parser", r"_PyPegen|_PyParser|_PyTokenizer|tok_|_PyLexer|.*_rule$|.*_raw$|_loop0_|_loop1_|"
               r"_gather_|_tmp_|_PyArena|PyArena|_PyAST_|tokenizeriter_|_Pypegen|_PyToken|"
               r"_PyRun_|PyRun_|_PyParser_ASTFromString|run_mod|translate_newlines|decode_str|"
               r"tok_|token_|_get_|_is_|invalid_|_PyLexer_|tokenizer_|_syntaxerror_range|"
               r"lookahead|_PyPegen_|_seq_|asdl_|_Py_asdl"),
    ("ast-objects", r"ast2obj_|obj2ast_|ast_type_|PyAST_|ast_repr|add_attributes|make_type|"
                    r"_PyAST_GetDocString|ast_clear|ast_dealloc|ast_type_init|astmodule"),
    ("compiler", r"compiler_|codegen_|_PyCodegen|_PyCompile|compile_|_PyCfg|cfg_|_PyAssemble|"
                 r"assemble_|symtable_|_PySymtable|PySymtable|_PyCode_|PyCode_|optimize_|fold_|"
                 r"astfold_|_PyFlowGraph|basicblock_|instr_sequence|_PyInstructionSequence|"
                 r"write_location|insert_superinstructions|resolve_jump_offsets|stackdepth|"
                 r"remove_unused_consts|_Py_Mangle|Py_CompileString|_PyAST_Compile|_PyAST_Optimize|"
                 r"analyze_|update_symbols|inline_comprehension|_Py_Validate|validate_|"
                 r"_PyCfgBuilder|calculate_stackdepth|mark_|convert_pseudo|duplicate_exits|"
                 r"optimize_cfg|label_exception_targets|add_checks_for_loads|fix_cell_offsets|"
                 r"_PyST_|ste_|_PyCode_New|intern_strings|intern_constants|"
                 r"_PyCode_Validate|_PyCode_InitAddressRange|PyCode_NewWithPosOnlyArgs|code_"),
    ("str", r"unicode|PyUnicode|_PyUnicode|UnicodeWriter|ucs1lib_|ucs2lib_|ucs4lib_|asciilib_|"
            r"_Py_HashBytes|siphash|pysiphash|_Py_KeyedHash|ascii_decode|fastsearch|default_find|"
            r"adaptive_find|two_way|_Py_DecodeUTF8|utf8_|_Py_EncodeUTF8|make_bloom_mask|tailmatch|"
            r"do_strip|do_argstrip|split_whitespace|MarkupIterator|render_field|do_markup|"
            r"parse_field|build_string|format_string_internal|_PyUnicode_FormatAdvanced|"
            r"get_integer|FieldNameIterator|field_name_split|output_markup|any_find_slice|"
            r"replace$|anylib_|_Py_normalize_encoding|_PyUnicode_JoinArray|_PyUnicode_Equal|"
            r"str_|_Py_str_|charmap|_Py_ascii|find_maxchar|_copy_characters|resize_compact|"
            r"_PyUnicodeWriter|ascii_iseq|_Py_bytes_lower|_PyUnicode_Copy|PyUnicode_Concat|"
            r"_PyUnicode_XStrip|_PyStaticUnicode|_Py_hashtable_hash_ptr|formatfloat|formatlong|"
            r"mainformatlong|_PyUnicode_FormatLong|unicode_format_arg|_Py_utf8|encode_wstr|"
            r"_Py_DecodeLocale|_Py_EncodeLocale|_PyUnicodeTranslate|fill_padding|calc_padding|"
            r"parse_internal_render_format_spec|format_obj|get_integer_value"),
    ("bytes", r"bytes|bytearray|_Py_bytes_|_PyBytes|PyBytes|PyByteArray|stringlib_|_PyByteArray|"
              r"return_self|_Py_strhex|_Py_hashtable"),
    ("int", r"long_|PyLong|_PyLong|k_mul|k_lopsided_mul|x_divrem|x_add|x_sub|int_|_Py_bit_length|"
            r"long_hash|l_divmod|l_mod|fast_mod|fast_floor_div|long_rich|_PyLong_Format|"
            r"long_to_decimal|pylong_|_PyLong_Copy|maybe_small_long|_PyLong_FromSTwoDigits"),
    ("float", r"float_|PyFloat|_PyFloat|_Py_dg_|PyOS_double_to_string|PyOS_string_to_double|"
              r"_Py_parse_inf_or_nan|Balloc|Bfree|multadd|lshift|pow5mult|quorem|mult$|diff$|"
              r"i2b|d2b|b2d|ulp|hi0bits|lo0bits|s2b|ratio|sulp|bigcomp|format_float_short|"
              r"_Py_string_to_number_with_underscores|_PyOS_ascii_strtod|_Py_double_round"),
    ("dict", r"dict|PyDict|_PyDict|insertdict|insert_to_emptydict|unicodekeys_lookup|_Py_dict_lookup|"
             r"lookdict|dictresize|new_keys_object|free_keys_object|clone_combined_dict_keys|"
             r"insert_split|insert_combined_dict|compare_unicode|do_lookup|dk_|find_empty_slot|"
             r"build_indices|copy_values|make_dict_from_instance_attributes|"
             r"_PyObject_MaterializeManagedDict|_PyObjectDict|_PyDictKeys|_PyDictView|"
             r"unicode_get_hash|insertion_resize|new_dict|_PyDict_|store_instance_attr|"
             r"_PyObject_StoreInstanceAttribute|_PyObject_TryGetInstanceAttribute|"
             r"_PyObject_GetManagedDict|_PyObject_InitInlineValues|_PyObject_SetManagedDict|"
             r"_PyObject_ClearManagedDict|_PyObject_IsInstanceDictEmpty|delete_index_from_values|"
             r"insert_into_splitdictkeys|_PyDictKeys_StringLookup|lookup_|setitem_"),
    ("list/tuple/set", r"list|PyList|_PyList|tuple|PyTuple|_PyTuple|set_|setiter|PySet|_PySet|"
                       r"frozenset|make_new_set|ins1|app1|binarysort|count_run|merge_|gallop_|"
                       r"unsafe_|safe_object_compare|islt|sortslice|reverse_slice|set_add_entry|"
                       r"set_lookkey|set_table_resize|set_update|set_contains|set_discard|"
                       r"_PySet_|make_new_frozenset|set_issubset|set_richcompare|set_intersection|"
                       r"set_difference|set_union|set_symmetric|setentry|set_insert_clean|"
                       r"_PyTuple_|_PyList_|_Py_union|structseq_new|PyStructSequence_New"),
    ("isinstance/abc", r"PyObject_IsInstance|object_isinstance|recursive_isinstance|PyObject_IsSubclass|"
                       r"object_issubclass|recursive_issubclass|abstract_issubclass|_abc_|_abc__|"
                       r"abstract_get_bases|check_class|_PyObject_RealIsInstance|_PyObject_RealIsSubclass|"
                       r"compute_abstract_methods|_abc_data|_in_weak_set|_add_to_weak_set"),
    ("attr/type", r"_PyType_Lookup|_PyType_|PyType_|type_|PyObject_GenericGetAttr|_PyObject_GenericGetAttr|"
                  r"PyObject_GenericSetAttr|_PyObject_GenericSetAttr|PyObject_GetAttr|PyObject_SetAttr|"
                  r"_PyObject_GetMethod|_PyObject_LookupAttr|PyObject_GetOptionalAttr|slot_|super_|"
                  r"object_|module_getattro|_Py_module_getattro|PyModule_|_PyModule|module_|"
                  r"_PyObject_LookupSpecial|_PyObject_GetAttrId|property_|member_get|member_set|"
                  r"PyMember_|getset_get|getset_set|method_get|descr_|classmethod_|staticmethod_|"
                  r"cm_descr_get|sm_descr_get|func_descr_get|find_name_in_mro|lookup_maybe_method|"
                  r"vectorcall_unbound|update_one_slot|mro_|_PyObject_GetInstanceDict|"
                  r"PyObject_HasAttr|_PyObject_SetAttrId|PyObject_DelAttr|_Py_type_getattro|"
                  r"_PyObject_GenericGetAttrWithDict|_PyObject_GetAttr|lookup_method|lookup_tp|"
                  r"tp_new_wrapper|wrap_|_PyStaticType|subtype_|best_base|solid_base|"
                  r"PyObject_SelfIter|_PyType_CacheGetItemForSpecialization|func_get|"
                  r"_PyObject_SetAttributeErrorContext|_PyObject_FunctionStr|lookup_special"),
    ("exceptions", r"_PyErr_|PyErr_|BaseException|_PyException|PyException_|exc_|_PyTraceBack|"
                   r"PyTraceBack_|tb_|OSError_|oserror_|StopIteration_|_Py_Offer_Suggestions|do_raise|"
                   r"_PyExc_|AttributeError_|KeyError_|ImportError_|SyntaxError_|NameError_|"
                   r"_Py_CalculateSuggestions|_PyErr_SetKeyError|format_exc|traceback_"),
    ("builtins", r"builtin_|_PyBuiltin|bltin"),
    ("iter/gen", r"gen_|_PyGen_|PyGen_|coro_|async_gen_|_PyCoro|enum_|reversed_|range|rangeiter|"
                 r"longrangeiter|iter_|calliter_|zip_|map_|filter_|seqiter|PySeqIter|itertools|"
                 r"_PyCallIter|islice_|chain_|product_|groupby_|_grouper|count_|repeat_|cycle_|"
                 r"accumulate_|tee|pairwise_|batched_|compress_|dropwhile_|takewhile_|starmap_|"
                 r"combinations_|permutations_|zip_longest|_PyGen|gen_send_ex|PyIter_|anextawaitable|"
                 r"make_gen|_Py_MakeCoro|compute_cr_origin|enumerate|_PyAsyncGen"),
    ("functools", r"functools|partial_|lru_cache|bounded_lru_cache|infinite_lru_cache|"
                  r"uncached_lru_cache|keyobject_|_functools|reduce|cmp_to_key|placeholder"),
    ("collections", r"deque|defdict_|_collections|tuplegetter_|dequeiter_|_count_elements"),
    ("operator", r"itemgetter_|attrgetter_|methodcaller_|_operator_|operator_|dotted_getattr"),
    ("io", r"_io_|_PyIO|textiowrapper|_io__|buffered|_bufferedreader|_bufferedwriter|fileio_|"
           r"stringio_|bytesio_|iobase_|_PyIncrementalNewlineDecoder|incrementalnewlinedecoder|"
           r"_textiowrapper|_PyIOBase|write_str|_io_TextIOWrapper|_PyFileIO|_Py_write|_Py_read|"
           r"_Py_open|_bufferedreader_|_bufferedwriter_|fileio|_io_open|_PyTextIOWrapper|"
           r"textiowrapper_|_io_StringIO|_io_BytesIO|_PyBytesIO|realize|resize_buffer"),
    ("codecs", r"_codecs|PyCodec_|_PyCodec|codec|utf_8_|_PyCodecInfo|normalizestring"),
    ("os", r"os_|posix_|_Py_stat|_Py_fstat|_Py_wstat|fill_time|_pystat_|path_converter|path_cleanup|"
           r"os__|DirEntry|ScandirIterator|_posix|PyOS_FSPath|structseq_|PyStructSequence|"
           r"_Py_fopen|_Py_dup|_Py_set_inheritable|_Py_get_inheritable|_PyLong_FromDev|"
           r"_PyLong_FromUid|_PyLong_FromGid|_Py_device_encoding|_Py_wgetcwd|_Py_isabs|"
           r"_Py_normpath|_Py_abspath|_Py_join_relfile|_Py_add_relfile|_PyOS_|PyOS_|_Py_wreadlink|"
           r"_Py_wrealpath|_Py_EncodeLocaleRaw|_Py_DecodeLocaleEx|_Py_closerange|_PyPathConfig"),
    ("time", r"time_|_PyTime_|PyTime_|pytime_|_PyDeadline"),
    ("contextvars", r"context|PyContext|_PyContext|contextvar|hamt|_PyHamt"),
    ("locks", r"_PyMutex|PyMutex|_PyParkingLot|_PySemaphore|PyThread_|_PyRecursiveMutex|lock_|rlock_|"
              r"_PyOnceFlag|_PyRWMutex|take_gil|drop_gil|_PyCriticalSection|PyCriticalSection|"
              r"_PyEvent|PyEvent|_PySeqLock|_Py_yield|parking_lot"),
    ("sys", r"sys_|_PySys_|PySys_"),
    ("import", r"_PyImport|PyImport_|_imp_|import_|imp_|_Py_ext_module|_PyImportZip|"
               r"_PyModule_CreateInitialized|_Py_IsFrozen|find_frozen|unmarshal_frozen_code|"
               r"is_builtin|_PyRuntime_|exec_builtin_or_dynamic|_PyFrozen"),
    ("abstract/object", r"PyNumber_|PyObject_GetItem|PyObject_SetItem|PySequence_|PyMapping_|"
                        r"PyObject_GetIter|binary_op|binary_iop|ternary_op|PyObject_RichCompare|"
                        r"do_richcompare|PyObject_Hash|PyObject_IsTrue|PyObject_Not|PyObject_Repr|"
                        r"PyObject_Str|PyObject_Format|PyObject_Size|PyObject_Length|_PyObject_|"
                        r"PyObject_|_Py_HashPointer|_Py_HashDouble|Py_ReprEnter|Py_ReprLeave|"
                        r"PyBuffer_|PyObject_GetBuffer|memory_|_PyManagedBuffer|mbuf_|"
                        r"_PySlice|PySlice_|slice_|_Py_NewReference|_Py_ForgetReference|"
                        r"_Py_Is|Py_Is|Py_GetConstant|bool_|PyBool_|none_|NoneType|"
                        r"notimplemented|ellipsis|_Py_hashtable|PyCell_|cell_|PyCapsule|"
                        r"_PyNamespace|namespace_|_PyEval_GetBuiltin|PyIndex_|_PyIndex"),
    ("math/numeric", r"math_|cmath_|_random|random_|genrand|_bisect|bisect_|heapq|_heapq|siftdown|"
                     r"siftup|_struct|s_pack|s_unpack|zlib|_hashlib|EVP_|_sha|_md5|_blake2|"
                     r"binascii_|_Py_bswap|array_|_decimal|mpd_|_mpd|PyDec|dec_|_Py_fma"),
    ("pickle", r"_pickle|Pickler|Unpickler|save_|load_|pickle"),
    ("datetime", r"datetime|date_|delta_|tzinfo_|new_datetime|new_date|new_delta|_PyDateTime|"
                 r"timezone_|time_time|local_timezone|ymd_to_ord|ord_to_ymd|days_before"),
    ("other-c-module", r"_csv|Reader_|_elementtree|expat|XML_|pyexpat|_asyncio|task_|future_|"
                       r"_ctypes|_sqlite|pysqlite|sqlite3|_socket|sock_|_ssl|select_|_locale|"
                       r"_tokenize|_string|_stat|_opcode|_symtable|_warnings|warn_|"
                       r"_PyWarnings|do_warn|already_warned|get_filter|setup_context|check_matched"),
]
_RULES = [(name, re.compile(rx)) for name, rx in _RULES]


def component_of(sym, dso):
    base = os.path.basename(dso or "")
    if sym is None:
        if base.startswith(("libc.", "libm.", "libpthread", "ld-linux", "libdl", "librt.")):
            return "libc"
        if base == "[kernel.kallsyms]" or base.startswith("[vdso"):
            return "kernel"
        if base.startswith("perf-") and base.endswith(".map"):
            return "interpreter"
        if ".so" in base:
            return "ext:" + _ext_name(dso)
        return "unknown"
    for name, rx in _RULES:
        if rx.match(sym):
            return name
    return "other-native"


_EXT_CACHE = {}
SO2DIST = {}


def _ext_name(dso):
    r = _EXT_CACHE.get(dso)
    if r is None:
        m = re.search(r"site-packages/([^/]+)", dso or "")
        if m:
            r = SO2DIST.get(os.path.basename(dso)) or m.group(1).split(".")[0]
        elif "lib-dynload" in (dso or ""):
            r = "stdlib-ext:" + os.path.basename(dso).split(".")[0]
        else:
            r = os.path.basename(dso or "?").split(".")[0]
        _EXT_CACHE[dso] = r
    return r


class Owners:
    """Map a Python frame's filename to an owner label."""

    def __init__(self, info):
        self.stdlib = info["stdlib"].rstrip("/") + "/"
        self.site = [p.rstrip("/") + "/" for p in info["site"]]
        self.top2dist = info["top2dist"]
        self.workload_dir = info["workload_dir"].rstrip("/") + "/"
        self.cache = {}

    def __call__(self, filename):
        r = self.cache.get(filename)
        if r is not None:
            return r
        if filename.startswith("<frozen "):
            mod = filename[8:-1]
            r = "stdlib:" + mod.split(".")[0]
        elif filename.startswith(self.workload_dir) or "/benches/workloads/" in filename:
            r = "workload"
        elif any(filename.startswith(s) for s in self.site):
            for s in self.site:
                if filename.startswith(s):
                    top = filename[len(s):].split("/")[0]
                    top = top[:-3] if top.endswith(".py") else top
                    r = "thirdparty:" + self.top2dist.get(top, top)
                    break
        elif filename.startswith(self.stdlib):
            top = filename[len(self.stdlib):].split("/")[0]
            r = "stdlib:" + (top[:-3] if top.endswith(".py") else top)
        elif filename.startswith("<"):
            r = "generated"
        else:
            r = "other:" + os.path.basename(filename)
        self.cache[filename] = r
        return r


def _env_info(python):
    code = r"""
import json, sys, sysconfig, site, importlib.metadata as md
top2dist = {}
for top, dists in md.packages_distributions().items():
    top2dist[top] = dists[0].lower().replace('_', '-')
so2dist = {}
for d in md.distributions():
    for f in d.files or ():
        if str(f).endswith('.so'):
            so2dist[str(f).split('/')[-1]] = d.metadata['Name'].lower().replace('_', '-')
print(json.dumps({"stdlib": sysconfig.get_path("stdlib"), "site": site.getsitepackages(),
                  "top2dist": top2dist, "so2dist": so2dist, "version": sys.version.split()[0],
                  "gil": getattr(sys, "_is_gil_enabled", lambda: True)(),
                  "executable": sys.executable}))
"""
    out = subprocess.run([python, "-c", code], capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def _pyframe(sym):
    # "py::Qual.name:/path/file.py"  or  "py::name:<frozen importlib._bootstrap>"
    body = sym[4:]
    i = body.find(":")
    while i != -1 and body[i + 1:i + 2] not in ("/", "<"):
        i = body.find(":", i + 1)
    if i == -1:
        return body, "?"
    return body[:i], body[i + 1:]


def parse_perf(data_path, symbolizer, owners, pids=None, stdlib_prefix=""):
    """Parse `perf script` output into per-sample records, aggregated on the fly."""
    proc = subprocess.Popen(["perf", "script", "-i", data_path, "-F", "pid,tid,ip,sym,dso"],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                            bufsize=1 << 20)
    agg = {
        "samples": 0, "owner": collections.Counter(), "component": collections.Counter(),
        "matrix": collections.Counter(), "pyself": collections.Counter(),
        "native": collections.Counter(), "entry": collections.Counter(),
        "reach": collections.Counter(), "stdlib_entry": collections.Counter(),
        "opcode_leaf": collections.Counter(), "unsymbolized": 0, "pids": collections.Counter(),
        "pyincl": collections.Counter(), "component_by_entry": collections.Counter(),
        "other_thread": collections.Counter(), "other_thread_samples": 0,
        "noframe_phase": collections.Counter(), "startup_cat": collections.Counter(),
    }
    frames = []
    pid = None
    line_rx = re.compile(r"^\s*([0-9a-f]+)\s+(.*)\s+\(([^()]*)\)\s*$")

    def flush():
        if frames:
            _account(agg, frames, symbolizer, owners, pid)

    for line in proc.stdout:
        if not line.strip():
            flush()
            frames = []
            continue
        if not line.startswith("\t"):
            flush()
            frames = []
            p = line.split()
            try:
                pid = tuple(int(x) for x in p[-1].split("/")) if p else None
            except ValueError:
                pid = None
            continue
        m = line_rx.match(line)
        if m:
            frames.append((int(m.group(1), 16), m.group(2), m.group(3)))
    flush()
    proc.wait()
    return agg


# Specialized CALL opcodes that do the builtin's work inline (no PyMethodDef.ml_meth call):
# Python/bytecodes.c _CALL_LEN -> PyObject_Length, _CALL_ISINSTANCE -> PyObject_IsInstance,
# _CALL_LIST_APPEND -> _PyList_AppendTakeRef, _CALL_TYPE_1/_STR_1/_TUPLE_1, _CALL_BUILTIN_CLASS
# (type's tp_vectorcall), CALL_ALLOC_AND_ENTER_INIT.
_INLINED_CALL_OPS = {"CALL_LEN", "CALL_ISINSTANCE", "CALL_LIST_APPEND", "CALL_TYPE_1", "CALL_STR_1",
                     "CALL_TUPLE_1", "CALL_BUILTIN_CLASS", "CALL_ALLOC_AND_ENTER_INIT"}

# Native entries that are call machinery, not a builtin's implementation (Python function
# frame setup when -X perf forces calls through vectorcall, generic CALL helpers).
_CALL_MACHINERY = {"_PyFunction_Vectorcall", "_Py_VectorCallInstrumentation_StackRefSteal",
                   "_PyObject_MakeTpCall", "_PyEval_Vector", "method_vectorcall", "slot_tp_call",
                   "_PyObject_Call_Prepend", "py_trampoline_evaluator", "PyUnstable_Code_GetExtra",
                   "_PyEvalFramePushAndInit", "<no-interpreter-frame>"}

_GC_ENTRY = {"gc_collect_main", "_Py_RunGC", "_PyGC_Collect", "gc_collect_region", "gc_collect_increment",
             "gc_collect_young", "gc_collect_full", "_PyGC_CollectNoFail"}


def _interp_like(sym):
    return sym is not None and (sym.startswith("_TAIL_CALL_") or sym.startswith("_PyEval_EvalFrame"))


def _account(agg, frames, symbolizer, owners, pidtid):
    pid, tid = pidtid if pidtid else (None, None)
    if pid != tid:
        # non-main threads (e.g. OpenBLAS workers spinning after `import numpy`): reported
        # separately, excluded from the main-thread shares
        agg["other_thread_samples"] += 1
        ip, sym, dso = frames[-1] if frames else (0, "?", "?")
        agg["other_thread"][os.path.basename(frames[0][2]) if frames else "?"] += 1
        return
    agg["samples"] += 1
    agg["pids"][pid] += 1
    resolved = []
    for ip, sym, dso in frames:
        if sym.startswith("py::"):
            resolved.append(("py", sym, dso))
            continue
        if "python3" in os.path.basename(dso) and not dso.endswith(".so"):
            s = symbolizer(ip)
            resolved.append(("c", s, dso))
        else:
            s = None if sym in ("[unknown]", "") else sym
            resolved.append(("c", s, dso))
    kind0, leaf, leaf_dso = resolved[0]
    if kind0 == "py":
        comp = "interpreter"
        leaf_name = "<py-trampoline>"
    else:
        comp = component_of(leaf, leaf_dso)
        if leaf is None and comp in ("unknown",):
            agg["unsymbolized"] += 1
        leaf_name = leaf or f"[{os.path.basename(leaf_dso)}]"
    # innermost Python frame and native frames below it
    py_idx = next((i for i, f in enumerate(resolved) if f[0] == "py"), None)
    native_ext_owner = None
    if py_idx is None:
        owner = "<no-python-frame>"
        pyfunc = None
        names = [f[1] or "" for f in resolved if f[0] == "c"]
        joined = " ".join(names)
        if re.search(r"Py_FinalizeEx|_Py_Finalize|finalize_modules|finalize_interp|_PyImport_Fini|"
                     r"_PyGC_Fini|finalize_remove_modules|_PyGC_CollectNoFail|Py_Exit|pymain_free", joined):
            phase = "finalization"
        elif re.search(r"Py_InitializeFromConfig|pyinit_|init_interp_main|pycore_|_PyRuntime_Initialize|"
                       r"Py_InitializeEx|pymain_init|init_importlib|_PyConfig", joined):
            phase = "initialization"
        elif re.search(r"_Py_RunGC|gc_collect", joined):
            phase = "gc"
        else:
            phase = "other:" + (names[-1] if names else "?")
        agg["noframe_phase"][phase] += 1
    else:
        qual, fname = _pyframe(resolved[py_idx][1])
        owner = owners(fname)
        pyfunc = f"{_short(fname)}:{qual}"
        # mypyc / C-extension code between leaf and the Python frame owns the sample
        for k in range(py_idx):
            d = resolved[k][2]
            if ".so" in os.path.basename(d) and "site-packages" in d:
                native_ext_owner = "thirdparty:" + _ext_name(d)
                break
        if native_ext_owner:
            owner = native_ext_owner + " (native)"
    agg["owner"][owner] += 1
    agg["component"][comp] += 1
    agg["matrix"][(owner, comp)] += 1
    agg["native"][leaf_name] += 1
    if pyfunc:
        agg["pyself"][pyfunc] += 1
    # Python inclusive (each function once per sample)
    seen = set()
    py_frames = [f for f in resolved if f[0] == "py"]
    for f in py_frames:
        q, fn = _pyframe(f[1])
        key = f"{_short(fn)}:{q}"
        if key not in seen:
            seen.add(key)
            agg["pyincl"][key] += 1
    # native entry: first non-dispatch native frame called by the innermost interpreter frame
    limit = py_idx if py_idx is not None else len(resolved)
    interp_i = next((i for i in range(limit) if resolved[i][0] == "c" and _interp_like(resolved[i][1])), None)
    if interp_i is None:
        entry, via = (leaf_name if py_idx is None else "<no-interpreter-frame>"), "none"
    elif interp_i == 0:
        entry, via = "<interpreter>", "interp"
        agg["opcode_leaf"][leaf_name.replace("_TAIL_CALL_", "")] += 1
    else:
        handler = resolved[interp_i][1]
        op = handler.replace("_TAIL_CALL_", "")
        # `call:<OPCODE>`: a CALL-family handler called it. Most reach the callee's
        # PyMethodDef.ml_meth / vectorcall (swappable); _INLINED_CALL_OPS do the work inline.
        via = ("call:" + op) if op.startswith(("CALL", "INSTRUMENTED_CALL")) else "op:" + op
        k = interp_i - 1
        while k > 0 and component_of(resolved[k][1], resolved[k][2]) == "calls":
            k -= 1
        entry = resolved[k][1] or f"[{os.path.basename(resolved[k][2])}]"
    gc_run = any(resolved[i][0] == "c" and resolved[i][1] in _GC_ENTRY for i in range(limit))
    if gc_run:
        entry, via = "<cyclic GC (eval breaker / allocation)>", "gc"
    agg["entry"][(entry, via)] += 1
    agg["component_by_entry"][(entry, comp)] += 1
    # reachability class
    if gc_run:
        reach = "gc-collector"
    elif comp == "profiler-artifact":
        reach = "profiler-artifact"
    elif owner.startswith("stdlib:"):
        reach = "stdlib-python-function"
        # stdlib entry: outermost frame of the innermost contiguous stdlib run
        j = py_idx
        last = j
        while j is not None and j < len(resolved):
            if resolved[j][0] == "py":
                q, fn = _pyframe(resolved[j][1])
                if not owners(fn).startswith("stdlib:"):
                    break
                last = j
            j += 1
        q, fn = _pyframe(resolved[last][1])
        agg["stdlib_entry"][f"{_short(fn)}:{q}"] += 1
    elif owner == "<no-python-frame>":
        reach = "no-python-frame"
    elif "(native)" in owner or owner.startswith("mypyc-native"):
        reach = "ext-native-code" if comp.startswith("ext:") or comp in ("unknown", "other-native") else "ext-calls-builtin:" + comp
    elif via == "interp" or comp == "interpreter":
        reach = "interpreter(tool code)"
    elif via.startswith("call:") and entry in _CALL_MACHINERY:
        reach = "interpreter(tool code)"  # Python-function call setup, generic call helpers
    elif via.startswith("call:") and entry == "type_call":
        reach = "class instantiation (type_call, tool code)"
    elif via.startswith("call:"):
        reach = ("builtin inlined by specialized CALL opcode(tool code)" if via[5:] in _INLINED_CALL_OPS
                 else "builtin-call(tool code)")
    elif via.startswith("op:"):
        reach = "opcode-internal(tool code)"
    else:
        reach = "other(tool code)"
    agg["reach"][reach] += 1
    agg["startup_cat"][_startup_category(resolved, py_idx, owner, entry, gc_run, owners)] += 1


_STARTUP_PY = (
    ("re.compile at import", ("re/_compiler.py", "re/_parser.py", "re/_casefix.py")),
    ("enum class creation", ("enum.py",)),
    ("dataclass creation", ("dataclasses.py",)),
    ("typing machinery", ("typing.py", "annotationlib.py")),
    ("namedtuple creation", ("collections/__init__.py:namedtuple",)),
    ("ABC class creation", ("<frozen abc>",)),
    ("inspect (signatures)", ("inspect.py",)),
)


def _startup_category(resolved, py_idx, owner, entry, gc_run, owners):
    if py_idx is None:
        return "no Python frame (init/finalize/threads)"
    if gc_run:
        return "cyclic GC"
    natives = [f[1] or "" for f in resolved[:py_idx] if f[0] == "c"]
    if any(n in ("marshal_loads", "r_object", "PyMarshal_ReadObjectFromString") for n in natives):
        return "unmarshal .pyc (marshal.loads)"
    if any(n.startswith(("_imp_create_dynamic", "_imp_exec_dynamic", "_PyImport_RunModInitFunc",
                         "_PyImport_LoadDynamicModule")) for n in natives) or "(native)" in owner:
        return "C-extension module init"
    pyfiles = []
    for f in resolved[py_idx:]:
        if f[0] == "py":
            q, fn = _pyframe(f[1])
            pyfiles.append(_short(fn) + ":" + q)
    for label, needles in _STARTUP_PY:
        if any(any(pf.startswith(nd) for nd in needles) for pf in pyfiles):
            return label
    if owner == "stdlib:importlib":
        if entry in ("<interpreter>",) or entry.startswith("<"):
            return "importlib bootstrap (Python code)"
        if re.match(r"os_|posix_|_Py_stat|_Py_wstat|DirEntry|os__", entry or ""):
            return "importlib: stat/listdir"
        if re.match(r"_io_|_Py_read|buffered|fileio", entry or ""):
            return "importlib: open/read files"
        if entry in ("builtin_compile",):
            return "compile source (missing .pyc)"
        return "importlib bootstrap (native helpers)"
    if entry in ("builtin___build_class__",) or (entry or "").startswith(("type_new", "type_call", "tp_new_wrapper")):
        return "class creation (other)"
    if entry in ("builtin_exec", "builtin_compile", "builtin_eval"):
        return "exec/compile/eval of generated code"
    top = pyfiles[0] if pyfiles else ""
    if owner.startswith("stdlib:"):
        return "stdlib module code (other)"
    return "third-party module code (other)"


def _short(fname):
    for marker in ("/site-packages/", "/lib/python3.15/", "/lib/python3.14/", "/lib/python3.15t/",
                   "/benches/workloads/"):
        i = fname.find(marker)
        if i != -1:
            return fname[i + len(marker):]
    return fname


# --------------------------------------------------------------------------------------
# cProfile summary
# --------------------------------------------------------------------------------------


def summarize_cprofile(paths, owners, nruns):
    import pstats

    st = pstats.Stats(*paths)
    total = sum(v[2] for v in st.stats.values()) or 1e-9
    builtins, pyfuncs = [], []
    for (fn, line, name), (cc, nc, tt, ct, callers) in st.stats.items():
        if fn == "~":
            builtins.append({"name": name, "calls": nc // max(nruns, 1), "tottime_s": tt / max(nruns, 1),
                             "tottime_share": tt / total, "cumtime_s": ct / max(nruns, 1)})
        else:
            pyfuncs.append({"func": f"{_short(fn)}:{line}({name})", "file": fn, "line": line,
                            "name": name, "owner": owners(fn), "calls": nc // max(nruns, 1),
                            "tottime_s": tt / max(nruns, 1), "tottime_share": tt / total,
                            "cumtime_s": ct / max(nruns, 1)})
    builtins.sort(key=lambda d: -d["tottime_s"])
    pyfuncs.sort(key=lambda d: -d["tottime_s"])
    by_owner = collections.Counter()
    for d in pyfuncs:
        by_owner[d["owner"]] += d["tottime_share"]
    by_owner["<builtins>"] = sum(d["tottime_share"] for d in builtins)
    return {"total_s_per_run": total / max(nruns, 1), "builtins": builtins[:60],
            "python": pyfuncs[:80], "owner_tottime_share": dict(by_owner.most_common())}


# --------------------------------------------------------------------------------------
# Orchestration
# --------------------------------------------------------------------------------------


def _norm(counter, n):
    return {k: round(v / n, 5) for k, v in counter.most_common()}


def cmd_profile(args):
    wl = os.path.abspath(args.workload)
    name = os.path.basename(wl)[:-3]
    scratch = os.path.abspath(args.scratch or tempfile.mkdtemp(prefix=f"prof-{name}-"))
    os.makedirs(scratch, exist_ok=True)
    py = args.python
    info = _env_info(py)
    info["workload_dir"] = os.path.dirname(wl)
    SO2DIST.update(info.get("so2dist", {}))
    owners = Owners(info)
    binary = os.path.realpath(subprocess.run([py, "-c", "import sys,os;print(os.path.realpath(sys.executable))"],
                                             capture_output=True, text=True).stdout.strip())
    whole = bool(args.whole_process)
    me = os.path.abspath(__file__)
    result = {"workload": name, "python": f"{info['version']}{'' if info['gil'] else 't'}",
              "binary": binary, "mode": "whole-process" if whole else "steady-state"}
    if args.update and os.path.exists(args.out):
        with open(args.out) as f:
            result.update(json.load(f))

    # 0. plain timing (no profiler) for reference and -X perf overhead
    meta = os.path.join(scratch, "time.json")
    if not (args.update and os.path.exists(meta)):
        subprocess.run([py, me, "_child", "time", wl, "--meta", meta, "--runs", str(args.time_runs)], check=True)
    with open(meta) as f:
        tm = json.load(f)
    result["wall_s"] = round(statistics.median(tm["walls"]), 4)
    result["timing"] = {"plain_runs": [round(x, 4) for x in tm["walls"]],
                        "plain_median_s": result["wall_s"],
                        "user_s_per_run": round(tm["utime"] / len(tm["walls"]), 4),
                        "sys_s_per_run": round(tm["stime"] / len(tm["walls"]), 4),
                        "kernel_share_rusage": round(tm["stime"] / max(tm["utime"] + tm["stime"], 1e-9), 4)}

    # 1. perf
    if not args.skip_perf:
        ctl, ack = os.path.join(scratch, "ctl.fifo"), os.path.join(scratch, "ack.fifo")
        for p in (ctl, ack):
            if os.path.exists(p):
                os.unlink(p)
            os.mkfifo(p)
        data = os.path.join(scratch, "perf.data")
        meta = os.path.join(scratch, "perf-meta.json")
        env = dict(os.environ, PYTHONPERFSUPPORT="1")
        cmd = ["perf", "record", "-q", "-e", "cpu-clock", "-F", str(args.freq), "-g", "-D", "-1",
               "--control", f"fifo:{ctl},{ack}", "-o", data, "--",
               py, me, "_child", "perf", wl, "--ctl", ctl, "--ack", ack, "--meta", meta,
               "--seconds", str(args.seconds), "--min-runs", "2"]
        subprocess.run(cmd, check=True, env=env)
        with open(meta) as f:
            pm = json.load(f)
        sym = Symbolizer(binary, os.path.join(scratch, "..", "nm-cache"))
        agg = parse_perf(data, sym, owners)
        n = max(agg["samples"], 1)
        result["samples"] = agg["samples"]
        result["perf"] = {"runs": len(pm["walls"]), "run_wall_median_s": round(statistics.median(pm["walls"]), 4),
                          "perf_overhead_ratio": round(statistics.median(pm["walls"]) / result["wall_s"], 3),
                          "deterministic": pm["deterministic"], "unsymbolized_leaf": agg["unsymbolized"],
                          "kernel_share_rusage": round(pm["stime"] / max(pm["utime"] + pm["stime"], 1e-9), 4),
                          "processes": len(agg["pids"])}
        result["python_owner"] = _norm(agg["owner"], n)
        result["native_component"] = _norm(agg["component"], n)
        result["matrix"] = [{"python_owner": o, "native_component": c, "share": round(v / n, 5)}
                            for (o, c), v in agg["matrix"].most_common(60)]
        result["top_python_self"] = [{"func": k, "share": round(v / n, 5)} for k, v in agg["pyself"].most_common(60)]
        result["top_python_inclusive"] = [{"func": k, "share": round(v / n, 5)} for k, v in agg["pyincl"].most_common(60)]
        result["top_native"] = [{"symbol": k, "share": round(v / n, 5)} for k, v in agg["native"].most_common(60)]
        result["native_entry"] = [{"entry": e, "via": via, "share": round(v / n, 5)}
                                  for (e, via), v in agg["entry"].most_common(60)]
        result["reach"] = _norm(agg["reach"], n)
        result["stdlib_entry"] = [{"func": k, "share": round(v / n, 5)} for k, v in agg["stdlib_entry"].most_common(40)]
        result["interpreter_leaf_opcodes"] = [{"opcode": k, "share": round(v / n, 5)}
                                              for k, v in agg["opcode_leaf"].most_common(30)]
        result["no_python_frame_phase"] = _norm(agg["noframe_phase"], n)
        if whole:
            result["startup_attribution"] = _norm(agg["startup_cat"], n)
        tot = agg["samples"] + agg["other_thread_samples"]
        result["perf"]["other_thread_samples_share"] = round(agg["other_thread_samples"] / max(tot, 1), 4)
        result["perf"]["other_thread_leaf_dso"] = dict(agg["other_thread"].most_common(5))
        if not args.keep_perf_data:
            os.unlink(data)

    # 2. cProfile
    if not args.skip_cprofile:
        outdir = os.path.join(scratch, "cprof")
        os.makedirs(outdir, exist_ok=True)
        for f in os.listdir(outdir):
            os.unlink(os.path.join(outdir, f))
        meta = os.path.join(scratch, "cprof-meta.json")
        cmd = [py, me, "_child", "cprofile", wl, "--outdir", outdir, "--meta", meta,
               "--seconds", str(args.cprofile_seconds)] + (["--whole-process"] if whole else [])
        subprocess.run(cmd, check=True)
        with open(meta) as f:
            cm = json.load(f)
        paths = [os.path.join(outdir, f) for f in sorted(os.listdir(outdir)) if f.endswith(".prof")]
        if whole:
            paths = [p for p in paths if not p.endswith("parent.prof")] or paths
        cp = summarize_cprofile(paths, owners, cm["runs"])
        result["cprofile"] = {"runs": cm["runs"], "total_s_per_run": round(cp["total_s_per_run"], 4),
                              "owner_tottime_share": {k: round(v, 4) for k, v in cp["owner_tottime_share"].items()},
                              "top_python": [{k: (round(v, 6) if isinstance(v, float) else v) for k, v in d.items()
                                              if k not in ("file", "line", "name")} for d in cp["python"][:40]]}
        result["cprofile_builtin_calls"] = [{k: (round(v, 6) if isinstance(v, float) else v) for k, v in d.items()}
                                            for d in cp["builtins"]]

        # 3. arg sizes for the top hotspots
        if not args.skip_argsizes:
            bkeys = []
            for d in cp["builtins"]:
                k = _builtin_key_from_cprofile(d["name"])
                if k and k not in bkeys and k != ("module:_lsprof", "disable") and "disable" not in d["name"]:
                    bkeys.append(k)
                if len(bkeys) >= args.top_builtins:
                    break
            pyt = []
            for d in cp["python"]:
                if d["owner"].startswith("stdlib:") and d["name"] != "<module>":
                    pyt.append((d["file"], d["line"], d["name"]))
                if len(pyt) >= args.top_stdlib:
                    break
            tpath = os.path.join(scratch, "targets.json")
            with open(tpath, "w") as f:
                json.dump({"builtins": bkeys, "python": pyt}, f)
            meta = os.path.join(scratch, "args-meta.json")
            adir = os.path.join(scratch, "args")
            os.makedirs(adir, exist_ok=True)
            for f in os.listdir(adir):
                os.unlink(os.path.join(adir, f))
            cmd = [py, me, "_child", "argsizes", wl, "--targets", tpath, "--meta", meta,
                   "--outdir", adir] + (["--whole-process"] if whole else [])
            subprocess.run(cmd, check=True)
            with open(meta) as f:
                am = json.load(f)
            if whole:
                am = {"arg_sizes": _merge_child_args(adir)}
            result["arg_sizes"] = am["arg_sizes"]
            result["arg_size_targets"] = {"builtins": [list(k) for k in bkeys], "python": [list(t) for t in pyt],
                                          "python_found": am.get("python_targets_found")}
    if whole and not (args.update and "importtime" in result):
        meta = os.path.join(scratch, "importtime.json")
        subprocess.run([py, me, "_child", "importtime", wl, "--meta", meta], check=True)
        with open(meta) as f:
            it = json.load(f)
        result["importtime"] = _summarize_importtime(it["importtime_self_us"], info)
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w") as f:
        json.dump(result, f, indent=1)
    print(f"wrote {args.out}: samples={result.get('samples')} wall={result['wall_s']}s")


def _summarize_importtime(data, info):
    import sys as _s

    stdlib_names = set(getattr(_s, "stdlib_module_names", ())) | {"encodings", "importlib", "_frozen_importlib"}
    per_import = {}
    for imp, mods in data.items():
        tot = sum(mods.values())
        std = sum(v for m, v in mods.items() if m.split(".")[0] in stdlib_names)
        top = collections.Counter()
        for m, v in mods.items():
            top[m.split(".")[0]] += v
        per_import[imp] = {"total_self_ms": round(tot / 1000, 2), "modules": len(mods),
                           "stdlib_share": round(std / max(tot, 1), 3),
                           "top_packages_ms": {k: round(v / 1000, 2) for k, v in top.most_common(6)},
                           "top_modules_ms": {k: round(v / 1000, 2) for k, v in
                                              sorted(mods.items(), key=lambda kv: -kv[1])[:8]}}
    allstd = collections.Counter()
    for imp, mods in data.items():
        for m, v in mods.items():
            if m.split(".")[0] in stdlib_names:
                allstd[m] += v
    return {"per_import": per_import,
            "stdlib_modules_total_ms": {k: round(v / 1000, 2) for k, v in allstd.most_common(30)}}


def _merge_child_args(adir):
    calls = collections.Counter()
    sizes = collections.defaultdict(list)
    types = collections.defaultdict(collections.Counter)
    for f in os.listdir(adir):
        if not f.endswith(".json"):
            continue
        with open(os.path.join(adir, f)) as fh:
            d = json.load(fh)
        calls.update(d["calls"])
        for k, v in d["sizes"].items():
            sizes[k].extend(v)
        for k, v in d["types"].items():
            types[k].update(v)
    rec = _ArgRecorder([], {})
    rec.calls, rec.sizes, rec.types = calls, sizes, types
    return rec.result()


# --------------------------------------------------------------------------------------
# Roll-up across a family
# --------------------------------------------------------------------------------------


def cmd_rollup(args):
    profs = []
    for p in args.files:
        with open(p) as f:
            profs.append(json.load(f))
    profs = [p for p in profs if "matrix" in p and (args.include_startup or p.get("mode") != "whole-process")]
    n = len(profs)
    cells = collections.Counter()
    comps = collections.Counter()
    owners = collections.Counter()
    reach = collections.Counter()
    entries = collections.Counter()
    stdlib_entry = collections.Counter()
    natives = collections.Counter()
    for p in profs:
        for c in p["matrix"]:
            cells[(c["python_owner"], c["native_component"])] += c["share"] / n
        for k, v in p["native_component"].items():
            comps[k] += v / n
        for k, v in p["python_owner"].items():
            owners[k.split(":")[0] if not k.startswith("stdlib:") else k] += v / n
        for k, v in p.get("reach", {}).items():
            reach[k] += v / n
        for e in p.get("native_entry", []):
            entries[(e["entry"], e["via"])] += e["share"] / n
        for e in p.get("stdlib_entry", []):
            stdlib_entry[e["func"]] += e["share"] / n
        for e in p.get("top_native", []):
            natives[e["symbol"]] += e["share"] / n
    print(f"# roll-up over {n} profiles: {', '.join(p['workload'] for p in profs)}\n")
    print("## top cells (owner x component), mean share")
    for (o, c), v in cells.most_common(args.top):
        print(f"| {o} | {c} | {v:.1%} |")
    print("\n## components")
    for k, v in comps.most_common(25):
        print(f"| {k} | {v:.1%} |")
    print("\n## owners (thirdparty/workload collapsed)")
    for k, v in owners.most_common(25):
        print(f"| {k} | {v:.1%} |")
    print("\n## reach")
    for k, v in reach.most_common():
        print(f"| {k} | {v:.1%} |")
    print("\n## native entries")
    for (e, via), v in entries.most_common(40):
        print(f"| {e} | {via} | {v:.2%} |")
    print("\n## stdlib entry functions")
    for k, v in stdlib_entry.most_common(40):
        print(f"| {k} | {v:.2%} |")
    print("\n## leaf symbols")
    for k, v in natives.most_common(30):
        print(f"| {k} | {v:.2%} |")


def main(argv=None):
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("profile")
    p.add_argument("workload")
    p.add_argument("--python", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--scratch")
    p.add_argument("--seconds", type=float, default=5.0)
    p.add_argument("--cprofile-seconds", type=float, default=3.0)
    p.add_argument("--freq", type=int, default=1999)
    p.add_argument("--time-runs", type=int, default=5)
    p.add_argument("--top-builtins", type=int, default=8)
    p.add_argument("--top-stdlib", type=int, default=6)
    p.add_argument("--whole-process", action="store_true")
    p.add_argument("--skip-perf", action="store_true")
    p.add_argument("--skip-cprofile", action="store_true")
    p.add_argument("--skip-argsizes", action="store_true")
    p.add_argument("--keep-perf-data", action="store_true")
    p.add_argument("--update", action="store_true",
                   help="merge into an existing --out JSON (re-run only the passes not skipped)")
    r = sub.add_parser("rollup")
    r.add_argument("files", nargs="+")
    r.add_argument("--top", type=int, default=15)
    r.add_argument("--include-startup", action="store_true")
    c = sub.add_parser("_child")
    c.add_argument("mode")
    c.add_argument("workload")
    c.add_argument("--ctl")
    c.add_argument("--ack")
    c.add_argument("--meta")
    c.add_argument("--seconds", type=float, default=5.0)
    c.add_argument("--min-runs", type=int, default=2)
    c.add_argument("--runs", type=int, default=5)
    c.add_argument("--outdir")
    c.add_argument("--targets")
    c.add_argument("--whole-process", action="store_true")
    a = ap.parse_args(argv)
    if a.cmd == "profile":
        cmd_profile(a)
    elif a.cmd == "rollup":
        cmd_rollup(a)
    else:
        {"perf": child_perf, "time": child_time, "cprofile": child_cprofile,
         "argsizes": child_argsizes, "importtime": child_importtime}[a.mode](a)


if __name__ == "__main__":
    main()
