"""Upper bounds for import-time candidates (docs/atlas/w2-startup.md §3), measured whole-process.

An upper bound replaces a pool's work by a lookup of results captured in a previous run of
the same program, so it measures what an ideal (zero-cost) kernel behind the same entry point
would save. Three modes per experiment, each a fresh process with `sitecustomize` injected
through PYTHONPATH (the same injection in every mode, so its own cost cancels):

    capture   run the original and record the results to a table (marshal)
    control   load the table and install the wrappers, but always call the original
    replay    load the table and return the recorded result when the key matches
              (the original runs on a miss); `verify` also runs the original and compares

saved Ir = Ir(control) - Ir(replay)  (callgrind whole process, --instr-atstart=yes)

Experiments:
    re        `re._compiler.compile(p, flags)` -> `_sre.compile(*recorded args)`: a compiler
              that produces exactly CPython's `_sre.compile` arguments at zero cost.
    dcexec    dataclasses' generated source: `exec(src, globals, ns)` in `dataclasses` and
              `eval/exec` in `collections.namedtuple` -> `exec(code_object)` of the code
              recorded for that exact source (parse + compile at zero cost).

Usage (dev box):
    python benches/startup/upper_bounds.py run --python PY --workdir W --exp re -o re.json
    python benches/startup/upper_bounds.py run --python PY --workdir W --exp dcexec -o dc.json
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import import_pools  # noqa: E402

SITECUSTOMIZE = r'''
import os, sys, marshal, atexit
_exp = os.environ.get("RQ_UB_EXP", "")
_mode = os.environ.get("RQ_UB_MODE", "")
_path = os.environ.get("RQ_UB_TABLE", "")
_stats = {"hit": 0, "miss": 0, "bad": 0}

def _load():
    try:
        with open(_path, "rb") as f:
            return marshal.load(f)
    except (OSError, EOFError, ValueError):
        return {}

def _save(new):
    if not new:
        return
    old = _load()
    for k, v in new.items():
        try:  # marshal takes exact types only (e.g. not a RegexFlag): skip what it refuses
            marshal.dumps((k, v))
        except ValueError:
            continue
        old[k] = v
    data = marshal.dumps(old)
    with open(_path, "wb") as f:
        f.write(data)

def _report():
    if os.environ.get("RQ_UB_STATS"):
        sys.stderr.write("RQ_UB %s %s %r\n" % (_exp, _mode, _stats))

if _exp == "re" and _mode:
    import re, _sre
    from re import _compiler
    _orig = _compiler.compile
    if _mode == "capture":
        _rec = {}
        class _Proxy:
            def __getattr__(self, n):
                return getattr(_sre, n)
            def compile(self, *a):
                if _cur:
                    # exact types (flags arrive as RegexFlag, opcodes as _NamedIntConstant): same values for _sre
                    _rec[_cur[-1]] = (a[0], int(a[1]), [int(x) for x in a[2]], int(a[3]), dict(a[4]), tuple(a[5]))
                return _sre.compile(*a)
        _cur = []
        _compiler._sre = _Proxy()
        def compile(p, flags=0):
            if isinstance(p, (str, bytes)):
                _cur.append((type(p) is str, p, int(flags)))
                try:
                    return _orig(p, flags)
                finally:
                    _cur.pop()
            return _orig(p, flags)
        _compiler.compile = compile
        atexit.register(lambda: _save(_rec))
    else:
        _tab = _load()
        _replay = _mode in ("replay", "verify")
        _sc = _sre.compile
        def compile(p, flags=0):
            if _replay and type(p) in (str, bytes):
                a = _tab.get((type(p) is str, p, int(flags)))
                if a is not None:
                    _stats["hit"] += 1
                    r = _sc(*a)
                    if _mode == "verify" and r != _orig(p, flags):
                        _stats["bad"] += 1
                    return r
            _stats["miss"] += 1
            return _orig(p, flags)
        _compiler.compile = compile
    atexit.register(_report)

if _exp == "dcexec" and _mode:
    import builtins, dataclasses, collections
    _bexec, _beval = builtins.exec, builtins.eval
    if _mode == "capture":
        _rec = {}
        def _wrap(fn, kind):
            def w(src, *a, **k):
                if type(src) is str:
                    _rec[(kind, src)] = compile(src, "<string>", kind)
                return fn(src, *a, **k)
            return w
        dataclasses.exec = _wrap(_bexec, "exec")
        collections.eval = _wrap(_beval, "eval")
        collections.exec = _wrap(_bexec, "exec")
        atexit.register(lambda: _save(_rec))
    else:
        _tab = _load()
        _replay = _mode in ("replay", "verify")
        def _wrap(fn, kind):
            def w(src, *a, **k):
                if _replay and type(src) is str:
                    c = _tab.get((kind, src))
                    if c is not None:
                        _stats["hit"] += 1
                        return fn(c, *a, **k)
                _stats["miss"] += 1
                return fn(src, *a, **k)
            return w
        dataclasses.exec = _wrap(_bexec, "exec")
        collections.eval = _wrap(_beval, "eval")
        collections.exec = _wrap(_bexec, "exec")
    atexit.register(_report)
'''


def callgrind_ir(py, argv, cwd, env) -> int:
    p = subprocess.run(["valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null",
                        "--instr-atstart=yes", py, *argv], cwd=cwd, env=env,
                       capture_output=True, text=True)
    m = re.search(r"Collected : (\d+)", p.stderr)
    return int(m.group(1)) if m else -1


def cmd_run(args) -> None:
    progs = import_pools.select(args)
    inj = tempfile.mkdtemp(prefix="ub-inject-", dir=args.workdir)
    with open(os.path.join(inj, "sitecustomize.py"), "w") as f:
        f.write(SITECUSTOMIZE)
    results = {}
    for name, argv in progs.items():
        table = os.path.join(inj, f"table-{abs(hash(name))}.marshal")
        base_env = import_pools.env_for()
        env = dict(base_env, PYTHONPATH=inj, RQ_UB_EXP=args.exp, RQ_UB_TABLE=table)
        # capture twice (dict order / lazily compiled patterns are deterministic, but be safe)
        for _ in range(2):
            subprocess.run([args.python, *argv], cwd=args.workdir, env=dict(env, RQ_UB_MODE="capture"),
                           capture_output=True)
        stats = {}
        for mode in ("verify", "replay", "control"):
            p = subprocess.run([args.python, *argv], cwd=args.workdir,
                               env=dict(env, RQ_UB_MODE=mode, RQ_UB_STATS="1"), capture_output=True, text=True)
            m = re.search(r"RQ_UB \S+ \S+ (\{.*\})", p.stderr)
            stats[mode] = m.group(1) if m else p.stderr[-300:]
        r = {
            "plain": callgrind_ir(args.python, argv, args.workdir, base_env),
            "control": callgrind_ir(args.python, argv, args.workdir, dict(env, RQ_UB_MODE="control")),
            "replay": callgrind_ir(args.python, argv, args.workdir, dict(env, RQ_UB_MODE="replay")),
            "stats": stats,
        }
        r["saved"] = r["control"] - r["replay"]
        r["saved_share_of_plain"] = round(r["saved"] / r["plain"], 4)
        results[name] = r
        print(f"{name:28s} plain {r['plain']/1e6:8.1f}M control {r['control']/1e6:8.1f}M replay "
              f"{r['replay']/1e6:8.1f}M saved {r['saved']/1e6:7.2f}M ({100*r['saved_share_of_plain']:.1f}%) "
              f"{stats['verify']}", flush=True)
        if args.output:
            with open(args.output, "w") as f:
                json.dump(results, f, indent=1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--python", required=True)
    p.add_argument("--workdir", required=True)
    p.add_argument("--exp", required=True, choices=("re", "dcexec"))
    p.add_argument("--programs", default="core", choices=("core", "import_startup", "all", "validate"))
    p.add_argument("--only")
    p.add_argument("-o", "--output")
    args = ap.parse_args()
    cmd_run(args)


if __name__ == "__main__":
    main()
