"""Logging record path: instructions per call (callgrind, region-instrumented).

Every cell runs in its own process under callgrind: setup, warm-up, then
(Ir(2n calls) - Ir(n calls)) / n, statements unrolled 10x, so fixed costs cancel
(noise floor about +-15 Ir per call). Variants of a cell run the same statement:

    orig   plain CPython
    on     rustique.enable(only=["logging"]) before `import logging`

Usage: python benches/targets/logging_cg.py [--only SUBSTR] [--variants orig,on] [-j 3] [-o out.json]
       python benches/targets/logging_cg.py report out.json
"""

from __future__ import annotations

import glob
import json
import os
import subprocess
import sys
import sysconfig
import tempfile
from concurrent.futures import ThreadPoolExecutor

UNROLL = 10

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
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_lg_bench_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__lg_bench_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper():
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-lg-bench")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_lg_bench_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "lg_bench.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", target, src])
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _lg_bench_v1  # type: ignore[import-not-found]

    return _lg_bench_v1


VARIANTS = {
    "orig": "",
    # every logging target, installed when `logging` is imported (lazy activation)
    "on": "import rustique; rustique.enable(only=['logging'])",
}

COMMON = """
import io, json, logging, sys, time
_sio = io.StringIO()
def _logger(name, fmt=None, level=logging.INFO, style='%', datefmt=None, formatter=None):
    lg = logging.getLogger(name)
    lg.handlers.clear()
    lg.propagate = False
    lg.setLevel(level)
    h = logging.StreamHandler(_sio)
    if formatter is not None:
        h.setFormatter(formatter)
    elif fmt is not None:
        h.setFormatter(logging.Formatter(fmt, datefmt, style))
    lg.addHandler(h)
    return lg
class JsonFormatter(logging.Formatter):
    def format(self, record):
        payload = {"ts": self.formatTime(record, self.datefmt), "level": record.levelname,
                   "logger": record.name, "msg": record.getMessage(), "module": record.module,
                   "line": record.lineno}
        if record.exc_info:
            payload["exc"] = self.formatException(record.exc_info)
        return json.dumps(payload)
class JsonDictFormatter(logging.Formatter):
    def format(self, record):
        record.message = record.getMessage()
        record.asctime = self.formatTime(record)
        return json.dumps(record.__dict__, default=str)
class Obj:
    def __str__(self):
        return 'obj'
class MyFormatter(logging.Formatter):
    pass
class OwnTime(logging.Formatter):
    def formatTime(self, record, datefmt=None):
        return "T"
class OverrideLevel(logging.Logger):
    def isEnabledFor(self, level):
        return level >= 30
F_OWNTIME = OwnTime('%(asctime)s %(levelname)-8s [%(name)s] %(message)s')
F_CONV = logging.Formatter('%(asctime)s %(message)s')
F_CONV.converter = lambda t: time.gmtime(t)
L_OVR = OverrideLevel('b.ovr')
L_OVR.setLevel(logging.WARNING)
v = 12345
s = 'hello'
f = 3.25
rec = logging.LogRecord('app.http', logging.INFO, '/srv/app/handlers.py', 42, 'x=%s', (v,), None, 'handle', None)
rec_obj = logging.LogRecord('app.http', logging.INFO, '/srv/app/handlers.py', 42, 'x=%s', (Obj(),), None, 'handle', None)
F_MSG = logging.Formatter()
F_BASIC = logging.Formatter(logging.BASIC_FORMAT)
F_TIME = logging.Formatter('%(asctime)s %(levelname)-8s [%(name)s] %(message)s')
F_DATEFMT = logging.Formatter('%(asctime)s %(message)s', '%Y-%m-%dT%H:%M:%S')
F_BRACE = logging.Formatter('{asctime} {levelname} {name}:{lineno} {message}', style='{')
F_SUB = MyFormatter('%(asctime)s %(levelname)-8s [%(name)s] %(message)s')
L_MSG = _logger('b.msg')
L_BASIC = _logger('b.basic', logging.BASIC_FORMAT)
L_TIME = _logger('b.time', '%(asctime)s %(levelname)-8s [%(name)s] %(message)s')
L_JSON = _logger('b.json', formatter=JsonFormatter())
L_JSOND = _logger('b.jsond', formatter=JsonDictFormatter())
L_BRACE = _logger('b.brace', '{asctime} {levelname} {name}:{lineno} {message}', style='{')
L_SUB = _logger('b.sub', formatter=MyFormatter('%(asctime)s %(levelname)-8s [%(name)s] %(message)s'))
L_OFF = _logger('b.off', level=logging.WARNING)
L_CHILD = logging.getLogger('b.time.child.leaf')
AD = logging.LoggerAdapter(L_TIME, {'svc': 'x'})
o = Obj()
import decimal, tempfile
DEC = decimal.Decimal('12.50')
_TF = tempfile.TemporaryFile('w+')
L_FILE = logging.getLogger('b.file')
L_FILE.handlers.clear(); L_FILE.propagate = False; L_FILE.setLevel(logging.INFO)
_HF = logging.StreamHandler(_TF)
_HF.setFormatter(logging.Formatter('%(asctime)s %(levelname)-8s [%(name)s] %(message)s'))
L_FILE.addHandler(_HF)
_HS = L_TIME.handlers[0]
L_ON = _logger('b.on', '%(message)s')
try:
    1 / 0
except ZeroDivisionError:
    EI = sys.exc_info()
"""

