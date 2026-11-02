"""Re-measure scoreboard cells above the bar alone (Phase 3's isolation method, docs/atlas/phase3.md §7).

    python sb_isolate.py instr --results instr-enabled-3.15.1.json --cpus 0-7 --reps 3 -o isolated-instr-3.15.1.txt
    python sb_isolate.py time --results time-enabled-3.15.1.json.shard*.json --cpus 2,10,20,30 --reps 2 -o isolated-time-3.15.1.txt

(in the build's clone, with its venv active). Reads the cells whose enabled ÷ builtin ratio
is above `--bar` (default 1.05) from the result files (`benches/scoreboard.py load_results`,
enabled tier; `instr` = sizes 0–100, `time` = 10k/1M), then measures each one again:

- instr: one `scoreboard.py instr --shards 1` run per cell and repetition, i.e. its own
  callgrind worker process (no allocator state left by a previous cell), up to one job per
  CPU of `--cpus` at a time (callgrind jobs may share a domain);
- time: one `scoreboard.py time --shards 1 -p 10 -n 5` run per cell and repetition, alone on
  one CPU of `--cpus` (one worker per CPU, each CPU in its own L3 domain, nothing else timing).

Cells are selected with exact `--type/--storage/--size` and the longest comma-free piece of
the op name as `--op` (the filter is a substring match on comma-separated pieces); when that
filter selects more than the one cell, the line says how many cells shared the worker.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import importlib.util
import json
import os
import queue
import statistics
import subprocess
import sys
import tempfile
import threading

REPO = os.getcwd()
SB = os.path.join(REPO, "benches", "scoreboard.py")


def load_sb():
    spec = importlib.util.spec_from_file_location("scoreboard", SB)
    sb = importlib.util.module_from_spec(spec)
    sys.modules["scoreboard"] = sb
    spec.loader.exec_module(sb)
    return sb


def cpu_list(s: str) -> list[int]:
    out = []
    for part in s.split(","):
        if "-" in part:
            a, b = part.split("-")
            out += list(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def op_filter(op: str) -> str:
    return max((p.strip() for p in op.split(",")), key=len)


def above(sb, files: list[str], small: bool, bar: float) -> list[tuple]:
    res, _ = sb.load_results(files)
    by_key = {c.key: c for c in sb.all_cells()}
    out = []
    for (build, key), e in res.items():
        c = by_key.get(key)
        if c is None or (c.n in sb.SMALL_SIZES) != small:
            continue
        r, st, _m = sb.cell_ratio(e, "enabled", c.n)
        if r and r > bar and st not in ("error", "mismatch"):
            out.append((build, key, c, r))
    return sorted(out, key=lambda x: -x[3])


def selected(c, flt: str) -> int:
    r = subprocess.run([sys.executable, SB, "list", "--type", c.type, "--storage", c.storage, "--size", str(c.n),
                        "--op", flt], capture_output=True, text=True)
    return len(r.stdout.split("\n")) - 1 if r.returncode == 0 else -1


def measure_instr(c, flt: str, cpu: int) -> tuple[float, float]:
    with tempfile.TemporaryDirectory() as d:
        j = os.path.join(d, "i.json")
        subprocess.run(["taskset", "-c", str(cpu), sys.executable, SB, "instr", "--impls", "enabled", "--type", c.type,
                        "--op", flt, "--storage", c.storage, "--size", str(c.n), "--shards", "1", "--quiet", "-o", j],
                       capture_output=True, text=True)
        for x in json.load(open(j))["cells"]:
            if x["key"] == c.key:
                return x.get("builtin", 0.0), x.get("enabled", 0.0)
    raise RuntimeError(f"{c.key}: not in the result")


def measure_time(c, flt: str, cpu: int) -> tuple[float, float, float, float, int]:
    import pyperf

    with tempfile.TemporaryDirectory() as d:
        j = os.path.join(d, "t.json")
        subprocess.run([sys.executable, SB, "time", "--type", c.type, "--op", flt, "--storage", c.storage,
                        "--size", str(c.n), "--impls", "enabled", "--shards", "1", "--affinity", str(cpu),
                        "-p", "10", "-n", "5", "-o", j], capture_output=True, text=True)
        s = pyperf.BenchmarkSuite.load(j + ".shard0.json")
        m = {b.get_name(): b for b in s.get_benchmarks()}
        bi, en = m[c.key + "/builtin"], m[c.key + "/enabled"]
        return bi.mean(), en.mean(), bi.stdev() / bi.mean(), en.stdev() / en.mean(), bi.get_nrun()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=("instr", "time"))
    ap.add_argument("--results", nargs="+", required=True)
    ap.add_argument("--cpus", required=True)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--bar", type=float, default=1.05)
    ap.add_argument("-o", required=True)
    a = ap.parse_args()
    sb = load_sb()
    cells = above(sb, a.results, a.mode == "instr", a.bar)
    cpus = cpu_list(a.cpus)
    lock = threading.Lock()
    out = open(a.o, "a")

    def emit(line: str) -> None:
        with lock:
            print(line, flush=True)
            out.write(line + "\n")
            out.flush()

    emit(f"# {a.mode}: {len(cells)} cells > {a.bar} in {', '.join(os.path.basename(f) for f in a.results)}; "
         f"{a.reps} runs each, cpus {a.cpus}")
    free: queue.Queue[int] = queue.Queue()
    for cpu in cpus:
        free.put(cpu)
    results: dict[str, list[float]] = {}

    def job(build, key, c, r0, rep):
        cpu = free.get()
        try:
            flt = op_filter(c.op.name)
            k = selected(c, flt)
            share = "" if k == 1 else f"  [filter --op {flt!r} selects {k} cells]"
            if a.mode == "instr":
                b, e = measure_instr(c, flt, cpu)
                ratio = e / b
                emit(f"{build} {key}: builtin {b:.1f} enabled {e:.1f} ratio {ratio:.3f} (as run {r0:.3f}; "
                     f"isolated, run {rep}){share}")
            else:
                b, e, rb, re_, nrun = measure_time(c, flt, cpu)
                ratio = e / b
                emit(f"{build} {key}: builtin {b * 1e6:.1f} us (rsd {rb:.3f}) enabled {e * 1e6:.1f} us (rsd {re_:.3f}) "
                     f"ratio {ratio:.3f} (as run {r0:.3f}; isolated, run {rep}, {nrun} runs, cpu {cpu}){share}")
            with lock:
                results.setdefault(f"{build} {key}", []).append(ratio)
        except Exception as ex:  # noqa: BLE001 - recorded, the other cells go on
            emit(f"{build} {key}: FAILED run {rep}: {ex!r}")
        finally:
            free.put(cpu)

    with cf.ThreadPoolExecutor(max_workers=len(cpus)) as ex:
        futs = [ex.submit(job, *cell, rep) for rep in range(1, a.reps + 1) for cell in cells]
        for f in futs:
            f.result()
    emit(f"# summary ({a.mode}): cell, as run, isolated runs, mean")
    still = 0
    for build, key, _c, r0 in cells:
        rs = results.get(f"{build} {key}", [])
        if rs:
            m = statistics.mean(rs)
            still += m > a.bar
            emit(f"#   {build} {key}: {r0:.3f} -> {' '.join(f'{x:.3f}' for x in rs)}  mean {m:.3f}"
                 f"{'  STILL > ' + str(a.bar) if m > a.bar else ''}")
    emit(f"# {len(cells)} cells re-measured; {still} still > {a.bar} (mean of the isolated runs)")
    out.write("ISO-DONE\n")
    out.close()


if __name__ == "__main__":
    main()
