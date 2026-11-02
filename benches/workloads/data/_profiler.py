#!/usr/bin/env python3
"""Profile a workload module (benches/workloads/<family>/<name>.py) the Phase-1 way.

Not a workload itself (leading underscore). Subcommands:

  profile <workload.py> --python <interp> --out <profile.json> [--seconds 4]
      Orchestrates five fresh processes of <interp>, each doing setup() + one warm-up run():
        time      plain timing of run() (wall_s, utime/stime per run)
        perf -X   `perf record -e cpu-clock -F 1999 -g` around a run() loop with
                  `python -X perf`, gated by a perf control FIFO so only steady-state
                  run() calls are sampled -> owner x component matrix
        perf      the same without -X perf (the perf trampoline turns off inlined
                  Python-to-Python calls and so inflates call overhead); gives the
                  undistorted native-component split
        cprofile  cProfile around one run(): tottime/cumtime/calls, builtin entries
        argsizes  sys.monitoring (CALL + PY_START) plus call-site probes inserted into the
                  workload's own AST around one run(): argument-size distributions
      and writes the shared profile JSON (see ~/rustique-research/PHASE1_BRIEF.md).

  rollup <profile.json>... --out <rollup.json>
      Family roll-up: mean shares of matrix cells, owners, components, native entries.

  driver ...   (internal: runs inside the profiled interpreter)

Symbolization: this perf build has no libelf, so every native frame is symbolized here:
`perf script -F ...,dso,dsoff` gives (DSO, file offset); the offset is mapped to a vaddr
through the DSO's PT_LOAD headers and looked up in `nm -n -S` (symtab, else dynsym).
This also handles the BOLT-relocated .text of python-build-standalone binaries (perf
report shows those as raw file offsets). Symbols map to components by (1) the prefix
rules of the shared brief, (2) an index of CPython source files (function name -> file
-> component), (3) library prefixes (zlib, OpenSSL), (4) the DSO name.
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))

# --------------------------------------------------------------------------------------
# driver (runs inside the profiled interpreter)
# --------------------------------------------------------------------------------------


def _load_workload(path, probe=None):
    import importlib.util

    name = "rq_workload_" + os.path.splitext(os.path.basename(path))[0]
    if probe is None:
        spec = importlib.util.spec_from_file_location(name, path)
        mod = importlib.util.module_from_spec(spec)
        sys.modules[name] = mod
        spec.loader.exec_module(mod)
        return mod
    import ast
    import types

    src = open(path, encoding="utf-8").read()
    tree = probe.transform(ast.parse(src, path))
    code = compile(tree, path, "exec")
    mod = types.ModuleType(name)
    mod.__file__ = path
    mod.__dict__.update(probe.globals())
    sys.modules[name] = mod
    exec(code, mod.__dict__)
    return mod


def _ctl(cmd):
    ctl = os.environ.get("RQ_PERF_CTL")
    if not ctl:
        return
    with open(ctl, "w") as f:
        f.write(cmd + "\n")
    with open(os.environ["RQ_PERF_ACK"]) as f:
        f.read(5)


def _env_info():
    import sysconfig

    try:
        import importlib.metadata as md

        pkgdist = {k: v[0] for k, v in md.packages_distributions().items()}
    except Exception:  # pragma: no cover
        pkgdist = {}
    return {
        "python": sys.version.split()[0] + ("t" if not getattr(sys, "_is_gil_enabled", lambda: True)() else ""),
        "executable": sys.executable,
        "stdlib": sysconfig.get_paths()["stdlib"],
        "purelib": sysconfig.get_paths()["purelib"],
        "pkgdist": pkgdist,
    }


def driver_main(args):
    out = {"env": _env_info(), "pid": os.getpid()}
    probe = None
    if args.mode == "argsizes":
        probe = ArgProbe(args.workload)
    mod = _load_workload(args.workload, probe)
    state = mod.setup()
    ref = mod.digest(mod.run(state))  # warm-up
    out["digest"] = ref
    if args.mode in ("time", "perf"):
        rusage = __import__("resource").getrusage
        walls, utimes, stimes = [], [], []
        if args.mode == "perf":
            _ctl("enable")
        t_start = time.perf_counter()
        while True:
            r0 = rusage(0)
            t0 = time.perf_counter()
            res = mod.run(state)
            t1 = time.perf_counter()
            r1 = rusage(0)
            walls.append(t1 - t0)
            utimes.append(r1.ru_utime - r0.ru_utime)
            stimes.append(r1.ru_stime - r0.ru_stime)
            if t1 - t_start >= args.seconds and len(walls) >= args.min_iters:
                break
        if args.mode == "perf":
            _ctl("disable")
        out.update(walls=walls, utimes=utimes, stimes=stimes,
                   digest_ok=mod.digest(res) == ref)
    elif args.mode == "cprofile":
        import cProfile
        import pstats

        prof = cProfile.Profile()
        t0 = time.perf_counter()
        prof.enable()
        res = mod.run(state)
        prof.disable()
        out["wall_under_cprofile"] = time.perf_counter() - t0
        out["digest_ok"] = mod.digest(res) == ref
        st = pstats.Stats(prof)
        rows = []
        for (fn, line, name), (cc, nc, tt, ct, _callers) in st.stats.items():
            rows.append({"file": fn, "line": line, "name": name, "ncalls": nc,
                         "primcalls": cc, "tottime": tt, "cumtime": ct})
        out["rows"] = rows
    elif args.mode == "argsizes":
        t0 = time.perf_counter()
        probe.start()
        try:
            res = mod.run(state)
        finally:
            probe.stop()
        out["wall_under_probe"] = time.perf_counter() - t0
        out["digest_ok"] = mod.digest(res) == ref
        out["sizes"] = probe.report()
    with open(args.out, "w") as f:
        json.dump(out, f)


class ArgProbe:
    """Argument-size recorder.

    - CALL events (all Python code): the callable and arg0 (= self for method descriptors).
    - PY_START events: all arguments of pure-Python stdlib/third-party functions, read from
      the new frame's locals.
    - Call-site probes: every call in the workload module's own code is rewritten to go
      through __rq_probe_m__/__rq_probe_f__, which see *all* arguments (e.g. the subject
      of pattern.match(s), the iterable of sep.join(xs)).
    Keys are cProfile labels ("<method 'split' of 'str' objects>", "file:line(func)") so
    they can be ranked by cProfile tottime; constructor calls are "<type 'mod.Name'>()".
    """

    DEEP = ("deepcopy", "asdict", "astuple", "dumps", "dump", "encode", "iterencode",
            "safe_dump", "replace")
    TOOL = 4

    def __init__(self, workload_path):
        import sysconfig
        import types

        self.workload = os.path.abspath(workload_path)
        self.stdlib = sysconfig.get_paths()["stdlib"]
        self.data = {}
        self.fn_types = (types.FunctionType, types.MethodType)
        self.c_types = (types.BuiltinFunctionType, types.MethodDescriptorType,
                        types.WrapperDescriptorType, types.MethodWrapperType,
                        types.ClassMethodDescriptorType)
        self.descr_types = (types.MethodDescriptorType, types.WrapperDescriptorType)
        self.code_ok = {}
        self.active = False
        self._probe_codes = set()

    # ---- AST rewrite of the workload ----
    def transform(self, tree):
        import ast

        class T(ast.NodeTransformer):
            def visit_Call(self, node):
                self.generic_visit(node)
                f = node.func
                if isinstance(f, ast.Name) and f.id in ("super", "locals", "globals", "vars",
                                                          "__import__"):
                    return node
                if isinstance(f, ast.Attribute):
                    new = ast.Call(ast.Name("__rq_probe_m__", ast.Load()),
                                   [f.value, ast.Constant(f.attr), *node.args], node.keywords)
                else:
                    new = ast.Call(ast.Name("__rq_probe_f__", ast.Load()),
                                   [f, *node.args], node.keywords)
                return ast.copy_location(new, node)

        tree = T().visit(tree)
        ast.fix_missing_locations(tree)
        return tree

    def globals(self):
        rec = self._record_c

        def __rq_probe_m__(obj, name, /, *args, **kw):
            fn = getattr(obj, name)
            if self.active:
                rec(fn, None, args, kw, via="site")
            return fn(*args, **kw)

        def __rq_probe_f__(fn, /, *args, **kw):
            if self.active:
                rec(fn, None, args, kw, via="site")
            return fn(*args, **kw)

        self._probe_codes = {__rq_probe_m__.__code__, __rq_probe_f__.__code__}
        return {"__rq_probe_m__": __rq_probe_m__, "__rq_probe_f__": __rq_probe_f__}

    # ---- sizes ----
    def _size(self, o, deep=False):
        import re as _re

        t = type(o)
        if t is str:
            return len(o), ("ascii" if o.isascii() else "nonascii")
        if t is bytes or t is bytearray:
            return len(o), None
        if t is memoryview:
            return o.nbytes, None
        if t is bool or o is None or t is float:
            return None, None
        if t is int:
            return o.bit_length(), "int-bits"
        if t is _re.Pattern:
            return len(o.pattern), "pattern"
        if deep:
            return self._deep(o), "nodes"
        if t in (list, tuple, dict, set, frozenset) or hasattr(t, "__len__") and t.__module__ in (
                "collections", "builtins"):
            try:
                return len(o), None
            except Exception:
                return None, None
        return None, "obj:" + getattr(t, "__qualname__", "?")

    def _deep(self, o, cap=2_000_000):
        seen = set()
        stack = [o]
        n = 0
        atomic = (str, int, float, bool, bytes, type(None))
        while stack and n < cap:
            x = stack.pop()
            n += 1
            if isinstance(x, atomic):
                continue
            i = id(x)
            if i in seen:
                continue
            seen.add(i)
            if isinstance(x, dict):
                stack.extend(x.keys())
                stack.extend(x.values())
            elif isinstance(x, (list, tuple, set, frozenset)):
                stack.extend(x)
            else:
                d = getattr(x, "__dict__", None)
                if isinstance(d, dict):
                    stack.extend(d.values())
                for s in getattr(type(x), "__slots__", ()) or ():
                    try:
                        stack.append(object.__getattribute__(x, s))
                    except Exception:
                        pass
        return n

    def _slot(self, key, label, size, kind, value=None):
        d = self.data.get(key)
        if d is None:
            d = self.data[key] = {}
        e = d.get(label)
        if e is None:
            e = d[label] = {"sizes": Counter(), "kinds": Counter(), "values": Counter(),
                            "n": 0}
        e["n"] += 1
        if size is not None:
            e["sizes"][size] += 1
        if kind is not None:
            e["kinds"][kind] += 1
        if value is not None and len(e["values"]) < 2000:
            e["values"][value] += 1

    def _arg(self, key, label, o, deep):
        try:
            size, kind = self._size(o, deep)
        except Exception:
            size, kind = None, "error"
        value = o if type(o) is str and len(o) <= 40 else None
        self._slot(key, label, size, kind, value)

    def _label_c(self, fn):
        import types

        t = type(fn)
        if isinstance(fn, types.BuiltinFunctionType):  # incl. 3.15's builtin_method (METH_METHOD)
            self_ = fn.__self__
            if self_ is None or isinstance(self_, types.ModuleType):
                return f"<built-in method {fn.__module__}.{fn.__name__}>", None
            for k in type(self_).__mro__:
                m = k.__dict__.get(fn.__name__)
                if m is not None:
                    return repr(m), self_
            return f"<built-in method {fn.__name__}>", self_
        if t in (types.MethodDescriptorType, types.WrapperDescriptorType,
                 types.ClassMethodDescriptorType):
            return repr(fn), None
        if t is types.MethodWrapperType:
            return f"<slot wrapper '{fn.__name__}' of '{type(fn.__self__).__name__}' objects>", fn.__self__
        if isinstance(fn, type):
            return f"<type '{fn.__module__}.{fn.__qualname__}'>()", None
        return None, None

    def _record_c(self, fn, arg0, args, kw, via):
        if not self.active:
            return
        self.active = False
        try:
            if isinstance(fn, self.fn_types):
                return
            label, self_ = self._label_c(fn)
            if label is None:
                return
            deep = any(d in label for d in self.DEEP)
            self._slot(label, "_calls_" + via, None, None)
            if self_ is not None:
                self._arg(label, "self", self_, False)
            if args is None:  # CALL event: only arg0 is known (self for descriptors)
                if arg0 is not sys.monitoring.MISSING:
                    lab = "self" if isinstance(fn, self.descr_types) else "arg0"
                    self._arg(label, lab, arg0, deep and lab == "arg0")
            else:
                for i, a in enumerate(args[:4]):
                    self._arg(label, f"arg{i}", a, deep)
        finally:
            self.active = True

    # ---- monitoring callbacks ----
    def _code_kind(self, code):
        k = self.code_ok.get(code)
        if k is None:
            fn = code.co_filename
            if code in self._probe_codes or fn == self.workload or fn == __file__:
                k = "skip"
            elif fn.startswith(self.stdlib) or fn.startswith("<frozen "):
                k = "lib"
            else:
                k = "skip" if not fn.endswith(".py") else "lib"
            self.code_ok[code] = k
        return k

    def _on_call(self, code, offset, fn, arg0):
        if not self.active:
            return None
        if self._code_kind(code) == "skip":
            return sys.monitoring.DISABLE
        if isinstance(fn, self.fn_types):
            return None
        self._record_c(fn, arg0, None, None, "call")
        return None

    def _on_start(self, code, offset):
        if not self.active:
            return None
        if self._code_kind(code) == "skip":
            return sys.monitoring.DISABLE
        self.active = False
        try:
            frame = sys._getframe(1)
            if frame.f_code is not code:
                return None
            key = f"{code.co_filename}:{code.co_firstlineno}({code.co_name})"
            self._slot(key, "_calls_py", None, None)
            names = code.co_varnames[:code.co_argcount + code.co_kwonlyargcount]
            loc = frame.f_locals
            deep = code.co_name in self.DEEP
            if code.co_name == "deepcopy" and loc.get("memo") is not None:
                deep = False  # only the outermost deepcopy call carries the graph size
            for nm in names[:4]:
                if nm in ("self", "cls"):
                    continue
                if nm in loc:
                    self._arg(key, nm, loc[nm], deep)
        except Exception:
            pass
        finally:
            self.active = True
        return None

    def start(self):
        mon = sys.monitoring
        mon.use_tool_id(self.TOOL, "rq-argsizes")
        mon.register_callback(self.TOOL, mon.events.CALL, self._on_call)
        mon.register_callback(self.TOOL, mon.events.PY_START, self._on_start)
        mon.set_events(self.TOOL, mon.events.CALL | mon.events.PY_START)
        self.active = True

    def stop(self):
        self.active = False
        mon = sys.monitoring
        mon.set_events(self.TOOL, 0)
        mon.free_tool_id(self.TOOL)

    def report(self):
        out = {}
        for key, labels in self.data.items():
            calls = {k[7:]: v["n"] for k, v in labels.items() if k.startswith("_calls_")}
            args = {}
            for lab, e in labels.items():
                if lab.startswith("_calls_"):
                    continue
                args[lab] = {
                    "n": e["n"],
                    "sizes": sorted(e["sizes"].items()),
                    "kinds": dict(e["kinds"].most_common(6)),
                    "values": e["values"].most_common(6),
                    "distinct_values": len(e["values"]),
                }
            out[key] = {"calls": calls, "args": args}
        return out


# --------------------------------------------------------------------------------------
# symbolization and classification
# --------------------------------------------------------------------------------------

_SUFFIX_RE = re.compile(r"(\.(cold|warm|hot|isra|constprop|part|lto_priv|llvm|org|localalias)"
                        r"(\.\d+)?)+$|\.\d+$")


def base_name(sym):
    s = sym
    for _ in range(4):
        n = _SUFFIX_RE.sub("", s)
        if n == s:
            break
        s = n
    return s


class ElfSyms:
    def __init__(self, path):
        self.path = path
        self.loads = []
        try:
            txt = subprocess.run(["readelf", "-lW", path], capture_output=True, text=True).stdout
        except OSError:
            txt = ""
        for line in txt.splitlines():
            p = line.split()
            if p and p[0] == "LOAD":
                off, vaddr, filesz = int(p[1], 16), int(p[2], 16), int(p[4], 16)
                self.loads.append((off, vaddr, filesz))
        self.addrs, self.ends, self.names = [], [], []
        for args in (["nm", "-n", "-S", "--defined-only", path],
                     ["nm", "-D", "-n", "-S", "--defined-only", path]):
            try:
                out = subprocess.run(args, capture_output=True, text=True).stdout
            except OSError:
                out = ""
            rows = []
            for line in out.splitlines():
                p = line.split()
                if len(p) == 4 and p[2] in "tTwWiI":
                    rows.append((int(p[0], 16), int(p[1], 16), p[3]))
                elif len(p) == 3 and p[1] in "tTwWiI":
                    rows.append((int(p[0], 16), 0, p[2]))
            if rows:
                rows.sort()
                self.addrs = [r[0] for r in rows]
                self.ends = [r[0] + (r[1] or 1 << 20) for r in rows]
                self.names = [r[2] for r in rows]
                break

    def lookup_off(self, off):
        vaddr = None
        for o, v, sz in self.loads:
            if o <= off < o + sz:
                vaddr = off - o + v
                break
        if vaddr is None:
            vaddr = off
        i = bisect.bisect_right(self.addrs, vaddr) - 1
        if i < 0 or vaddr >= self.ends[i]:
            return None
        return self.names[i]


# component rules -----------------------------------------------------------------------

PREFIX_RULES = [
    ("perf-trampoline", ("py_trampoline_evaluator", "_PyPerfTrampoline", "perf_trampoline",
                         "_PyPerf_", "perf_map_", "trampoline_")),
    ("interpreter", ("_PyEval_", "_TAIL_CALL_", "_PyFrame_", "_Py_Specialize", "PyEval_",
                     "_Py_HandlePending", "_PyStackRef", "_Py_call_instrumentation",
                     "_PyCode_", "_Py_Instrument", "_PyJIT", "_PyOptimizer")),
    ("calls", ("_PyObject_Vectorcall", "PyObject_Vectorcall", "_PyFunction_Vectorcall",
               "method_vectorcall", "cfunction_", "PyObject_Call", "_PyObject_Call",
               "_PyObject_MakeTpCall", "_Py_VectorCall", "PyVectorcall", "_PyVectorcall",
               "_PyCFunction")),
    ("memory/gc", ("gc_", "_PyObject_GC_", "PyObject_GC_", "_PyMem", "PyMem_", "mi_", "_mi_",
                   "pymalloc", "_Py_Dealloc", "_PyGC_", "PyObject_Malloc", "PyObject_Free",
                   "_PyObject_Malloc", "_PyObject_Free", "PyObject_Realloc",
                   "_PyObject_Realloc", "_Py_NewReference", "_PyType_AllocNoTrack",
                   "PyType_GenericAlloc", "PyObject_Calloc", "_PyObject_Calloc",
                   "_PyObject_New", "PyObject_Init", "_Py_DecRef", "Py_DecRef",
                   "_PyFreeList", "_Py_FreeList", "free_list")),
    ("str", ("unicode_", "PyUnicode", "_PyUnicode", "ucs1lib_", "ucs2lib_", "ucs4lib_",
             "asciilib_", "_PyUnicodeWriter", "PyUnicodeWriter")),
    ("bytes", ("bytes", "bytearray", "_Py_bytes_", "PyBytes", "_PyBytes", "PyByteArray",
               "_PyByteArray")),
    ("int", ("long_", "PyLong", "_PyLong", "k_mul", "x_divrem", "x_add", "x_sub",
             "k_lopsided_mul")),
    ("float", ("float_", "PyFloat", "_PyFloat", "_Py_dg_", "PyOS_double_to_string",
               "PyOS_string_to_double")),
    ("dict", ("dict_", "PyDict", "_PyDict", "insertdict", "dictkeys", "dictiter", "dictitems",
              "dictview", "dictvalues", "dictresize", "_Py_dict_lookup", "unicodekeys_lookup",
              "lookdict", "new_keys_object", "free_keys_object")),
    ("list/tuple/set", ("list_", "PyList", "_PyList", "listiter", "tuple", "PyTuple",
                        "_PyTuple", "set_", "PySet", "_PySet", "frozenset", "setiter")),
    ("attr/type", ("_PyType_Lookup", "PyObject_GenericGetAttr", "_PyObject_GenericGetAttr",
                   "PyObject_GenericSetAttr", "_PyObject_GenericSetAttr",
                   "_PyObject_GetMethod", "slot_tp_", "type_", "PyType_", "_PyType_",
                   "PyObject_GetAttr", "PyObject_SetAttr", "_PyObject_LookupAttr",
                   "PyObject_GetOptionalAttr", "find_name_in_mro")),
    ("re-engine", ("sre_", "_sre", "SRE_")),
    ("json-accel", ("encoder_", "scanstring", "scan_once", "py_encode_basestring", "_json_")),
]
SUFFIX_RULES = [("memory/gc", ("_dealloc", "_traverse"))]
# argument parsing / result building in C functions is call overhead; string hashing is str
EXTRA_RULES = [
    ("calls", ("vgetargs", "_PyArg_", "PyArg_", "do_mkvalue", "Py_BuildValue",
               "_Py_BuildValue", "va_build_value", "do_mktuple", "do_mkdict", "converttuple",
               "convertitem", "convertsimple", "seterror", "vgetargskeywords")),
    ("str", ("pysiphash", "_Py_HashBytes", "siphash")),
]

FILE_RULES = [
    ("Objects/unicodeobject.c", "str"), ("Objects/unicodectype.c", "str"),
    ("Objects/unicode_", "str"), ("Objects/stringlib/", "str"),
    ("Python/formatter_unicode.c", "str"), ("Python/codecs.c", "str"),
    ("Modules/_codecsmodule.c", "str"), ("Objects/bytesobject.c", "bytes"),
    ("Objects/bytearrayobject.c", "bytes"), ("Objects/bytes_methods.c", "bytes"),
    ("Objects/longobject.c", "int"), ("Python/mystrtoul.c", "int"),
    ("Objects/floatobject.c", "float"), ("Python/dtoa.c", "float"),
    ("Python/pystrtod.c", "float"), ("Objects/dictobject.c", "dict"),
    ("Objects/listobject.c", "list/tuple/set"), ("Objects/tupleobject.c", "list/tuple/set"),
    ("Objects/setobject.c", "list/tuple/set"), ("Objects/typeobject.c", "attr/type"),
    ("Objects/descrobject.c", "attr/type"), ("Objects/object.c", "attr/type"),
    ("Objects/moduleobject.c", "attr/type"), ("Objects/call.c", "calls"),
    ("Objects/methodobject.c", "calls"), ("Objects/funcobject.c", "calls"),
    ("Objects/classobject.c", "calls"), ("Objects/obmalloc.c", "memory/gc"),
    ("Objects/mimalloc/", "memory/gc"), ("Python/gc", "memory/gc"),
    ("Python/ceval", "interpreter"), ("Python/generated_cases", "interpreter"),
    ("Python/specialize.c", "interpreter"), ("Python/frame.c", "interpreter"),
    ("Python/bytecodes.c", "interpreter"), ("Objects/frameobject.c", "interpreter"),
    ("Objects/codeobject.c", "interpreter"), ("Python/instrumentation.c", "interpreter"),
    ("Python/perf_trampoline.c", "perf-trampoline"), ("Python/perf_jit", "perf-trampoline"),
    ("Objects/abstract.c", "abstract"), ("Objects/genobject.c", "generators"),
    ("Objects/exceptions.c", "exceptions"), ("Python/errors.c", "exceptions"),
    ("Python/traceback.c", "exceptions"), ("Objects/iterobject.c", "iter/range/slice"),
    ("Objects/enumobject.c", "iter/range/slice"), ("Objects/rangeobject.c", "iter/range/slice"),
    ("Objects/sliceobject.c", "iter/range/slice"), ("Objects/boolobject.c", "int"),
    ("Objects/cellobject.c", "interpreter"), ("Objects/weakrefobject.c", "other-objects"),
    ("Objects/memoryobject.c", "bytes"), ("Objects/namespaceobject.c", "attr/type"),
    ("Objects/odictobject.c", "dict"),
    ("Modules/_sre/", "re-engine"), ("Modules/_json.c", "json-accel"),
    ("Modules/_pickle.c", "pickle"), ("Modules/_datetimemodule.c", "datetime"),
    ("Modules/_zoneinfo.c", "datetime"), ("Modules/_decimal/", "decimal"),
    ("Modules/zlibmodule.c", "zlib"), ("Modules/_hashopenssl.c", "hashlib"),
    ("Modules/_hacl/", "hashlib"), ("Modules/sha", "hashlib"), ("Modules/md5module.c", "hashlib"),
    ("Modules/blake2module.c", "hashlib"), ("Modules/hmacmodule.c", "hashlib"),
    ("Modules/expat/", "expat"), ("Modules/pyexpat.c", "expat"),
    ("Modules/_elementtree.c", "elementtree"), ("Modules/_csv.c", "csv-accel"),
    ("Modules/_io/", "io"), ("Modules/posixmodule.c", "os"), ("Modules/_stat.c", "os"),
    ("Modules/unicodedata", "unicodedata"), ("Modules/_collectionsmodule.c", "collections"),
    ("Modules/itertoolsmodule.c", "itertools"), ("Modules/_functoolsmodule.c", "functools"),
    ("Modules/_operator.c", "operator"), ("Modules/_randommodule.c", "random"),
    ("Modules/mathmodule.c", "math"), ("Modules/_struct.c", "struct"),
    ("Modules/binascii.c", "binascii"), ("Modules/_bisectmodule.c", "bisect"),
    ("Modules/_heapqmodule.c", "heapq"), ("Modules/_statisticsmodule.c", "statistics"),
    ("Modules/timemodule.c", "time"), ("Modules/_uuidmodule.c", "uuid"),
    ("Modules/_abc.c", "abc"), ("Modules/_bz2module.c", "bz2"), ("Modules/_lzmamodule.c", "lzma"),
    ("Modules/_zstd/", "zstd"), ("Modules/_lsprof.c", "profiler"),
    ("Modules/_tracemalloc.c", "profiler"), ("Python/import.c", "import"),
    ("Python/marshal.c", "import"), ("Python/bltinmodule.c", "builtins-module"),
    ("Python/", "runtime"), ("Objects/", "other-objects"),
]

LIB_RULES = [
    ("expat", ("normal_", "little2_", "big2_", "XmlInitEncoding", "XmlUtf8", "XmlParse",
               "XmlPrologState", "Xml", "XML_", "initScan", "unknown_isName",
               "unknown_toUtf8", "latin1_toUtf8", "ascii_toUtf8", "utf8_toUtf8",
               "checkCharRefNumber")),
    ("zlib", ("deflate", "inflate", "crc32", "adler32", "longest_match", "fill_window",
              "compress_block", "_tr_", "zng_", "slide_hash", "zlib_", "updatewindow",
              "chunkmemset", "chunkcopy", "build_tree", "pqdownheap", "gen_bitlen",
              "send_tree", "scan_tree", "zcalloc", "zcfree", "compare256", "functable",
              "emit_", "insert_string", "quick_insert", "update_hash", "flush_pending",
              "read_buf", "bi_", "send_bits", "compress_block", "detect_data_type",
              "chunksize", "inflate_table", "fold_", "crc_", "adler_")),
    ("hashlib", ("EVP_", "evp_", "ossl_", "OPENSSL_", "CRYPTO_", "SHA1", "SHA256", "SHA512",
                 "sha1_", "sha256_", "sha512_", "MD5", "md5_", "Hacl_", "python_hashlib_",
                 "KECCAK", "keccak", "SHA3", "ERR_", "OBJ_", "BLAKE2", "blake2", "EVPXOF",
                 "md_", "digest_", "ossl", "openssl")),
    ("libm", ("__ieee754_", "__exp", "__log", "__pow", "__sin", "__cos")),
]


def _src_index(cpython_src, cache_path):
    if os.path.exists(cache_path):
        with open(cache_path) as f:
            return json.load(f)
    defre = re.compile(r"^(?:[A-Za-z_][\w \*]*\s\**)?([A-Za-z_]\w*)\s*\(")
    index = {}
    for top in ("Objects", "Python", "Modules", "Parser"):
        for dp, _dn, fns in os.walk(os.path.join(cpython_src, top)):
            for fn in fns:
                if not fn.endswith((".c", ".h")):
                    continue
                full = os.path.join(dp, fn)
                rel = os.path.relpath(full, cpython_src)
                if "/clinic/" in rel:  # attribute Argument Clinic wrappers to their module
                    rel = rel.replace("/clinic/", "/").removesuffix(".h")
                if "test" in rel.lower() and "Modules/_test" in rel:
                    continue
                try:
                    lines = open(full, encoding="utf-8", errors="replace").read().splitlines()
                except OSError:
                    continue
                for ln in lines:
                    if not ln or ln[0] in " \t#/{}*" or ln.startswith(("return", "if", "else")):
                        continue
                    m = defre.match(ln)
                    if m and not ln.rstrip().endswith(";"):
                        index.setdefault(m.group(1), rel)
    with open(cache_path, "w") as f:
        json.dump(index, f)
    return index


class Classifier:
    def __init__(self, cpython_src, cache_dir):
        self.index = _src_index(cpython_src, os.path.join(cache_dir, "cpython_src_index.json"))
        self.cache = {}

    def component(self, sym, dso):
        key = (sym, dso)
        c = self.cache.get(key)
        if c is None:
            c = self.cache[key] = self._component(sym, dso)
        return c

    def _component(self, sym, dso):
        dbase = os.path.basename(dso or "")
        if dso and ("kernel" in dso or dso.startswith("[k")):
            return "kernel"
        if sym is None:
            if dbase.startswith("libc.") or dbase.startswith("ld-linux"):
                return "libc"
            if dbase.startswith("libm."):
                return "libm"
            return self._dso_family(dbase, dso) or "unknown"
        if sym.startswith("py::"):
            return "perf-trampoline"
        if dbase.startswith("libc.") or dbase.startswith("ld-linux"):
            return "libc"
        if dbase.startswith("libm."):
            return "libm"
        fam = self._dso_family(dbase, dso)
        if fam:
            return fam
        b = base_name(sym)
        for comp, suffixes in SUFFIX_RULES:
            if b.endswith(suffixes):
                return comp
        for comp, prefixes in PREFIX_RULES + EXTRA_RULES:
            if b.startswith(prefixes):
                return comp
        rel = self.index.get(b)
        if rel:
            for prefix, comp in FILE_RULES:
                if rel.startswith(prefix):
                    return comp
            if rel.startswith("Modules/"):
                name = os.path.basename(rel).removesuffix(".c").removesuffix("module")
                return name.lstrip("_") or "other-modules"
            return "other:" + rel
        for comp, prefixes in LIB_RULES:
            if b.startswith(prefixes):
                return comp
        return "other"

    @staticmethod
    def _dso_family(dbase, dso=""):
        if not dbase or dbase.startswith("python3") or dbase.startswith("libpython"):
            return None
        if "/site-packages/" in (dso or ""):
            top = dso.split("/site-packages/", 1)[1].split("/")[0]
            if top in ("numpy", "numpy.libs"):
                return "numpy"
            if top in ("pandas", "pandas.libs"):
                return "pandas(C/Cython)"
            return "ext:" + top
        if "perf-" in dbase and dbase.endswith(".map"):
            return "perf-trampoline"
        if dbase.startswith(("_multiarray", "_umath", "lapack_lite", "_pocketfft", "libopenblas",
                             "libscipy_openblas", "_simd", "mtrand", "_generator", "bit_generator",
                             "_mt19937", "_pcg64", "_philox", "_sfc64", "_common", "_bounded",
                             "libgfortran", "libquadmath")):
            return "numpy"
        if dbase.endswith(".so") and ("cpython-3" in dbase or "abi3" in dbase):
            return "ext:" + dbase.split(".")[0]
        if dbase.startswith("libstdc++") or dbase.startswith("libgcc"):
            return "libstdc++"
        return "lib:" + dbase.split(".so")[0]


# --------------------------------------------------------------------------------------
# perf parsing
# --------------------------------------------------------------------------------------

_LINE_RE = re.compile(r"^\s*([0-9a-f]+) (.*) \((.*)\)$")

# entry components that are interpreter-side work (bytecode-driven dealloc, generic
# attribute lookup, call dispatch) rather than an accelerable C entry point
INTERP_SIDE = {"memory/gc", "interpreter", "perf-trampoline", "attr/type", "calls"}
BUILTIN_TYPE_COMPONENTS = {"str", "bytes", "int", "float", "dict", "list/tuple/set",
                           "iter/range/slice", "builtins-module", "abstract", "other-objects",
                           "generators", "exceptions"}


GC_FRAMES = ("gc_collect", "_Py_RunGC", "_PyGC_Collect", "collect_with_callback",
             "gc_collect_main", "gc_collect_increment", "gc_collect_region")


def category(entry, entry_component, owner, in_gc=False):
    """Where a sample's time could be accelerated (see the doc for definitions)."""
    if in_gc:
        return "cyclic GC"
    if entry not in ("(interpreter)", "(call machinery)") and entry_component not in INTERP_SIDE:
        if entry_component in BUILTIN_TYPE_COMPONENTS:
            return "C entry: builtin type/function"
        return "C entry: C-accelerated module"
    if owner == "workload":
        return "interpreter: user code"
    if owner.startswith("stdlib:"):
        return "interpreter: pure-Python stdlib"
    if owner.startswith("thirdparty:"):
        return "interpreter: third-party Python"
    return "interpreter: other"


