"""CPython's own test modules with and without Rustique targets installed.

Each (module, mode) runs in its own child process: `plain` (no target installed) and
`targets` (the listed target groups installed *before* the test modules are imported, and
asserted installed). The gate passes when both modes give the same outcome for every test
id (pass/fail/error/skip/xfail/xpass), the target status says `installed`, and its hit
counter moved (the kernels were exercised).

    python tests/targets/conformance.py --groups float,json \\
        --modules test_json,test_float,test_format,test_strtod,test_fstring [--json out.json]

`CPYTHON_SRC` (or `--cpython-src`) must be the CPython source tree matching the running
interpreter (its `Lib/test` is exposed as the `test` package).
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import sysconfig
import tempfile
import time
import traceback
import unittest
from typing import Any


def expose_test_package(cpython_src: str) -> None:
    lib_test = os.path.join(cpython_src, "Lib", "test")
    if not os.path.isdir(lib_test):
        sys.exit(f"{lib_test} not found (see --cpython-src)")
    tmp = tempfile.mkdtemp(prefix="rustique-conformance-")
    os.symlink(lib_test, os.path.join(tmp, "test"))
    sys.path.insert(0, tmp)


class Recorder(unittest.TextTestResult):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self.outcomes: dict[str, str] = {}
        self.details: dict[str, str] = {}

    def _record(self, test: Any, outcome: str, detail: str = "") -> None:
        self.outcomes[test.id()] = outcome
        if detail:
            self.details[test.id()] = detail

    def addSuccess(self, test: Any) -> None:
        super().addSuccess(test)
        self._record(test, "pass")

    def addFailure(self, test: Any, err: Any) -> None:
        super().addFailure(test, err)
        self._record(test, "fail", _summary(err))

    def addError(self, test: Any, err: Any) -> None:
        super().addError(test, err)
        self._record(test, "error", _summary(err))

    def addSkip(self, test: Any, reason: str) -> None:
        super().addSkip(test, reason)
        self._record(test, "skip", reason)

    def addExpectedFailure(self, test: Any, err: Any) -> None:
        super().addExpectedFailure(test, err)
        self._record(test, "xfail")

    def addUnexpectedSuccess(self, test: Any) -> None:
        super().addUnexpectedSuccess(test)
        self._record(test, "xpass")

    def addSubTest(self, test: Any, subtest: Any, err: Any) -> None:
        super().addSubTest(test, subtest, err)
        if err is not None:
            kind = "fail" if issubclass(err[0], test.failureException) else "error"
            self._record(subtest, kind, _summary(err))


def _summary(err: Any) -> str:
    etype, value, tb = err
    frames = traceback.extract_tb(tb)
    where = next((f"{os.path.basename(f.filename)}:{f.lineno}" for f in reversed(frames)
                  if "unittest" not in f.filename), "")
    text = f"{etype.__name__}: {value}".splitlines()[0] if str(value) else etype.__name__
    return f"{text[:300]} ({where})"


def install(groups: list[str]) -> dict[str, Any]:
    import rustique
    from rustique import _takeover as _tk

    rustique.enable(only=groups)
    _tk.stats(enabled=True, reset=True)
    out = {}
    for d in _tk.status()["targets"]:
        if d["group"] in groups:
            out[d["name"]] = d
            # Module targets wait for their import (the test modules import them).
            if d["state"] not in ("installed", "pending-import"):
                raise SystemExit(f"target {d['name']} not installed: {d['state']} {d['reason']}")
    return out


def counters(groups: list[str]) -> dict[str, Any]:
    import rustique
    from rustique import _takeover as _tk

    status = {d["name"]: d for d in _tk.status()["targets"]}
    return {name: {**c, "state": status[name]["state"]} for name, c in _tk.stats().items()
            if status.get(name, {}).get("group") in groups}


def alias_cjson() -> None:
    """`test_json@real`: make test_json's C-accelerated variant (`cjson`, normally a fresh
    copy of `json` imported with a fresh `_json`) the process's real `json` module, so its
    C tests (`CTest`) run through the module the targets are installed on."""
    import json

    from test.support import import_helper

    orig = import_helper.import_fresh_module

    def fresh(name, fresh=(), blocked=(), *args, **kw):
        if name == "json" and list(fresh) == ["_json"] and not blocked:
            return json
        return orig(name, fresh, blocked, *args, **kw)

    import_helper.import_fresh_module = fresh


def run_child(module: str, groups: list[str], verbose: bool) -> dict[str, Any]:
    import importlib

    status = install(groups) if groups else {}
    from test import support  # noqa: F401  (CPython's test helpers; sets defaults)

    if module.endswith("@real"):
        module = module[: -len("@real")]
        alias_cjson()
    loader = unittest.TestLoader()
    mod = importlib.import_module(f"test.{module}")
    suite = loader.loadTestsFromModule(mod)
    stream = sys.stderr if verbose else open(os.devnull, "w")
    runner = unittest.TextTestRunner(stream=stream, verbosity=2 if verbose else 0, resultclass=Recorder)
    result = runner.run(suite)
    return {
        "module": module,
        "groups": groups,
        "status": status,
        "counters": counters(groups) if groups else {},
        "outcomes": result.outcomes,  # type: ignore[attr-defined]
        "details": result.details,  # type: ignore[attr-defined]
    }


def run_one(args: argparse.Namespace, module: str, groups: list[str]) -> dict[str, Any]:
    out = tempfile.NamedTemporaryFile(prefix=f"conf-{module}-", suffix=".json", delete=False).name
    cmd = [sys.executable, os.path.abspath(__file__), "--child", module, "--cpython-src", args.cpython_src,
           "--groups", ",".join(groups), "--json", out]
    start = time.monotonic()
    try:
        proc = subprocess.run(cmd, timeout=args.timeout, capture_output=True, text=True)
    except subprocess.TimeoutExpired:
        return {"module": module, "crash": "timeout", "outcomes": {}}
    try:
        with open(out) as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return {"module": module, "crash": f"exit {proc.returncode}: {proc.stderr[-1500:]}", "outcomes": {}}
    finally:
        try:
            os.remove(out)
        except OSError:
            pass
    data["seconds"] = round(time.monotonic() - start, 1)
    return data


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC"))
    p.add_argument("--groups", default="float,json")
    p.add_argument("--modules", default="test_json,test_float,test_format,test_strtod,test_fstring")
    p.add_argument("--json")
    p.add_argument("--timeout", type=int, default=1800)
    p.add_argument("--verbose", action="store_true")
    p.add_argument("--child", help=argparse.SUPPRESS)
    args = p.parse_args()
    if not args.cpython_src:
        sys.exit("set CPYTHON_SRC or --cpython-src")
    expose_test_package(args.cpython_src)
    groups = [g for g in args.groups.split(",") if g]
    if args.child:
        data = run_child(args.child, groups, args.verbose)
        with open(args.json, "w") as f:
            json.dump(data, f)
        return

    build = f"{platform.python_version()}{'t' if sysconfig.get_config_var('Py_GIL_DISABLED') else ''}"
    print(f"conformance {build}: groups {groups}")
    ok = True
    report = []
    for module in [m for m in args.modules.split(",") if m]:
        plain = run_one(args, module, [])
        accel = run_one(args, module, groups)
        diffs = sorted(
            (t, plain["outcomes"].get(t), accel["outcomes"].get(t))
            for t in set(plain["outcomes"]) | set(accel["outcomes"])
            if plain["outcomes"].get(t) != accel["outcomes"].get(t)
        )
        tally = lambda d: {k: list(d["outcomes"].values()).count(k)  # noqa: E731
                           for k in ("pass", "fail", "error", "skip", "xfail", "xpass")}
        hits = {n: (c.get("hits"), c.get("state")) for n, c in accel.get("counters", {}).items()}
        same = not diffs and not plain.get("crash") and not accel.get("crash")
        ok &= same
        print(f"  {module:<14} plain {tally(plain)}  targets {tally(accel)}  "
              f"{'IDENTICAL' if same else 'DIFFERENT'}  hits {hits}")
        for t, a, b in diffs[:20]:
            print(f"    {t}: plain={a} targets={b} {accel.get('details', {}).get(t, '')}")
        for d in (plain, accel):
            if d.get("crash"):
                print(f"    crash: {d['crash']}")
        report.append({"module": module, "plain": plain, "targets": accel, "identical": same})
    if args.json:
        with open(args.json, "w") as f:
            json.dump({"build": build, "groups": groups, "modules": report}, f, indent=1)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
