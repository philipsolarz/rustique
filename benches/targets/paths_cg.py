"""Instruction counts per call for the `posixpath`, `fnmatch` and `pathlib` targets.

Every (cell, variant) runs in its own process under callgrind, instrumented only around the
measured region (client requests): setup, warm-up, then Ir(2n calls) - Ir(n calls) over n,
statements unrolled 10x, so fixed costs cancel (noise ~ +-15 Ir per call). `orig` is plain
CPython; `rs` installs the group's kernels first (`rustique._rustique._t_<group>.install()`).
A cell's setup may also define the input that sends the kernel to its fallback (str
subclass, PathLike, patched dependency, tool active): those cells must stay <= 1.05 too.

Usage (on the dev box):
    python benches/targets/paths_cg.py [--jit] [--only SUBSTR] [-j 4] [-o out.json]
    python benches/targets/paths_cg.py list
    python benches/targets/paths_cg.py report out.json [more.json ...]
"""

from __future__ import annotations

import concurrent.futures as cf
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
static struct PyModuleDef def = {PyModuleDef_HEAD_INIT, "_paths_cg_v1", NULL, 0, methods, slots};
PyMODINIT_FUNC PyInit__paths_cg_v1(void) { return PyModuleDef_Init(&def); }
"""


def helper():
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-paths-cg")
    os.makedirs(cache, exist_ok=True)
    target = os.path.join(cache, "_paths_cg_v1" + sysconfig.get_config_var("EXT_SUFFIX"))
    if not os.path.exists(target):
        src = os.path.join(cache, "paths_cg.c")
        with open(src, "w") as f:
            f.write(HELPER_C)
        inc = sysconfig.get_paths()["include"]
        subprocess.check_call(["cc", "-O2", "-shared", "-fPIC", f"-I{inc}", "-o", target, src])
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _paths_cg_v1  # type: ignore[import-not-found]

    return _paths_cg_v1


COMMON = r"""
import os, posixpath, fnmatch, pathlib, sys, types, genericpath
from pathlib import Path, PurePosixPath
join = posixpath.join; split = posixpath.split; splitext = posixpath.splitext
basename = posixpath.basename; dirname = posixpath.dirname; isabs = posixpath.isabs
relpath = posixpath.relpath; abspath = posixpath.abspath; commonpath = posixpath.commonpath
normcase = posixpath.normcase
d = '/tmp/rq-fs-k2j3/project/src/tests'
f = 'test_api_12.py'
p = d + '/' + f
pb = os.fsencode(p); db = os.fsencode(d); fb = os.fsencode(f)
pu = '/home/jürgen/Dokumente/übersicht.txt'
pw = '/home/你好/文件.py'
pe = '/home/user/\U0001f600/notes.md'
class S(str): pass
class PL:
    def __init__(self, s): self.s = s
    def __fspath__(self): return self.s
