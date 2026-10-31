"""Measurements behind docs/TAKEOVER.md: what each takeover mechanism costs.

Sections (instruction counts come from callgrind with client requests around exactly the
measured region; one valgrind process at a time, at most CHUNK cells each):

    handicap   specializer handicap of a non-exact container type: Ir per operation for
               the exact builtin, a slot-inheriting subclass (the floor for any foreign
               type) and the current rustique type (§3.1)
    construct  Ir per construct for literals/displays/comprehensions/calls: CPython, a
               zero-cost call (the floor for any call-based rewrite), an O(1) subclass
               adoption, and the actual native-tier rewrite to rustique types (§3.3)
    kernels    safe-tier kernels vs CPython's originals in the same process: fast paths
               and fallbacks (§3.4)
    pep523     cost of a pass-through PEP 523 frame-evaluation hook (candidate 3)
    compile    wall time of the native-tier rewrite at compile time and import time
    compat     boundary compatibility matrix of the candidates (no valgrind)

Usage:
    python benches/takeover_overhead.py [SECTION ...] [-o results.json]
    python benches/takeover_overhead.py report results.json

Needs valgrind (+ /usr/include/valgrind/callgrind.h) and a C compiler for the callgrind
sections. Build rustique optimised first (maturin develop --profile dev-release).
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
import textwrap
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CHUNK = 16  # cells per valgrind process (callgrind memory grows per measured region)

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
/* The cheapest possible call: the floor of any call-based rewrite. */
static PyObject *h_ident(PyObject *m, PyObject *x) { return Py_NewRef(x); }

/* O(1) adoption of a fresh exact list's items into a list subclass instance. */
static PyTypeObject *list_sub = NULL;
static PyObject *h_set_list_sub(PyObject *m, PyObject *t) { Py_XSETREF(list_sub, (PyTypeObject *)Py_NewRef(t)); Py_RETURN_NONE; }
static PyObject *h_adopt_list(PyObject *m, PyObject *x) {
    if (!PyList_CheckExact(x) || Py_REFCNT(x) != 1 || list_sub == NULL) {
        PyErr_SetString(PyExc_TypeError, "adopt_list: needs a fresh exact list");
        return NULL;
    }
    PyListObject *r = (PyListObject *)list_sub->tp_alloc(list_sub, 0);
    if (!r) return NULL;
    PyListObject *s = (PyListObject *)x;
    r->ob_item = s->ob_item; Py_SET_SIZE(r, Py_SIZE(s)); r->allocated = s->allocated;
    s->ob_item = NULL; Py_SET_SIZE(s, 0); s->allocated = 0;
    return (PyObject *)r;
}

/* A pass-through PEP 523 hook. */
static Py_ssize_t frames = 0;
static PyObject *hook(PyThreadState *ts, struct _PyInterpreterFrame *f, int exc) {
    frames++;
    return _PyEval_EvalFrameDefault(ts, f, exc);
}
static PyObject *h_pep523(PyObject *m, PyObject *on) {
    _PyInterpreterState_SetEvalFrameFunc(PyInterpreterState_Get(), PyObject_IsTrue(on) ? hook : _PyEval_EvalFrameDefault);
    return PyLong_FromSsize_t(frames);
}

static PyMethodDef methods[] = {
    {"start", h_start, METH_NOARGS, NULL}, {"stop", h_stop, METH_NOARGS, NULL},
    {"dump", h_dump, METH_O, NULL}, {"ident", h_ident, METH_O, NULL},
    {"set_list_sub", h_set_list_sub, METH_O, NULL}, {"adopt_list", h_adopt_list, METH_O, NULL},
    {"pep523", h_pep523, METH_O, NULL}, {NULL, NULL, 0, NULL},
};
static PyModuleDef_Slot slots[] = {
#ifdef Py_GIL_DISABLED
    {Py_mod_gil, Py_MOD_GIL_NOT_USED},
#endif
    {0, NULL}};
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_takeover_bench_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__takeover_bench_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper():
    """Build (once per ABI) and import the C helper."""
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-takeover-bench")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_takeover_bench_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "takeover_bench.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", target, src])
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _takeover_bench_v1  # type: ignore[import-not-found]

    return _takeover_bench_v1


# --------------------------------------------------------------------------------------
# Cells. A cell is (section, label, column, source of `fn(n)`, divisor, setup).
# --------------------------------------------------------------------------------------

UNROLL = 10


def unrolled(stmt, params):
    body = "".join(f"        {stmt}\n" for _ in range(UNROLL))
    return f"def fn(n, {params}):\n    for _ in range(n):\n{body}", UNROLL


def looped(stmt, params, per):
    return f"def fn(n, {params}):\n    for _ in range(n):\n        {stmt}\n", per


def handicap_cells():
    import rustique as rs

    class sublist(list):
        __slots__ = ()

    class subdict(dict):
        __slots__ = ()

    class subset(set):
        __slots__ = ()

    types = {"list": [("exact", list), ("subclass", sublist), ("rs", rs.List)],
             "dict": [("exact", dict), ("subclass", subdict), ("rs", rs.Dict)],
             "set": [("exact", set), ("subclass", subset), ("rs", rs.Set)]}
    P = "x, i, v, k"
    seq8 = lambda T: T(range(8))  # noqa: E731
    d8 = lambda T: T(dict.fromkeys(range(8), 1))  # noqa: E731
    specs = [
        ("list", "baseline", unrolled("x", P), seq8, (1, 5, 3)),
        ("list", "x[i]", unrolled("x[i]", P), seq8, (1, 5, 3)),
        ("list", "x[i] = v", unrolled("x[i] = v", P), seq8, (1, 5, 3)),
        ("list", "len(x)", unrolled("len(x)", P), seq8, (1, 5, 3)),
        ("list", "if x:", unrolled("if x: pass", P), seq8, (1, 5, 3)),
        ("list", "v in x (hit at 0)", unrolled("v in x", P), seq8, (1, 0, 3)),
        ("list", "v in x (miss, len 8)", unrolled("v in x", P), seq8, (1, -1, 3)),
        ("list", "a, b, c = x", unrolled("a, b, c = x", P), lambda T: T(range(3)), (1, 5, 3)),
        ("list", "x.append(v)", unrolled("x.append(v)", P), lambda T: T(), (1, 5, 3)),
        ("list", "for v in x (per item)", looped("for v in x: pass", P, 100), lambda T: T(range(100)), (1, 5, 3)),
        ("dict", "baseline", unrolled("x", P), d8, (1, 5, 3)),
        ("dict", "d[k]", unrolled("x[k]", P), d8, (1, 5, 3)),
        ("dict", "d[k] = v", unrolled("x[k] = v", P), d8, (1, 5, 3)),
        ("dict", "k in d", unrolled("k in x", P), d8, (1, 5, 3)),
        ("dict", "d.get(k)", unrolled("x.get(k)", P), d8, (1, 5, 3)),
        ("dict", "len(d)", unrolled("len(x)", P), d8, (1, 5, 3)),
        ("dict", "for k in d (per item)", looped("for v in x: pass", P, 100), lambda T: T(dict.fromkeys(range(100), 1)), (1, 5, 3)),
        ("set", "baseline", unrolled("x", P), seq8, (1, 5, 3)),
        ("set", "k in s", unrolled("k in x", P), seq8, (1, 5, 3)),
        ("set", "s.add(k) (present)", unrolled("x.add(k)", P), seq8, (1, 5, 3)),
        ("set", "len(s)", unrolled("len(x)", P), seq8, (1, 5, 3)),
        ("set", "for v in s (per item)", looped("for v in x: pass", P, 100), lambda T: T(range(100)), (1, 5, 3)),
    ]
    cells = []
    for kind, label, (src, div), make, args in specs:
        for col, T in types[kind]:
            def setup(src=src, make=make, T=T, args=args):
                ns = {}
                exec(compile(src, "<handicap>", "exec"), ns)
                fn = ns["fn"]
                return (lambda n: fn(n, make(T), *args)), None
            cells.append((f"{kind}: {label}", col, setup, div))
    return cells


def construct_cells():
    import rustique as rs
    from rustique._takeover import _rewrite

    h = helper()

    class Sub(list):
        __slots__ = ()

    h.set_list_sub(Sub)
    c100 = "[" + ", ".join(map(str, range(100))) + "]"
    d100 = "{" + ", ".join(f"'k{i}': {i}" for i in range(100)) + "}"
    s100 = "{" + ", ".join(map(str, range(100))) + "}"
    specs = [
        ("list", "[]", "[]"), ("list", "[a]", "[a]"), ("list", "[a, ..., h] (8)", "[a, b, c, d, e, f, g, h]"),
        ("list", "[0, ..., 99]", c100), ("list", "[x for x in r8]", "[x for x in r8]"),
        ("list", "[x for x in r100]", "[x for x in r100]"), ("list", "[*l4, *l4]", "[*l4, *l4]"),
        ("list", "list(l8)", "list(l8)"), ("dict", "{}", "{}"), ("dict", "{a: b}", "{a: b}"),
        ("dict", "{8 pairs}", "{a: a, b: b, c: c, d: d, e: e, f: f, g: g, h: h}"), ("dict", "{100 const pairs}", d100),
        ("dict", "{x: x for x in r8}", "{x: x for x in r8}"), ("dict", "{**d4, **d4b}", "{**d4, **d4b}"),
        ("dict", "dict(d8)", "dict(d8)"), ("set", "{a}", "{a}"), ("set", "{a, ..., h} (8)", "{a, b, c, d, e, f, g, h}"),
        ("set", "{0, ..., 99}", s100), ("set", "{x for x in r8}", "{x for x in r8}"), ("set", "set(l8)", "set(l8)"),
        ("call", "f(*l8)", "f(*l8)"), ("call", "f(**d8s)", "f(**d8s)"),
    ]
    env = dict(a=1, b=2, c=3, d=4, e=5, f=6, g=7, h=8, r8=list(range(8)), r100=list(range(100)), l4=[1, 2, 3, 4],
               l8=list(range(8)), d4={1: 1, 2: 2, 3: 3, 4: 4}, d4b={5: 5, 6: 6, 7: 7, 8: 8}, d8=dict.fromkeys(range(8), 0),
               d8s={f"k{i}": i for i in range(8)})
    params = ", ".join(env)
    cells = []
    for kind, label, expr in specs:
        cols = [("cpython", "plain", None)]
        if kind in ("list", "dict", "set"):
            cols.append(("call floor", "wrap", h.ident))
            if kind == "list":
                cols.append(("O(1) subclass adopt", "wrap", h.adopt_list))
            cols.append(("native tier (rs)", "rewrite", None))
        else:  # starred calls receiving a native-tier container
            cols.append(("native tier (rs)", "rsargs", None))
        for col, how, W in cols:
            def setup(expr=expr, how=how, W=W, kind=kind):
                e = expr if how in ("plain", "rewrite", "rsargs") else f"W({expr})"
                src, _ = unrolled(f"_ = {e}", params + ", W, fn_")
                src = src.replace("def fn(", "def fn_bench(")
                ns = {}
                if how == "rewrite":
                    ns.update(_rewrite.helpers())
                    code = _rewrite.compile_rewritten(src, "<construct>")
                else:
                    code = compile(src, "<construct>", "exec")
                exec(code, ns)
                fn = ns["fn_bench"]
                local = dict(env)
                if how == "rsargs":
                    local["l8"], local["d8s"] = rs.List(env["l8"]), rs.Dict(env["d8s"])
                callee = lambda *a, **k: None  # noqa: E731
                local.pop("f")
                return (lambda n: fn(n, f=callee, W=W, fn_=None, **local)), None
            cells.append((f"{kind}: {label}", col, setup, UNROLL))
    cells.append(("loop baseline", "cpython", lambda: _baseline(params, env), UNROLL))
    return cells


def _baseline(params, env):
    src, _ = unrolled("_ = a", params)
    ns = {}
    exec(compile(src, "<base>", "exec"), ns)
    fn = ns["fn"]
    return (lambda n: fn(n, **env)), None


def kernel_cells():
    import random

    from rustique import _rustique

    K = _rustique._takeover
    rng = random.Random(3)
    i8 = [5, 3, 8, 1, 9, 2, 7, 4]
    env = dict(i8=i8, f8=[x + 0.5 for x in i8], s8=["pear", "fig", "kiwi", "apple", "lime", "date", "plum", "yuzu"],
               t8=[(x, x) for x in i8], i100=rng.sample(range(1000), 100), s100=[f"w{x}" for x in rng.sample(range(1000), 100)],
               i1k=rng.sample(range(10**6), 1000), o8=[object() for _ in range(8)], ti8=tuple(i8),
               m40=list(range(20)) + [object() for _ in range(20)], e0=[], a=3, k=lambda v: -v)
    params = ", ".join(env)
    specs = [
        ("x[i] (untouched)", "i8[3]"), ("x.append(v); x.pop() (untouched)", "i8.append(a); i8.pop()"),
        ("y = x.copy(); y.sort(), 8 ints", "y = i8.copy(); y.sort()"), ("y = x.copy(); y.sort(), 8 floats", "y = f8.copy(); y.sort()"),
        ("y = x.copy(); y.sort(), 8 strs", "y = s8.copy(); y.sort()"), ("sorted(8 ints)", "sorted(i8)"),
        ("sorted(8 ints, reverse=True)", "sorted(i8, reverse=True)"), ("sorted(100 ints)", "sorted(i100)"),
        ("sorted(100 strs)", "sorted(s100)"), ("sorted(1000 ints)", "sorted(i1k)"),
        ("fallback: sorted(8 tuples)", "sorted(t8)"), ("fallback: sorted(8 ints, key=f)", "sorted(i8, key=k)"),
        ("fallback: sorted([])", "sorted(e0)"),
        ("v in 8 ints (miss)", "-1 in i8"), ("v in 8 ints (hit at 0)", "5 in i8"), ("v in 8 strs (miss)", "'zz' in s8"),
        ("v in 100 ints (miss)", "-1 in i100"), ("v in tuple of 8 ints (miss)", "-1 in ti8"), ("v in [] (miss)", "-1 in e0"),
        ("fallback: v in 8 objects (miss)", "a in o8"), ("fallback: v in 8 tuples (miss)", "(0, 0) in t8"),
        ("fallback: v in 20 ints + 20 objects (miss)", "-1 in m40"),
        ("x.index(v), 8 ints (at 4)", "i8.index(9)"), ("x.count(v), 8 ints", "i8.count(5)"),
        ("x.count(v), 100 strs", "s100.count('w5')"), ("fallback: x.index(v, 0)", "i8.index(9, 0)"),
        ("fallback: x.count(v), 8 objects", "o8.count(a)"), ("fallback: x.index(v), 8 tuples (at 7)", "t8.index((4, 4))"),
    ]
    cells = []
    for label, stmt in specs:
        for col in ("cpython", "kernels"):
            def setup(stmt=stmt, col=col):
                src, _ = unrolled(stmt, params)
                ns = {}
                exec(compile(src, "<kernels>", "exec"), ns)
                fn = ns["fn"]
                (K.install if col == "kernels" else K.uninstall)()
                return (lambda n: fn(n, **env)), K.uninstall
            cells.append((label, col, setup, UNROLL))
    return cells


PEP523_SRC = textwrap.dedent("""
    def fib(n):
        return n if n < 2 else fib(n - 1) + fib(n - 2)
    class O:
        def m(self, v):
            return v
    def calls(n, o):
        for i in range(n):
            o.m(i)
    def gen(n):
        for i in range(n):
            yield i
    def gens(n):
        for _ in gen(n):
            pass
    def loop(n, x):
        t = 0
        for i in range(n):
            t += x[i & 7]
        return t
    def comp(n):
        return [i * 2 for i in range(n)]
