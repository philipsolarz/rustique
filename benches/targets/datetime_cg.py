"""datetime group: per-call instructions (callgrind, region-instrumented), plain vs Rustique.

Every cell runs in its own process under callgrind: setup (imports, optional install of
the datetime targets), warm-up, then Ir(2n calls) - Ir(n calls) over n, statements
unrolled 10x, so fixed costs cancel (noise floor about +-15 Ir per call). `orig` is plain
CPython; `rq` installs the group with `rustique._rustique._t_datetime.install()`.

Usage: python benches/targets/datetime_cg.py [--only SUBSTR] [--jit] [-o out.json]
       python benches/targets/datetime_cg.py report out.json
"""

import glob
import json
import os
import subprocess
import sys
import sysconfig
import tempfile

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
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_dt_bench_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__dt_bench_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper():
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-dt-bench")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_dt_bench_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "dt_bench.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        tmp = target + f".{os.getpid()}.tmp"
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", tmp, src])
        os.replace(tmp, target)
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _dt_bench_v1  # type: ignore[import-not-found]

    return _dt_bench_v1


COMMON = """
import datetime as D, time, locale, _strptime, zoneinfo
from datetime import datetime, date, timezone, timedelta
class Sub(datetime):
    pass
dt = datetime(2024, 3, 15, 13, 45, 30, 123456)
dtu = datetime(2024, 3, 15, 13, 45, 30, tzinfo=timezone.utc)
dto = datetime(2024, 3, 15, 13, 45, 30, tzinfo=timezone(timedelta(hours=1)))
dtz = datetime(2024, 3, 15, 13, 45, 30, tzinfo=zoneinfo.ZoneInfo('Europe/Berlin'))
d0 = date(2024, 3, 15)
t0 = D.time(13, 45, 30, 123456)
sdt = Sub(2024, 3, 15, 13, 45, 30)
st = time.localtime(1710510330)
tup = (2024, 3, 15, 13, 45, 30, 4, 75, 0)
LOG = '15/Mar/2024:13:45:30 +0100'
LOGF = '%d/%b/%Y:%H:%M:%S %z'
ISO = '2024-03-15 13:45:30'
ISOF = '%Y-%m-%d %H:%M:%S'
ISOZ = '2024-03-15T13:45:30.123456+02:00'
ISOZF = '%Y-%m-%dT%H:%M:%S.%f%z'
AMPM = '03/15/2024 01:45 PM'
AMPMF = '%m/%d/%Y %I:%M %p'
_strptime._strptime_time('2024', '%Y')
"""

RQ = "import rustique._takeover as _rq; _rq.enable(only=[\"datetime\"])"
V = {"orig": "", "rq": RQ}

# cell -> (statement, extra setup run before the variant's setup)
CELLS = {
    # --- strptime: fast paths ---------------------------------------------------------
    "strptime log %d/%b/%Y:%H:%M:%S %z": ("datetime.strptime(LOG, LOGF)", ""),
    "strptime %Y-%m-%d %H:%M:%S": ("datetime.strptime(ISO, ISOF)", ""),
    "strptime iso %f%z": ("datetime.strptime(ISOZ, ISOZF)", ""),
    "strptime %m/%d/%Y %I:%M %p": ("datetime.strptime(AMPM, AMPMF)", ""),
    "date.strptime %Y-%m-%d": ("date.strptime('2024-03-15', '%Y-%m-%d')", ""),
    "time.strptime %Y-%m-%d %H:%M:%S": ("time.strptime(ISO, ISOF)", ""),
    # --- strptime: fallbacks ----------------------------------------------------------
    "strptime fallback: subclass receiver": ("Sub.strptime(ISO, ISOF)", ""),
    "strptime fallback: unsupported %U": ("datetime.strptime('2024 11 5', '%Y %U %w')", ""),
    "strptime fallback: no match (error)": (
        "try:\n            datetime.strptime('2024-13-01', '%Y-%m-%d')\n        except ValueError:\n            pass", ""),
    "strptime fallback: LC_TIME en_US": ("datetime.strptime(ISO, ISOF)",
                                         "locale.setlocale(locale.LC_TIME, 'en_US.UTF-8')"),
    "strptime fallback: dependency changed (deps)": ("datetime.strptime(ISO, ISOF)", "_strptime.isinstance = isinstance"),
    "strptime fallback: _regex_cache miss (deopt)": ("_strptime._regex_cache.clear(); datetime.strptime(ISO, ISOF)", ""),
    "strptime fallback: tracing tool (tools)": ("datetime.strptime(ISO, ISOF)",
                                                "import sys; sys.monitoring.use_tool_id(3, 'x'); "
                                                "sys.monitoring.set_events(3, sys.monitoring.events.PY_START)"),
    # --- strftime ---------------------------------------------------------------------
    "strftime %Y-%m-%d %H:%M:%S": ("dt.strftime(ISOF)", ""),
    "strftime %Y-%m (utc)": ("dtu.strftime('%Y-%m')", ""),
    "strftime %Y-%m-%d %H:00 (fixed offset)": ("dto.strftime('%Y-%m-%d %H:00')", ""),
    "strftime %a %d %b %Y %H:%M %Z (ZoneInfo)": ("dtz.strftime('%a %d %b %Y %H:%M %Z')", ""),
    "strftime %A (ZoneInfo)": ("dtz.strftime('%A')", ""),
    "strftime %a, %d %b %Y %H:%M:%S GMT (utc)": ("dtu.strftime('%a, %d %b %Y %H:%M:%S GMT')", ""),
    "date.strftime %Y-%m-%d": ("d0.strftime('%Y-%m-%d')", ""),
    "time.strftime method %H:%M:%S.%f": ("t0.strftime('%H:%M:%S.%f')", ""),
    "f-string {dt:%Y-%m-%d}": ("f'{dt:%Y-%m-%d}'", ""),
    "time.strftime(fmt, struct_time)": ("time.strftime(ISOF, st)", ""),
    "time.strftime(fmt, tuple)": ("time.strftime(ISOF, tup)", ""),
    "time.strftime fallback: year < 1000": ("time.strftime(ISOF, (999,) + tup[1:])", ""),
    "strftime fallback: subclass": ("sdt.strftime(ISOF)", ""),
    "strftime fallback: unsupported %U": ("dt.strftime('%Y %U')", ""),
    "strftime fallback: names in en_US": ("dt.strftime('%a %b')", "locale.setlocale(locale.LC_TIME, 'en_US.UTF-8')"),
    "strftime numeric in en_US": ("dt.strftime(ISOF)", "locale.setlocale(locale.LC_TIME, 'en_US.UTF-8')"),
    # --- isoformat --------------------------------------------------------------------
    "isoformat()": ("dt.isoformat()", ""),
    "isoformat() utc": ("dtu.isoformat()", ""),
    "isoformat(timespec='minutes') ZoneInfo": ("dtz.isoformat(timespec='minutes')", ""),
    "str(dt)": ("str(dt)", ""),
    "date.isoformat()": ("d0.isoformat()", ""),
    "time.isoformat()": ("t0.isoformat()", ""),
    "isoformat(sep=' ', timespec='seconds')": ("dt.isoformat(sep=' ', timespec='seconds')", ""),
    "isoformat fallback: subclass": ("sdt.isoformat()", ""),
    "isoformat fallback: unknown timespec (error)": (
        "try:\n            dt.isoformat(timespec='bogus')\n        except ValueError:\n            pass", ""),
    "fromisoformat full tz": ("datetime.fromisoformat(ISOZ)", ""),
}