CALL_WRAPPERS = ("type_call", "_PyObject_MakeTpCall", "PyObject_Call", "_PyObject_Call",
                 "PyObject_Vectorcall", "_PyObject_Vectorcall", "vectorcall_", "slot_tp_call",
                 "wrapperdescr_call", "wrapper_call", "method_vectorcall", "cfunction_",
                 "_PyFunction_Vectorcall", "PyVectorcall", "_Py_VectorCall", "methoddescr_",
                 "classmethoddescr_", "builtin_vectorcall", "builtin_next")


class PerfParser:
    def __init__(self, classifier, owner_fn, pyname_fn):
        self.cls = classifier
        self.owner_fn = owner_fn
        self.pyname_fn = pyname_fn
        self.elfs = {}
        self.symcache = {}

    def sym(self, dso, off):
        key = (dso, off)
        s = self.symcache.get(key, 0)
        if s == 0:
            e = self.elfs.get(dso)
            if e is None:
                e = self.elfs[dso] = ElfSyms(dso) if os.path.isfile(dso) else None
            s = e.lookup_off(off) if e is not None else None
            self.symcache[key] = s
        return s

    def parse(self, data_file):
        proc = subprocess.Popen(["perf", "script", "-i", data_file, "-F",
                                 "tid,period,ip,sym,dso,dsoff"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                errors="replace")
        samples = []
        cur = None
        for line in proc.stdout:
            line = line.rstrip("\n")
            if not line.strip():
                if cur is not None and cur[1]:
                    samples.append(cur)
                cur = None
                continue
            if not line.startswith("\t") and not line.startswith(" "):
                p = line.split()
                period = int(p[1]) if len(p) > 1 and p[1].isdigit() else 1
                cur = (period, [])
                continue
            m = _LINE_RE.match(line)
            if not m or cur is None:
                continue
            ip, sym, dsopart = m.groups()
            if sym.startswith("py::"):
                cur[1].append(("py", sym[4:], None))
                continue
            dso, _, off = dsopart.rpartition("+0x")
            if not dso:
                dso, off = dsopart, "0"
            leaf = not cur[1]
            o = int(off, 16)
            s = self.sym(dso, o if leaf else max(o - 1, 0))
            cur[1].append(("n", s, dso))
        if cur is not None and cur[1]:
            samples.append(cur)
        proc.wait()
        return samples

    def aggregate(self, samples):
        cls = self.cls
        total = 0
        owner = Counter()
        comp = Counter()
        cell = Counter()
        pyself = Counter()
        native = Counter()
        entry = Counter()
        entry_owner = defaultdict(Counter)
        entry_comp = {}
        cats = Counter()
        cat_owner = defaultdict(Counter)
        for period, frames in samples:
            total += period
            kind, s, dso = frames[0]
            if kind == "py":
                leafc = "perf-trampoline"
                leafname = "py::"
            else:
                leafc = cls.component(s, dso)
                leafname = base_name(s) if s else f"[{os.path.basename(dso)}]"
                if s is None:  # unresolved (e.g. libc internals): name it by its caller
                    for k2, s4, _d4 in frames[1:4]:
                        if k2 == "n" and s4 is not None:
                            leafname += "<-" + base_name(s4)
                            break
            own = "<no-python-frame>"
            pyfunc = "<none>"
            boundary = None
            for i, (k, s2, d2) in enumerate(frames):
                if k == "py":
                    qual, _, fname = s2.partition(":")
                    own = self.owner_fn(fname)
                    if fname == "<string>" and qual.startswith("__create_fn__"):
                        own = "stdlib:dataclasses(generated)"
                    pyfunc = self.pyname_fn(fname, qual)
                    if boundary is None:
                        boundary = i
                    break
                if boundary is None and cls.component(s2, d2) in ("interpreter",
                                                                  "perf-trampoline"):
                    boundary = i
            if boundary is None:
                boundary = len(frames)
            in_gc = any(k3 == "n" and s5 is not None and base_name(s5).startswith(GC_FRAMES)
                        for k3, s5, _d5 in frames[:boundary])
            ent = "(interpreter)" if boundary == 0 else None
            if ent is None:
                for j in range(boundary - 1, -1, -1):
                    k, s3, d3 = frames[j]
                    if k == "py" or s3 is None:
                        continue
                    b3 = base_name(s3)
                    if b3.startswith(CALL_WRAPPERS):
                        continue
                    ent = b3
                    entry_comp.setdefault(ent, cls.component(s3, d3))
                    break
                if ent is None:
                    ent = "(call machinery)"
            owner[own] += period
            comp[leafc] += period
            cell[(own, leafc)] += period
            pyself[pyfunc] += period
            native[(leafname, leafc)] += period
            entry[ent] += period
            entry_owner[ent][own] += period
            cat = category(ent, entry_comp.get(ent, "interpreter"), own, in_gc)
            cats[cat] += period
            cat_owner[cat][own] += period

        def sh(x):
            return round(x / total, 5) if total else 0.0

        return {
            "samples": len(samples),
            "python_owner": {k: sh(v) for k, v in owner.most_common()},
            "native_component": {k: sh(v) for k, v in comp.most_common()},
            "matrix": [{"python_owner": o, "native_component": c, "share": sh(v)}
                       for (o, c), v in cell.most_common(60)],
            "top_python_self": [{"func": f, "share": sh(v)} for f, v in pyself.most_common(40)],
            "top_native": [{"symbol": n, "component": c, "share": sh(v)}
                           for (n, c), v in native.most_common(60)],
            "categories": {k: sh(v) for k, v in cats.most_common()},
            "category_owners": {k: {o: sh(w) for o, w in cat_owner[k].most_common(12)}
                                for k in cats},
            "top_native_entry": [
                {"entry": e, "component": entry_comp.get(e, e.strip("()")), "share": sh(v),
                 "owners": {o: sh(w) for o, w in entry_owner[e].most_common(4)}}
                for e, v in entry.most_common(40)],
        }


# --------------------------------------------------------------------------------------
# orchestration
# --------------------------------------------------------------------------------------

GROUP2 = {"xml", "email", "concurrent", "importlib", "urllib", "http", "multiprocessing"}


def make_owner_fns(env, workload):
    stdlib = env["stdlib"].rstrip("/") + "/"
    pkgdist = env.get("pkgdist", {})
    wl = os.path.abspath(workload)
    me = os.path.abspath(__file__)

    def modname(fname):
        if fname.startswith("<frozen "):
            return fname[8:-1].replace(".", "/") + ".py"
        if fname.startswith(stdlib) and "/site-packages/" not in fname:
            return fname[len(stdlib):]
        return None

    def owner(fname):
        if fname == wl:
            return "workload"
        if fname == me:
            return "<driver>"
        rel = modname(fname)
        if rel is not None:
            parts = rel.removesuffix(".py").split("/")
            if parts[-1] == "__init__":
                parts = parts[:-1]
            if parts[0] in GROUP2 and len(parts) > 1:
                return "stdlib:" + ".".join(parts[:2])
            return "stdlib:" + parts[0]
        if "/site-packages/" in fname:
            top = fname.split("/site-packages/", 1)[1].split("/")[0].removesuffix(".py")
            return "thirdparty:" + pkgdist.get(top, top)
        return "other:" + os.path.basename(fname)

    def pyname(fname, qual):
        if fname == wl:
            return f"workload:{os.path.basename(fname)}:{qual}"
        rel = modname(fname)
        if rel is not None:
            return f"{rel}:{qual}"
        if "/site-packages/" in fname:
            return fname.split("/site-packages/", 1)[1] + ":" + qual
        return f"{os.path.basename(fname)}:{qual}"

    return owner, pyname


def _run_driver(py, workload, mode, out, seconds=4.0, min_iters=3, perf_x=False, env=None):
    cmd = [py] + (["-X", "perf"] if perf_x else []) + [os.path.abspath(__file__), "driver",
                                                       workload, "--mode", mode, "--out", out,
                                                       "--seconds", str(seconds),
                                                       "--min-iters", str(min_iters)]
    return cmd


def run_perf(py, workload, tmpdir, seconds, perf_x):
    ctl = os.path.join(tmpdir, "ctl")
    ack = os.path.join(tmpdir, "ack")
    for p in (ctl, ack):
        if os.path.exists(p):
            os.unlink(p)
        os.mkfifo(p)
    data = os.path.join(tmpdir, "perf-x.data" if perf_x else "perf.data")
    out = os.path.join(tmpdir, "drv-perf-x.json" if perf_x else "drv-perf.json")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack)
    cmd = (["perf", "record", "-q", "-e", "cpu-clock", "-F", "1999", "-g", "-D", "-1",
            "--control", f"fifo:{ctl},{ack}", "-o", data, "--"]
           + _run_driver(py, workload, "perf", out, seconds, 3, perf_x))
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    with open(out) as f:
        return data, json.load(f)


