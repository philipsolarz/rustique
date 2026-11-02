"""Conformance of accelerated targets (docs/ACCELERATION.md §7.1): CPython's own test
modules pass with Rustique enabled exactly as without.

Each module runs twice, each time in fresh subprocesses: plain, and with
`rustique._takeover.enable(only=TARGETS)` called *before* the test module (and so the
modules it tests) is imported. Outcomes are recorded per test id (pass / fail / error /
skip / xfail / xpass, or the whole module crashing) and compared: any test id whose outcome
differs is a conformance failure. The enabled run also asserts that every target named in
`--expect-installed` (default: the target names in `--targets`) ended up installed (a
module target installs when the test module imports its module).

The CPython sources must match the running interpreter (`$CPYTHON_SRC`, set on the dev
box): `Lib/test` is exposed as package `test` without shadowing the stdlib.

Usage
    python tests/conformance/run_target_suites.py --targets json \\
        --modules test.test_json [--expect-installed json.dumps,json.loads] \\
        [--json out.json] [--allow allowed.txt] [--timeout 1800] [--verbose]

`--modules` takes comma-separated module names (`test.test_json`, a package runs via its
`load_tests`). `--allow` lists test ids whose differences are documented (one per line,
`#` comments). Exit status is non-zero on any unexplained difference, a crash or timeout
of the enabled run that the plain run does not have, or a target not installed.

From a test or script: `compare(modules, targets)` returns the same report as a dict.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import subprocess
import sys
import sysconfig
import tempfile
import time
import traceback
import unittest
from typing import Any

HERE = os.path.abspath(__file__)


def expose_test_package(cpython_src: str) -> None:
    """Make CPython's `Lib/test` importable as `test` without shadowing the stdlib."""
    lib_test = os.path.join(cpython_src, "Lib", "test")
    if not os.path.isdir(lib_test):
        sys.exit(f"{lib_test} not found (set $CPYTHON_SRC or --cpython-src)")
    tmp = tempfile.mkdtemp(prefix="rustique-targets-")
    os.symlink(lib_test, os.path.join(tmp, "test"))
    sys.path.insert(0, tmp)


class Recorder(unittest.TextTestResult):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self.outcomes: dict[str, dict[str, str]] = {}

    def _record(self, test: Any, outcome: str, detail: str = "") -> None:
        # Sub-tests are recorded under their own id; a failing sub-test marks its id.
        self.outcomes[test.id()] = {"outcome": outcome, "detail": detail}

    def addSuccess(self, test: unittest.TestCase) -> None:
        super().addSuccess(test)
        self.outcomes.setdefault(test.id(), {"outcome": "pass", "detail": ""})

    def addFailure(self, test: unittest.TestCase, err: Any) -> None:
        super().addFailure(test, err)
        self._record(test, "fail", _summary(err))

    def addError(self, test: Any, err: Any) -> None:
        super().addError(test, err)
        self._record(test, "error", _summary(err))

    def addSkip(self, test: unittest.TestCase, reason: str) -> None:
        super().addSkip(test, reason)
        self._record(test, "skip", reason)

    def addExpectedFailure(self, test: unittest.TestCase, err: Any) -> None:
        super().addExpectedFailure(test, err)
        self._record(test, "xfail", _summary(err))

    def addUnexpectedSuccess(self, test: unittest.TestCase) -> None:
        super().addUnexpectedSuccess(test)
        self._record(test, "xpass")

    def addSubTest(self, test: unittest.TestCase, subtest: Any, err: Any) -> None:
        super().addSubTest(test, subtest, err)
        if err is not None:
            kind = "fail" if issubclass(err[0], test.failureException) else "error"
            self.outcomes[subtest.id()] = {"outcome": kind, "detail": _summary(err)}


def _summary(err: Any) -> str:
    etype, value, tb = err
    frames = traceback.extract_tb(tb)
    where = next((f"{os.path.basename(f.filename)}:{f.lineno}" for f in reversed(frames) if "unittest" not in f.filename), "")
    text = f"{etype.__name__}: {value}".splitlines()[0] if str(value) else etype.__name__
    return f"{text[:300]} ({where})"