""")


def pep523_cells():
    h = helper()
    specs = [("fib(15) (per call)", lambda ns: (lambda n: [ns["fib"](15) for _ in range(n)]), 1973),
             ("o.m(i) method call", lambda ns: (lambda n: ns["calls"](n * 10, ns["O"]())), 10),
             ("for _ in generator (per item)", lambda ns: (lambda n: ns["gens"](n * 10)), 10),
             ("t += x[i & 7] loop, no calls", lambda ns: (lambda n: ns["loop"](n * 10, list(range(8)))), 10),
             ("[i * 2 for i in range(n)] (per item)", lambda ns: (lambda n: ns["comp"](n * 10)), 10)]
    cells = []
    for label, make, div in specs:
        for col in ("default", "pass-through hook"):
            def setup(make=make, col=col):
                ns = {}
                exec(compile(PEP523_SRC, "<pep523>", "exec"), ns)
                h.pep523(col != "default")
                return make(ns), (lambda: h.pep523(False))
            cells.append((label, col, setup, div))
    return cells


SECTIONS = {"handicap": handicap_cells, "construct": construct_cells, "kernels": kernel_cells, "pep523": pep523_cells}
N_LOOPS = {"handicap": 400, "construct": 300, "kernels": 200, "pep523": 6}


# --------------------------------------------------------------------------------------
# Worker (runs under callgrind) and driver
# --------------------------------------------------------------------------------------


def worker(section, lo, hi):
    h = helper()
    cells = SECTIONS[section]()[lo:hi]
    n = N_LOOPS[section]
    jit = getattr(sys, "_jit", None)
    # The JIT compiles a loop after thousands of back edges: warm up long enough for it.
    warm = max(2, n // 2) if not (jit and jit.is_enabled()) else max(n * 15, 6000) // (1 if section != "pep523" else 100)
    for i, (label, col, setup, _div) in enumerate(cells):
        run, cleanup = setup()
        run(warm)  # warm up / specialise (/ JIT-compile)
        run(n)
        h.start()
        run(n)
        h.dump(f"{lo + i}|a")
        h.stop()
        h.start()
        run(2 * n)
        h.dump(f"{lo + i}|b")
        h.stop()
        if cleanup:
            cleanup()


def run_section(section, env_extra=None):
    cells = SECTIONS[section]()
    n = N_LOOPS[section]
    totals = {}
    for lo in range(0, len(cells), CHUNK):
        hi = min(len(cells), lo + CHUNK)
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "cg.out")
            cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
                   "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable, os.path.abspath(__file__),
                   "_worker", section, str(lo), str(hi)]
            env = {**os.environ, "PYTHONHASHSEED": "0", **(env_extra or {})}
            p = subprocess.run(cmd, env=env, capture_output=True, text=True)
            if p.returncode:
                sys.exit(f"{section} worker failed:\n{p.stderr[-3000:]}")
            for fn in glob.glob(out + "*"):
                name = tot = None
                with open(fn) as f:
                    for line in f:
                        if line.startswith("desc: Trigger: Client Request: "):
                            name = line.split("Client Request: ", 1)[1].strip()
                        elif line.startswith(("totals:", "summary:")):
                            tot = int(line.split()[1])
                            break
                if name:
                    totals[name] = tot
    rows = {}
    for i, (label, col, _setup, div) in enumerate(cells):
        a, b = totals[f"{i}|a"], totals[f"{i}|b"]
        rows.setdefault(label, {})[col] = (b - a) / (n * div)  # fixed costs cancel
    return rows


def compile_section():
    """Wall time: compile() vs the native-tier rewrite on stdlib sources, and import with
    the hook from a cold vs a warm bytecode cache."""
    import importlib.util

    from rustique._takeover import _rewrite

    names = ["typing", "argparse", "dataclasses", "enum", "inspect", "pathlib._local", "json.encoder", "collections"]
    rows = {}
    total_plain = total_rw = 0.0
    for name in names:
        path = importlib.util.find_spec(name).origin
        with open(path, "rb") as f:
            src = f.read()

        def best(fn, k=7):
            t = []
            for _ in range(k):
                t0 = time.perf_counter()
                fn()
                t.append(time.perf_counter() - t0)
            return min(t)

        plain = best(lambda: compile(src, path, "exec", dont_inherit=True))
        rw = best(lambda: _rewrite.compile_rewritten(src, path))
        n = _rewrite.rewrite_tree(__import__("ast").parse(src))[1]
        rows[name] = {"lines": src.count(b"\n"), "compile ms": plain * 1e3, "rewrite+compile ms": rw * 1e3,
                      "ratio": rw / plain, "rewritten nodes": n}
        total_plain += plain
        total_rw += rw
    rows["total"] = {"compile ms": total_plain * 1e3, "rewrite+compile ms": total_rw * 1e3, "ratio": total_rw / total_plain}
    # Import through the hook: cold (rewrite + write the opt-rsnative pyc) vs warm (pyc hit).
    # Both variants import rustique and enable the safe tier before the clock starts, so the
    # only difference is the native tier for `big_mod`.
    with tempfile.TemporaryDirectory() as d:
        src = importlib.util.find_spec("argparse").origin
        mod = os.path.join(d, "big_mod.py")
        with open(src, "rb") as f, open(mod, "wb") as g:
            g.write(f.read())
        pre = "import time, rustique, ast, importlib.util, re, fnmatch, marshal; "
        variants = {"plain": pre + "rustique.enable(); ", "native": pre + "rustique.enable(native='big_mod'); "}

        def once(prefix, cold):
            if cold:
                for f in glob.glob(os.path.join(d, "__pycache__", "*")):
                    os.remove(f)
            code = prefix + "t0 = time.perf_counter(); import big_mod; print(time.perf_counter() - t0)"
            out = subprocess.run([sys.executable, "-c", code], cwd=d, capture_output=True, text=True,
                                 env={**os.environ, "PYTHONPATH": d}, check=True).stdout
            return float(out)

        row = {}
        for name, prefix in variants.items():
            row[f"cold {name} ms"] = min(once(prefix, True) for _ in range(5)) * 1e3
            once(prefix, False)
            row[f"warm {name} ms"] = min(once(prefix, False) for _ in range(5)) * 1e3
        rows["import a copy of argparse"] = row
    return rows


def compat_section():
    sys.path.insert(0, HERE)
    import takeover_compat  # benches/takeover_compat.py

    names, results = takeover_compat.run(time_it=False)
    return {"names": names, "rows": {f"{k}|{label}": row for (k, label), row in results.items()}}


def table(title, rows, cols, ref):
    out = [f"\n### {title}\n", "| case | " + " | ".join(cols) + " |", "|---|" + "---:|" * len(cols)]
    for label, r in rows.items():
        base = r.get(ref)
        cells = []
        for c in cols:
            v = r.get(c)
            if v is None:
                cells.append("–")
            elif c == ref or not base:
                cells.append(f"{v:.0f}")
            else:
                cells.append(f"{v:.0f} ({v / base:.2f}×)")
        out.append(f"| {label} | " + " | ".join(cells) + " |")
    return "\n".join(out)


def report(data):
    meta = data.get("meta", {})
    print(f"Python {meta.get('python')} ({'GIL' if meta.get('gil', True) else 'free-threaded'}), JIT={meta.get('jit')}")
    if "handicap" in data:
        rows = data["handicap"]
        base = {k.split(":")[0]: v for k, v in rows.items() if k.endswith("baseline")}
        adj = {}
        for k, r in rows.items():
            if k.endswith("baseline"):
                continue
            b = base[k.split(":")[0]] if "per item" not in k else {c: 0 for c in r}
            adj[k] = {c: r[c] - b[c] for c in r}
        print(table("Specializer handicap (Ir per op, baseline subtracted)", adj, ["exact", "subclass", "rs"], "exact"))
    if "construct" in data:
        rows = dict(data["construct"])
        base = rows.pop("loop baseline")["cpython"]
        adj = {k: {c: v - base for c, v in r.items()} for k, r in rows.items()}
        print(table("Construct cost (Ir per construct, loop subtracted)", adj,
                    ["cpython", "call floor", "O(1) subclass adopt", "native tier (rs)"], "cpython"))
    if "kernels" in data:
        print(table("Safe-tier kernels (Ir per statement)", data["kernels"], ["cpython", "kernels"], "cpython"))
    if "pep523" in data:
        print(table("PEP 523 pass-through hook (Ir per unit)", data["pep523"], ["default", "pass-through hook"], "default"))
    if "compile" in data:
        print("\n### Native-tier compile/import cost (wall, best of 7/5)\n")
        for k, r in data["compile"].items():
            print(f"- {k}: " + ", ".join(f"{a} {b:.2f}" if isinstance(b, float) else f"{a} {b}" for a, b in r.items()))
    if "compat" in data:
        sys.path.insert(0, HERE)
        import takeover_compat

        names = data["compat"]["names"]
        res = {tuple(k.split("|", 1)): v for k, v in data["compat"]["rows"].items()}
        print("\n### Boundary compatibility\n")
        print(takeover_compat.table(names, res))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "_worker":
        worker(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
        return
    if len(sys.argv) > 1 and sys.argv[1] == "report":
        with open(sys.argv[2]) as f:
            report(json.load(f))
        return
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("sections", nargs="*", default=["handicap", "construct", "kernels", "pep523", "compile", "compat"])
    ap.add_argument("-o", "--output", default=None)
    ap.add_argument("--jit", action="store_true", help="run the callgrind sections with PYTHON_JIT=1")
    args = ap.parse_args()
    jit = getattr(sys, "_jit", None)
    data = {"meta": {"python": sys.version.split()[0], "gil": getattr(sys, "_is_gil_enabled", lambda: True)(),
                     "jit": bool(args.jit and jit and jit.is_available())}}
    for s in args.sections:
        t0 = time.time()
        if s in SECTIONS:
            data[s] = run_section(s, {"PYTHON_JIT": "1"} if args.jit else None)
        elif s == "compile":
            data[s] = compile_section()
        elif s == "compat":
            data[s] = compat_section()
        else:
            sys.exit(f"unknown section {s}")
        print(f"[{s}: {time.time() - t0:.0f}s]", file=sys.stderr)
    if args.output:
        with open(args.output, "w") as f:
            json.dump(data, f, indent=1)
    report(data)


if __name__ == "__main__":
    main()
