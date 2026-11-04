"""Large `sorted()` cells (one execution each, callgrind region): sets/frozensets of ints or
floats in iteration order (long natural runs), Phase 3's scoreboard inputs, and lists.

    python benches/targets/sort_big_cells.py [-j 8] [-k substr]
"""
import os, sys
from concurrent.futures import ThreadPoolExecutor
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

def setup(kind, cont, n):
    vals = {"int": "rng.sample(range(-(10**6), 10**6), N)",
            "float": "[v + 0.5 for v in rng.sample(range(-(10**6), 10**6), N)]",
            "wide": "rng.sample(range(10**8), N)"}[kind]
    return f"import random\nrng = random.Random(1000 + {n})\nN = {n}\nx = {cont}({vals})\nsorted(x[:100] if isinstance(x, list) else list(x)[:100])\n"

CELLS = [(f"sorted({c} {k} {n})", "sorted(x)", setup(k, c, n))
         for n in (10_000, 100_000, 1_000_000) for k in ("int", "float", "wide") for c in ("set", "frozenset", "list")]
CELLS += [(f"sorted(list(set {k} {n}))", "sorted(y)", setup(k, "set", n) + "y = list(x)\n") for n in (100_000, 1_000_000) for k in ("int", "float")]
CELLS += [(f"list(set {k} {n}).sort()", "y.sort()", setup(k, "set", n) + "y = list(x)\n") for n in (1_000_000,) for k in ("int", "float")]

def lst(kind, order, n):
    base = {"int": f"list(range({n}))", "float": f"[i / 7 for i in range({n})]", "str": f"[f'k{{i:08d}}' for i in range({n})]"}[kind]
    o = {"asc": "", "desc": "x.reverse()\n", "halves": "x = x[N // 2:] + x[:N // 2]\n", "rand": "rng.shuffle(x)\n",
         "nearly": "for _ in range(max(1, N // 50)):\n    i, j = rng.randrange(N), rng.randrange(N); x[i], x[j] = x[j], x[i]\n"}[order]
    return f"import random\nrng = random.Random(7)\nN = {n}\nx = {base}\n{o}"

CELLS += [(f"sorted(list {k} {o} {n})", "sorted(x)", lst(k, o, n)) for k in ("int", "float", "str")
          for o in ("asc", "desc", "halves", "nearly", "rand") for n in (5000, 20_000, 100_000)]


def main():
    jobs = int(sys.argv[sys.argv.index("-j") + 1]) if "-j" in sys.argv else 6
    cells = CELLS
    if "-k" in sys.argv:
        sub = sys.argv[sys.argv.index("-k") + 1]
        cells = [c for c in cells if sub in c[0]]
    def one(c):
        name, stmt, st = c
        groups = os.environ.get("SB_ENABLE", "builtins,containers")
        en = "rustique.enable()" if groups == "all" else f"rustique.enable(only={groups.split(',')!r})"
        return name, region(stmt, st), region(stmt, st + f"import rustique; {en}\n")
    with ThreadPoolExecutor(jobs) as ex:
        for name, a, b in ex.map(one, cells):
            print(f"{name:40s} {a:13,d} {b:13,d} {b / a:6.3f}", flush=True)

if __name__ == "__main__":
    main()
