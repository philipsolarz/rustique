"""pyperf cells for groups `copy` and `dataclasses` on large inputs (wall time; claims
≥ 1.5× only, docs/ACCELERATION.md §7.4). Each cell runs plain and with
`rustique._takeover.enable(only=["copy", "dataclasses"])` in separate pyperf workers.

    python benches/targets/copy_pyperf.py [--fast] [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile

SETUP = r"""
import copy, dataclasses, datetime, enum
class E(enum.Enum):
    A = 1
    B = 2
@dataclasses.dataclass
class Item:
    sku: str
    qty: int
    price: float
@dataclasses.dataclass
class Order:
    id: int
    items: list
    tags: tuple
    when: datetime.datetime
    state: E
class Node:
    def __init__(self, i, kids):
        self.i = i; self.name = f"n{i}"; self.kids = kids
def rec(i):
    return {"id": i, "name": f"user{i}", "email": f"u{i}@x.org", "score": i * 0.5,
            "tags": ["a", "b", str(i % 7)], "active": i % 2 == 0, "meta": {"k": i, "v": None}}
records = [rec(i) for i in range(10_000)]
orders = [Order(i, [Item(f"s{j}", j, 1.5) for j in range(4)], ("x", "y"), datetime.datetime(2024, 1, 1), E.A)
          for i in range(1_000)]
def tree(d, i=0):
    return Node(i, [] if d == 0 else [tree(d - 1, i * 3 + k) for k in range(3)])
nodes = tree(7)
config = {f"section{i}": {"enabled": True, "paths": [f"/p/{j}" for j in range(10)], "limits": (1, 2, 3)}
          for i in range(500)}
big = list(range(100_000))
"""

CELLS = [
    ("deepcopy 10k records", "copy.deepcopy(records)"),
    ("deepcopy 1k dataclass orders", "copy.deepcopy(orders)"),
    ("deepcopy object tree (3^7 nodes)", "copy.deepcopy(nodes)"),
    ("deepcopy config (500 sections)", "copy.deepcopy(config)"),
    ("copy.copy list 100k", "copy.copy(big)"),
    ("asdict 1k orders", "[dataclasses.asdict(o) for o in orders]"),
    ("astuple 1k orders", "[dataclasses.astuple(o) for o in orders]"),
]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fast", action="store_true")
    ap.add_argument("-o", "--out")
    a = ap.parse_args()
    rows = []
    for name, stmt in CELLS:
        means = {}
        for mode in ("plain", "enabled"):
            setup = SETUP
            if mode == "enabled":
                setup = "from rustique import _takeover as _rt; _rt.enable(only=['copy', 'dataclasses'])\n" + SETUP
            with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as f:
                f.write(setup)
                path = f.name
            out = path + ".json"
            cmd = [sys.executable, "-m", "pyperf", "timeit", "-q", "-o", out,
                   "-s", f"exec(open({path!r}).read())", stmt]
            if a.fast:
                cmd.append("--fast")
            subprocess.run(cmd, check=True, capture_output=True)
            import pyperf

            means[mode] = pyperf.BenchmarkSuite.load(out).get_benchmarks()[0].median()
            os.unlink(path)
            os.unlink(out)
        r = means["enabled"] / means["plain"]
        rows.append({"cell": name, "plain_s": means["plain"], "enabled_s": means["enabled"], "ratio": r})
        print(f"{name:40s} plain {means['plain'] * 1e3:9.2f} ms  enabled {means['enabled'] * 1e3:9.2f} ms  {r:6.3f}",
              flush=True)
    if a.out:
        with open(a.out, "w") as f:
            json.dump({"python": sys.version, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