# name -> statement (each runs UNROLL times per loop iteration)
CELLS = {
    # end-to-end records through StreamHandler(StringIO)
    "info %(message)s": "L_MSG.info('x=%s', v)",
    "info BASIC_FORMAT": "L_BASIC.info('x=%s', v)",
    "info asctime fmt": "L_TIME.info('x=%s', v)",
    "info asctime fmt, child logger": "L_CHILD.info('x=%s', v)",
    "info asctime fmt, 3 args": "L_TIME.info('a=%s b=%d c=%.2f', s, v, f)",
    "info asctime fmt, no args": "L_TIME.info('hello')",
    "info asctime fmt, extra=": "L_TIME.info('x=%s', v, extra={'rid': 'r1'})",
    "info JSON formatter (subclass)": "L_JSON.info('x=%s', v)",
    "info JSON __dict__ formatter": "L_JSOND.info('x=%s', v)",
    "info {-style": "L_BRACE.info('x=%s', v)",
    "info, time.strftime replaced [deps]": "L_TIME.info('x=%s', v)",
    "info Formatter subclass [fallback]": "L_SUB.info('x=%s', v)",
    "info obj arg [fallback]": "L_TIME.info('x=%s', o)",
    "info %(message)s, obj arg [fallback]": "L_MSG.info('x=%s', o)",
    "info stack_info=True [fallback]": "L_TIME.info('x=%s', v, stack_info=True)",
    "exception [fallback]": "L_TIME.error('x=%s', v, exc_info=EI)",
    "adapter info": "AD.info('x=%s', v)",
    "logging.info root (disabled)": "logging.info('x=%s', v)",
    # disabled calls
    "debug disabled": "L_OFF.debug('x=%s', v)",
    "info disabled": "L_OFF.info('x=%s', v)",
    "debug disabled, no args": "L_OFF.debug('hello')",
    "debug disabled, kwargs": "L_OFF.debug('x=%s', v, exc_info=False)",
    "isEnabledFor": "L_OFF.isEnabledFor(10)",
    # pieces
    "LogRecord()": "logging.LogRecord('app.http', 20, '/srv/app/handlers.py', 42, 'x=%s', (v,), None, 'handle', None)",
    "makeRecord": "L_TIME.makeRecord('app', 20, '/srv/app/h.py', 42, 'x=%s', (v,), None, 'handle')",
    "findCaller": "L_TIME.findCaller()",
    "getMessage": "rec.getMessage()",
    "getMessage obj arg": "rec_obj.getMessage()",
    "format %(message)s": "F_MSG.format(rec)",
    "format BASIC_FORMAT": "F_BASIC.format(rec)",
    "format asctime": "F_TIME.format(rec)",
    "format datefmt": "F_DATEFMT.format(rec)",
    "format {-style [fallback]": "F_BRACE.format(rec)",
    "format formatTime overridden [fallback, steady]": "F_OWNTIME.format(rec)",
    "format formatTime overridden + plain [fallback, alternating]": "F_OWNTIME.format(rec); F_TIME.format(rec)",
    "formatTime custom converter [fallback, steady]": "F_CONV.formatTime(rec)",
    "formatTime custom converter + plain [fallback, alternating]": "F_CONV.formatTime(rec); F_TIME.formatTime(rec)",
    "debug disabled, isEnabledFor overridden [fallback]": "L_OVR.debug('x=%s', v)",
    "LogRecord(keywords) [fallback]": "logging.LogRecord(name='a', level=20, pathname='/p.py', lineno=1, msg='m', args=(), exc_info=None)",
    "LogRecord(keywords) + plain [fallback, alternating]": "logging.LogRecord(name='a', level=20, pathname='/p.py', lineno=1, msg='m', args=(), exc_info=None); logging.LogRecord('a', 20, '/p.py', 1, 'm', (), None)",
    "findCaller(stack_info=True) [fallback]": "L_TIME.findCaller(True)",
    "format subclass [fallback]": "F_SUB.format(rec)",
    "format obj arg [fallback]": "F_TIME.format(rec_obj)",
    "format %(message)s, obj arg [fallback, steady]": "F_MSG.format(rec_obj)",
    "format %(message)s, obj arg + plain [fallback, alternating]": "F_MSG.format(rec_obj); F_MSG.format(rec)",
    "formatTime": "F_TIME.formatTime(rec)",
    "formatTime datefmt": "F_TIME.formatTime(rec, '%Y-%m-%dT%H:%M:%S')",
    "formatTime subclass": "F_SUB.formatTime(rec)",
    # wave 2: Handler.handle, Decimal arguments, enabled level calls
    "Handler.handle StringIO": "_HS.handle(rec)",
    "info Decimal arg": "L_TIME.info('total %s', DEC)",
    "Handler.handle file stream [fallback, steady]": "_HF.handle(rec)",
    "Handler.handle file + StringIO [fallback, alternating]": "_HF.handle(rec); _HS.handle(rec)",
    "info to file stream": "L_FILE.info('x=%s', v)",
    "info enabled calls only (Logger.level steps aside)": "L_ON.info('x=%s', v)",
}


