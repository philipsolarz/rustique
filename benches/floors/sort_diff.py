"""Differential check of the list.sort prototype: kernel vs CPython's own sort, by identity
(stability) on random lists of ints / floats (incl. -0.0/0.0) / strs (1-, 2-, 4-byte kinds, equal
values as distinct objects), structured orders, reverse=True/False, n = 0..300.

    python benches/floors/sort_diff.py [trials]
"""
import random, sys
import rustique

def gen(rng):
    kind = rng.choice(["int", "float", "str", "ustr", "bigint"])
    n = rng.choice([0, 1, 2, 3, 5, 8, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 200, 300])
    k = rng.choice([2, 5, 50, 10**6])
    if kind == "int": v = [rng.randrange(-k, k) + 1000 for _ in range(n)]
    elif kind == "bigint": v = [rng.randrange(-2**70, 2**70) if rng.random() < .3 else rng.randrange(k) for _ in range(n)]
    elif kind == "float": v = [rng.choice([0.0, -0.0, 1.5, rng.uniform(-k, k)]) for _ in range(n)]
    elif kind == "str": v = ["".join(rng.choice("ab k0") for _ in range(rng.randrange(0, 12))) for _ in range(n)]
    else: v = ["".join(rng.choice(["a", "é", "€", "😀", "k"]) for _ in range(rng.randrange(0, 6))) for _ in range(n)]
    # equal values as distinct objects
    v = [(x + "") if isinstance(x, str) else (x * 1 if not isinstance(x, float) else float(repr(x))) for x in v]
    order = rng.choice(["rand", "asc", "desc", "halves", "nearly"])
    if order == "asc": v.sort()
    elif order == "desc": v.sort(reverse=True)
    elif order == "halves": v.sort(); h = len(v) // 2; v = v[h:] + v[:h]
    elif order == "nearly":
        v.sort()
        for _ in range(max(1, len(v) // 20)):
            if v: i, j = rng.randrange(len(v)), rng.randrange(len(v)); v[i], v[j] = v[j], v[i]
    return v, rng.random() < 0.3

def main():
    trials = int(sys.argv[1]) if len(sys.argv) > 1 else 3000
    rng = random.Random(12345)
    cases = [gen(rng) for _ in range(trials)]
    rustique.enable(only=["containers", "builtins"])
    got = []
    for v, rev in cases:
        a = v[:]; a.sort(reverse=rev); b = sorted(v, reverse=rev)
        got.append(([id(x) for x in a], [id(x) for x in b]))
    rustique.disable()
    bad = 0
    for (v, rev), (ga, gb) in zip(cases, got):
        a = v[:]; a.sort(reverse=rev)
        want = [id(x) for x in a]
        if ga != want or gb != want:
            bad += 1
            if bad <= 5: print("MISMATCH", rev, v[:20])
    print(f"{trials} cases, {bad} mismatches")
    sys.exit(1 if bad else 0)

main()
