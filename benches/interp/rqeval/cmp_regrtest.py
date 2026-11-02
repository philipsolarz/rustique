"""Compare regrtest summaries (run_tests.sh logs) of variants against the plain run.

    python benches/interp/rqeval/cmp_regrtest.py <outdir>
"""
import glob, os, re, sys

d = sys.argv[1]


def parse(path):
    s = open(path, errors="replace").read()
    failed = set()
    for m in re.finditer(r"(\d+) tests? failed(?: again)?:\n((?:    .*\n)+)", s):
        failed |= set(m.group(2).split())
    tot = re.search(r"Total tests: run=([\d,]+)(?: failures=([\d,]+))?(?: skipped=([\d,]+))?", s)
    files = re.search(r"Total test files: run=([\d/]+)(?: failed=(\d+))?", s)
    res = re.search(r"Result: (\S+)", s)
    return failed, (tot.groups() if tot else None), (files.groups() if files else None), (res.group(1) if res else "RUNNING/none")


base = parse(os.path.join(d, "plain.log"))
print(f"plain: tests {base[1]} files {base[2]} result {base[3]}; failed files {len(base[0])}")
for f in sorted(glob.glob(os.path.join(d, "*.log"))):
    name = os.path.basename(f)[:-4]
    if name == "plain":
        continue
    x = parse(f)
    inst = os.path.join(d, "installed." + name)
    n = sum(1 for _ in open(inst)) if os.path.exists(inst) else 0
    print(f"{name}: tests {x[1]} files {x[2]} result {x[3]}; processes with the variant installed: {n}")
    print(f"   failed only with variant: {sorted(x[0] - base[0])}")
    print(f"   failed only plain:        {sorted(base[0] - x[0])}")