def _rm_perf_map(drv, keep_dir=None):
    path = f"/tmp/perf-{drv['pid']}.map"
    try:
        if keep_dir:
            shutil.move(path, os.path.join(keep_dir, os.path.basename(path)))
        else:
            os.unlink(path)
    except OSError:
        pass


def _dist(sizes):
    """sizes: [(value, count)] -> percentiles + log2 histogram."""
    if not sizes:
        return None
    sizes = sorted(sizes)
    n = sum(c for _, c in sizes)

    def pct(p):
        k = p * (n - 1)
        acc = 0
        for v, c in sizes:
            acc += c
            if acc > k:
                return v
        return sizes[-1][0]

    hist = Counter()
    for v, c in sizes:
        b = "0" if v == 0 else f"{1 << (v.bit_length() - 1)}-{(1 << v.bit_length()) - 1}"
        hist[b] += c
    order = sorted(hist, key=lambda b: int(b.split("-")[0]))
    return {"n": n, "p50": pct(0.5), "p90": pct(0.9), "p99": pct(0.99), "max": sizes[-1][0],
            "histogram": {b: hist[b] for b in order}}


def profile_main(args):
    py = args.python
    wl = os.path.abspath(args.workload)
    name = os.path.splitext(os.path.basename(wl))[0]
    tmpdir = tempfile.mkdtemp(prefix=f"rqprof-{name}-", dir=args.tmp)
    t_all = time.time()
    log = lambda *a: print(f"[{name} {time.time() - t_all:6.1f}s]", *a, file=sys.stderr,
                           flush=True)
    steps = set(args.steps.split(","))
    if steps != set(ALL_STEPS):
        # partial re-run: update an existing profile in place
        with open(args.out) as f:
            result = json.load(f)
        if "argsizes" in steps:
            steps.add("cprofile")
    else:
        result = {"workload": name}

    # 1. plain timing (always: also yields env info and the reference digest)
    out = os.path.join(tmpdir, "drv-time.json")
    subprocess.run(_run_driver(py, wl, "time", out,
                               args.time_seconds if "time" in steps else 0, 1), check=True)
    with open(out) as f:
        tm = json.load(f)
    env = tm["env"]
    owner_fn, pyname_fn = make_owner_fns(env, wl)
    if "time" not in steps:
        steps_cprofile_argsizes(args, py, wl, tmpdir, result, owner_fn, pyname_fn, log)
        return _finish(args, result, tmpdir, log)
    result["python"] = env["python"]
    result["wall_s"] = round(statistics.median(tm["walls"]), 4)
    result["timing"] = {
        "runs": len(tm["walls"]),
        "wall_median_s": round(statistics.median(tm["walls"]), 4),
        "wall_min_s": round(min(tm["walls"]), 4),
        "wall_max_s": round(max(tm["walls"]), 4),
        "utime_per_run_s": round(statistics.median(tm["utimes"]), 4),
        "stime_per_run_s": round(statistics.median(tm["stimes"]), 4),
        "digest": tm["digest"],
        "digest_stable": tm["digest_ok"],
    }
    log("time", result["wall_s"])

    cls = Classifier(args.cpython_src, args.cache)
    parser = PerfParser(cls, owner_fn, pyname_fn)

    # 2. perf with -X perf (owners)
    data, drv = run_perf(py, wl, tmpdir, args.seconds, True)
    samples = parser.parse(data)
    agg = parser.aggregate(samples)
    _rm_perf_map(drv, tmpdir if args.keep else None)
    result["samples"] = agg["samples"]
    result["wall_s_under_perf_x"] = round(statistics.median(drv["walls"]), 4)
    for k in ("python_owner", "native_component", "matrix", "top_python_self", "top_native",
              "top_native_entry", "categories", "category_owners"):
        result[k] = agg[k]
    log("perf -X perf samples", agg["samples"])

    # 3. perf without -X perf (undistorted native split)
    data2, drv2 = run_perf(py, wl, tmpdir, args.seconds, False)
    agg2 = parser.aggregate(parser.parse(data2))
    _rm_perf_map(drv2, tmpdir if args.keep else None)
    result["plain_perf"] = {
        "samples": agg2["samples"],
        "wall_s": round(statistics.median(drv2["walls"]), 4),
        "native_component": agg2["native_component"],
        "categories": agg2["categories"],
        "top_native": agg2["top_native"][:40],
        "top_native_entry": [{k: v for k, v in e.items() if k != "owners"}
                             for e in agg2["top_native_entry"][:40]],
    }
    log("perf plain samples", agg2["samples"])
    steps_cprofile_argsizes(args, py, wl, tmpdir, result, owner_fn, pyname_fn, log)
    return _finish(args, result, tmpdir, log)


