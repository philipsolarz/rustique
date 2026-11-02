"""Validate ``rustique._estimate`` (``python -m rustique estimate``) on the workload set: predict
each workload's enabled/plain ratio from a plain profiled run (+ replay) and an enabled run
with counters, and compare with the measured ratios (docs/atlas/w2-dx.md §5).

    python benches/dx/estimate_workloads.py all [workload|family ...] [-o out.json]
    python benches/dx/estimate_workloads.py plain data/csv_etl       # one side, JSON on stdout
    python benches/dx/estimate_workloads.py counters data/csv_etl

``plain``: setup(), warm-up run(), then 3 timed run()s (median: the total time), then one
run() under the profiler, then the replay of the sampled calls. ``counters``: enable() before
the workload is imported, setup(), warm-up, counters reset, one run(), stats(). Measured
ratios: Phase 3 (docs/atlas/phase3.md §4, first replicate, run.py compare -n 5 -p 5, 3.15.1
and 3.15.1t), or ``--measured file.json`` from ``run.py compare -o``.
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "workloads"))

PHASE3 = {  # workload: (3.15.1 ratio, 3.15.1t ratio)
    "data/backup_archive": (0.996, 0.999), "data/billing_engine": (0.970, 0.975), "data/config_merge": (1.013, 1.001),
    "data/csv_etl": (0.625, 0.640), "data/fs_inventory": (0.743, 0.735), "data/log_analytics": (0.594, 0.609),
    "data/pandas_etl": (1.000, 1.000), "data/report_render": (0.436, 0.423), "data/state_snapshot": (0.651, 0.476),
    "data/text_corpus": (1.000, 1.001), "data/tz_events": (0.591, 0.615), "data/xml_feeds": (1.000, 1.005),
    "devtools/black_format": (0.990, 0.999), "devtools/coverage_pyflakes_tests": (0.995, 0.999),
    "devtools/import_startup": (1.000, 1.001), "devtools/isort_stdlib": (0.771, 0.196),
    "devtools/pip_resolve": (0.949, 0.947), "devtools/pycodestyle_stdlib": (0.985, 0.999),
    "devtools/pyflakes_stdlib": (0.997, 1.005), "devtools/pylint_stdlib": (1.002, 0.999),
    "devtools/pytest_networkx": (0.948, 0.954), "devtools/rich_render": (0.998, 1.000),
    "devtools/sphinx_autodoc": (0.977, 0.988), "devtools/sqlglot_transpile": (0.968, 0.974),
    "web/aiohttp_app": (0.910, 0.925), "web/asyncio_http": (0.791, 0.815), "web/django_blog": (0.997, 0.995),
    "web/email_mime": (0.995, 1.001), "web/fastapi_pydantic": (0.983, 0.986), "web/flask_jinja": (0.990, 0.986),
    "web/json_api": (0.835, 0.835), "web/logging_service": (0.817, 0.864), "web/requests_local": (0.941, 0.944),
    "web/sqlalchemy_orm": (0.997, 1.005),
}


def side_plain(name: str) -> dict:
    import run as runner
    from rustique import _estimate

    mod = runner.load(name)
    state = mod.setup()
    mod.run(state)
    times = []
    for _ in range(3):
        t0 = time.perf_counter_ns()
        mod.run(state)
        times.append(time.perf_counter_ns() - t0)
    sampler = _estimate.Sampler().start()
    for _ in range(3):
        mod.run(state)
    sampler.stop()
    prof = _estimate.Profiler().start()
    mod.run(state)
    prof.stop()
    res = prof.result(plain_ns=statistics.median(times))
    res["unscaled"] = prof.result()
    t0 = time.perf_counter()
    res["replay"] = _estimate.replay(prof)
    res["replay_s"] = time.perf_counter() - t0
    res["plain_ns"] = statistics.median(times)
    res["sampled"] = sampler.shares()
    return res


def side_counters(name: str) -> dict:
    import rustique

    rustique.enable()
    import run as runner

    mod = runner.load(name)
    state = mod.setup()
    mod.run(state)
    rustique.stats(True)
    rustique.stats(reset=True)
    t0 = time.perf_counter_ns()
    mod.run(state)
    t = time.perf_counter_ns() - t0
    return {"stats": rustique.stats(), "enabled_ns_one_run": t}


def one(side: str, name: str, py: str) -> dict:
    r = subprocess.run([py, __file__, side, name], capture_output=True, text=True, timeout=1800)
    if r.returncode:
        raise RuntimeError(f"{side} {name}: {r.stderr[-1500:]}")
    return json.loads(r.stdout.strip().splitlines()[-1])


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("cmd", choices=("all", "plain", "counters", "repredict"))
    p.add_argument("workloads", nargs="*")
    p.add_argument("-o", "--output")
    p.add_argument("--measured", help="run.py compare -o JSON to compare against (default: Phase 3)")
    p.add_argument("--build", default="3.15.1", choices=("3.15.1", "3.15.1t"))
    p.add_argument("--mode", default="samples", choices=("samples", "hybrid", "profile"),
                   help="repredict: where Python-level targets' time comes from (see _estimate.predict)")
    a = p.parse_args()
    if a.cmd == "plain":
        print(json.dumps(side_plain(a.workloads[0]), default=str))
        return
    if a.cmd == "counters":
        print(json.dumps(side_counters(a.workloads[0]), default=str))
        return
    import run as runner
    from rustique import _estimate

    if a.cmd == "repredict":  # recompute predictions from a saved -o file (new constants)
        rows = json.load(open(a.workloads[0]))
        errs = []
        for r in rows:
            pl, co = r["raw"]["plain"], r["raw"]["counters"]
            pred = _estimate.predict(pl, pl["replay"], co["stats"], pl["plain_ns"],
                                     None if a.mode == "profile" else pl.get("sampled"), mode=a.mode)
            r["predicted"], r["rows"] = pred["ratio"], pred["rows"][:8]
            if r["measured"] is not None:
                errs.append(pred["ratio"] - r["measured"])
            print(f"{r['workload']:36s} predicted {pred['ratio']:.3f} measured {r['measured']} "
                  f"error {pred['ratio'] - r['measured']:+.3f} (under targets {pred['under_targets'] * 100:.1f}%)")
        print(f"\n{len(errs)} workloads: mean abs error {statistics.mean(abs(e) for e in errs):.3f}, "
              f"max {max(errs, key=abs):+.3f}, bias {statistics.mean(errs):+.3f}; "
              f"within 0.02: {sum(abs(e) <= 0.02 for e in errs)}, within 0.05: {sum(abs(e) <= 0.05 for e in errs)}")
        if a.output:
            json.dump(rows, open(a.output, "w"), indent=1, default=str)
        return
    names = runner.select(a.workloads or ["web", "data", "devtools"])
    measured = {w: v[0 if a.build == "3.15.1" else 1] for w, v in PHASE3.items()}
    if a.measured:
        measured = {r["workload"]: r["ratio"] for r in json.load(open(a.measured))}
    rows = []
    for w in names:
        try:
            pl = one("plain", w, sys.executable)
            co = one("counters", w, sys.executable)
        except Exception as exc:  # noqa: BLE001
            print(f"{w}: FAILED {exc}", flush=True)
            continue
        pred = _estimate.predict(pl, pl["replay"], co["stats"], pl["plain_ns"], pl.get("sampled"))
        m = measured.get(w)
        rows.append({"workload": w, "predicted": pred["ratio"], "measured": m, "under": pred["under_targets"],
                     "plain_ms": pl["plain_ns"] / 1e6, "instrumented_ms": pl["wall_ns_instrumented"] / 1e6,
                     "replay_s": pl["replay_s"], "kappa": pl["kappa"], "rows": pred["rows"][:8],
                     "raw": {"plain": pl, "counters": co}})
        err = f"{pred['ratio'] - m:+.3f}" if m is not None else "-"
        print(f"{w:36s} predicted {pred['ratio']:.3f} measured {m if m is not None else '-'} error {err} "
              f"(under targets {pred['under_targets'] * 100:.1f}%, profiled run {pl['wall_ns_instrumented'] / 1e6:.0f} ms "
              f"vs {pl['plain_ns'] / 1e6:.0f} ms, replay {pl['replay_s']:.1f} s)", flush=True)
    errs = [r["predicted"] - r["measured"] for r in rows if r["measured"] is not None]
    if errs:
        print(f"\n{len(errs)} workloads: mean abs error {statistics.mean(abs(e) for e in errs):.3f}, "
              f"max {max(errs, key=abs):+.3f}, bias {statistics.mean(errs):+.3f}; "
              f"within 0.02: {sum(abs(e) <= 0.02 for e in errs)}, within 0.05: {sum(abs(e) <= 0.05 for e in errs)}")
    if a.output:
        json.dump(rows, open(a.output, "w"), indent=1, default=str)


if __name__ == "__main__":
    main()