def cells(only=None):
    return [c for c in CELLS if not only or any(o in c for o in only.split("|"))]


CELL_SETUP = {
    # a pinned dependency replaced (by an equivalent wrapper): the format targets suspend
    "info, time.strftime replaced [deps]": "_sf = time.strftime\ntime.strftime = lambda *a: _sf(*a)",
}


def worker(cell, variant):
    h = helper()
    ns = {}
    exec(VARIANTS[variant], ns)
    exec(COMMON, ns)
    exec(CELL_SETUP.get(cell, ""), ns)
    stmt = CELLS[cell]
    names = [k for k in ns if not k.startswith("__") and k.isidentifier()]
    binds = "".join(f"    {k} = _g[{k!r}]\n" for k in names)
    ns["_g"] = ns
    body = "".join(f"        {stmt}\n" for _ in range(UNROLL))
    exec(f"def fn(_n):\n{binds}    for _ in range(_n):\n{body}\n    _sio.seek(0); _sio.truncate()\n", ns)
    fn = ns["fn"]
    n = 100
    jit = getattr(sys, "_jit", None)
    fn(3000 if jit and jit.is_enabled() else 50)  # JIT: enough back edges to build traces
    for _ in range(1000):  # pending calls (suspensions) run at eval-breaker checks
        pass
    fn(n)
    h.start()
    fn(n)
    h.dump("a")
    h.stop()
    h.start()
    fn(2 * n)
    h.dump("b")
    h.stop()


def run_cell(cell, variant, env):
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable,
               os.path.abspath(__file__), "_worker", cell, variant]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True)
        if r.returncode:
            return None, (r.stderr or r.stdout)[-1500:]
        tot = {}
        for fn in glob.glob(out + "*"):
            name = t = None
            with open(fn) as f:
                for line in f:
                    if line.startswith("desc: Trigger: Client Request: "):
                        name = line.split("Client Request: ", 1)[1].strip()
                    elif line.startswith(("totals:", "summary:")):
                        t = int(line.split()[1])
                        break
            if name:
                tot[name] = t
        return (tot["b"] - tot["a"]) / (100 * UNROLL), None


def main():
    a = sys.argv[1:]
    if a and a[0] == "_worker":
        worker(a[1], a[2])
        return
    if a and a[0] == "report":
        report(a[1])
        return
    only = a[a.index("--only") + 1] if "--only" in a else None
    variants = (a[a.index("--variants") + 1] if "--variants" in a else "orig,on").split(",")
    jobs = int(a[a.index("-j") + 1]) if "-j" in a else 3
    out = a[a.index("-o") + 1] if "-o" in a else None
    jit = "--jit" in a
    env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "1" if jit else "0"}
    helper()
    todo = [(c, v) for c in cells(only) for v in variants]
    with ThreadPoolExecutor(jobs) as ex:
        res = list(ex.map(lambda cv: run_cell(*cv, env), todo))
    rows = {}
    for (c, v), (ir, err) in zip(todo, res):
        rows.setdefault(c, {})[v] = ir if err is None else f"ERR {err.strip().splitlines()[-1][:160]}"
    for c, vs in rows.items():
        base = vs.get("orig")
        line = f"{c:40s}"
        for v in variants:
            ir = vs.get(v)
            if isinstance(ir, float):
                r = f" ({ir / base:.3f})" if isinstance(base, float) and v != "orig" else ""
                line += f" {v} {ir:9.0f}{r}"
            else:
                line += f" {v} {ir}"
        print(line, flush=True)
    if out:
        with open(out, "w") as f:
            json.dump({"python": sys.version, "jit": jit, "rows": rows}, f, indent=1)


def report(path):
    data = json.load(open(path))
    print(f"# {data['python'].split()[0]} jit={data['jit']}")
    for c, vs in data["rows"].items():
        base = vs.get("orig")
        parts = []
        for v, ir in vs.items():
            if isinstance(ir, float) and isinstance(base, float) and v != "orig":
                parts.append(f"{v} {ir:.0f} ({ir / base:.3f})")
            elif isinstance(ir, float):
                parts.append(f"{v} {ir:.0f}")
            else:
                parts.append(f"{v} {ir}")
        print(f"| {c} | " + " | ".join(parts) + " |")


if __name__ == "__main__":
    main()
