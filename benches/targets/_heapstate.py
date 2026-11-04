"""Do allocation-heavy cells that reach no Rustique kernel move with `rustique.enable()`?
Each cell: plain, `import rustique`, `import rustique; rustique.enable()`, and enable() of
single groups (Ir per call, callgrind, fresh process each)."""
import sys
sys.path.insert(0, "benches/targets")
from concurrent.futures import ThreadPoolExecutor
from _callgrind import per_call

D100 = "import random\nr = random.Random(1100)\nsrc = [f's{v}' for v in r.sample(range(10**7), 100)]\nbd = dict(zip(src, range(100)))\nx = dict(bd)"
cells = {
    "frozenset() iter": ("for e in x: pass", "x = frozenset()"),
    "{}.copy()": ("x.copy()", "x = {}"),
    "dict(x) 8 int": ("dict(x)", "x = {i: i for i in range(8)}"),
    "{1:1}.values() iter": ("for e in x.values(): pass", "x = {1: 1}"),
    "list(['s'])": ("list(x)", "x = ['s1234']"),
    "dict100.copy()": ("x.copy()", D100),
}
variants = {
    "plain": "",
    "import": "import rustique\n",
    "enable": "import rustique; rustique.enable()\n",
    "enable-containers": "import rustique; rustique.enable(only=['containers'])\n",
    "enable-no-core": "import rustique; rustique.enable(exclude=['containers','builtins','json','float','copy','dataclasses','set'])\n",
}
def one(item):
    (name, (stmt, setup)), (vname, pre) = item
    return name, vname, per_call(stmt, pre + "import gc; gc.collect()\n" + setup, env={"PYTHONHASHSEED": "0"})
items = [(c, v) for c in cells.items() for v in variants.items()]
res = {}
with ThreadPoolExecutor(12) as ex:
    for name, vname, ir in ex.map(one, items):
        res.setdefault(name, {})[vname] = ir
print(f"{'cell':22s} " + " ".join(f"{v:>18s}" for v in variants))
for name in cells:
    print(f"{name:22s} " + " ".join(f"{res[name][v]:18.1f}" for v in variants))
