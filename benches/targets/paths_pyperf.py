"""Wall-time cells (pyperf) for large inputs of the `posixpath`, `fnmatch` and `pathlib`
groups: plain CPython vs `rustique._takeover.enable(only=[group])`, each side in its own
pyperf worker processes. Only ratios <= 0.67 (or >= 1.5) are claimed from wall time on the
shared box; small-input and fallback cells are callgrind's job (paths_cg.py).

    python benches/targets/paths_pyperf.py [--fast] [-o out.json]
"""

from __future__ import annotations

import json
import subprocess
import sys

SETUP = r"""
import os, posixpath, fnmatch, pathlib
names = [f"{s}_{i}{e}" for i in range(400) for s, e in [("main", ".py"), ("utils", ".txt"), ("README", ""), ("data", ".json"),
         ("t", ".pyc"), ("x", ".tar.gz"), ("img", ".png"), ("s", ".py"), ("c", ".cfg"), ("n", ".md")] ][:10000]
root = "/tmp/rq-fs-k2j3/project"
dirs = [f"{root}/src/pkg{i % 17}/sub{i % 5}" for i in range(1000)]
files = [posixpath.join(d, n) for d, n in zip(dirs, names)]
base = pathlib.PosixPath(root)
ppaths = [pathlib.PosixPath(f) for f in files]
for p in ppaths: str(p)
"""

CELLS = [
    ("fnmatch", "fnmatch.filter(10k names, '*.py')", "fnmatch.filter(names, '*.py')"),
    ("fnmatch", "12 ignore patterns x 1k names", "[n for n in names[:1000] if not any(fnmatch.fnmatch(n, p) for p in ('*.pyc', '*.tmp', '*.bak', '*~', '.DS_Store', '*.log', 'Thumbs.db', '*.swp', '*.egg-info', '*.o', '*.so', 'coverage.*'))]"),
    ("posixpath", "join + splitext over 1k files", "[posixpath.splitext(posixpath.join(d, n))[1] for d, n in zip(dirs, names)]"),
    ("posixpath", "relpath over 1k files", "[posixpath.relpath(f, root) for f in files]"),
    ("posixpath", "split/dirname/basename over 1k", "[(posixpath.dirname(f), posixpath.basename(f), posixpath.split(f)) for f in files]"),
    ("pathlib", "str(base / n) over 1k", "[str(base / n) for n in names[:1000]]"),
    ("pathlib", "relative_to + parts + stem over 1k", "[(p.relative_to(base).parts, p.stem) for p in ppaths]"),
    ("pathlib", "Path(f).parent.name over 1k", "[pathlib.PosixPath(f).parent.name for f in files]"),
]


def run(group: str, stmt: str, enabled: bool, fast: bool) -> float:
    setup = SETUP
    if enabled:
        setup = f"import rustique._takeover as _rt; _rt.enable(only=[{group!r}])\n" + setup
    cmd = [sys.executable, "-m", "pyperf", "timeit", "-q", "--json", "-s", setup, stmt]
    if fast:
        cmd.insert(4, "--fast")
    out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    data = json.loads(out)
    values = [v for run in data["benchmarks"][0]["runs"] for v in run.get("values", [])]
    values.sort()
    return values[len(values) // 2]


def main() -> None:
    fast = "--fast" in sys.argv
    out = sys.argv[sys.argv.index("-o") + 1] if "-o" in sys.argv else None
    rows = []
    for group, name, stmt in CELLS:
        a = run(group, stmt, False, fast)
        b = run(group, stmt, True, fast)
        rows.append({"group": group, "cell": name, "plain_s": a, "enabled_s": b, "ratio": b / a})
        print(f"{group:10s} {name:42s} {a * 1e6:10.1f} us {b * 1e6:10.1f} us {b / a:6.3f}", flush=True)
    if out:
        with open(out, "w") as f:
            json.dump({"python": sys.version, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
