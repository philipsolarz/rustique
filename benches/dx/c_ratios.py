"""Per-call ratios of the C-level targets on typical small inputs, and the time saved per hit
of the slot targets: the constants of ``rustique._estimate`` (C_RATIO, SLOT_SAVING_NS) for
calls the estimator cannot replay (docs/atlas/w2-dx.md §5).

    python benches/dx/c_ratios.py [--rounds 5]

Each statement runs in a fresh process per side (plain, ``rustique.enable()``), ``timeit``
autorange, best of 5; rounds interleaved; median over rounds. Wall time on a shared box:
indicative (the estimator only needs the first digit).
"""

import argparse
import json
import statistics
import subprocess
import sys

SETUP = """
import datetime, time
ints = list(range(20, 0, -1)); strs = [f"k{i:03d}" for i in range(20, 0, -1)]; fl = [i * 0.5 for i in range(20)]
tup = tuple(strs); bools = [True] * 10
dt = datetime.datetime(2026, 11, 2, 12, 30, 15); d = dt.date(); t = dt.time(); tm = dt.timetuple()
small = ("GET", "POST", "PUT", "DELETE")
"""
CASES = {  # target: [statements] (the ratio is the geometric mean over its statements)
    "list.sort": ["x = ints[:]; x.sort()", "x = strs[:]; x.sort()"],
    "list.index": ["strs.index('k010')", "ints.index(3)"],
    "list.count": ["strs.count('k010')", "ints.count(3)"],
    "tuple.index": ["tup.index('k010')"],
    "tuple.count": ["tup.count('k010')"],
    "builtins.sum": ["sum(ints)", "sum(fl)"],
    "builtins.min": ["min(ints)", "min(3, 7)", "min(strs)"],
    "builtins.max": ["max(ints)", "max(3, 7)", "max(fl)"],
    "builtins.sorted": ["sorted(ints)", "sorted(strs)"],
    "builtins.any": ["any(bools)"],
    "builtins.all": ["all(bools)"],
    "datetime.date.strftime": ["dt.strftime('%Y-%m-%d %H:%M:%S')", "d.strftime('%d/%m/%Y')"],
    "datetime.time.strftime": ["t.strftime('%H:%M')"],
    "time.strftime": ["time.strftime('%Y-%m-%d %H:%M:%S', tm)"],
    "datetime.datetime.isoformat": ["dt.isoformat()"],
    "datetime.date.isoformat": ["d.isoformat()"],
    "datetime.time.isoformat": ["t.isoformat()"],
    "tuple.__contains__": ["'PUT' in small", "'HEAD' in small"],
    "list.__contains__": ["'k010' in strs", "'zz' in strs"],
    "float.repr": ["repr(0.1 + 0.2)", "repr(1234.5678)"],
}
SLOTS = ("tuple.__contains__", "list.__contains__", "float.repr")

CHILD = """
import json, sys, timeit
if sys.argv[1] == "1":
    import rustique; rustique.enable()
setup = sys.argv[2]
out = {}
for stmt in json.loads(sys.argv[3]):
    tm = timeit.Timer(stmt, setup)
    n, _ = tm.autorange()
    out[stmt] = min(tm.repeat(5, n)) / n * 1e9
print(json.dumps(out))
"""


def side(enabled: bool, stmts: list[str]) -> dict:
    r = subprocess.run([sys.executable, "-c", CHILD, "1" if enabled else "0", SETUP, json.dumps(stmts)],
                       capture_output=True, text=True, check=True)
    return json.loads(r.stdout)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--rounds", type=int, default=5)
    a = p.parse_args()
    stmts = [s for v in CASES.values() for s in v]
    plain: dict[str, list] = {s: [] for s in stmts}
    fast: dict[str, list] = {s: [] for s in stmts}
    for _ in range(a.rounds):
        for s, v in side(False, stmts).items():
            plain[s].append(v)
        for s, v in side(True, stmts).items():
            fast[s].append(v)
    ratios, savings = {}, {}
    print(f"{'target':30} {'statement':42} {'plain ns':>9} {'rustique ns':>11} {'ratio':>6}")
    for target, ss in CASES.items():
        rs, sv = [], []
        for s in ss:
            pm, fm = statistics.median(plain[s]), statistics.median(fast[s])
            rs.append(fm / pm)
            sv.append(pm - fm)
            print(f"{target:30} {s:42} {pm:9.1f} {fm:11.1f} {fm / pm:6.3f}")
        ratios[target] = round(statistics.geometric_mean(rs), 2)
        savings[target] = round(statistics.mean(sv), 1)
    print("\nC_RATIO =", json.dumps({k: v for k, v in ratios.items() if k not in SLOTS}))
    print("SLOT_SAVING_NS =", json.dumps({k: savings[k] for k in SLOTS}))


if __name__ == "__main__":
    main()