ps = S(p); ds = S(d); fs = S(f)
ppl = PL(p); dpl = PL(d)
pp = Path(p); str(pp)
root = '/tmp/rq-fs-k2j3/project'
pats = ['*.pyc', '*.tmp', '*.bak', '*~', '.DS_Store', '*.log', 'Thumbs.db', '*.swp', '*.egg-info', '*.o', '*.so', 'coverage.*']
fnmatch_ = fnmatch.fnmatch; fnmatchcase = fnmatch.fnmatchcase; ffilter = fnmatch.filter
names = ['main.py', 'utils_3.txt', 'config.json', 'models.pyc', 'README', 'index.html', 'a.tmp', 'b.bak'] * 4
"""

RS = {
    "posixpath": "from rustique._rustique import _t_posixpath as _T; _T.install()",
    "fnmatch": "from rustique._rustique import _t_fnmatch as _T; _T.install()",
    "pathlib": "from rustique._rustique import _t_pathlib as _T; _T.install()",
}

PATCH_FSPATH = "_of = os.fspath; os.fspath = lambda x: _of(x)"
PATCH_GETSEP = "posixpath._get_sep = types.FunctionType(posixpath._get_sep.__code__, posixpath.__dict__)"
TOOLS = "import cProfile; _prof = cProfile.Profile(); _prof.enable()"

# (group, cell name, statement, extra setup run before install for both variants)
CELLS: list[tuple[str, str, str, str]] = [
    ("posixpath", "join(d, f)", "join(d, f)", ""),
    ("posixpath", "join(root, 'src', 'pkg', f)", "join(root, 'src', 'pkg', f)", ""),
    ("posixpath", "join(d, '/abs')", "join(d, '/etc/passwd')", ""),
    ("posixpath", "join(db, fb) bytes", "join(db, fb)", ""),
    ("posixpath", "join(latin1, f)", "join(pu, f)", ""),
    ("posixpath", "join(ucs2, ucs4)", "join(pw, pe)", ""),
    ("posixpath", "join(ds, fs) str subclass", "join(ds, fs)", ""),
    ("posixpath", "join(PathLike, f)", "join(dpl, f)", ""),
    ("posixpath", "join(Path, f)", "join(pp, f)", ""),
    ("posixpath", "join(d, f) os.fspath patched", "join(d, f)", PATCH_FSPATH),
    ("posixpath", "join(d, f) _get_sep patched", "join(d, f)", PATCH_GETSEP),
    ("posixpath", "join(d, f) cProfile on", "join(d, f)", TOOLS),
    ("posixpath", "split(p)", "split(p)", ""),
    ("posixpath", "split(pb) bytes", "split(pb)", ""),
    ("posixpath", "split(ps) str subclass", "split(ps)", ""),
    ("posixpath", "split(PathLike)", "split(ppl)", ""),
    ("posixpath", "split(Path)", "split(pp)", ""),
    ("posixpath", "split(p) os.fspath patched", "split(p)", PATCH_FSPATH),
    ("posixpath", "split(p) cProfile on", "split(p)", TOOLS),
    ("posixpath", "splitext(f)", "splitext(f)", ""),
    ("posixpath", "splitext(p)", "splitext(p)", ""),
    ("posixpath", "splitext(pb) bytes", "splitext(pb)", ""),
    ("posixpath", "splitext(pw) ucs2", "splitext(pw)", ""),
    ("posixpath", "splitext(ps) str subclass", "splitext(ps)", ""),
    ("posixpath", "splitext(PathLike)", "splitext(ppl)", ""),
    ("posixpath", "splitext(Path)", "splitext(pp)", ""),
    ("posixpath", "splitext(p) os.fspath patched", "splitext(p)", PATCH_FSPATH),
    ("posixpath", "basename(p)", "basename(p)", ""),
    ("posixpath", "basename(pb) bytes", "basename(pb)", ""),
    ("posixpath", "basename(ps) str subclass", "basename(ps)", ""),
    ("posixpath", "basename(PathLike)", "basename(ppl)", ""),
    ("posixpath", "basename(Path)", "basename(pp)", ""),
    ("posixpath", "basename(p) os.fspath patched", "basename(p)", PATCH_FSPATH),
    ("posixpath", "basename(p) cProfile on", "basename(p)", TOOLS),
    ("posixpath", "dirname(p)", "dirname(p)", ""),
    ("posixpath", "dirname(pb) bytes", "dirname(pb)", ""),
    ("posixpath", "dirname(ps) str subclass", "dirname(ps)", ""),
    ("posixpath", "dirname(PathLike)", "dirname(ppl)", ""),
    ("posixpath", "dirname(p) os.fspath patched", "dirname(p)", PATCH_FSPATH),
    ("posixpath", "isabs(p)", "isabs(p)", ""),
    ("posixpath", "isabs(pb) bytes", "isabs(pb)", ""),
    ("posixpath", "isabs(ps) str subclass", "isabs(ps)", ""),
    ("posixpath", "isabs(PathLike)", "isabs(ppl)", ""),
    ("posixpath", "isabs(p) os.fspath patched", "isabs(p)", PATCH_FSPATH),
    ("posixpath", "normcase(p)", "normcase(p)", ""),
    ("posixpath", "normcase(PathLike)", "normcase(ppl)", ""),
    ("posixpath", "relpath(p, root)", "relpath(p, root)", ""),
    ("posixpath", "relpath(pb, rootb) bytes", "relpath(pb, rootb)", "rootb = os.fsencode(root)"),
    ("posixpath", "relpath(f) relative", "relpath(f)", ""),
    ("posixpath", "relpath(ps, root) str subclass", "relpath(ps, root)", ""),
    ("posixpath", "relpath(p, root) os.fspath patched", "relpath(p, root)", PATCH_FSPATH),
    ("posixpath", "abspath(p)", "abspath(p)", ""),
    ("posixpath", "abspath(f) relative", "abspath(f)", ""),
    ("posixpath", "abspath(ps) str subclass", "abspath(ps)", ""),
    ("posixpath", "commonpath([p, d])", "commonpath([p, d])", ""),
    ("posixpath", "commonpath((pb, db)) bytes", "commonpath((pb, db))", ""),
    ("fnmatch", "fnmatch(f, '*.py')", "fnmatch_(f, '*.py')", ""),
    ("fnmatch", "fnmatch(f, 'coverage.*') miss", "fnmatch_(f, 'coverage.*')", ""),
    ("fnmatch", "fnmatch(f, '[!t]*_[0-9]*.py')", "fnmatch_(f, '[!t]*_[0-9]*.py')", ""),
    ("fnmatch", "fnmatch(fb, b'*.py') bytes", "fnmatch_(fb, b'*.py')", ""),
    ("fnmatch", "fnmatch(fs, '*.py') str subclass", "fnmatch_(fs, '*.py')", ""),
    ("fnmatch", "fnmatch(Path, '*.py')", "fnmatch_(pp, '*.py')", ""),
    ("fnmatch", "fnmatch(f, '*.py') os.fspath patched", "fnmatch_(f, '*.py')", PATCH_FSPATH),
    ("fnmatch", "fnmatch(f, '*.py') cProfile on", "fnmatch_(f, '*.py')", TOOLS),
    ("fnmatch", "fnmatchcase(f, '*.py')", "fnmatchcase(f, '*.py')", ""),
    ("fnmatch", "fnmatchcase(fs, '*.py') str subclass", "fnmatchcase(fs, '*.py')", ""),
    ("fnmatch", "any(fnmatch(f, pat) for 12 pats)", "any(fnmatch_(f, pat) for pat in pats)", ""),
    ("fnmatch", "filter(32 names, '*.py')", "ffilter(names, '*.py')", ""),
    ("fnmatch", "filter(iter, '*.py')", "ffilter(iter(names), '*.py')", ""),
    ("pathlib", "Path(p)", "Path(p)", ""),
    ("pathlib", "str(Path(p))", "str(Path(p))", ""),
    ("pathlib", "Path(d) / f", "Path(d) / f", ""),
    ("pathlib", "str(pp / f)", "str(pp / f)", ""),
    ("pathlib", "pp.name", "pp.name", ""),
    ("pathlib", "pp.suffix", "pp.suffix", ""),
    ("pathlib", "pp.stem", "pp.stem", ""),
    ("pathlib", "pp.parent", "pp.parent", ""),
    ("pathlib", "pp.parts", "pp.parts", ""),
    ("pathlib", "pp.with_suffix('.txt')", "pp.with_suffix('.txt')", ""),
    ("pathlib", "pp.relative_to(root)", "pp.relative_to(root)", ""),
    ("pathlib", "hash(Path(p))", "hash(Path(p))", ""),
    ("pathlib", "Path(p) == pp", "Path(p) == pp", ""),
    ("pathlib", "pp.as_posix()", "pp.as_posix()", ""),
    ("pathlib", "pp.is_absolute()", "pp.is_absolute()", ""),
]


def cells(only=None):
    out = []
    for group, name, stmt, extra in CELLS:
        if only and not any(o in f"{group} {name}" for o in only.split(",")):
            continue
        for v in ("orig", "rs"):
            out.append((group, name, v, stmt, extra))
    return out


def worker(idx, only):
    h = helper()
    group, name, v, stmt, extra = cells(only)[idx]
    ns: dict = {}
    exec(COMMON, ns)
    if v == "rs":
        exec(RS[group], ns)
    tools = extra == TOOLS
    if extra and not tools:
        exec(extra, ns)
    names = [k for k in ns if not k.startswith("__") and k.isidentifier() and not k.startswith("_")]
    binds = "".join(f"    {k} = _g[{k!r}]\n" for k in names)
    ns["_g"] = ns
    body = "".join(f"        {stmt}\n" for _ in range(UNROLL))
    exec(f"def fn(_n):\n{binds}    for _ in range(_n):\n{body}", ns)
    fn = ns["fn"]
    jit = getattr(sys, "_jit", None)
    jit_on = bool(jit and jit.is_enabled())
    n = 200
    if tools:
        exec(extra, ns)
    fn(30000 if jit_on else 50)
    fn(n)
    h.start()
    fn(n)
    h.dump("a")
    h.stop()
    h.start()
    fn(2 * n)
    h.dump("b")
    h.stop()
    if tools:
        ns["_prof"].disable()


def run_cell(idx, only, env):
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        cmd = ["valgrind", "--tool=callgrind", "--instr-atstart=no", "--dump-instr=no", "--dump-line=no",
               "--compress-strings=no", "-q", f"--callgrind-out-file={out}", sys.executable, os.path.abspath(__file__),
               "_worker", str(idx), only or ""]
        r = subprocess.run(cmd, env=env, capture_output=True, text=True)
        if r.returncode:
            return None, (r.stderr.strip().splitlines() or ["?"])[-1][:160]
        tot = {}
        for fn in glob.glob(out + "*"):
            name = t = None
            with open(fn) as fh:
                for line in fh:
                    if line.startswith("desc: Trigger: Client Request: "):
                        name = line.split("Client Request: ", 1)[1].strip()
                    elif line.startswith(("totals:", "summary:")):
                        t = int(line.split()[1])
                        break
            if name:
                tot[name] = t
        return (tot["b"] - tot["a"]) / (200 * UNROLL), None


def main():
    args = sys.argv[1:]
    if args and args[0] == "_worker":
        worker(int(args[1]), args[2] or None)
        return
    if args and args[0] == "report":
        report(args[1:])
        return
    if args and args[0] == "list":
        for c in cells(None):
            print(*c[:3])
        return
    jit = "--jit" in args
    only = args[args.index("--only") + 1] if "--only" in args else None
    out = args[args.index("-o") + 1] if "-o" in args else None
    jobs = int(args[args.index("-j") + 1]) if "-j" in args else 4
    variants = args[args.index("--variants") + 1].split(",") if "--variants" in args else ["orig", "rs"]
    env = {**os.environ, "PYTHONHASHSEED": "0", "PYTHON_JIT": "1" if jit else "0"}
    todo = [(i, c) for i, c in enumerate(cells(only)) if c[2] in variants]
    rows: dict = {}
    with cf.ThreadPoolExecutor(jobs) as ex:
        futs = {ex.submit(run_cell, i, only, env): c for i, c in todo}
        for fu in cf.as_completed(futs):
            group, name, v, _s, _e = futs[fu]
            ir, err = fu.result()
            rows.setdefault(f"{group}: {name}", {})[v] = ir if err is None else f"ERR {err}"
    order = [f"{g}: {n}" for g, n, _s, _e in CELLS if f"{g}: {n}" in rows]
    for k in order:
        vs = rows[k]
        o, r = vs.get("orig"), vs.get("rs")
        ratio = f"{r / o:.3f}" if isinstance(o, float) and isinstance(r, float) else ""
        fo = f"{o:.0f}" if isinstance(o, float) else str(o)
        fr = f"{r:.0f}" if isinstance(r, float) else str(r)
        print(f"{k:52s} {fo:>8} {fr:>8} {ratio}", flush=True)
    if out:
        with open(out, "w") as fh:
            json.dump({"python": sys.version, "jit": jit, "rows": {k: rows[k] for k in order}}, fh, indent=1)


def report(paths):
    for path in paths:
        data = json.load(open(path))
        print(f"# {data['python'].split()[0]} jit={data['jit']}")
        print("| cell | orig Ir | rs Ir | ratio |\n|---|---:|---:|---:|")
        for k, vs in data["rows"].items():
            o, r = vs.get("orig"), vs.get("rs")
            ratio = f"{r / o:.3f}" if isinstance(o, float) and isinstance(r, float) else ""
            fo = f"{o:.0f}" if isinstance(o, float) else str(o)
            fr = f"{r:.0f}" if isinstance(r, float) else str(r)
            print(f"| {k} | {fo} | {fr} | {ratio} |")


if __name__ == "__main__":
    main()
