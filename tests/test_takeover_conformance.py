"""Native tier against CPython's own test suites (docs/TAKEOVER.md §5.4).

CPython's test modules are imported *through the native-tier import hook* and run with
unittest, three ways, each module in its own subprocess:

    plain      no rewrite (the reference)
    identity   rewritten, with every target mapped back to the builtin type: the rewritten
               code must behave exactly like the original. This validates the rewrite
               itself (scoping, cells, evaluation order, positions, annotations, ...) on a
               large body of real code; any difference is a rewriter bug.
    rustique   rewritten to rustique.List/Dict/Set: how CPython's own literal-heavy code
               fares with Rustique containers (a measurement, not a gate).

Runs only when ``CPYTHON_SRC`` points at a CPython checkout matching the interpreter
(``Lib/test`` is used). Report:

    CPYTHON_SRC=... python tests/test_takeover_conformance.py [--modules test.test_dict] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import traceback
import unittest

import pytest

#: Test module -> helper modules rewritten with it (they hold the literals the tests use).
MODULES = {
    "test.test_list": ["test.list_tests", "test.seq_tests"],
    "test.test_tuple": ["test.seq_tests"],
    "test.test_dict": ["test.mapping_tests"],
    "test.test_set": [],
    "test.test_dictviews": [],
    "test.test_grammar": [],
    "test.test_scope": [],
    "test.test_super": [],
    "test.test_class": [],
    "test.test_named_expressions": [],
    "test.test_copy": [],
    "test.test_sort": [],
}

CPYTHON_SRC = os.environ.get("CPYTHON_SRC")


def _expose(cpython_src: str) -> None:
    lib_test = os.path.join(cpython_src, "Lib", "test")
    tmp = tempfile.mkdtemp(prefix="rustique-native-conf-")
    os.symlink(lib_test, os.path.join(tmp, "test"))
    sys.path.insert(0, tmp)


def child(cpython_src: str, module: str, mode: str) -> dict[str, str]:
    """Import `module` (rewritten per `mode`) and run its tests: {test id: outcome}."""
    import builtins
    import importlib

    _expose(cpython_src)
    if mode != "plain":
        from rustique._takeover import _hook

        targets = {k: getattr(builtins, k) for k in ("list", "dict", "set")} if mode == "identity" else None
        _hook.install((module, *MODULES.get(module, [])), targets=targets)
    mod = importlib.import_module(module)
    suite = unittest.defaultTestLoader.loadTestsFromModule(mod)
    outcomes: dict[str, str] = {}

    class Rec(unittest.TestResult):
        def addSuccess(self, t):
            outcomes[t.id()] = "pass"

        def addFailure(self, t, err):
            outcomes[t.id()] = "fail: " + _first_line(err)

        def addError(self, t, err):
            outcomes[t.id()] = "error: " + _first_line(err)

        def addSkip(self, t, reason):
            outcomes[t.id()] = "skip"

        def addExpectedFailure(self, t, err):
            outcomes[t.id()] = "xfail"

        def addUnexpectedSuccess(self, t):
            outcomes[t.id()] = "xpass"

    suite.run(Rec())
    return outcomes


def _first_line(err) -> str:
    etype, value, tb = err
    where = traceback.extract_tb(tb)[-1]
    return f"{etype.__name__}: {str(value).splitlines()[0][:200] if str(value) else ''} ({os.path.basename(where.filename)}:{where.lineno})"


def run(cpython_src: str, module: str, mode: str, timeout: int = 900) -> dict[str, str]:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        out = f.name
    cmd = [sys.executable, os.path.abspath(__file__), "--child", module, mode, "--cpython-src", cpython_src, "--json", out]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout,
                          env={**os.environ, "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1"})
    try:
        with open(out) as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return {f"{module}::<crash>": f"crash (exit {proc.returncode}): {proc.stderr[-500:]}"}
    finally:
        os.remove(out)


def summarize(outcomes: dict[str, str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for o in outcomes.values():
        k = o.split(":")[0]
        counts[k] = counts.get(k, 0) + 1
    return counts


@pytest.mark.skipif(not CPYTHON_SRC, reason="set CPYTHON_SRC to a CPython checkout matching this interpreter")
@pytest.mark.parametrize("module", list(MODULES))
def test_identity_rewrite_matches_plain(module):
    plain = run(CPYTHON_SRC, module, "plain")
    ident = run(CPYTHON_SRC, module, "identity")
    assert plain and not any("<crash>" in k for k in plain), plain
    diff = {t: (plain.get(t), ident.get(t)) for t in set(plain) | set(ident) if plain.get(t) != ident.get(t)}
    assert not diff


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cpython-src", default=CPYTHON_SRC, required=CPYTHON_SRC is None)
    ap.add_argument("--modules", default=",".join(MODULES))
    ap.add_argument("--modes", default="plain,identity,rustique")
    ap.add_argument("--json")
    ap.add_argument("--child", nargs=2, help=argparse.SUPPRESS)
    args = ap.parse_args()
    if args.child:
        result = child(args.cpython_src, *args.child)
        with open(args.json, "w") as f:
            json.dump(result, f)
        return
    results: dict[str, dict[str, dict[str, str]]] = {}
    for module in args.modules.split(","):
        results[module] = {}
        for mode in args.modes.split(","):
            results[module][mode] = run(args.cpython_src, module, mode)
        plain = results[module].get("plain", {})
        line = [f"{module:32}"]
        for mode, out in results[module].items():
            c = summarize(out)
            line.append(f"{mode}: {c.get('pass', 0)}/{len(out)} pass")
            if mode != "plain" and plain:
                changed = sum(1 for t in plain if out.get(t) != plain[t])
                line.append(f"({changed} differ from plain)")
        print("  ".join(line), flush=True)
    if args.json:
        with open(args.json, "w") as f:
            json.dump(results, f, indent=1)


if __name__ == "__main__":
    main()
