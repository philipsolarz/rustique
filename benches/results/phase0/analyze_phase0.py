"""Phase 0 analysis helpers (run on the benchmark box by run_phase0.sh, or locally).

    python analyze_phase0.py scoreboard [--impl enabled] [--repo DIR] FILES...
        Parity counts / geometric means of a scoreboard run, split into the cells whose
        code path the kernels change ("kernel path") and the rest; headline cells.
        Run from (or with --repo pointing at) a Rustique checkout: uses its
        benches/scoreboard.py to load and classify cells, and needs `rustique` importable.
    python analyze_phase0.py matrix-compare OLD.md NEW.md
        Cell-by-cell comparison of two `scoreboard.py report` matrices.
    python analyze_phase0.py pyperformance PLAIN.json ENABLED.json
        Ratios (enabled / plain), geometric mean, absolute plain wall times.
"""

from __future__ import annotations

import argparse
import importlib.util
import math
import os
import re
import statistics
import sys

KERNEL_BUILTINS = {"sorted(x)", "sum(x)", "min(x)", "max(x)", "any(x)", "all(x)"}
KERNEL_SEQ = {"hit in x", "miss in x", "x.index(hit)", "x.count(hit)"}
SIZES = [0, 1, 8, 100, 10_000, 1_000_000]
SIZE_LABEL = {0: "0", 1: "1", 8: "8", 100: "100", 10_000: "10k", 1_000_000: "1M"}


def kernel_path(typ: str, op: str) -> bool:
    """Cells whose code path `rustique.enable()` changes (README: list.sort/sorted,
    list/tuple index/count/in, builtins sum/min/max/sorted/any/all)."""
    return (op in KERNEL_BUILTINS or (typ in ("list", "tuple") and op in KERNEL_SEQ)
            or (typ == "list" and op == "x.sort()"))


def geomean(xs: list[float]) -> float:
    xs = [x for x in xs if x and x > 0]
    return math.exp(sum(map(math.log, xs)) / len(xs)) if xs else float("nan")


