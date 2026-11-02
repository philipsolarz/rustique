"""The dataclass kernels and the opt-in `interp` tier together (callgrind, Ir per call):
plain / interp tier only / data-model groups only / both.

    python benches/targets/datamodel_interp_cells.py [--jobs 4]
"""
import os
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(__file__))
from _callgrind import per_call  # noqa: E402

DM = ["enum", "generated", "contextlib", "functools"]
BODY = """
import dataclasses
@dataclasses.dataclass(frozen=True)
class F:
    x: int
    y: int
@dataclasses.dataclass(frozen=True, slots=True, kw_only=True)
class E:
    a: int
    b: str = "s"
@dataclasses.dataclass(frozen=True)
class Post:
    x: int
    def __post_init__(self):
        pass
class Plain:
    def __init__(self, a, b=1):
        self.a = a
        self.b = b
"""
CELLS = {
    "F(x=1, y=2) frozen keywords": "F(x=1, y=2)",
    "F(1, 2) frozen positional": "F(1, 2)",
    "E(a=1, b='t') slots kw_only": "E(a=1, b='t')",
    "Post(x=1) __post_init__ (no kernel)": "Post(x=1)",
    "Plain(1, b=2) plain class keywords": "Plain(1, b=2)",
}
MODES = {
    "plain": "",
    "interp": "import rustique\nrustique.enable(only=['interp'], tiers=['interp'])\n",
    "dm": "import rustique\nrustique.enable(only=%r)\n" % DM,
    "both": "import rustique\nrustique.enable(only=%r, tiers=['interp'])\n" % (DM + ["interp"]),
}


def run(name):
    return name, {m: per_call(CELLS[name], pre + BODY) for m, pre in MODES.items()}


with ThreadPoolExecutor(int(sys.argv[sys.argv.index("--jobs") + 1]) if "--jobs" in sys.argv else 4) as ex:
    for name, r in ex.map(run, CELLS):
        p = r["plain"]
        print(f"{name:40s} plain={p:>7,.0f}  " + "  ".join(f"{m}={r[m]:>7,.0f} ({r[m] / p:.3f})" for m in ("interp", "dm", "both")))
