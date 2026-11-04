"""Diff two whole-process callgrind profiles (e.g. `python -c pass` with and without
RUSTIQUE=1): totals per ELF object, the functions whose exclusive cost changed most, and
the inclusive cost of chosen functions.

    valgrind --tool=callgrind --callgrind-out-file=a.out python -c pass
    RUSTIQUE=1 valgrind --tool=callgrind --callgrind-out-file=b.out python -c pass
    python benches/startup/cgdiff.py a.out b.out [-n 40] [--incl dlopen_implementation ...]

Needs `callgrind_annotate` on PATH. Function keys are `object:function` (file names
stripped), so the same function in two runs matches even if the source path differs.
"""

from __future__ import annotations

import argparse
import collections
import os
import re
import subprocess

LINE = re.compile(r"^\s*([\d,]+)\s+(?:\(\s*[\d.]+%\)\s+)?(.*?)\s*$")


def annotate(path: str, inclusive: bool) -> dict[str, int]:
    out = subprocess.run(
        ["callgrind_annotate", "--threshold=100", f"--inclusive={'yes' if inclusive else 'no'}",
         "--show-percs=no", "--context=0", "--auto=no", path],
        capture_output=True, text=True, check=True).stdout
    funcs: dict[str, int] = collections.Counter()
    started = False
    for line in out.splitlines():
        if "file:function" in line:
            started = True
            continue
        if not started or not line.strip() or line.startswith("-"):
            continue
        m = LINE.match(line)
        if not m:
            continue
        ir = int(m.group(1).replace(",", ""))
        name = m.group(2)
        obj = ""
        mo = re.search(r"\[(.*)\]$", name)
        if mo:
            obj = os.path.basename(mo.group(1))
            name = name[: mo.start()].strip()
        fn = name.split(":", 1)[1] if ":" in name else name
        funcs[f"{obj}:{fn}"] += ir
    return funcs


def by_object(funcs: dict[str, int]) -> dict[str, int]:
    objs: dict[str, int] = collections.Counter()
    for k, v in funcs.items():
        objs[k.split(":", 1)[0]] += v
    return objs


def total(path: str) -> int:
    with open(path) as f:
        for line in f:
            if line.startswith(("summary:", "totals:")):
                return int(line.split()[1])
    raise SystemExit(f"no totals in {path}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("-n", type=int, default=40)
    ap.add_argument("--incl", nargs="*", default=[], help="functions to show inclusive cost for (substring)")
    args = ap.parse_args()
    ta, tb = total(args.a), total(args.b)
    print(f"total: {ta:,} -> {tb:,}  delta {tb - ta:+,} ({tb / ta:.4f}x)")
    ea, eb = annotate(args.a, False), annotate(args.b, False)
    oa, ob = by_object(ea), by_object(eb)
    print("\nper object (exclusive):")
    for o in sorted(set(oa) | set(ob), key=lambda o: -abs(ob.get(o, 0) - oa.get(o, 0))):
        d = ob.get(o, 0) - oa.get(o, 0)
        if d:
            print(f"  {d:>+12,}  {oa.get(o, 0):>12,} -> {ob.get(o, 0):>12,}  {o}")
    print(f"\ntop {args.n} exclusive deltas:")
    keys = sorted(set(ea) | set(eb), key=lambda k: -abs(eb.get(k, 0) - ea.get(k, 0)))
    for k in keys[: args.n]:
        print(f"  {eb.get(k, 0) - ea.get(k, 0):>+12,}  {k}")
    if args.incl:
        ia, ib = annotate(args.a, True), annotate(args.b, True)
        print("\ninclusive:")
        for pat in args.incl:
            for k in sorted(set(ia) | set(ib)):
                if pat in k:
                    print(f"  {ia.get(k, 0):>12,} -> {ib.get(k, 0):>12,}  {k}")


if __name__ == "__main__":
    main()