def run_child(module_name: str, targets: list[str] | None, verbose: bool) -> dict[str, Any]:
    """Runs in the subprocess: optionally enable, then import and run one test module."""
    status_before = None
    if targets is not None:
        from rustique import _rustique, _takeover

        # Validate at install (not at each target's first call): the states reported are
        # final, for modules imported later too.
        _rustique._accel._set_validation("eager")
        _takeover.enable(only=targets)
        status_before = _takeover.status()
    import importlib

    from test import support

    support.verbose = verbose
    module = importlib.import_module(module_name)
    tests = unittest.TestLoader().loadTestsFromModule(module)
    stream = sys.stderr if verbose else open(os.devnull, "w")
    runner = unittest.TextTestRunner(stream=stream, verbosity=2 if verbose else 0, resultclass=Recorder)
    result = runner.run(tests)
    out: dict[str, Any] = {"module": module_name, "outcomes": result.outcomes}  # type: ignore[attr-defined]
    if targets is not None:
        from rustique import _takeover

        st = _takeover.status()
        out["targets"] = {t["name"]: [t["state"], t["reason"]] for t in st["targets"]}
        out["targets_at_enable"] = {t["name"]: t["state"] for t in status_before["targets"]}  # type: ignore[index]
    return out


_ADDR = re.compile(r"0x[0-9a-fA-F]+")


def _by_normalized_id(outcomes: dict[str, dict[str, str]]) -> dict[str, list[dict[str, str]]]:
    """Group outcomes by test id with memory addresses replaced by `0x?`."""
    out: dict[str, list[dict[str, str]]] = {}
    for tid, o in outcomes.items():
        out.setdefault(_ADDR.sub("0x?", tid), []).append(o)
    return out


def _merge(outs: list[dict[str, str]]) -> dict[str, str]:
    """One record for a (possibly collided) normalized id: outcomes joined, first detail."""
    return {"outcome": "+".join(sorted(o["outcome"] for o in outs)), "detail": outs[0].get("detail", "")}


def _run(module: str, targets: list[str] | None, cpython_src: str, timeout: int, verbose: bool) -> dict[str, Any]:
    fd, out = tempfile.mkstemp(prefix="conf-", suffix=".json")
    os.close(fd)
    cmd = [sys.executable, HERE, "--child", module, "--cpython-src", cpython_src, "--json", out]
    if targets is not None:
        cmd += ["--targets", ",".join(targets)]
    if verbose:
        cmd.append("--verbose")
    start = time.monotonic()
    try:
        proc = subprocess.run(cmd, timeout=timeout, capture_output=not verbose, text=True)
    except subprocess.TimeoutExpired:
        return {"module": module, "status": "timeout", "outcomes": {}}
    try:
        with open(out) as f:
            data = json.load(f)
        data["status"] = "ran"
    except (OSError, json.JSONDecodeError):
        data = {"module": module, "status": f"crash (exit {proc.returncode})", "outcomes": {},
                "stderr": (proc.stderr or "")[-3000:]}
    finally:
        try:
            os.remove(out)
        except OSError:
            pass
    data["seconds"] = round(time.monotonic() - start, 1)
    return data