def cells(only=None):
    out = []
    for c, (stmt, pre) in CELLS.items():
        if only and only not in c:
            continue
        for v, setup in V.items():
            out.append((c, v, stmt, pre, setup))
    return out


def worker(idx, only):
    h = helper()
    c, v, stmt, pre, setup = cells(only)[idx]
    ns = {}
    exec(COMMON, ns)
    exec(pre, ns)
    exec(setup, ns)
    names = [k for k in ns if not k.startswith("__") and k.isidentifier() and k not in ("_t", "_rq", "_f")]
    binds = "".join(f"    {k} = _g[{k!r}]\n" for k in names)
    ns["_g"] = ns
    body = "".join(f"        {stmt}\n" for _ in range(UNROLL))
    exec(f"def fn(_n):\n{binds}    for _ in range(_n):\n{body}", ns)
    fn = ns["fn"]
    jit = getattr(sys, "_jit", None)
    jit_on = bool(jit and jit.is_enabled())
    n = 100
    fn(3000 if jit_on else 30)
    fn(n)
    h.start()
    fn(n)
    h.dump("a")
    h.stop()
    h.start()
    fn(2 * n)
    h.dump("b")
    h.stop()


def run_cell(idx, only, env):
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable,
               os.path.abspath(__file__), "_worker", str(idx), only or ""]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True)
        if r.returncode:
            return None, r.stderr[-2000:]
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
    if len(sys.argv) > 1 and sys.argv[1] == "_worker":
        worker(int(sys.argv[2]), sys.argv[3] or None)
        return
    if len(sys.argv) > 1 and sys.argv[1] == "report":
        report(sys.argv[2])
        return
    jit = "--jit" in sys.argv
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
    out = sys.argv[sys.argv.index("-o") + 1] if "-o" in sys.argv else None
    variants = sys.argv[sys.argv.index("--variants") + 1].split(",") if "--variants" in sys.argv else None
    env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "1" if jit else "0", "TZ": "UTC"}
    helper()
    rows = {}
    for i, (c, v, _s, _p, _setup) in enumerate(cells(only)):
        if variants and v not in variants:
            continue
        ir, err = run_cell(i, only, env)
        rows.setdefault(c, {})[v] = ir if err is None else f"ERR {err.strip().splitlines()[-1][:160]}"
        base = rows[c].get("orig")
        ratio = f"{ir / base:.3f}" if isinstance(ir, float) and isinstance(base, float) and v != "orig" else ""
        shown = f"{ir:.0f}" if isinstance(ir, float) else rows[c][v]
        print(f"{c:46s} {v:5s} {shown:>10} {ratio}", flush=True)
    if out:
        with open(out, "w") as f:
            json.dump({"python": sys.version, "jit": jit, "rows": rows}, f, indent=1)


def report(path):
    data = json.load(open(path))
    print(f"# {data['python'].split()[0]} jit={data['jit']}")
    print("| cell | orig Ir | Rustique Ir | ratio |\n|---|---:|---:|---:|")
    for c, vs in data["rows"].items():
        o, r = vs.get("orig"), vs.get("rq")
        if isinstance(o, float) and isinstance(r, float):
            print(f"| {c} | {o:.0f} | {r:.0f} | {r / o:.3f} |")
        else:
            print(f"| {c} | {o} | {r} | |")


if __name__ == "__main__":
    main()
