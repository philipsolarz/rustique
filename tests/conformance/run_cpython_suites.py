"""Conformance gate: CPython's own container test suites, run against Rustique types.

CPython's `Lib/test` parametrises its container tests by the class under test
(`type2test` in `seq_tests`/`list_tests`/`mapping_tests`, `thetype`/`basetype` in
`test_set`). This script subclasses those test classes with the Rustique types and runs
them, each suite in its own subprocess (a crash or hang is recorded, not fatal).

Suites
    list       test_list.ListTest            (list_tests.CommonTest, seq_tests.CommonTest)
    tuple      test_tuple.TupleTest          (seq_tests.CommonTest)
    dict       test_dict.GeneralMappingTests, mapping_tests.TestHashMappingProtocol
    set        test_set.TestSet
    frozenset  test_set.TestFrozenSet

Suites whose Rustique type does not exist yet are reported as `missing`. Test classes that
hard-code builtin literals (e.g. `test_dict.DictTest`, `test_set.TestBasicOps*`) need the
takeover mechanism (docs/TAKEOVER.md) and are not part of this gate yet.

The CPython sources must match the running interpreter's version:

    git clone --depth 1 --branch v3.15.1 --filter=blob:none --sparse \\
        https://github.com/python/cpython.git cpython-3.15.1
    git -C cpython-3.15.1 sparse-checkout set Lib/test

Usage
    python tests/conformance/run_cpython_suites.py --cpython-src cpython-3.15.1 \\
        [--suite list,dict] [--json out.json] [--verbose]

Exit status is non-zero when any test fails, errors or crashes (expected failures listed
in `--allow` are reported but do not fail the gate).
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

SUITES: dict[str, list[tuple[str, str, dict[str, str]]]] = {
    # suite: [(module, class, {attribute: rustique class name})]
    "list": [("test.test_list", "ListTest", {"type2test": "List"})],
    "tuple": [("test.test_tuple", "TupleTest", {"type2test": "Tuple"})],
    "dict": [
        ("test.test_dict", "GeneralMappingTests", {"type2test": "Dict"}),
        ("test.mapping_tests", "TestHashMappingProtocol", {"type2test": "Dict"}),
    ],
    "set": [("test.test_set", "TestSet", {"thetype": "Set", "basetype": "Set"})],
    "frozenset": [("test.test_set", "TestFrozenSet", {"thetype": "FrozenSet", "basetype": "FrozenSet"})],
}


def expose_test_package(cpython_src: str) -> None:
    """Make CPython's `Lib/test` importable as `test` without shadowing the stdlib."""
    lib_test = os.path.join(cpython_src, "Lib", "test")
    if not os.path.isdir(lib_test):
        sys.exit(f"{lib_test} not found (see --cpython-src)")
    tmp = tempfile.mkdtemp(prefix="rustique-conformance-")
    os.symlink(lib_test, os.path.join(tmp, "test"))
    sys.path.insert(0, tmp)


class Recorder(unittest.TextTestResult):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self.outcomes: dict[str, dict[str, str]] = {}

    def _record(self, test: unittest.TestCase, outcome: str, detail: str = "") -> None:
        self.outcomes[test.id()] = {"outcome": outcome, "detail": detail}

    def addSuccess(self, test: unittest.TestCase) -> None:
        super().addSuccess(test)
        self._record(test, "pass")

    def addFailure(self, test: unittest.TestCase, err: Any) -> None:
        super().addFailure(test, err)
        self._record(test, "fail", _summary(err))

    def addError(self, test: unittest.TestCase, err: Any) -> None:
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


def _summary(err: Any) -> str:
    etype, value, tb = err
    frames = traceback.extract_tb(tb)
    where = next((f"{os.path.basename(f.filename)}:{f.lineno}" for f in reversed(frames)
                  if "unittest" not in f.filename), "")
    text = f"{etype.__name__}: {value}".splitlines()[0] if str(value) else etype.__name__
    return f"{text[:300]} ({where})"