def compare(
    modules: list[str],
    targets: list[str],
    *,
    expect_installed: list[str] | None = None,
    allow: set[str] | None = None,
    cpython_src: str | None = None,
    timeout: int = 1800,
    verbose: bool = False,
) -> dict[str, Any]:
    """Run each module plain and enabled; return the per-module comparison."""
    src = cpython_src or os.environ["CPYTHON_SRC"]
    allow = allow or set()
    if expect_installed is None:
        expect_installed = [t for t in targets if "." in t]
    report: dict[str, Any] = {"python": platform.python_version()
                              + ("t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""),
                              "targets": targets, "modules": {}, "ok": True}
    installed: dict[str, list[Any]] = {}
    for m in modules:
        plain = _run(m, None, src, timeout, verbose)
        enabled = _run(m, targets, src, timeout, verbose)
        # Test ids may embed object reprs (`subTest(parse=<function urlparse at 0x...>)`),
        # which differ between processes: compare per normalized id, as outcome multisets.
        pa, ea = _by_normalized_id(plain["outcomes"]), _by_normalized_id(enabled["outcomes"])
        diffs = []
        for tid in sorted(set(pa) | set(ea)):
            a = pa.get(tid, [{"outcome": "absent"}])
            b = ea.get(tid, [{"outcome": "absent"}])
            if sorted(o["outcome"] for o in a) != sorted(o["outcome"] for o in b):
                allowed = tid in allow or any(_ADDR.sub("0x?", x) == tid for x in allow)
                diffs.append({"id": tid, "plain": _merge(a), "enabled": _merge(b), "allowed": allowed})
        counts = {}
        for o in enabled["outcomes"].values():
            counts[o["outcome"]] = counts.get(o["outcome"], 0) + 1
        bad = [d for d in diffs if not d["allowed"]] or plain["status"] != enabled["status"]
        for name, st in enabled.get("targets", {}).items():
            if st[0] == "installed" or name not in installed:
                installed[name] = st
        report["modules"][m] = {"plain": plain["status"], "enabled": enabled["status"], "counts": counts,
                                "differences": diffs, "seconds": [plain["seconds"], enabled["seconds"]],
                                "stderr": enabled.get("stderr", "")}
        report["ok"] &= not bad
    missing = {t: installed.get(t, ["absent", None]) for t in expect_installed if installed.get(t, [""])[0] != "installed"}
    report["not_installed"] = missing
    report["ok"] &= not missing
    return report


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC"))
    p.add_argument("--targets", help="comma-separated groups or target names for enable(only=...)")
    p.add_argument("--modules", help="comma-separated test modules")
    p.add_argument("--expect-installed", help="targets that must be installed (default: dotted names in --targets)")
    p.add_argument("--json")
    p.add_argument("--allow")
    p.add_argument("--timeout", type=int, default=1800)
    p.add_argument("--verbose", action="store_true")
    p.add_argument("--child", help=argparse.SUPPRESS)
    a = p.parse_args()
    if not a.cpython_src:
        sys.exit("set $CPYTHON_SRC or --cpython-src")
    expose_test_package(a.cpython_src)
    targets = [t.strip() for t in a.targets.split(",") if t.strip()] if a.targets else None
    if a.child:
        data = run_child(a.child, targets, a.verbose)
        with open(a.json, "w") as f:
            json.dump(data, f)
        return
    if not (targets and a.modules):
        sys.exit("--targets and --modules are required")
    allow: set[str] = set()
    if a.allow:
        with open(a.allow) as f:
            allow = {ln.split("#")[0].strip() for ln in f if ln.split("#")[0].strip()}
    expect = [t.strip() for t in a.expect_installed.split(",")] if a.expect_installed else None
    modules = [m.strip() for m in a.modules.split(",") if m.strip()]
    rep = compare(modules, targets, expect_installed=expect, allow=allow, cpython_src=a.cpython_src,
                  timeout=a.timeout, verbose=a.verbose)
    print(f"target conformance — {rep['python']} — targets {','.join(targets)}")
    for m, r in rep["modules"].items():
        c = " ".join(f"{k}={v}" for k, v in sorted(r["counts"].items()))
        nd = len([d for d in r["differences"] if not d["allowed"]])
        print(f"  {m:40s} plain={r['plain']:10s} enabled={r['enabled']:10s} {c}  differences={nd}"
              f" ({r['seconds'][0]}s/{r['seconds'][1]}s)")
        for d in r["differences"]:
            tag = "allowed" if d["allowed"] else "DIFF"
            print(f"    {tag} {d['id']}: {d['plain']['outcome']} -> {d['enabled']['outcome']} {d['enabled'].get('detail', '')}")
        if r["stderr"]:
            print("    stderr:", r["stderr"][-500:])
    for t, st in rep["not_installed"].items():
        print(f"  NOT INSTALLED {t}: {st}")
    print("OK" if rep["ok"] else "FAILED")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(rep, f, indent=1)
    sys.exit(0 if rep["ok"] else 1)


if __name__ == "__main__":
    main()
