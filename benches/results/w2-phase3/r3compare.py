"""Headline replicate 3 (z07/, 07786e2) against replicate 2 (final/, 7fec7e4): per family, overall,
per-workload moves > 2%, worst, digests.   python r3compare.py"""
import importlib.util, os, statistics
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("A", os.path.join(HERE, "analyze_w2phase3.py"))
A = importlib.util.module_from_spec(spec); spec.loader.exec_module(A)
r3 = {b: A.rows(os.path.join(HERE, "z07"), b) for b in A.BUILDS}
r2 = {b: A.rows(os.path.join(HERE, "final"), b) for b in A.BUILDS}
groups = [(f, lambda w, f=f: A.fam(w) == f) for f in A.FAMS] + [
    ("wave 1's 34", lambda w: A.fam(w) in A.W1FAMS), ("the 31 new", lambda w: A.fam(w) not in A.W1FAMS), ("all 65", lambda w: True)]
print("| family (n) | 3.15.1 rep 3 (07786e2) | 3.15.1 rep 2 (7fec7e4) | 3.15.1t rep 3 | 3.15.1t rep 2 |")
print("|---|---:|---:|---:|---:|")
for label, sel in groups:
    cells = []
    for b in A.BUILDS:
        x = [r["ratio"] for w, r in r3[b].items() if sel(w)]
        y = [r["ratio"] for w, r in r2[b].items() if sel(w)]
        cells += [f"**{A.gm(x):.3f}** ({len(x)})" if x else "—", f"{A.gm(y):.3f}"]
    print(f"| {label} | " + " | ".join(cells) + " |")
print()
for b in A.BUILDS:
    rs = r3[b]
    if not rs:
        continue
    rat = sorted((r["ratio"], w) for w, r in rs.items())
    moves = [(w, round(rs[w]["ratio"], 3), round(r2[b][w]["ratio"], 3)) for w in rs if w in r2[b] and abs(rs[w]["ratio"] / r2[b][w]["ratio"] - 1) > 0.02]
    print(f"{b}: n={len(rs)}; median {statistics.median(x for x, _ in rat):.3f}; best {rat[0][1]} {rat[0][0]:.3f}; worst {rat[-1][1]} {rat[-1][0]:.3f}; "
          f"> 1.02: {[(w, round(x, 3)) for x, w in rat if x > 1.02]}; mismatches: {[w for w, r in rs.items() if not r['same_result']]}; "
          f"plain rep3/rep2 {A.gm([rs[w]['plain_s'] / r2[b][w]['plain_s'] for w in rs if w in r2[b]]):.4f}; moves > 2% (rep 3, rep 2): {moves}")