ALL_STEPS = ("time", "perf", "cprofile", "argsizes")


def steps_cprofile_argsizes(args, py, wl, tmpdir, result, owner_fn, pyname_fn, log):
    # 4. cProfile
    out = os.path.join(tmpdir, "drv-cprofile.json")
    subprocess.run(_run_driver(py, wl, "cprofile", out), check=True)
    with open(out) as f:
        cp = json.load(f)
    rows = cp["rows"]
    tot = sum(r["tottime"] for r in rows) or 1.0
    builtin = [r for r in rows if r["file"] == "~"]
    builtin.sort(key=lambda r: -r["tottime"])
    by_calls = sorted(builtin, key=lambda r: -r["ncalls"])[:20]
    keep = builtin[:40] + [r for r in by_calls if r not in builtin[:40]]
    result["cprofile_builtin_calls"] = [
        {"name": r["name"], "calls": r["ncalls"], "tottime_s": round(r["tottime"], 5),
         "tottime_share": round(r["tottime"] / tot, 5)} for r in keep]
    pyrows = [r for r in rows if r["file"] != "~" and not r["file"].endswith("_profiler.py")]
    pyrows.sort(key=lambda r: -r["tottime"])
    result["cprofile_python"] = [
        {"func": pyname_fn(r["file"], r["name"]) + f"@{r['line']}", "owner": owner_fn(r["file"]),
         "calls": r["ncalls"], "tottime_s": round(r["tottime"], 5),
         "cumtime_s": round(r["cumtime"], 5), "tottime_share": round(r["tottime"] / tot, 5)}
        for r in pyrows[:40]]
    own_tt = Counter()
    for r in rows:
        own_tt["<builtins>" if r["file"] == "~" else owner_fn(r["file"])] += r["tottime"]
    result["cprofile_owner_tottime_share"] = {k: round(v / tot, 5)
                                              for k, v in own_tt.most_common()}
    result["cprofile_meta"] = {"total_tottime_s": round(tot, 4),
                               "wall_under_cprofile_s": round(cp["wall_under_cprofile"], 4),
                               "total_calls": sum(r["ncalls"] for r in rows),
                               "builtin_calls": sum(r["ncalls"] for r in builtin),
                               "digest_ok": cp["digest_ok"]}
    log("cprofile", result["cprofile_meta"]["total_calls"], "calls")

    # 5. argument sizes for the top ~10 builtin/stdlib hotspots (ranked by cProfile tottime)
    out = os.path.join(tmpdir, "drv-argsizes.json")
    subprocess.run(_run_driver(py, wl, "argsizes", out), check=True)
    with open(out) as f:
        az = json.load(f)
    sizes = az["sizes"]
    ranked = []
    for r in builtin:
        ranked.append((r["tottime"], r["name"], r["name"], r["ncalls"]))
    for r in rows:
        if r["file"] in ("~", wl) or r["file"].endswith("_profiler.py"):
            continue
        key = f"{r['file']}:{r['line']}({r['name']})"
        ranked.append((r["tottime"], key, pyname_fn(r["file"], r["name"]), r["ncalls"]))
    ranked.sort(key=lambda x: -x[0])
    arg_sizes = {}
    for tt, key, pretty, ncalls in ranked:
        if len(arg_sizes) >= args.top_args:
            break
        rec = sizes.get(key)
        if not rec:
            continue
        args_out = {}
        for lab, e in rec["args"].items():
            d = _dist([tuple(x) for x in e["sizes"]])
            if d is None and not e["values"] and not e["kinds"]:
                continue
            item = d or {"n": e["n"]}
            unit = next(iter(e["kinds"]), None)
            if e["kinds"]:
                item["kinds"] = e["kinds"]
            if e["values"] and e["distinct_values"] <= 50:
                item["values"] = e["values"]
            item["unit"] = {"ascii": "chars", "nonascii": "chars", "nodes": "nodes(deep)",
                            "int-bits": "bits", "pattern": "pattern chars"}.get(unit, "len")
            args_out[lab] = item
        if args_out:
            arg_sizes[pretty] = {"cprofile_tottime_s": round(tt, 5), "cprofile_calls": ncalls,
                                 "observed_calls": rec["calls"], "args": args_out}
    # constructor calls (types) are invisible to cProfile: add the busiest ones separately
    ctor = sorted(((sum(v["calls"].values()), k) for k, v in sizes.items()
                   if k.startswith("<type ")), reverse=True)[:8]
    for n, k in ctor:
        if n < 1000:
            continue
        rec = sizes[k]
        args_out = {}
        for lab, e in rec["args"].items():
            d = _dist([tuple(x) for x in e["sizes"]])
            item = d or {"n": e["n"]}
            if e["kinds"]:
                item["kinds"] = e["kinds"]
            if e["values"] and e["distinct_values"] <= 50:
                item["values"] = e["values"]
            args_out[lab] = item
        arg_sizes[k] = {"cprofile_tottime_s": None, "cprofile_calls": None,
                        "observed_calls": rec["calls"], "args": args_out}
    result["arg_sizes"] = arg_sizes
    result["arg_sizes_meta"] = {"wall_under_probe_s": round(az["wall_under_probe"], 3),
                                "digest_ok": az["digest_ok"]}
    log("argsizes", len(arg_sizes))


