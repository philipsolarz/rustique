"""Where real programs spend their startup: whole-process pools (docs/atlas/w2-startup.md §3).

Each program is one short `python ...` process (an import, an app definition, a CLI's
`--version`/`--help`, `pytest --co` on an empty directory). For every program:

    perf      `perf record -e instructions:u -c 20011 -g` (and `-e cycles` for CPU time incl.
              kernel) over --reps fresh processes of `python -X perf ...`; every sample is
              classified into a pool by its stack (frame pointers; Python frames come from
              the perf map that `-X perf` writes):
                - leaf-native class (native frames below the innermost Python frame):
                  unmarshal (`marshal.loads`/`r_object`/frozen code), compile (source ->
                  code: parser/compiler), `_sre.compile`, C-extension load (dlopen+PyInit),
                  kernel (syscall wrapper name, else page fault/interrupt), other user code;
                - pool (innermost Python frame that is a "marker", walking outwards):
                  a module body `<module>` (owner stdlib / third-party / own), the import
                  system (frozen importlib, zipimport, importlib/__init__, pkgutil),
                  importlib.metadata / importlib.resources, `site`, `re` (compile at import),
                  class machinery (dataclasses, enum, typing, collections/namedtuple,
                  functools, abc, inspect); samples without a Python frame are process start
                  (ld.so), interpreter init, the main compile, finalization.
              Totals (instructions:u, cycles, task-clock) come from `perf stat` WITHOUT
              `-X perf` (median of 3); the -X perf inflation is reported per program.
    importtime  `-X importtime`: per-module self/cumulative us (median of 5 runs), top
              modules, and the sum of import self time by owner.
    callgrind whole-process Ir (`--instr-atstart=yes`, PYTHONHASHSEED=0) and the inclusive Ir
              of non-recursive C pools (unmarshal, compile, _sre.compile, extension load)
              via callgrind_annotate --inclusive=yes.
    wall      -r interleaved rounds of every program (median, p10-p90 ms).

Usage (dev box; PY is a plain venv's python with the packages of the devtools/web pins):

    python benches/startup/import_pools.py perf --python PY --workdir W -o perf.json [--reps 10]
    python benches/startup/import_pools.py importtime --python PY --workdir W -o it.json
    python benches/startup/import_pools.py callgrind --python PY --workdir W -o cg.json
    python benches/startup/import_pools.py wall --python PY --workdir W -o wall.json [-r 30]
    python benches/startup/import_pools.py gc --python PY --workdir W -o gc.json
    python benches/startup/import_pools.py table perf.json [--callgrind cg.json] [--wall wall.json]

`--only a,b` restricts the program set; `--programs core|import_startup|all`.
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
import time
from collections import Counter, defaultdict

# --------------------------------------------------------------------------------------
# programs
# --------------------------------------------------------------------------------------

FASTAPI = """\
from fastapi import FastAPI
from pydantic import BaseModel
class Item(BaseModel):
    name: str
    price: float
    tags: list[str] = []
app = FastAPI()
@app.get("/items/{item_id}")
def read(item_id: int, q: str | None = None) -> Item:
    return Item(name=q or "x", price=1.0)
@app.post("/items")
def create(item: Item) -> Item:
    return item
"""

SQLALCHEMY = """\
from sqlalchemy import ForeignKey, String
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column, relationship
class Base(DeclarativeBase):
    pass
class User(Base):
    __tablename__ = "users"
    id: Mapped[int] = mapped_column(primary_key=True)
    name: Mapped[str] = mapped_column(String(30))
    posts: Mapped[list["Post"]] = relationship(back_populates="author")
class Post(Base):
    __tablename__ = "posts"
    id: Mapped[int] = mapped_column(primary_key=True)
    author_id: Mapped[int] = mapped_column(ForeignKey("users.id"))
    author: Mapped[User] = relationship(back_populates="posts")
"""

PYDANTIC = """\
import pydantic
class Address(pydantic.BaseModel):
    street: str
    city: str
    zip: str | None = None
class User(pydantic.BaseModel):
    id: int
    name: str
    emails: list[str] = []
    address: Address | None = None
    tags: dict[str, float] = {}
"""

DJANGO = (
    "import django\nfrom django.conf import settings\n"
    "settings.configure(INSTALLED_APPS=['django.contrib.contenttypes', 'django.contrib.auth'],"
    " DATABASES={'default': {'ENGINE': 'django.db.backends.sqlite3', 'NAME': ':memory:'}})\n"
    "django.setup()\nimport django.db.models\n"
)

FLASK = "from flask import Flask\napp = Flask('app')\n@app.route('/')\ndef index():\n    return 'hi'\n"

TYPER_APP = """\
import typer
app = typer.Typer()
@app.command()
def hello(name: str, count: int = 1, loud: bool = False):
    \"\"\"Say hello.\"\"\"
    for _ in range(count):
        typer.echo(name.upper() if loud else name)
@app.command()
def bye(name: str):
    \"\"\"Say goodbye.\"\"\"
    typer.echo(f"bye {name}")
if __name__ == "__main__":
    app()