def load_scoreboard_module(repo: str):
    path = os.path.join(repo, "benches", "scoreboard.py")
    spec = importlib.util.spec_from_file_location("scoreboard", path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["scoreboard"] = mod
    spec.loader.exec_module(mod)
    return mod


def cmd_scoreboard(args: argparse.Namespace) -> None:
    sb = load_scoreboard_module(args.repo)
    cells, _ = sb.load_results(args.files)
    by_key = {c.key: c for c in sb.all_cells()}
    impl = args.impl
    rows = []  # (build, cell, ratio, status, metric, noise_b, noise_i)
    for (build, key), e in cells.items():
        cell = by_key.get(key)
        if cell is None:
            continue
        r, st, metric = sb.cell_ratio(e, impl, cell.n)
        noise = e.get("noise", {})
        rows.append((build, cell, r, st, metric, noise.get("builtin"), noise.get(impl)))
    for build in sorted({r[0] for r in rows}):
        print(f"=== {build} impl={impl}")
        for label, small in (("0-100 elements (instructions)", True), ("10k/1M elements (wall time)", False)):
            sel = [r for r in rows if r[0] == build and ((r[1].n in sb.SMALL_SIZES) == small)]
            sel = [r for r in sel if r[3] != "missing"]
            if not sel:
                continue
            valid = [r for r in sel if r[2] is not None and r[2] > 0 and r[3] not in ("error", "mismatch")]
            par = [r for r in valid if r[2] <= 1.05]
            mism = [r for r in sel if r[3] == "mismatch"]
            err = [r for r in sel if r[3] == "error"]
            metrics = {}
            for r in valid:
                metrics[r[4]] = metrics.get(r[4], 0) + 1
            print(f"-- {label}: cells {len(sel)}, with ratio {len(valid)} (metrics {metrics}), "
                  f"<= 1.05: {len(par)}/{len(sel)}, geomean {geomean([r[2] for r in valid]):.4f}, "
                  f"mismatch {len(mism)}, error {len(err)}")
            for r in mism + err:
                print(f"     {r[3]}: {r[1].key}")
            kern = [r for r in valid if kernel_path(r[1].type, r[1].op.name)]
            other = [r for r in valid if not kernel_path(r[1].type, r[1].op.name)]
            for name, grp in (("kernel path", kern), ("unchanged code", other)):
                if not grp:
                    continue
                rs = [r[2] for r in grp]
                print(f"   {name}: {len(grp)} cells, geomean {geomean(rs):.4f}, median {statistics.median(rs):.4f}, "
                      f"> 1.05: {sum(x > 1.05 for x in rs)}, < 0.95: {sum(x < 0.95 for x in rs)}, "
                      f"min {min(rs):.3f}, max {max(rs):.3f}")
            above = sorted((r for r in valid if r[2] > 1.05), key=lambda r: -r[2])
            print(f"   cells > 1.05 ({len(above)}):" + ("" if len(above) <= 60 else " (top 60)"))
            for r in above[:60]:
                nz = "" if small else f"  noise builtin {r[5] or 0:.3f} {impl} {r[6] or 0:.3f}"
                k = "K" if kernel_path(r[1].type, r[1].op.name) else " "
                print(f"     {k} {r[2]:.3f}  {r[1].key}{nz}")
            kabove = [r for r in above if kernel_path(r[1].type, r[1].op.name)]
            if kabove:
                print(f"   kernel-path cells > 1.05 ({len(kabove)}):")
                for r in kabove:
                    nz = "" if small else f"  noise builtin {r[5] or 0:.3f} {impl} {r[6] or 0:.3f}"
                    print(f"     K {r[2]:.3f}  {r[1].key}{nz}")
            if small:
                kw = sorted(kern, key=lambda r: -r[2])[:5]
                print("   worst kernel-path cells: " + ", ".join(f"{r[1].key} {r[2]:.3f}" for r in kw))
        print("-- headline cells (1M, wall-time ratio, rel. std builtin/enabled)")
        wanted = [("list", "max(x)", "float"), ("list", "min(x)", "int"), ("list", "x.sort()", "int"),
                  ("list", "sorted(x)", "int"), ("list", "hit in x", "int"), ("list", "miss in x", "int"),
                  ("list", "x.count(hit)", "int"), ("list", "x.index(hit)", "int"), ("list", "sum(x)", "int"),
                  ("list", "sum(x)", "float"), ("dict", "min(x)", "int"), ("set", "max(x)", "int"),
                  ("tuple", "max(x)", "float")]
        idx = {(r[1].type, r[1].op.name, r[1].storage, r[1].n): r for r in rows if r[0] == build}
        for t, op, s in wanted:
            for n in (10_000, 1_000_000):
                r = idx.get((t, op, s, n))
                if r and r[2] is not None:
                    print(f"     {t:5} {op:14} {s:6} {SIZE_LABEL[n]:>3}: {r[2]:.3f}  ({r[5] or 0:.3f}/{r[6] or 0:.3f})")


def parse_matrix(path: str) -> dict[tuple[str, str, str, str, int], tuple[float | None, str]]:
    out = {}
    typ = build = None
    head = re.compile(r"^### (\w+) — (\S+)")
    with open(path) as f:
        for line in f:
            m = head.match(line)
            if m:
                typ, build = m.group(1), m.group(2)
                continue
            if not typ or not line.startswith("| ") or line.startswith("| op ") or line.startswith("|---"):
                continue
            parts = [p.strip() for p in re.split(r"(?<!\\)\|", line.strip())[1:-1]]
            if len(parts) != 2 + len(SIZES):
                continue
            op, storage, vals = parts[0].replace("\\|", "|"), parts[1], parts[2:]
            for n, v in zip(SIZES, vals):
                if not v:
                    continue
                raw = v.replace("*", "")
                try:
                    num = float(raw.rstrip("≠"))
                except ValueError:
                    num = None
                out[(build, typ, op, storage, n)] = (num, raw)
    return out


def cmd_matrix_compare(args: argparse.Namespace) -> None:
    old, new = parse_matrix(args.old), parse_matrix(args.new)
    ob = {k[0] for k in old}
    nb = {k[0] for k in new}
    print(f"old builds {sorted(ob)} ({len(old)} cells), new builds {sorted(nb)} ({len(new)} cells)")
    strip = lambda k: k[1:]  # noqa: E731  (compare across build labels)
    o2 = {strip(k): v for k, v in old.items()}
    n2 = {strip(k): v for k, v in new.items()}
    common = sorted(set(o2) & set(n2), key=lambda k: (k[0], k[1], k[2], k[3]))
    print(f"common cells {len(common)}, only old {len(set(o2) - set(n2))}, only new {len(set(n2) - set(o2))}")
    for k in sorted(set(o2) ^ set(n2))[:20]:
        print(f"   only {'old' if k in o2 else 'new'}: {k} {(o2.get(k) or n2.get(k))[1]}")
    status_changes = [(k, o2[k][1], n2[k][1]) for k in common if (o2[k][0] is None) != (n2[k][0] is None)
                      or ("≠" in o2[k][1]) != ("≠" in n2[k][1])]
    print(f"status changes: {len(status_changes)}")
    for k, a, b in status_changes[:30]:
        print(f"   {k}: {a} -> {b}")
    for label, small in (("0-100 elements (instruction-count ratios)", True), ("10k/1M (wall-time ratios)", False)):
        pairs = [(k, o2[k][0], n2[k][0]) for k in common if (k[3] <= 100) == small
                 and o2[k][0] is not None and n2[k][0] is not None]
        if not pairs:
            continue
        diffs = [b - a for _, a, b in pairs]
        rr = [b / a for _, a, b in pairs if a > 0 and b > 0]
        print(f"-- {label}: {len(pairs)} cells")
        print(f"   new-old: identical (2 dp) {sum(abs(d) < 0.005 for d in diffs)}, |d|<=0.01 {sum(abs(d) <= 0.0101 for d in diffs)}, "
              f"|d|<=0.02 {sum(abs(d) <= 0.0201 for d in diffs)}, |d|>0.05 {sum(abs(d) > 0.0501 for d in diffs)}")
        print(f"   new/old: geomean {geomean(rr):.4f}, median {statistics.median(rr):.4f}, "
              f"p05 {sorted(rr)[len(rr) // 20]:.3f}, p95 {sorted(rr)[len(rr) * 19 // 20]:.3f}")
        crossed_up = [(k, a, b) for k, a, b in pairs if a <= 1.05 < b]
        crossed_dn = [(k, a, b) for k, a, b in pairs if b <= 1.05 < a]
        print(f"   crossed the 1.05 bar: up {len(crossed_up)}, down {len(crossed_dn)}")
        for k, a, b in sorted(crossed_up + crossed_dn, key=lambda x: -abs(x[2] - x[1]))[:40]:
            kk = "K" if kernel_path(k[0], k[1]) else " "
            print(f"     {kk} {a:.2f} -> {b:.2f}  {k[0]}/{k[1]}/{k[2]}/{SIZE_LABEL.get(k[3], k[3])}")
        kern = [(k, a, b) for k, a, b in pairs if kernel_path(k[0], k[1])]
        if kern:
            print(f"   kernel-path cells: {len(kern)}, geomean old {geomean([a for _, a, _ in kern]):.4f}, "
                  f"new {geomean([b for _, _, b in kern]):.4f}")
        big = sorted(pairs, key=lambda x: -abs(math.log(x[2] / x[1])) if x[1] > 0 and x[2] > 0 else 0)[:args.top]
        print(f"   largest relative changes (top {args.top}):")
        for k, a, b in big:
            kk = "K" if kernel_path(k[0], k[1]) else " "
            print(f"     {kk} {a:.2f} -> {b:.2f} ({b / a:.3f}x)  {k[0]}/{k[1]}/{k[2]}/{SIZE_LABEL.get(k[3], k[3])}")


def cmd_pyperformance(args: argparse.Namespace) -> None:
    import pyperf

    p = {b.get_name(): b for b in pyperf.BenchmarkSuite.load(args.plain).get_benchmarks()}
    e = {b.get_name(): b for b in pyperf.BenchmarkSuite.load(args.enabled).get_benchmarks()}
    common = sorted(set(p) & set(e))
    print(f"plain {len(p)} results, enabled {len(e)} results, common {len(common)}")
    for n in sorted(set(p) - set(e)):
        print(f"   only plain: {n}")
    for n in sorted(set(e) - set(p)):
        print(f"   only enabled: {n}")
    ratios = []
    print("| benchmark | plain mean | plain std | enabled mean | enabled std | ratio (enabled/plain) |")
    print("|---|---:|---:|---:|---:|---:|")
    for n in common:
        bp, be = p[n], e[n]
        r = be.mean() / bp.mean()
        ratios.append(r)
        print(f"| {n} | {fmt_t(bp.mean())} | {fmt_t(bp.stdev()) if bp.get_nvalue() > 1 else '-'} | "
              f"{fmt_t(be.mean())} | {fmt_t(be.stdev()) if be.get_nvalue() > 1 else '-'} | {r:.3f} |")
    print(f"geometric mean of mean ratios: {geomean(ratios):.4f} over {len(ratios)}")
    print(f"median ratio {statistics.median(ratios):.4f}; > 1.05: {sum(r > 1.05 for r in ratios)}; "
          f"< 0.95: {sum(r < 0.95 for r in ratios)}")
    total = sum(p[n].mean() for n in common)
    print(f"sum of plain per-benchmark mean times (one iteration each): {total:.3f} s")


def fmt_t(s: float) -> str:
    for unit, f in (("s", 1), ("ms", 1e3), ("us", 1e6), ("ns", 1e9)):
        if s * f >= 1:
            return f"{s * f:.3g} {unit}"
    return f"{s * 1e9:.3g} ns"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("scoreboard")
    s.add_argument("--impl", default="enabled")
    s.add_argument("--repo", default=os.getcwd())
    s.add_argument("files", nargs="+")
    m = sub.add_parser("matrix-compare")
    m.add_argument("old")
    m.add_argument("new")
    m.add_argument("--top", type=int, default=25)
    pp = sub.add_parser("pyperformance")
    pp.add_argument("plain")
    pp.add_argument("enabled")
    args = ap.parse_args()
    {"scoreboard": cmd_scoreboard, "matrix-compare": cmd_matrix_compare, "pyperformance": cmd_pyperformance}[args.cmd](args)


if __name__ == "__main__":
    main()