def _finish(args, result, tmpdir, log):
    result["method"] = {
        "perf": "perf record -e cpu-clock -F 1999 -g (frame pointers), -X perf, control-FIFO "
                "gated to steady-state run() calls after one warm-up; user space only "
                "(perf_event_paranoid=2, kernel time is in timing.stime_per_run_s)",
        "owner": "innermost Python frame (perf-map trampoline) of each sample",
        "component": "leaf native symbol -> component (prefix rules, CPython source index, "
                     "library prefixes, DSO)",
        "entry": "top_native_entry: outermost non-call-wrapper native frame below the "
                 "innermost interpreter frame (the C entry point reached from bytecode), "
                 "inclusive of its callees",
        "shares": "period-weighted fractions of all samples in the gated region",
    }
    with open(args.out, "w") as f:
        json.dump(result, f, indent=1)
    if not args.keep:
        shutil.rmtree(tmpdir, ignore_errors=True)
    log("done ->", args.out)


def reparse_main(args):
    """Recompute the perf aggregates of a profile from a --keep'd temp dir (after changing
    the classifier), without re-running anything."""
    with open(args.profile) as f:
        result = json.load(f)
    tmp = args.tmpdir
    with open(os.path.join(tmp, "drv-time.json")) as f:
        env = json.load(f)["env"]
    owner_fn, pyname_fn = make_owner_fns(env, args.workload)
    parser = PerfParser(Classifier(args.cpython_src, args.cache), owner_fn, pyname_fn)
    for data, drvname, plain in (("perf-x.data", "drv-perf-x.json", False),
                                 ("perf.data", "drv-perf.json", True)):
        with open(os.path.join(tmp, drvname)) as f:
            pid = json.load(f)["pid"]
        kept = os.path.join(tmp, f"perf-{pid}.map")
        live = f"/tmp/perf-{pid}.map"
        if os.path.exists(kept) and not os.path.exists(live):
            shutil.copy(kept, live)
        try:
            agg = parser.aggregate(parser.parse(os.path.join(tmp, data)))
        finally:
            if os.path.exists(kept) and os.path.exists(live):
                os.unlink(live)
        if plain:
            result["plain_perf"].update(
                native_component=agg["native_component"], categories=agg["categories"],
                top_native=agg["top_native"][:40],
                top_native_entry=[{k: v for k, v in e.items() if k != "owners"}
                                  for e in agg["top_native_entry"][:40]])
        else:
            for k in ("python_owner", "native_component", "matrix", "top_python_self",
                      "top_native", "top_native_entry", "categories", "category_owners"):
                result[k] = agg[k]
    with open(args.profile, "w") as f:
        json.dump(result, f, indent=1)


