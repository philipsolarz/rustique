"""Compare regrtest summaries (run_tests.sh logs): the tier against the noop harness (the
gate: no test file may fail only with the tier), and both against plain.

    python benches/interp/regrtest/cmp_regrtest.py <outdir>
"""
import os
import re
import sys

d = sys.argv[1]


def parse(path):
    s = open(path, errors="replace").read()
    failed = set()
    for m in re.finditer(r"(\d+) tests? (?:failed|crashed)(?: again)?:\n((?:    .*\n)+)", s):
        failed |= set(m.group(2).split())
    tot = re.search(r"Total tests: run=([\d,]+)(?: failures=([\d,]+))?(?: skipped=([\d,]+))?", s)
    files = re.search(r"Total test files: run=([\d/]+)(?: failed=(\d+))?", s)
    res = re.search(r"Result: (\S+)", s)
    return failed, (tot.groups() if tot else None), (files.groups() if files else None), (res.group(1) if res else "RUNNING/none")


runs = {n: parse(os.path.join(d, n + ".log")) for n in ("plain", "noop", "tier") if os.path.exists(os.path.join(d, n + ".log"))}
for n, r in runs.items():
    inst = os.path.join(d, "installed." + n)
    k = sum(1 for _ in open(inst)) if os.path.exists(inst) else 0
    print(f"{n:6s} tests {r[1]} files {r[2]} result {r[3]}; failed files {len(r[0])}; processes with the variant: {k}")
gate = 0
if "tier" in runs and "noop" in runs:
    only_tier = sorted(runs["tier"][0] - runs["noop"][0])
    only_noop = sorted(runs["noop"][0] - runs["tier"][0])
    print(f"failed only with the tier (vs noop): {only_tier}")
    print(f"failed only without the tier (vs noop): {only_noop}")
    gate = 1 if only_tier else 0
if "noop" in runs and "plain" in runs:
    print(f"harness footprint (noop vs plain): {sorted(runs['noop'][0] - runs['plain'][0])}")


def outcomes(path):
    """{test id: outcome} from a regrtest --junit-xml file."""
    import xml.etree.ElementTree as ET
    out = {}
    for case in ET.parse(path).iter("testcase"):
        tid = f"{case.get('classname')}.{case.get('name')}"
        st = "pass"
        for child in case:
            if child.tag in ("failure", "error", "skipped"):
                st = child.tag
        out[tid] = st
    return out


xml = {n: os.path.join(d, n + ".xml") for n in ("noop", "tier")}
if all(os.path.exists(p) for p in xml.values()):
    a, b = outcomes(xml["noop"]), outcomes(xml["tier"])
    diff = sorted((t, a.get(t), b.get(t)) for t in set(a) | set(b) if a.get(t) != b.get(t))
    print(f"per-test outcomes: noop {len(a)}, tier {len(b)}; differences: {len(diff)}")
    for t, x, y in diff[:40]:
        print(f"   {t}: noop={x} tier={y}")
    gate |= 1 if diff else 0
sys.exit(gate)