def run_child(suite_name: str, verbose: bool) -> dict[str, Any]:
    import importlib

    import rustique as rs

    outcomes: dict[str, dict[str, str]] = {}
    missing = []
    loader = unittest.TestLoader()
    tests = unittest.TestSuite()
    for module_name, class_name, attrs in SUITES[suite_name]:
        resolved = {}
        for attr, rs_name in attrs.items():
            cls = getattr(rs, rs_name, None)
            if cls is None:
                missing.append(rs_name)
            resolved[attr] = cls
        if any(v is None for v in resolved.values()):
            continue
        module = importlib.import_module(module_name)
        base = getattr(module, class_name)
        sub = type(f"Rustique{class_name}", (base,), {**resolved, "__module__": module_name})
        tests.addTests(loader.loadTestsFromTestCase(sub))
    if missing and not tests.countTestCases():
        return {"suite": suite_name, "status": "missing", "missing": sorted(set(missing)), "outcomes": {}}
    stream = sys.stderr if verbose else open(os.devnull, "w")
    runner = unittest.TextTestRunner(stream=stream, verbosity=2 if verbose else 0, resultclass=Recorder)
    result = runner.run(tests)
    outcomes = result.outcomes  # type: ignore[attr-defined]
    return {"suite": suite_name, "status": "ran", "outcomes": outcomes}


def run_suite(args: argparse.Namespace, suite: str) -> dict[str, Any]:
    out = tempfile.NamedTemporaryFile(prefix=f"conf-{suite}-", suffix=".json", delete=False).name
    cmd = [sys.executable, os.path.abspath(__file__), "--child", suite, "--cpython-src", args.cpython_src, "--json", out]
    if args.verbose:
        cmd.append("--verbose")
    start = time.monotonic()
    try:
        proc = subprocess.run(cmd, timeout=args.timeout, capture_output=not args.verbose, text=True)
        rc = proc.returncode
    except subprocess.TimeoutExpired:
        return {"suite": suite, "status": "timeout", "outcomes": {}, "seconds": args.timeout}
    seconds = time.monotonic() - start
    try:
        with open(out) as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        tail = (proc.stderr or "")[-2000:] if not args.verbose else ""
        return {"suite": suite, "status": f"crash (exit {rc})", "outcomes": {}, "seconds": seconds, "stderr": tail}
    finally:
        try:
            os.remove(out)
        except OSError:
            pass
    data["seconds"] = seconds
    return data


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cpython-src", default=os.environ.get("CPYTHON_SRC"), required=os.environ.get("CPYTHON_SRC") is None,
                   help="CPython source checkout matching the running interpreter (or $CPYTHON_SRC)")
    p.add_argument("--suite", default=",".join(SUITES), help="comma-separated: " + ",".join(SUITES))
    p.add_argument("--json", help="write full results here")
    p.add_argument("--allow", help="file of test ids allowed to fail (documented deviations), one per line")
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--verbose", action="store_true")
    p.add_argument("--child", help=argparse.SUPPRESS)
    args = p.parse_args()

    expose_test_package(args.cpython_src)
    if args.child:
        data = run_child(args.child, args.verbose)
        with open(args.json, "w") as f:
            json.dump(data, f)
        return

    allowed: set[str] = set()
    if args.allow:
        with open(args.allow) as f:
            allowed = {line.split("#")[0].strip() for line in f if line.split("#")[0].strip()}

    gil = getattr(sys, "_is_gil_enabled", lambda: True)()
    build = f"{platform.python_version()}{'t' if sysconfig.get_config_var('Py_GIL_DISABLED') else ''}"
    print(f"CPython conformance gate — {build} (GIL {'on' if gil else 'off'})")
    results = []
    failed = False
    for suite in [s.strip() for s in args.suite.split(",") if s.strip()]:
        data = run_suite(args, suite)
        results.append(data)
        counts: dict[str, int] = {}
        for o in data["outcomes"].values():
            counts[o["outcome"]] = counts.get(o["outcome"], 0) + 1
        bad = {t: o for t, o in data["outcomes"].items() if o["outcome"] in ("fail", "error", "xpass")}
        unexpected = {t: o for t, o in bad.items() if t.split(".")[-1] not in allowed and t not in allowed}
        status = data["status"]
        if status == "missing":
            print(f"  {suite:10} missing ({', '.join(data['missing'])} not implemented)")
            continue
        if status != "ran":
            failed = True
            print(f"  {suite:10} {status}")
            if data.get("stderr"):
                print("    " + data["stderr"].replace("\n", "\n    "))
            continue
        total = sum(counts.values())
        summary = ", ".join(f"{k} {v}" for k, v in sorted(counts.items()))
        print(f"  {suite:10} {counts.get('pass', 0)}/{total} pass ({summary}) in {data['seconds']:.1f}s")
        for test_id, o in sorted(bad.items()):
            mark = "allowed" if test_id not in unexpected else o["outcome"].upper()
            print(f"    {mark:8} {test_id.split('.', 2)[-1]}: {o['detail']}")
        failed |= bool(unexpected)
    if args.json:
        with open(args.json, "w") as f:
            json.dump({"build": build, "gil": gil, "results": results}, f, indent=1)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