# --------------------------------------------------------------------------------------
# roll-up
# --------------------------------------------------------------------------------------


def rollup_main(args):
    profs = []
    for p in args.profiles:
        with open(p) as f:
            profs.append(json.load(f))
    n = len(profs)

    def mean_of(getter):
        acc = Counter()
        for pr in profs:
            for k, v in getter(pr).items():
                acc[k] += v / n
        return acc

    cells = mean_of(lambda pr: {(c["python_owner"], c["native_component"]): c["share"]
                                for c in pr["matrix"]})
    owners = mean_of(lambda pr: pr["python_owner"])
    comps = mean_of(lambda pr: pr["native_component"])
    comps_plain = mean_of(lambda pr: pr["plain_perf"]["native_component"])
    entries = mean_of(lambda pr: {e["entry"]: e["share"] for e in pr["top_native_entry"]})
    entry_comp = {}
    for pr in profs:
        for e in pr["top_native_entry"]:
            entry_comp.setdefault(e["entry"], e["component"])
    pyself = mean_of(lambda pr: {f["func"]: f["share"] for f in pr["top_python_self"]})
    cats = mean_of(lambda pr: pr["categories"])
    cats_plain = mean_of(lambda pr: pr["plain_perf"]["categories"])
    out = {
        "workloads": [pr["workload"] for pr in profs],
        "categories": {k: round(v, 4) for k, v in cats.most_common()},
        "categories_plain": {k: round(v, 4) for k, v in cats_plain.most_common()},
        "top_cells": [{"python_owner": o, "native_component": c, "mean_share": round(v, 4)}
                      for (o, c), v in cells.most_common(args.top)],
        "owners": {k: round(v, 4) for k, v in owners.most_common()},
        "components": {k: round(v, 4) for k, v in comps.most_common()},
        "components_plain": {k: round(v, 4) for k, v in comps_plain.most_common()},
        "entries": [{"entry": k, "component": entry_comp.get(k), "mean_share": round(v, 4)}
                    for k, v in entries.most_common(60)],
        "python_self": [{"func": k, "mean_share": round(v, 4)}
                        for k, v in pyself.most_common(60)],
    }
    with open(args.out, "w") as f:
        json.dump(out, f, indent=1)


