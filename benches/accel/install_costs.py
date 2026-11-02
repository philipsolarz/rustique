"""Install costs with validation at install (`eager`) vs at first call (`first-call`, the
default on GIL builds): target T9 (IMPACT_ATLAS §14.4), DECISIONS §5 item 3 budgets.
Callgrind regions in fresh processes (GC off in the region), via
benches/startup/install_costs.py's worker.

    python benches/accel/install_costs.py [--only json,copy] [-o out.json]

Cells per mode:
    import M lazy        `import M` with every group pending (`_accel.enable()` first)
    import M lazy+call   the same, then one call of a target of M (its validation)
    enable G (imported)  `_accel.enable(only=[G])` with G's module imported
    enable all (fresh)   `_accel.enable()` after `import rustique` (= RUSTIQUE=1's work)
plus `import M plain` (nothing enabled) as the reference.
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "startup"))
from install_costs import region  # noqa: E402

ACCEL = "import rustique; A = rustique._rustique._accel"
CALLS = {
    "json": "json.dumps({'a': 1})",
    "copy": "copy.deepcopy([1, [2]])",
    "logging": "logging.Formatter('%(message)s').format(logging.LogRecord('a', 20, 'p', 1, 'm', (), None))",
    "random": "random.Random(1).randint(1, 9)",
    "pathlib": "str(pathlib.PurePosixPath('a') / 'b')",
    "fnmatch": "fnmatch.fnmatch('a.py', '*.py')",
    "textwrap": "textwrap.wrap('a b c', 3)",
    "statistics": "statistics.mean([1, 2])",
    "urllib.parse": "urllib.parse.quote('a b')",
    "dataclasses": "dataclasses.fields",
    "datetime": "datetime.datetime(2026, 1, 2).isoformat()",
}
ENABLES = [("logging", "logging"), ("copy", "copy"), ("posixpath", "posixpath"), ("fnmatch", "fnmatch"),
           ("pathlib", "pathlib"), ("json", "json"), ("random", "random"), ("dataclasses", "dataclasses")]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--only", default="")
    ap.add_argument("-o", "--output")
    a = ap.parse_args()
    only = set(a.only.split(",")) if a.only else None
    py = sys.executable
    res: dict[str, dict[str, int]] = {}
    for mod, call in CALLS.items():
        if only and mod.split(".")[0] not in only:
            continue
        res[f"import {mod} plain"] = {"plain": region(py, ACCEL, f"import {mod}")}
    for mode in ("eager", "first-call"):
        pre = f"{ACCEL}; A._set_validation({mode!r})"
        r: dict[str, int] = {}
        for mod, call in CALLS.items():
            if only and mod.split(".")[0] not in only:
                continue
            r[f"import {mod} lazy"] = region(py, pre + "; A.enable()", f"import {mod}")
            r[f"import {mod} lazy+call"] = region(py, pre + "; A.enable()", f"import {mod}\n{call}")
        for group, mod in ENABLES:
            if only and group not in only:
                continue
            r[f"enable {group} (imported)"] = region(py, f"{pre}; import {mod}", f"A.enable(only=[{group!r}])")
        if not only:
            r["enable all (fresh)"] = region(py, pre, "A.enable()")
        for k, v in r.items():
            res.setdefault(k, {})[mode] = v
    print(f"{'cell':34} {'plain':>11} {'eager':>11} {'first-call':>11}  first-call vs plain")
    for k, v in res.items():
        if "plain" in v:
            continue
        base = k.replace(" lazy+call", " plain").replace(" lazy", " plain")
        p = res.get(base, {}).get("plain")
        extra = ""
        if p and " lazy" in k:
            fc = v["first-call"]
            extra = f"  {fc / p:.3f} (+{fc - p:,}; budget +{max(0.05 * p, 100_000):,.0f})"
        print(f"{k:34} {p or '':>11} {v['eager']:>11,} {v['first-call']:>11,}{extra}", flush=True)
    if a.output:
        with open(a.output, "w") as f:
            json.dump(res, f, indent=1)


if __name__ == "__main__":
    main()
