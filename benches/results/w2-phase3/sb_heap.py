"""Heap-offset check of scoreboard cells (0-100 elements, callgrind) that stay above 1.05 alone.

    python sb_heap.py --isolated isolated-instr-3.15.1.txt [--offsets 0,1,2,3] -o heap-3.15.1.txt
        (cwd = the repository root, in the build's venv; benches/targets/_callgrind.py)

A cell that reaches no Rustique kernel can still move by tens of instructions between two
processes: where its allocations land relative to pymalloc's pool boundaries depends on what
was allocated before (core's finding, benches/targets/_heapstate.py). So each cell is measured
plain and enabled, each at several heap offsets: offset k allocates k junk objects of each
of nine small size classes (8..504 bytes, kept alive) after the inputs are built and before
the measured region. Instructions per execution of the cell's statement, with the board's
own inputs (`make_env`), each (mode, offset) in its own fresh callgrind process
(`_callgrind.per_call`, statement unrolled, warmed, Ir(2n) - Ir(n)). Reported: plain and
enabled at offset 0 (the default) and the minimum over the offsets, with both ratios; a cell
whose min-over-offsets ratio is <= 1.05 moves with heap state, not with Rustique.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import os
import re
import sys

sys.path.insert(0, os.path.join(os.getcwd(), "benches", "targets"))
from _callgrind import per_call  # noqa: E402

SETUP = """
import sys, os
sys.path.insert(0, os.path.join({repo!r}, "benches"))
{pre}
import scoreboard as _sb
_c = [c for c in _sb.all_cells() if c.key == {key!r}][0]
globals().update(_sb.make_env(_c.type, _c.storage, _c.n, {impl!r}))
_junk = [bytearray(s) for s in (8, 24, 40, 56, 88, 120, 184, 248, 504) for _ in range({k})]
"""


def cells_from(path: str) -> list[str]:
    out = []
    for line in open(path):
        m = re.match(r"#\s+\S+ (\S.*?): [\d.]+ -> .*STILL > 1\.05", line)
        if m:
            out.append(m.group(1))
    return out


def stmt_of(key: str) -> str:
    sys.path.insert(0, os.path.join(os.getcwd(), "benches"))
    import scoreboard as sb

    c = [c for c in sb.all_cells() if c.key == key][0]
    if c.op.prep:
        raise ValueError(f"{key}: mutating op (prep), not supported here")
    return c.op.code("builtin", c.n, c.storage)


def measure(job):
    key, mode, k = job
    pre = "import rustique; rustique.enable()" if mode == "enabled" else "import rustique"
    impl = "enabled" if mode == "enabled" else "builtin"
    setup = SETUP.format(repo=os.getcwd(), pre=pre, key=key, impl=impl, k=k)
    return job, per_call(stmt_of(key), setup, env={"PYTHONHASHSEED": "0"}, python=sys.executable)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--isolated", nargs="+", required=True, help="sb_isolate.py instr outputs")
    ap.add_argument("--cells", help="extra cell keys, ';'-separated")
    ap.add_argument("--offsets", default="0,1,2,3")
    ap.add_argument("-j", type=int, default=8)
    ap.add_argument("-o", required=True)
    a = ap.parse_args()
    keys = []
    for p in a.isolated:
        keys += cells_from(p)
    keys += [k for k in (a.cells or "").split(";") if k]
    keys = list(dict.fromkeys(keys))
    offs = [int(x) for x in a.offsets.split(",")]
    jobs = [(key, mode, k) for key in keys for mode in ("plain", "enabled") for k in offs]
    res = {}
    with cf.ThreadPoolExecutor(a.j) as ex:
        for (key, mode, k), ir in ex.map(measure, jobs):
            res[(key, mode, k)] = ir
    lines = [f"# heap offsets {offs} (k junk objects of each of 9 size classes before the region); Ir per execution",
             "# cell | plain by offset | enabled by offset | ratio at offset 0 | ratio of the minima | verdict"]
    for key in keys:
        p = [res[(key, "plain", k)] for k in offs]
        e = [res[(key, "enabled", k)] for k in offs]
        r0, rmin = e[0] / p[0], min(e) / min(p)
        verdict = "<= 1.05 at the minimum: heap state" if rmin <= 1.05 else "STILL > 1.05 at the minimum"
        lines.append(f"{key} | " + " ".join(f"{x:.1f}" for x in p) + " | " + " ".join(f"{x:.1f}" for x in e)
                     + f" | {r0:.3f} | {rmin:.3f} | {verdict}")
    open(a.o, "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
