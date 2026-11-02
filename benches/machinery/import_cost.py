#!/usr/bin/env python3
"""Import-time cost of the class-creation hooks (dataclass / namedtuple / contextmanager
recognition) of the w2-machinery prototypes.

    python benches/machinery/import_cost.py [-r 5] [--groups dc,nt,ctx]

Cells (user-space instructions of a whole `python -c ...` process, `perf stat`, median of
`-r`; PYTHONHASHSEED=0):
  synthetic   import of a generated module with 200 dataclasses (plain, frozen, slots,
              kw_only, defaults, default_factory), 50 namedtuples, 50 @contextmanager
  <package>   `import <package>` for real packages that create many of them at import
Each cell runs plain (`import rustique._rustique` first, nothing enabled, so the extension
load cancels) and with `proto_enable(groups)` before the import.
"""

import argparse
import os
import statistics
import subprocess
import sys
import tempfile

PKGS = ["rich.console", "sqlglot", "black", "pydantic", "sqlalchemy.orm", "django.db.models",
        "fastapi", "pip._internal.cli.main"]


def synth(path):
    lines = ["import collections, contextlib, dataclasses", "from dataclasses import dataclass, field", ""]
    for i in range(200):
        kind = i % 5
        deco = ["@dataclass", "@dataclass(frozen=True)", "@dataclass(slots=True)",
                "@dataclass(frozen=True, slots=True, kw_only=True)", "@dataclass(order=True)"][kind]
        lines += [deco, f"class D{i}:", "    a: int", "    b: str = ''", "    c: float = 0.0"]
        if kind == 2:
            lines += ["    tags: list = field(default_factory=list)"]
        lines += [""]
    for i in range(50):
        lines += [f"N{i} = collections.namedtuple('N{i}', 'x y z', defaults=(0,))"]
    for i in range(50):
        lines += ["@contextlib.contextmanager", f"def cm{i}(a=1):", "    yield a", ""]
    with open(path, "w") as f:
        f.write("\n".join(lines))


def instr(code, env):
    r = subprocess.run(["perf", "stat", "-x,", "-e", "instructions:u", "--", sys.executable, "-c", code],
                       capture_output=True, text=True, env=env)
    if r.returncode:
        raise RuntimeError(r.stderr[-1500:])
    return int([ln for ln in r.stderr.splitlines() if "instructions" in ln][-1].split(",")[0])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-r", type=int, default=5)
    ap.add_argument("--groups", default="dc,nt,ctx")
    a = ap.parse_args()
    d = tempfile.mkdtemp()
    synth(os.path.join(d, "synth_models.py"))
    env = dict(os.environ, PYTHONHASHSEED="0", PYTHONPATH=d, PYTHONDONTWRITEBYTECODE="1")
    # warm the .pyc caches of the packages
    for m in ["synth_models"] + PKGS:
        subprocess.run([sys.executable, "-c", f"import {m}"], env=env, capture_output=True)
    pre_plain = "import rustique._rustique as R; import dataclasses, collections, contextlib; "
    pre_proto = pre_plain + f"R.proto_enable({a.groups!r}); "
    b0 = statistics.median(instr(pre_plain + "pass", env) for _ in range(a.r))
    q0 = statistics.median(instr(pre_proto + "pass", env) for _ in range(a.r))
    print(f"proto_enable fixed cost (imports the recognizer module): {(q0 - b0) / 1e6:.2f} M instructions")
    print(f"{'cell':28} {'plain Minstr':>12} {'proto Minstr':>12} {'delta Minstr':>12} {'ratio':>7}")
    for m in ["synth_models"] + PKGS:
        try:
            p = statistics.median(instr(pre_plain + f"import {m}", env) for _ in range(a.r))
            q = statistics.median(instr(pre_proto + f"import {m}", env) for _ in range(a.r))
        except RuntimeError as e:
            print(m, "ERR", str(e)[-200:])
            continue
        dp, dq = p - b0, q - q0
        print(f"{m:28} {dp / 1e6:12.2f} {dq / 1e6:12.2f} {(dq - dp) / 1e6:12.2f} {dq / dp:7.3f}", flush=True)


if __name__ == "__main__":
    main()
