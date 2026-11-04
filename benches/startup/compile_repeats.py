"""How much of the source compiled at import time (`compile`/`exec`/`eval` of strings:
dataclass methods, namedtuple `__new__`, `typing.ForwardRef` strings, SQLAlchemy/attrs code
generation) repeats within one process: the most an in-process compile memo could save
(docs/atlas/w2-startup.md §3). Wraps the three builtins through an injected `sitecustomize`
(PYTHONPATH) and reports calls, distinct sources, the callers, and the wall time of all calls
vs the time of the repeated ones (calls after the first of each (builtin, mode, source)).

    python benches/startup/compile_repeats.py --python PY [--runs 3] pydantic fastapi pytest ...
"""

from __future__ import annotations

import argparse
import os
import subprocess
import tempfile

SITECUSTOMIZE = r'''
import builtins, sys, atexit, time, collections
_c = collections.Counter(); _t = collections.Counter(); _who = collections.Counter()
_orig = {n: getattr(builtins, n) for n in ("compile", "exec", "eval")}
def _wrap(n):
    f = _orig[n]
    def w(src, *a, **k):
        if type(src) is str:
            fr = sys._getframe(1)
            mode = n if n != "compile" else (a[1] if len(a) > 1 else k.get("mode"))
            key = (n, mode, src)
            _c[key] += 1
            _who[fr.f_code.co_filename.rsplit("/", 1)[-1] + ":" + fr.f_code.co_name] += 1
            t0 = time.perf_counter_ns(); r = f(src, *a, **k); _t[key] += time.perf_counter_ns() - t0
            return r
        return f(src, *a, **k)
    return w
for _n in _orig:
    setattr(builtins, _n, _wrap(_n))
def _rep():
    tot = sum(_t.values())
    rep = sum(_t[k] * (_c[k] - 1) / _c[k] for k in _c)
    sys.stderr.write("CNT calls=%d distinct=%d time_total_ms=%.2f time_in_repeats_ms=%.2f callers=%r\n"
                     % (sum(_c.values()), len(_c), tot / 1e6, rep / 1e6, _who.most_common(5)))
atexit.register(_rep)
'''


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--python", required=True)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("modules", nargs="+")
    args = ap.parse_args()
    with tempfile.TemporaryDirectory() as d:
        with open(os.path.join(d, "sitecustomize.py"), "w") as f:
            f.write(SITECUSTOMIZE)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("PYTHON", "RUSTIQUE"))}
        env["PYTHONPATH"] = d
        for m in args.modules:
            last = ""
            for _ in range(args.runs):  # the last run: warm page cache and pyc files
                p = subprocess.run([args.python, "-c", f"import {m}"], env=env, capture_output=True, text=True)
                last = next((line for line in p.stderr.splitlines() if line.startswith("CNT")), p.stderr[-300:])
            print(f"{m}: {last}")


if __name__ == "__main__":
    main()