def main(argv=None):
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("driver")
    d.add_argument("workload")
    d.add_argument("--mode", required=True, choices=["time", "perf", "cprofile", "argsizes"])
    d.add_argument("--out", required=True)
    d.add_argument("--seconds", type=float, default=4.0)
    d.add_argument("--min-iters", type=int, default=3)
    p = sub.add_parser("profile")
    p.add_argument("workload")
    p.add_argument("--python", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--seconds", type=float, default=4.0)
    p.add_argument("--time-seconds", type=float, default=4.0)
    p.add_argument("--top-args", type=int, default=10)
    p.add_argument("--cpython-src", default=os.path.expanduser("~/src/cpython-3.15.1"))
    p.add_argument("--cache", default=os.path.expanduser("~/rustique-research/w-data"))
    p.add_argument("--tmp", default=None)
    p.add_argument("--keep", action="store_true")
    p.add_argument("--steps", default=",".join(ALL_STEPS),
                   help="subset of time,perf,cprofile,argsizes; a subset updates --out in place")
    q = sub.add_parser("reparse")
    q.add_argument("profile")
    q.add_argument("--workload", required=True)
    q.add_argument("--tmpdir", required=True)
    q.add_argument("--cpython-src", default=os.path.expanduser("~/src/cpython-3.15.1"))
    q.add_argument("--cache", default=os.path.expanduser("~/rustique-research/w-data"))
    r = sub.add_parser("rollup")
    r.add_argument("profiles", nargs="+")
    r.add_argument("--out", required=True)
    r.add_argument("--top", type=int, default=15)
    a = ap.parse_args(argv)
    {"driver": driver_main, "profile": profile_main, "rollup": rollup_main,
     "reparse": reparse_main}[a.cmd](a)


if __name__ == "__main__":
    main()