"""

RICH = "from rich.console import Console\nConsole(force_terminal=True, width=80).print('[bold red]hi[/] there', {'a': [1, 2]})\n"

VAL_RE = ("import re\n"
          "for i in range(300):\n"
          "    re.compile(rf'(?P<k{i}>[a-z_]+{i})\\s*=\\s*(\\d+|\"[^\"]*\")(?:,\\s*|$)')\n")
VAL_DC = ("import dataclasses\n"
          "for i in range(60):\n"
          "    ns = {'__annotations__': {f'f{j}': int for j in range(5)}}\n"
          "    dataclasses.dataclass(type(f'C{i}', (), ns))\n")

# benches/workloads/devtools/import_startup.py IMPORTS (one process each)
IMPORT_STARTUP = (
    "", "requests", "django.db.models", "flask", "fastapi", "pandas", "sqlalchemy.orm", "boto3",
    "rich.console", "click", "pydantic", "numpy", "httpx", "aiohttp", "jinja2", "yaml", "attrs",
    "typer", "pytest", "black", "pip._internal.cli.main",
)


def programs(workdir: str, which: str) -> dict[str, list[str]]:
    os.makedirs(workdir, exist_ok=True)
    empty = os.path.join(workdir, "empty")
    os.makedirs(empty, exist_ok=True)
    small = os.path.join(workdir, "small.json")
    with open(small, "w") as f:
        json.dump({"a": [1, 2, 3], "b": {"c": "d", "e": None}, "f": 1.5}, f)
    app = os.path.join(workdir, "typer_app.py")
    with open(app, "w") as f:
        f.write(TYPER_APP)
    core = {
        "bare": ["-c", "pass"],
        "django_setup": ["-c", DJANGO],
        "flask_app": ["-c", FLASK],
        "fastapi_app": ["-c", FASTAPI],
        "sqlalchemy_orm": ["-c", SQLALCHEMY],
        "pydantic_model": ["-c", PYDANTIC],
        "requests": ["-c", "import requests"],
        "pytest_co": ["-m", "pytest", "--co", "-q", "-p", "no:cacheprovider", empty],
        "pip_version": ["-m", "pip", "--version"],
        "black_version": ["-m", "black", "--version"],
        "json_tool": ["-m", "json.tool", small],
        "typer_help": [app, "--help"],
        "rich_print": ["-c", RICH],
    }
    ist = {f"is:{m or '<bare>'}": ["-c", (f"import {m}; " if m else "") + "import sys; print(len(sys.modules))"]
           for m in IMPORT_STARTUP}
    if which == "validate":
        # known cases for the classifier: the pool's Ir should match the Ir difference
        return {
            "val_re_base": ["-c", "import re"],
            "val_re": ["-c", VAL_RE],
            "val_dc_base": ["-c", "import dataclasses"],
            "val_dc": ["-c", VAL_DC],
        }
    if which == "core":
        return core
    if which == "import_startup":
        return ist
    return {**core, **ist}


def env_for() -> dict[str, str]:
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("RUSTIQUE", "PYTHON", "VIRTUAL_ENV", "COVERAGE"))}
    env["PYTHONHASHSEED"] = "0"
    env["COLUMNS"] = "80"
    # numpy's OpenBLAS starts one thread per core at import; they spin ~2 s of CPU on a
    # 32-core box (not wall time), which would swamp the main thread's pools
    env["OPENBLAS_NUM_THREADS"] = "1"
    env["TERM"] = "dumb"
    return env


def check_run(py: str, argv: list[str], cwd: str, env) -> None:
    p = subprocess.run([py, *argv], cwd=cwd, env=env, capture_output=True, text=True)
    if p.returncode not in (0, 5):  # pytest --co on an empty dir exits 5
        raise SystemExit(f"{argv[:3]} failed ({p.returncode}):\n{p.stderr[-2000:]}")


# --------------------------------------------------------------------------------------
# classification
# --------------------------------------------------------------------------------------

_SUFFIX = re.compile(r"\.(cold|warm|org\.\d+|lto_priv\.\d+|constprop\.\d+|isra\.\d+|part\.\d+|\d+)$")


def base(sym: str) -> str:
    for _ in range(4):
        n = _SUFFIX.sub("", sym)
        if n == sym:
            break
        sym = n
    return sym.split("@@")[0]


MARSHAL = {"r_object", "read_object", "PyMarshal_ReadObjectFromString", "marshal_loads",
           "marshal_loads_impl", "_imp_get_frozen_object", "r_ref", "r_string", "r_PyLong"}
COMPILE = {"_PyAST_Compile", "_PyParser_ASTFromString", "_PyPegen_run_parser_from_string",
           "_PyPegen_run_parser", "Py_CompileStringObject", "builtin_compile",
           "builtin_compile_impl", "_PyParser_ASTFromFile", "Py_CompileStringExFlags"}
SRE = {"_sre_compile", "_sre_compile_impl"}
EXTLOAD = {"_imp_create_dynamic", "_imp_create_dynamic_impl", "_PyImport_RunModInitFunc",
           "_PyImport_LoadDynamicModuleWithSpec", "_imp_exec_dynamic", "_imp_create_builtin",
           "create_builtin", "_imp_exec_builtin", "exec_builtin_or_dynamic"}
SYSCALL = {
    "stat": ("stat", "fstatat", "__fstatat64", "fstatat64", "statx", "__GI___fstatat64",
             "__fstat64", "fstat", "lstat", "__xstat", "__lxstat", "__fxstat", "newfstatat"),
    "open/close": ("open", "openat", "__open64", "__libc_open64", "__GI___open64_nocancel",
                   "__open_nocancel", "close", "__close", "__libc_close", "__open64_nocancel",
                   "__close_nocancel"),
    "read": ("read", "__libc_read", "__read_nocancel", "pread64", "__libc_pread64", "__read"),
    "listdir": ("getdents64", "__getdents64", "readdir", "__readdir64", "opendir",
                "__opendir", "fdopendir", "closedir"),
    "mmap": ("mmap", "__mmap", "mmap64", "munmap", "__munmap", "mprotect", "__mprotect",
             "brk", "__brk", "sbrk", "__sbrk", "madvise"),
}
SYSCALL_BY_NAME = {n: k for k, ns in SYSCALL.items() for n in ns}
INIT = ("Py_InitializeFromConfig", "pyinit_main", "init_interp_main", "pymain_init",
        "_PyRuntime_Initialize", "pyinit_core", "Py_PreInitialize", "_Py_InitializeMain",
        "pyinit_config", "init_importlib", "init_importlib_external")
FINI = ("Py_FinalizeEx", "_Py_Finalize", "finalize_modules", "Py_Exit", "Py_RunMain.cold",
        "_PyGC_CollectNoFail", "finalize_interp_clear", "pymain_free")
MAINRUN = ("pymain_run_python", "pymain_run_command", "pymain_run_module", "pymain_run_file",
           "_PyRun_SimpleStringFlagsWithName", "PyRun_SimpleStringFlags", "_PyRun_AnyFileObject")

# the innermost of these on the stack names the phase of init / finalization
PHASES = {"_PyGC_CollectNoFail", "gc_collect_main", "_PyGC_Collect", "finalize_modules",
          "finalize_modules_clear_weaklist", "finalize_clear_modules_dict",
          "_PyImport_ClearModules", "finalize_interp_clear", "_PyInterpreterState_Clear",
          "interpreter_clear", "_PyAtExit_Call", "call_py_exitfuncs", "_PyThread_FiniType",
          "wait_for_thread_shutdown", "flush_std_files", "_PyGC_Fini", "_PyUnicode_Fini",
          "pycore_interp_init", "init_interp_main", "_PyImport_InitCore", "init_importlib",
          "init_importlib_external", "init_sys_streams", "create_stdio", "_PyUnicode_InitTypes",
          "pyinit_config", "_PyRuntime_Initialize", "_PyBuiltin_Init", "_PySys_Create",
          "init_interp_settings", "pycore_init_types", "pycore_init_builtins",
          "_Py_InitializeRecursionLimits", "init_import_site", "_PyFaulthandler_Init",
          "_PyPerfTrampoline_Init", "_PyCodec_InitRegistry", "_PyStructSequence_InitBuiltin",
          "_PyTypes_InitTypes", "_PyStaticType_InitBuiltin", "type_ready"}

CLASS_MACHINERY = {
    "dataclasses.py": "dataclasses", "enum.py": "enum", "typing.py": "typing",
    "collections/__init__.py": "collections/namedtuple", "functools.py": "functools",
    "<frozen abc>": "abc", "abc.py": "abc", "inspect.py": "inspect",
    "annotationlib.py": "annotationlib", "types.py": "types",
}
IMPORTLIB_FILES = ("<frozen importlib._bootstrap>", "<frozen importlib._bootstrap_external>",
                   "<frozen zipimport>", "<frozen importlib.util>", "<frozen importlib.machinery>",
                   "importlib/__init__.py", "importlib/util.py", "importlib/machinery.py",
                   "importlib/_abc.py", "pkgutil.py", "zipimport.py")
RE_FILES = ("re/__init__.py", "re/_parser.py", "re/_compiler.py", "re/_casefix.py",
            "re/_constants.py", "sre_compile.py", "sre_parse.py")


class Owner:
    """filename -> stdlib:<mod> / thirdparty:<pkg> / own / generated."""

    def __init__(self, stdlib_dirs: list[str], own_files: set[str]):
        self.stdlib = [d.rstrip("/") + "/" for d in stdlib_dirs]
        self.own = own_files
        self.cache: dict[str, str] = {}

    def __call__(self, fname: str) -> str:
        r = self.cache.get(fname)
        if r is not None:
            return r
        if fname.startswith("<frozen "):
            r = "stdlib:" + fname[8:-1].split(".")[0]
        elif fname == "<string>" or fname in self.own:
            r = "own"
        elif fname.startswith("<"):
            r = "generated"
        elif "/site-packages/" in fname:
            rest = fname.split("/site-packages/", 1)[1]
            r = "thirdparty:" + rest.split("/")[0].removesuffix(".py").split(".")[0]
        else:
            r = "other:" + os.path.basename(fname)
            for d in self.stdlib:
                if fname.startswith(d):
                    rest = fname[len(d):]
                    r = "stdlib:" + rest.split("/")[0].removesuffix(".py")
                    break
        self.cache[fname] = r
        return r


def tail_match(fname: str, names) -> str | None:
    for n in names:
        if fname == n or fname.endswith("/" + n):
            return n
    return None


def layer(o: str) -> str:
    return o.split(":")[0] if ":" in o else o


def py_pool(pyframes: list[tuple[str, str]], owner: Owner) -> tuple[str, str]:
    """(pool, subkey) from the Python stack, innermost first.

    Walk outwards to the first marker frame (import system, metadata, site, re, class
    machinery). Reaching a module body `<module>` first means the sample is code run at
    import time: its pool is the layer of the INNERMOST Python frame (stdlib / thirdparty /
    own), so a third-party metaclass running under the program's own module body counts as
    third-party.
    """
    last = len(pyframes) - 1
    inner = owner(pyframes[0][1]) if pyframes else "?"
    if pyframes and pyframes[0][1] == "<string>" and last > 0:
        inner = "generated"
    for i, (qual, fname) in enumerate(pyframes):
        if qual == "<module>":
            if fname == "<string>" and i != last:
                continue  # generated source exec'd (dataclasses, namedtuple, .pth lines)
            if owner(fname).startswith("stdlib:site"):
                return "site/.pth", "site <module>"
            return "run:" + layer(inner), inner
        if fname.startswith("<frozen site>") or fname.endswith("/site.py"):
            return "site/.pth", qual
        if tail_match(fname, IMPORTLIB_FILES):
            return "importlib", qual
        if "importlib/metadata/" in fname:
            return "importlib.metadata", qual
        if "importlib/resources/" in fname:
            return "importlib.resources", qual
        if tail_match(fname, RE_FILES):
            return "re.compile", qual
        m = tail_match(fname, CLASS_MACHINERY)
        if m:
            return "class machinery", CLASS_MACHINERY[m]
    if pyframes:
        return "run:" + layer(inner), inner
    return "?", "?"


def classify(frames: list[tuple[str, str, str]], owner: Owner):
    """frames: (kind 'py'|'n'|'k', sym, dso), leaf first. -> (pool, subkey, leaf)."""
    pyframes = []
    first_py = None
    for i, (k, s, _d) in enumerate(frames):
        if k == "py":
            qual, _, fname = s.partition(":")
            pyframes.append((qual, fname))
            if first_py is None:
                first_py = i
    native = frames[: first_py if first_py is not None else len(frames)]
    names = [base(s) for k, s, _d in native if k == "n" and s]
    nameset = set(names)
    # leaf class
    if frames[0][0] == "k":
        user = next((base(s) for k, s, _d in frames if k == "n" and s), "")
        leaf = "kernel:" + SYSCALL_BY_NAME.get(user, "fault/other")
    elif nameset & MARSHAL:
        leaf = "unmarshal"
    elif nameset & EXTLOAD:
        leaf = "ext-load"
    elif nameset & SRE:
        leaf = "_sre.compile"
    elif nameset & COMPILE:
        leaf = "compile"
    elif frames[0][2].endswith(("ld-linux-x86-64.so.2",)):
        leaf = "ld.so"
    else:
        leaf = "user"
    if frames[0][0] == "k" and (nameset & MARSHAL):
        leaf = "kernel:unmarshal"
    if first_py is None:
        allnames = {base(s) for k, s, _d in frames if k == "n" and s}
        if allnames & set(FINI):
            return "finalize", "", leaf
        if allnames & set(INIT):
            return "init (native)", "", leaf
        if allnames & set(MAINRUN):
            return "main (native: -c compile, runpy entry)", "", leaf
        if "main" in allnames or "Py_BytesMain" in allnames or "Py_RunMain" in allnames:
            return "main (native other)", "", leaf
        if any(k == "n" for k, _s, _d in frames):
            return "process (ld.so/libc start, exit)", "", leaf
        return "process (kernel only: exec/exit)", "", leaf
    if leaf in ("unmarshal", "kernel:unmarshal"):
        return "unmarshal", pyframes[0][0], leaf
    if leaf == "ext-load":
        init = next((s for s in names if s.startswith("PyInit_")), None)
        if init is None:
            init = "builtin:other" if nameset & {"_imp_create_builtin", "create_builtin", "_imp_exec_builtin"} \
                else "dlopen/other"
        return "C-ext load", init, leaf
    pool, sub = py_pool(pyframes, owner)
    if pool == "importlib" and sub == "_compile_bytecode" and not leaf.startswith("kernel"):
        # native work under _compile_bytecode is marshal.loads (r_object frames can be lost
        # by frame-pointer unwinding through BOLT-split functions); _fix_co_filename is a no-op
        # when co_filename already matches (checked on these installs)
        return "unmarshal", "_compile_bytecode", "unmarshal"
    return pool, sub, leaf


def parse_perf(data: str, owner: Owner, comm_ok=lambda c: c.startswith("python"), pids=None):
    """Yield (period, frames) from perf script (frame-pointer callchains). Only the main thread
    of each process (tid == pid). `pids` collects the process ids (their perf maps)."""
    proc = subprocess.Popen(["perf", "script", "--no-inline", "-i", data, "-F",
                             "comm,pid,tid,period,ip,sym,dso"], stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, errors="replace")
    head = None
    frames: list = []
    line_re = re.compile(r"^\s*([0-9a-f]+)\s+(.*?)\s+\((.*)\)$")
    for line in proc.stdout:
        line = line.rstrip("\n")
        if not line.strip():
            if head is not None and frames and comm_ok(head[0]):
                yield head[1], frames
            head, frames = None, []
            continue
        if not line.startswith("\t"):
            p = line.split()
            # "<comm> <pid>/<tid> <period>"
            try:
                pid, _, tid = p[-2].partition("/")
                if pids is not None:
                    pids.add(int(pid))
                head = (p[0] if pid == tid else "<thread>", int(p[-1]))
            except (ValueError, IndexError):
                head = (p[0], 1)
            frames = []
            continue
        m = line_re.match(line)
        if not m:
            continue
        ip, sym, dso = m.groups()
        if sym.startswith("py::"):
            frames.append(("py", sym[4:], dso))
        elif int(ip, 16) >= 0xffff800000000000 or dso in ("[kernel.kallsyms]", "[unknown]") and int(ip, 16) >= 0xffff800000000000:
            frames.append(("k", sym, dso))
        else:
            frames.append(("n", None if sym == "[unknown]" else sym, dso))
    if head is not None and frames and comm_ok(head[0]):
        yield head[1], frames
    proc.wait()


def stdlib_dirs(py: str) -> list[str]:
    out = subprocess.check_output([py, "-c", "import sysconfig; print(sysconfig.get_path('stdlib'))"],
                                  text=True).strip()
    return [out, os.path.realpath(out)]


# --------------------------------------------------------------------------------------
# commands
# --------------------------------------------------------------------------------------

def perf_stat(py: str, argv: list[str], cwd: str, env, xperf=False, runs=3) -> dict:
    vals = defaultdict(list)
    for _ in range(runs):
        cmd = ["perf", "stat", "-x", ",", "-e", "instructions:u,cycles:u,cycles,task-clock,page-faults",
               py, *(["-X", "perf"] if xperf else []), *argv]
        p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
        for line in p.stderr.splitlines():
            parts = line.split(",")
            if len(parts) > 3 and parts[2] in ("instructions:u", "cycles:u", "cycles", "task-clock", "page-faults"):
                try:
                    vals[parts[2]].append(float(parts[0]))
                except ValueError:
                    pass
    return {k: statistics.median(v) for k, v in vals.items()}


def cmd_perf(args) -> None:
    progs = select(args)
    env = env_for()
    owner_fn = None
    results = {}
    tmp = tempfile.mkdtemp(prefix="pools-", dir=args.workdir)
    for name, argv in progs.items():
        check_run(args.python, argv, args.workdir, env)
        own = {os.path.abspath(a) for a in argv if a.endswith(".py")}
        owner_fn = Owner(stdlib_dirs(args.python), own)
        res = {"argv": argv, "stat": perf_stat(args.python, argv, args.workdir, env),
               "stat_xperf": perf_stat(args.python, argv, args.workdir, env, xperf=True, runs=3)}
        ninstr = res["stat_xperf"].get("instructions:u", 1e8)
        # enough processes for ~--samples samples; a low sample rate (period 1M) so that the
        # kernel never throttles (throttling under-samples high-IPC phases: biased shares)
        reps = max(args.reps, min(args.max_reps, int(args.samples * args.period / ninstr) + 1))
        res["reps"] = reps
        pids: set[int] = set()
        for ev, period in (("instructions:u", args.period), ("cycles", args.period)):
            data = os.path.join(tmp, f"{abs(hash(name))}-{ev.replace(':', '_')}.data")
            loop = " ; ".join(["\"$@\""] * reps)
            cmd = ["perf", "record", "-q", "-o", data, "-e", ev, "-c", str(period), "-g", "--",
                   "bash", "-c", loop + " ; true", "_", args.python, "-X", "perf", *argv]
            subprocess.run(cmd, cwd=args.workdir, env=env, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
            pools: Counter = Counter()
            sub: Counter = Counter()
            leafc: Counter = Counter()
            cell: Counter = Counter()
            leafsym: dict = defaultdict(Counter)
            phase: dict = defaultdict(Counter)
            total = 0
            nsamp = 0
            unresolved = 0
            for period_, frames in parse_perf(data, owner_fn, pids=pids):
                pool, sk, leaf = classify(frames, owner_fn)
                # a perf map deleted before `perf script` (another profiler cleaning /tmp)
                # leaves interpreter frames without any Python frame: count them
                if pool.startswith(("process", "main (native other)")) and any(
                        k == "n" and s_ and base(s_).startswith(("_PyEval_EvalFrame", "_PyEval_Vector"))
                        for k, s_, _d in frames):
                    unresolved += period_
                pools[pool] += period_
                sub[(pool, sk)] += period_
                leafc[leaf] += period_
                cell[(pool, leaf)] += period_
                k0, s0, d0 = frames[0]
                leafsym[pool][base(s0) if s0 and k0 != "py" else (k0 + ":" + os.path.basename(d0 or "?"))] += period_
                if pool in ("finalize", "init (native)"):
                    ph = next((base(s) for k, s, _d in frames if k == "n" and s and base(s) in PHASES), "?")
                    phase[pool][ph] += period_
                total += period_
                nsamp += 1
            os.unlink(data)
            sh = (lambda v: round(v / total, 5)) if total else (lambda v: 0.0)
            expect = reps * res["stat_xperf"].get(ev if ev != "cycles" else "cycles", 0)
            res[ev] = {
                "samples": nsamp, "total": total,
                # sampled periods / counted events: ~1.0 unless samples were lost (throttling)
                "coverage": round(total / expect, 3) if expect else None,
                "unresolved": round(unresolved / total, 4) if total else None,
                "pools": {k: sh(v) for k, v in pools.most_common()},
                "leaf": {k: sh(v) for k, v in leafc.most_common()},
                "cells": [[p, l, sh(v)] for (p, l), v in cell.most_common(80)],
                "sub": [[p, s, sh(v)] for (p, s), v in sub.most_common(120)],
                "leafsym": {p: [[n, sh(v)] for n, v in c.most_common(15)] for p, c in leafsym.items()},
                "phase": {p: [[n, sh(v)] for n, v in c.most_common(12)] for p, c in phase.items()},
            }
        for pid in pids:  # only this program's maps: concurrent runs keep theirs
            try:
                os.unlink(f"/tmp/perf-{pid}.map")
            except OSError:
                pass
        results[name] = res
        print(f"{name:28s} Ir {res['stat'].get('instructions:u', 0)/1e6:8.1f}M  "
              f"task {res['stat'].get('task-clock', 0):6.1f} ms  "
              f"reps {reps} cov {res['instructions:u']['coverage']}/{res['cycles']['coverage']} "
              f"unres {res['instructions:u']['unresolved']}/{res['cycles']['unresolved']} "
              f"top {list(res['instructions:u']['pools'].items())[:4]}", flush=True)
        if args.output:
            dump(args.output, results)
    os.rmdir(tmp)


def cmd_importtime(args) -> None:
    progs = select(args)
    env = env_for()
    results = {}
    for name, argv in progs.items():
        runs = []
        for _ in range(args.runs):
            p = subprocess.run([args.python, "-X", "importtime", *argv], cwd=args.workdir, env=env,
                               capture_output=True, text=True)
            mods = {}
            order = []
            for line in p.stderr.splitlines():
                m = re.match(r"import time:\s+(\d+) \|\s+(\d+) \|(\s*)(\S+)", line)
                if m:
                    selfus, cum, ind, mod = int(m.group(1)), int(m.group(2)), len(m.group(3)), m.group(4)
                    mods[mod] = (selfus, cum, ind)
                    order.append(mod)
            runs.append(mods)
        allmods = set().union(*runs)
        med = {m: (statistics.median(r[m][0] for r in runs if m in r),
                   statistics.median(r[m][1] for r in runs if m in r),
                   runs[0].get(m, (0, 0, 0))[2]) for m in allmods}
        top_level = sum(v[1] for v in med.values() if v[2] == 1)
        self_total = sum(v[0] for v in med.values())
        results[name] = {"modules": len(med), "self_total_us": self_total, "top_cum_us": top_level,
                         "top_self": sorted(([m, v[0], v[1]] for m, v in med.items()), key=lambda x: -x[1])[:40]}
        print(f"{name:28s} modules {len(med):5d}  import self total {self_total/1000:7.1f} ms", flush=True)
    dump(args.output, results)


# Inclusive Ir of C entry points that do not nest in each other (summed per pool). The 3.15
# tail-calling interpreter breaks callgrind's inclusive costs of Python-level code, so only
# leaf C pools are read here; the perf pools are the reference (validated: --programs validate).
CG_FUNCS = {
    "unmarshal": ("marshal_loads", "_imp_get_frozen_object"),
    "parser": ("_PyPegen_run_parser_from_string",),
    "_sre.compile": ("_sre_compile",),
    "ext-load": ("_imp_create_dynamic",),
}


def cmd_callgrind(args) -> None:
    progs = select(args)
    env = env_for()
    results = {}
    for name, argv in progs.items():
        with tempfile.TemporaryDirectory(dir=args.workdir) as d:
            out = os.path.join(d, "cg.out")
            p = subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=yes",
                                f"--callgrind-out-file={out}", args.python, *argv],
                               cwd=args.workdir, env=env, capture_output=True, text=True)
            m = re.search(r"Collected : (\d+)", p.stderr)
            total = int(m.group(1)) if m else 0
            ann = subprocess.run(["callgrind_annotate", "--inclusive=yes", "--threshold=100", out],
                                 capture_output=True, text=True).stdout
        incl = {}
        for line in ann.splitlines():
            mm = re.match(r"\s*([\d,]+) \([^)]*\)\s+\S*:(\S+)", line)
            if not mm:
                mm = re.match(r"\s*([\d,]+)\s+\S*:(\S+)", line)
            if mm:
                fn = base(mm.group(2).split("'")[0])
                incl[fn] = max(incl.get(fn, 0), int(mm.group(1).replace(",", "")))
        pools = {}
        for pool, fns in CG_FUNCS.items():
            # outermost entry points only: the max of the listed (nested) entries
            pools[pool] = sum(incl.get(f, 0) for f in fns)
        results[name] = {"ir": total, "inclusive": pools,
                         "shares": {k: round(v / total, 5) if total else 0 for k, v in pools.items()}}
        print(f"{name:28s} Ir {total/1e6:8.1f}M  {results[name]['shares']}", flush=True)
        dump(args.output, results)


GC_VARIANTS = {
    "plain": ("", ""),
    "gc.disable() first": ("import gc; gc.disable()\n", ""),
    "os._exit(0) at end": ("", "\nimport os, sys; sys.stdout.flush(); os._exit(0)\n"),
    "gc.freeze() at end": ("", "\nimport gc; gc.freeze()\n"),
}


def cmd_gc(args) -> None:
    """Whole-process instructions:u of the `-c` programs with GC/exit variants (perf stat, median
    of --runs): what automatic collections during import and the finalization (its full
    collection) cost. Measurement only: none of the variants is a candidate as such."""
    progs = {k: v for k, v in select(args).items() if v[0] == "-c"}
    env = env_for()
    res = {}
    for name, argv in progs.items():
        row = {}
        for vname, (pre, post) in GC_VARIANTS.items():
            row[vname] = perf_stat(args.python, ["-c", pre + argv[1] + post], args.workdir, env,
                                   runs=args.runs).get("instructions:u", 0)
        res[name] = row
        base_ = row["plain"]
        print(f"{name:28s} " + "  ".join(f"{k}: {v/1e6:7.1f}M ({100*(v-base_)/base_:+.1f}%)" for k, v in row.items()),
              flush=True)
    dump(args.output, res)


def cmd_wall(args) -> None:
    progs = select(args)
    env = env_for()
    times = {n: [] for n in progs}
    for n, argv in progs.items():
        check_run(args.python, argv, args.workdir, env)
    for _ in range(args.rounds):
        for n, argv in progs.items():
            t0 = time.perf_counter_ns()
            subprocess.run([args.python, *argv], cwd=args.workdir, env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            times[n].append((time.perf_counter_ns() - t0) / 1e6)
    res = {}
    for n, t in times.items():
        s = sorted(t)
        res[n] = {"median_ms": statistics.median(t), "p10": s[len(s) // 10], "p90": s[(9 * len(s)) // 10]}
        print(f"{n:28s} {res[n]['median_ms']:8.2f} ms ({res[n]['p10']:.2f}-{res[n]['p90']:.2f})")
    dump(args.output, res)


POOL_COLS = [
    ("init+process", ("process (ld.so/libc start, exit)", "process (kernel only: exec/exit)",
                      "init (native)", "main (native: -c compile, runpy entry)", "main (native other)")),
    ("finalize", ("finalize",)),
    ("site/.pth", ("site/.pth",)),
    ("importlib", ("importlib",)),
    ("unmarshal", ("unmarshal",)),
    ("C-ext init", ("C-ext load",)),
    ("re.compile", ("re.compile",)),
    ("class mach.", ("class machinery",)),
    ("metadata", ("importlib.metadata", "importlib.resources")),
    ("run: stdlib", ("run:stdlib",)),
    ("run: 3rd-party", ("run:thirdparty",)),
    ("run: own/other", ("run:own", "run:generated", "run:other", "?")),
]


def normalize(perf: dict) -> dict:
    """Apply the _compile_bytecode -> unmarshal rule of classify() to data recorded before it."""
    for r in perf.values():
        for ev in ("instructions:u", "cycles"):
            d = r[ev]
            moved = sum(v for p_, k, v in d["sub"] if p_ == "importlib" and k == "_compile_bytecode")
            if moved and not d.get("normalized"):
                d["pools"]["importlib"] = d["pools"].get("importlib", 0) - moved
                d["pools"]["unmarshal"] = d["pools"].get("unmarshal", 0) + moved
                d["sub"] = [[p_ if not (p_ == "importlib" and k == "_compile_bytecode") else "unmarshal", k, v]
                            for p_, k, v in d["sub"]]
                d["normalized"] = True
    return perf


def cmd_table(args) -> None:
    perf = normalize(json.load(open(args.perf)))
    cg = json.load(open(args.callgrind)) if args.callgrind else {}
    wall = json.load(open(args.wall)) if args.wall else {}
    for extra in args.merge or []:  # re-runs of single programs replace the originals
        perf.update(normalize(json.load(open(extra))))
    names = [n for n in perf if (args.set == "all" or (args.set == "core") != n.startswith("is:"))]
    for ev in ("instructions:u", "cycles"):
        print(f"\n#### pools, % of {ev} (perf -X perf, frame pointers)\n")
        hdr = ["program", "Ir (M)", "CPU ms", "wall ms"] + [c for c, _ in POOL_COLS] + ["(src compile)"]
        print("| " + " | ".join(hdr) + " |")
        print("|" + "---|" * len(hdr))
        colsum = defaultdict(list)
        for name in names:
            r = perf[name]
            pools = r[ev]["pools"]
            row = [name, f"{r['stat'].get('instructions:u', 0)/1e6:.0f}",
                   f"{r['stat'].get('task-clock', 0):.1f}",
                   f"{wall[name]['median_ms']:.1f}" if name in wall else ""]
            for col, keys in POOL_COLS:
                v = sum(pools.get(k, 0) for k in keys)
                if name not in ("bare", "is:<bare>"):
                    colsum[col].append(v)
                row.append(f"{100 * v:.1f}")
            # parse+compile of source strings at run time (generated code), across all pools
            v = sum(x for _p, leaf, x in r[ev]["cells"] if leaf == "compile")
            if name not in ("bare", "is:<bare>"):
                colsum["(src compile)"].append(v)
            row.append(f"{100 * v:.1f}")
            print("| " + " | ".join(row) + " |")
        if args.set == "import_startup":
            # the workload as a whole: 21 processes, shares weighted by each process's events
            key = "instructions:u" if ev == "instructions:u" else "cycles"
            w = {n: perf[n]["stat"].get(key, 0) for n in names}
            tw = sum(w.values())
            row = ["import_startup (21 processes)", f"{tw/1e6:.0f}" if ev == "instructions:u" else "",
                   f"{sum(perf[n]['stat'].get('task-clock', 0) for n in names):.1f}", ""]
            for col, keys in POOL_COLS:
                v = sum(w[n] * sum(perf[n][ev]["pools"].get(k, 0) for k in keys) for n in names) / tw
                row.append(f"**{100 * v:.1f}**")
            v = sum(w[n] * sum(x for _p, leaf, x in perf[n][ev]["cells"] if leaf == "compile") for n in names) / tw
            row.append(f"**{100 * v:.1f}**")
            print("| " + " | ".join(row) + " |")
        cols = [c for c, _ in POOL_COLS] + ["(src compile)"]
        print("| **mean** (w/o bare) | | | | " + " | ".join(f"**{100 * statistics.mean(colsum[c]):.1f}**" for c in cols) + " |")
        print("| **max** | | | | " + " | ".join(f"{100 * max(colsum[c]):.1f}" for c in cols) + " |")
    # sub-pools: mean share over programs (instructions:u) of (pool, subkey) and (pool, leaf)
    ev = "instructions:u"
    for title, pools_wanted, by in (
            ("importlib: innermost import-system function", ("importlib",), "sub"),
            ("class machinery: module", ("class machinery",), "sub"),
            ("re.compile: innermost re function", ("re.compile",), "sub"),
            ("run:thirdparty: innermost owner", ("run:thirdparty",), "sub"),
            ("run:stdlib: innermost owner", ("run:stdlib",), "sub"),
            ("metadata/resources: function", ("importlib.metadata", "importlib.resources"), "sub"),
            ("C-ext load: PyInit", ("C-ext load",), "sub"),
            ("leaf class per pool", None, "cells")):
        acc = defaultdict(list)
        for name in names:
            r = perf[name][ev]
            seen = defaultdict(float)
            if by == "sub":
                for p_, k, v in r["sub"]:
                    if p_ in pools_wanted:
                        seen[k] += v
            else:
                for p_, leaf, v in r["cells"]:
                    seen[f"{p_} / {leaf}"] += v
            for k in set(seen) | set(acc):
                acc[k].append(seen.get(k, 0.0))
        for k in acc:
            acc[k] += [0.0] * (len(names) - len(acc[k]))
        rows = sorted(acc.items(), key=lambda kv: -statistics.mean(kv[1]))[: args.top]
        print(f"\n#### {title} (% of whole-process instructions; mean / max over {len(names)} programs)\n")
        print("| key | mean | max | programs > 1% |")
        print("|---|---|---|---|")
        for k, vs in rows:
            print(f"| {k} | {100 * statistics.mean(vs):.2f} | {100 * max(vs):.2f} | {sum(v > 0.01 for v in vs)} |")
    if any("phase" in perf[n][ev] for n in names):
        for pool in ("finalize", "init (native)"):
            acc = defaultdict(list)
            for name in names:
                ph = dict((k, v) for k, v in perf[name][ev].get("phase", {}).get(pool, []))
                for k in set(ph) | set(acc):
                    acc[k].append(ph.get(k, 0.0))
            print(f"\n#### {pool}: phase (innermost marker on the stack), mean % of whole process\n")
            for k, vs in sorted(acc.items(), key=lambda kv: -sum(kv[1]))[:10]:
                print(f"- {k}: {100 * sum(vs) / len(names):.2f}")
    if cg:
        print("\n#### callgrind (whole process Ir, inclusive leaf C pools, %)\n")
        print("| program | Ir (M) | unmarshal | parser | _sre.compile | ext-load |")
        print("|---|---|---|---|---|---|")
        for name, r in cg.items():
            s_ = r["shares"]
            print(f"| {name} | {r['ir']/1e6:.1f} | " + " | ".join(f"{100*s_[k]:.1f}" for k in CG_FUNCS) + " |")


def select(args) -> dict[str, list[str]]:
    progs = programs(args.workdir, args.programs)
    if args.only:
        keep = args.only.split(",")
        progs = {k: v for k, v in progs.items() if k in keep}
    return progs


def dump(path, obj) -> None:
    if path:
        with open(path, "w") as f:
            json.dump(obj, f, indent=1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("perf", "importtime", "callgrind", "wall", "gc"):
        p = sub.add_parser(name)
        p.add_argument("--python", required=True)
        p.add_argument("--workdir", required=True)
        p.add_argument("--programs", default="all", choices=("core", "import_startup", "all", "validate"))
        p.add_argument("--only")
        p.add_argument("-o", "--output")
        if name == "perf":
            p.add_argument("--reps", type=int, default=10, help="minimum processes per program")
            p.add_argument("--max-reps", type=int, default=200)
            p.add_argument("--samples", type=int, default=12000, help="target samples per program")
            p.add_argument("--period", type=int, default=1000003)
        if name in ("importtime", "gc"):
            p.add_argument("--runs", type=int, default=5)
        if name == "wall":
            p.add_argument("-r", "--rounds", type=int, default=30)
    t = sub.add_parser("table")
    t.add_argument("perf")
    t.add_argument("--callgrind")
    t.add_argument("--wall")
    t.add_argument("--set", default="core", choices=("core", "import_startup", "all"))
    t.add_argument("--merge", action="append", help="perf JSON of re-run programs (replace)")
    t.add_argument("--top", type=int, default=15)
    args = ap.parse_args()
    {"perf": cmd_perf, "importtime": cmd_importtime, "callgrind": cmd_callgrind,
     "wall": cmd_wall, "gc": cmd_gc, "table": cmd_table}[args.cmd](args)


if __name__ == "__main__":
    main()
