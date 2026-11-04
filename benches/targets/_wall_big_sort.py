import subprocess, sys
code = r"""
import random, time, sys
kind, cont, n, mode = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
rng = random.Random(1000 + n)
vals = rng.sample(range(-(10**6), 10**6), n)
if kind == "float": vals = [v + 0.5 for v in vals]
if kind == "wide": vals = rng.sample(range(10**8), n)
x = {"set": set, "frozenset": frozenset, "list": list}[cont](vals)
if mode == "on":
    import rustique; rustique.enable()
best = 9e9
for _ in range(9):
    t = time.perf_counter(); sorted(x); best = min(best, time.perf_counter() - t)
print(round(best * 1e3, 2))
"""
for kind, cont, n in [] if len(sys.argv) > 1 else [("int","set",10**6),("float","set",10**6),("int","frozenset",10**6),("float","frozenset",10**6),("int","list",10**6),("wide","set",10**6),("int","set",10**5),("float","set",10**5)]:
    r = {}
    for m in ("plain", "on", "plain", "on"):
        t = float(subprocess.run([sys.executable, "-c", code, kind, cont, str(n), m], capture_output=True, text=True).stdout.strip())
        r[m] = min(r.get(m, 9e9), t)
    print(f"sorted({cont} {kind} {n}): plain {r['plain']} ms  on {r['on']} ms  ratio {r['on']/r['plain']:.3f}", flush=True)

code2 = r"""
import random, time, sys
kind, order, n, mode = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
base = list(range(n)) if kind == "int" else [i / 7 for i in range(n)] if kind == "float" else [f"k{i:08d}" for i in range(n)]
if order == "desc": base = base[::-1]
if order == "halves": base = base[n // 2:] + base[:n // 2]
if order == "rand": random.Random(5).shuffle(base)
if order == "nearly":
    r = random.Random(7)
    for _ in range(max(1, n // 50)):
        i, j = r.randrange(n), r.randrange(n); base[i], base[j] = base[j], base[i]
x = base
if mode == "on":
    import rustique; rustique.enable()
best = 9e9
for _ in range(9):
    t = time.perf_counter(); sorted(x); best = min(best, time.perf_counter() - t)
print(round(best * 1e3, 2))
"""
SIZES2 = [int(v) for v in sys.argv[1].split(",")] if len(sys.argv) > 1 else (10**5, 10**6)
ORDERS2 = sys.argv[2].split(",") if len(sys.argv) > 2 else ("asc", "desc", "halves", "rand", "nearly")
for kind in ("int", "float", "str"):
    for order in ORDERS2:
        for n in SIZES2:
            r = {}
            for m in ("plain", "on", "plain", "on"):
                t = float(subprocess.run([sys.executable, "-c", code2, kind, order, str(n), m], capture_output=True, text=True).stdout.strip())
                r[m] = min(r.get(m, 9e9), t)
            print(f"sorted(list {kind} {order} {n}): plain {r['plain']} ms  on {r['on']} ms  ratio {r['on']/r['plain']:.3f}", flush=True)
