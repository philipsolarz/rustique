"""How much of real patterns the `re` port compiles itself (docs/targets/re.md): over the
PyPI corpus (tests/targets/data/re_pypi_corpus.json.gz, with each pattern's own flags), the
literal patterns of the standard library and the patterns real programs compile at import
(an optional `upper_bounds.py` capture table), the share the port accepts, and why the rest
falls back (CPython raises / warns / outside the port).

    python benches/targets/re_coverage.py [--startup-table re-corpus-all-3.15.1.marshal]
"""

from __future__ import annotations

import argparse
import ast
import gzip
import json
import marshal
import os
import sys
import warnings
from collections import Counter

import rustique
from rustique import _rustique

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tests", "targets"))

A = _rustique._accel
NAMES = {"A": 256, "ASCII": 256, "I": 2, "IGNORECASE": 2, "M": 8, "MULTILINE": 8, "S": 16, "DOTALL": 16,
         "U": 32, "UNICODE": 32, "X": 64, "VERBOSE": 64}


def cpython_outcome(p, flags):
    from re import _compiler, _parser
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        try:
            _compiler._code(_parser.parse(p, flags), flags)
        except Exception:  # noqa: BLE001
            return "raises"
    return "warns" if w else "ok"


def classify(items):
    c = Counter()
    for p, fl in items:
        if A._re_compile_args(p, fl) is not None:
            c["port"] += 1
            continue
        o = cpython_outcome(p, fl)
        c["fallback: CPython " + o if o != "ok" else "fallback: outside the port"] += 1
    return c


def report(name, items):
    c = classify(items)
    n = sum(c.values())
    valid = n - c["fallback: CPython raises"] - c["fallback: CPython warns"]
    print(f"{name}: {n} compiles; port {c['port']} = {100 * c['port'] / max(n, 1):.1f}% of all, "
          f"{100 * c['port'] / max(valid, 1):.2f}% of those CPython compiles silently")
    for k, v in sorted(c.items()):
        print(f"    {k:36s} {v}")
    return dict(c)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--startup-table")
    ap.add_argument("-o")
    a = ap.parse_args()
    rustique.enable(only=["re"])
    here = os.path.dirname(os.path.abspath(__file__))
    with gzip.open(os.path.join(here, "..", "..", "tests", "targets", "data", "re_pypi_corpus.json.gz"), "rt") as f:
        rows = json.load(f)
    pypi = []
    for r in rows:
        p = r["p"]
        if r["b"]:
            try:
                p = p.encode("latin-1")
            except UnicodeEncodeError:
                continue
        pypi.append((p, sum({NAMES.get(n, 0) for n in r["f"]})))
    out = {"pypi": report("PyPI corpus (own flags)", pypi)}
    src = os.environ.get("CPYTHON_SRC")
    if src:
        from test_re import _source_constants
        pats = set()
        for root, _d, files in os.walk(os.path.join(src, "Lib")):
            for fn in files:
                if fn.endswith(".py"):
                    pats.update(_source_constants(os.path.join(root, fn), only_re_calls=True))
        out["stdlib"] = report("stdlib literal patterns", [(p, 0) for p in pats])
    if a.startup_table:
        with open(a.startup_table, "rb") as f:
            tab = marshal.load(f)
        items = [(k[1], k[2]) for k in tab if len(k) == 3 and isinstance(k[0], bool)]
        out["startup"] = report("compiled at import by 12 real programs", items)
    if a.o:
        with open(a.o, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
