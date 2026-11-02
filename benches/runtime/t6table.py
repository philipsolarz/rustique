#!/usr/bin/env python3
"""Target T6 (IMPACT_ATLAS §14.4) from gcprobe sweeps: the gc tier against a base policy.

    python benches/runtime/t6table.py DIR [DIR2 ...] --python 3.15.1 [--base default] [--policy tier]
        [--instr instr.json]

T6: workloads whose GC share (automatic collections, base policy) is > 10%: geometric mean of
the wall ratio <= 0.87, peak RSS <= 1.05x, max pause <= the base's; every other workload <= 1.02.
Wall ratios: median of per-round paired ratios (meaningful on the quiet box); `est` = 1 - GC
share x (1 - GC work ratio), the deterministic proxy (it under-states savings on full
collections, which cost more per object than young ones); `instr`: instruction ratio of the
measured runs from benches/runtime/tier_instr.py, when given.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics as st
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gctable as T  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--python", default="3.15.1")
    ap.add_argument("--base", default="default")
    ap.add_argument("--policy", default="tier")
    ap.add_argument("--instr")
    ap.add_argument("--min-gc", type=float, default=0.10)
    a = ap.parse_args()
    by: dict = {}
    for d in a.dirs:
        for (py, wl, pol), rounds in T.load(d).items():
            if py == a.python and pol in (a.base, a.policy):
                s = T.summarize_cell(rounds)
                if s:
                    by.setdefault(wl, {})[pol] = s
    instr = {}
    if a.instr:
        for r in json.load(open(a.instr))["rows"]:
            if r["python"] == a.python and r["b"] == a.policy and r["a"] == a.base:
                instr[r["workload"]] = r["instr_ratio"]
    print(f"### T6, {a.python}: {a.policy} vs {a.base}\n")
    print("workload | GC % | GC work × | est × | instr × | wall × (paired) | largest coll. × | max pause × | peak RSS ×")
    print("---|---|---|---|---|---|---|---|---")
    sel, others = [], []
    for wl in sorted(by):
        b, p = by[wl].get(a.base), by[wl].get(a.policy)
        if not b or not p:
            continue
        w = p["work"] / b["work"] if b["work"] else 1.0
        est = 1 - b["gc_share"] * (1 - w)
        r = T.paired_ratio(b, p)[0]
        row = (wl, b["gc_share"], w, est, instr.get(wl), r,
               p["size_max"] / b["size_max"] if b["size_max"] else float("nan"),
               p["pause_max_s"] / b["pause_max_s"] if b["pause_max_s"] else float("nan"),
               p["hwm_kb"] / b["hwm_kb"] if b["hwm_kb"] > 0 else float("nan"))
        (sel if b["gc_share"] > a.min_gc else others).append(row)
    for wl, g, w, est, ins, r, sz, pm, m in sel:
        print(f"{wl} | {100 * g:.1f} | {w:.2f} | {est:.3f} | {'' if ins is None else f'{ins:.3f}'} | {r:.3f} | "
              f"{sz:.2f} | {pm:.2f} | {m:.3f}")
    if sel:
        gm = st.geometric_mean(r for *_, r, _, _, _ in sel)
        ge = st.geometric_mean(e for _, _, _, e, *_ in sel)
        gi = [x[4] for x in sel if x[4]]
        print(f"\nGC > {100 * a.min_gc:.0f}%: {len(sel)} workloads; geomean wall {gm:.3f}, est {ge:.3f}"
              + (f", instr {st.geometric_mean(gi):.3f}" if gi else "")
              + f"; max RSS {max(x[8] for x in sel):.3f}; max largest-collection {max(x[6] for x in sel):.2f}")
    if others:
        worst = max(others, key=lambda x: x[5])
        print(f"others: {len(others)} workloads; worst wall {worst[5]:.3f} ({worst[0]}), worst est "
              f"{max(x[3] for x in others):.3f}, max RSS {max(x[8] for x in others):.3f}")


if __name__ == "__main__":
    main()
