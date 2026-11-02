"""Gate cells for routing positional calls of frozen dataclasses to the `__init__` kernel
through `tp_alloc` (docs/targets/generated.md, proposed ACCELERATION §3.6 amendment).

    python benches/targets/datamodel_alloc_cells.py [--jobs 4]
"""
import os, sys
from concurrent.futures import ThreadPoolExecutor
sys.path.insert(0, os.path.dirname(__file__))
from _callgrind import per_call  # noqa: E402

ON = "from rustique import _takeover as _T\n_T.enable(only=['generated'])\n"
BASE = """
import dataclasses
@dataclasses.dataclass(frozen=True)
class F:
    x: int
    y: int
class P:
    def __init__(self, x, y):
        self.x = x
        self.y = y
class Sub(F):
    def __init__(self, x, y):
        super().__init__(x, y)
class Sub2(F):
    pass
@dataclasses.dataclass
class Plain:
    x: int
    y: int
classes = (F, P)
def poly():
    for c in classes:
        c(1, 2)
def g():
    F(1, 2)
"""
CELLS = {
    "F(1, 2) frozen positional": (BASE, "F(1, 2)"),
    "Sub(1, 2) own __init__ (window)": (BASE, "Sub(1, 2)"),
    "Sub2(1, 2) inherited __init__": (BASE, "Sub2(1, 2)"),
    "poly site F/P": (BASE, "poly()"),
    "Plain(1, 2) non-frozen dataclass": (BASE, "Plain(1, 2)"),
    "F(1, 2) under sys.monitoring LINE (tool: fallback)": (
        BASE + "import sys\nm = sys.monitoring\nm.use_tool_id(3, 't')\nm.register_callback(3, m.events.LINE, lambda c, l: None)\nm.set_events(3, m.events.LINE)\n",
        "F(1, 2)",
    ),
    "F(1, 2) under setprofile (tool: fallback)": (BASE + "import sys\nsys.setprofile(lambda *a: None)\n", "F(1, 2)"),
    "site specialized while on, then disable(): g()": (
        BASE + "for _ in range(100): g()\n" + "from rustique import _takeover as _T2\n_T2.disable()\n",
        "g()",
    ),
    "disable() then enable() again: g()": (
        BASE + "for _ in range(100): g()\n" + "from rustique import _takeover as _T2\n_T2.disable()\nfor _ in range(100): g()\n_T2.enable(only=['generated'])\n",
        "g()",
    ),
}

def run(name):
    setup, stmt = CELLS[name]
    plain = per_call(stmt, setup.replace("from rustique import _takeover as _T2\n_T2.disable()\n", "").replace("_T2.enable(only=['generated'])\n", ""))
    on = per_call(stmt, ON + setup)
    return name, plain, on

with ThreadPoolExecutor(int(sys.argv[sys.argv.index("--jobs") + 1]) if "--jobs" in sys.argv else 4) as ex:
    for name, plain, on in ex.map(run, CELLS):
        print(f"{name:48s} plain={plain:>8,.0f}  on={on:>8,.0f}  ({on / plain:.3f})")
