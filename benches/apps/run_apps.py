"""Real applications with and without `rustique.enable()` (the Phase 2 headline numbers).

Each workload is a small, deterministic, pure-Python program built on a real library or
on typical container-heavy stdlib code. `--enable` calls `rustique.enable()` before
anything else is imported, exactly as a user would. Both variants run in fresh processes
under `pyperf command`, so import time and every builtin call are included.

Workloads
    graph   networkx: random graph, BFS shortest paths, components, MST, clustering,
            pure-Python PageRank
    lint    pyflakes over a directory of Python sources (default: this repository's
            `tests/` and `benches/`; pass --lint-path to use e.g. CPython's Lib/test)
    etl     csv parse -> group-by with dicts/sets -> sort -> aggregate -> json dump

Usage
    python benches/apps/run_apps.py run graph [--enable]        # one run (for profiling)
    python benches/apps/run_apps.py bench [--apps graph,lint,etl] [-p 5] -o apps.json
    python benches/apps/run_apps.py compare apps.json

Requires `networkx` and `pyflakes` in the running interpreter's environment.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def maybe_enable(enable: bool) -> None:
    if enable:
        import rustique

        rustique.enable()


# --------------------------------------------------------------------------------------
# Workloads
# --------------------------------------------------------------------------------------


def app_graph() -> object:
    import networkx as nx
    from networkx.algorithms.link_analysis.pagerank_alg import _pagerank_python

    g = nx.gnm_random_graph(6_000, 30_000, seed=42)
    for u, v in g.edges():
        g[u][v]["weight"] = (u * 31 + v * 17) % 97 + 1
    total = 0
    for source in range(0, 6_000, 600):
        total += sum(nx.single_source_shortest_path_length(g, source).values())
    components = sorted((len(c) for c in nx.connected_components(g)), reverse=True)
    mst_weight = sum(d["weight"] for _, _, d in nx.minimum_spanning_edges(g, algorithm="kruskal", data=True))
    clustering = nx.average_clustering(g)
    ranks = _pagerank_python(g, tol=1e-8)
    top = sorted(ranks, key=ranks.__getitem__, reverse=True)[:10]
    return total, components[:3], mst_weight, round(clustering, 6), top


def app_lint(paths: list[str]) -> object:
    from pyflakes import api, reporter

    class Count(reporter.Reporter):
        def __init__(self) -> None:
            self.n = 0

        def unexpectedError(self, filename: str, message: str) -> None:
            self.n += 1

        def syntaxError(self, *args: object) -> None:
            self.n += 1

        def flake(self, message: object) -> None:
            self.n += 1

    rep = Count()
    files = 0
    for path in paths:
        for root, dirs, names in os.walk(path):
            dirs.sort()
            for name in sorted(names):
                if name.endswith(".py"):
                    files += 1
                    api.checkPath(os.path.join(root, name), rep)
    return files, rep.n


def app_etl() -> object:
    import csv
    import io
    import json
    import random
    from collections import Counter, defaultdict

    rng = random.Random(7)
    cities = [f"city{i}" for i in range(200)]
    products = [f"sku{i:05d}" for i in range(5_000)]
    buf = io.StringIO()
    w = csv.writer(buf)
    w.writerow(["order", "city", "product", "qty", "price", "tags"])
    for order in range(150_000):
        tags = "|".join(rng.sample(["new", "promo", "gift", "bulk", "vip", "eu", "us"], rng.randrange(0, 4)))
        w.writerow([order, rng.choice(cities), rng.choice(products), rng.randrange(1, 9), round(rng.uniform(1, 500), 2), tags])
    rows = list(csv.DictReader(io.StringIO(buf.getvalue())))

    revenue: dict[str, float] = defaultdict(float)
    by_product: dict[str, list[int]] = defaultdict(list)
    tag_sets: dict[str, set[str]] = defaultdict(set)
    tag_counts: Counter[str] = Counter()
    for r in rows:
        qty, price = int(r["qty"]), float(r["price"])
        revenue[r["city"]] += qty * price
        by_product[r["product"]].append(qty)
        tags = [t for t in r["tags"].split("|") if t]
        tag_sets[r["city"]].update(tags)
        tag_counts.update(tags)
    top_cities = sorted(revenue.items(), key=lambda kv: kv[1], reverse=True)[:20]
    product_stats = {p: (len(q), sum(q), max(q), min(q)) for p, q in by_product.items()}
    best = sorted(product_stats, key=lambda p: (product_stats[p][1], p), reverse=True)[:50]
    common_tags = set.intersection(*tag_sets.values()) if tag_sets else set()
    report = {
        "top_cities": [[c, round(v, 2)] for c, v in top_cities],
        "best_products": [[p, *product_stats[p]] for p in best],
        "tags": tag_counts.most_common(),
        "common_tags": sorted(common_tags),
        "orders": len(rows),
    }
    text = json.dumps(report, sort_keys=True)
    return len(text), json.loads(text)["orders"]


APPS = {"graph": app_graph, "lint": None, "etl": app_etl}


def run_one(args: argparse.Namespace) -> None:
    maybe_enable(args.enable)
    if args.app == "lint":
        paths = args.lint_path or [os.path.join(REPO, "tests"), os.path.join(REPO, "benches")]
        result = app_lint(paths)
    else:
        result = APPS[args.app]()  # type: ignore[misc]
    if args.print:
        print(result)


# --------------------------------------------------------------------------------------
# Benchmark driver
# --------------------------------------------------------------------------------------


def bench(args: argparse.Namespace) -> None:
    out = args.output
    if os.path.exists(out):
        os.remove(out)
    for app in args.apps.split(","):
        for variant in ("plain", "enabled"):
            cmd = [sys.executable, "-m", "pyperf", "command", "-p", str(args.processes), "-n", "1", "-w", "1",
                   "--name", f"{app}/{variant}", "--append", out, "--quiet", "--",
                   sys.executable, os.path.abspath(__file__), "run", app]
            if variant == "enabled":
                cmd.append("--enable")
            for p in args.lint_path or []:
                cmd += ["--lint-path", p]
            print(f"{app}/{variant} ...", flush=True)
            subprocess.check_call(cmd)
    compare_file(out)


def compare_file(path: str) -> None:
    import pyperf

    suite = pyperf.BenchmarkSuite.load(path)
    by = {b.get_name(): b for b in suite.get_benchmarks()}
    apps = sorted({n.split("/")[0] for n in by})
    print("| app | plain | enabled | ratio |")
    print("|---|---:|---:|---:|")
    for app in apps:
        p, e = by.get(f"{app}/plain"), by.get(f"{app}/enabled")
        if p and e:
            print(f"| {app} | {p.mean():.3f} s ± {p.stdev():.3f} | {e.mean():.3f} s ± {e.stdev():.3f} | "
                  f"{e.mean() / p.mean():.3f} |")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("app", choices=sorted(APPS))
    r.add_argument("--enable", action="store_true")
    r.add_argument("--print", action="store_true")
    r.add_argument("--lint-path", action="append")
    b = sub.add_parser("bench")
    b.add_argument("--apps", default="graph,lint,etl")
    b.add_argument("-p", "--processes", type=int, default=5)
    b.add_argument("-o", "--output", default="apps.json")
    b.add_argument("--lint-path", action="append")
    c = sub.add_parser("compare")
    c.add_argument("file")
    args = p.parse_args()
    if args.cmd == "run":
        run_one(args)
    elif args.cmd == "bench":
        bench(args)
    else:
        compare_file(args.file)


if __name__ == "__main__":
    main()
