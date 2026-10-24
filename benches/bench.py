"""Honest benchmark harness for Rustique (ARCHITECTURE §1.8).

Rules this harness enforces (the historical benchmarks broke all of them):

* Fresh inputs for every repetition: data and containers are rebuilt (untimed) before
  each timed call, so no case ever times an already-consumed iterator or an
  already-sorted list.
* Every result is verified against the builtin implementation's result; a mismatch or
  an exception is reported as a failure (never swallowed) and makes the run exit
  non-zero.
* Best-of-N and median are both reported; GC is collected before and disabled during
  each timed call; the order of implementations rotates between repetitions.
* Ratios are relative to the builtin (`list`/`dict`/`set`) time: < 1.0 means faster.

Usage:
    python benches/bench.py                    # sizes 1e3, 1e5, 1e6; 7 repetitions
    python benches/bench.py --quick            # sizes 1e3, 1e5; 3 repetitions
    python benches/bench.py --filter map --json results.json
Build with optimisations first: `maturin develop --profile dev-release` (or --release).
"""

from __future__ import annotations

import argparse
import array
import gc
import json
import os
import platform
import random
import statistics
import subprocess
import sys
import time
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from typing import Any

import rustique as rs
from rustique import Dict, List, Set, X

try:
    import numpy as np
except ImportError:  # NumPy is optional: its columns are skipped, never faked.
    np = None  # type: ignore[assignment]


# --------------------------------------------------------------------------------------
# Result normalisation and verification
# --------------------------------------------------------------------------------------


def to_builtin(x: Any) -> Any:
    """Convert a result to builtin types for comparison with the reference result."""
    if isinstance(x, List):
        return x.to_list()
    if isinstance(x, Dict):
        return {k: to_builtin(v) for k, v in x.to_dict().items()}
    if isinstance(x, Set):
        return x.to_set()
    if isinstance(x, array.array):
        return x.tolist()
    if np is not None and isinstance(x, (np.ndarray, np.generic)):
        return x.tolist()
    if isinstance(x, (list, tuple)):
        return [to_builtin(v) for v in x]
    if isinstance(x, dict):
        return {k: to_builtin(v) for k, v in x.items()}
    if isinstance(x, (set, frozenset)):
        return set(x)
    return x


def same(a: Any, b: Any) -> bool:
    """Exact equality of normalised results: same types, same values, NaN equal to NaN,
    dicts compared in insertion order."""
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        return list(a) == list(b) and all(same(a[k], b[k]) for k in a)
    if isinstance(a, float) and isinstance(b, float):
        return a == b or (a != a and b != b)
    return type(a) is type(b) and a == b


# --------------------------------------------------------------------------------------
# Cases
# --------------------------------------------------------------------------------------


@dataclass
class Impl:
    name: str
    run: Callable[[Any], Any]
    prepare: Callable[[Any], Any] = lambda data: data  # untimed, fresh per repetition
    normalize: Callable[[Any], Any] = to_builtin


@dataclass
class Case:
    name: str
    make_data: Callable[[int, random.Random], Any]  # untimed, fresh per repetition
    impls: list[Impl]
    max_n: int = 10**9
    note: str = ""


def ints(n: int, rng: random.Random) -> list[int]:
    return [rng.randrange(-(10**9), 10**9) for _ in range(n)]


def floats(n: int, rng: random.Random) -> list[float]:
    return [rng.uniform(-1e6, 1e6) for _ in range(n)]


def strs(n: int, rng: random.Random) -> list[str]:
    return [f"k{rng.randrange(10**12):012d}" for _ in range(n)]


def np_impl(name: str, run: Callable[[Any], Any], prepare: Callable[[Any], Any]) -> list[Impl]:
    return [Impl(name, run, prepare)] if np is not None else []


def arr_q(data: Iterable[int]) -> array.array[int]:
    return array.array("q", data)


def iterate_sum(c: Iterable[Any]) -> Any:
    s = 0
    for x in c:
        s += x
    return s


def index_sum(prep: tuple[Any, list[int]]) -> int:
    c, idx = prep
    s = 0
    for i in idx:
        s += c[i]
    return int(s)


def contains_count(prep: tuple[Any, list[Any]]) -> int:
    c, probes = prep
    return sum(1 for p in probes if p in c)


def append_all(prep: tuple[Any, list[Any]]) -> Any:
    c, data = prep
    for x in data:
        c.append(x)
    return c


def sort_inplace(c: Any) -> Any:
    c.sort()
    return c


def lookup_sum(prep: tuple[Any, list[Any]]) -> int:
    d, keys = prep
    s = 0
    for k in keys:
        s += d[k]
    return s


def with_indices(data: list[Any]) -> tuple[list[Any], list[int]]:
    rng = random.Random(len(data))
    return data, [rng.randrange(len(data)) for _ in range(min(len(data), 100_000))]


def with_probes(data: list[Any]) -> tuple[list[Any], list[Any]]:
    rng = random.Random(len(data))
    k = max(5, min(200, 10**7 // max(1, len(data))))
    hits = [data[rng.randrange(len(data))] for _ in range(k // 2)]
    misses = [10**12 + i for i in range(k - k // 2)]
    return data, hits + misses


def build_cases() -> list[Case]:
    cases: list[Case] = []

    # ---- construction ------------------------------------------------------------------
    cases.append(
        Case(
            "construct: from list[int]",
            ints,
            [
                Impl("list", list),
                Impl("List", List),
                Impl("List[int]", List[int]),
                Impl("array('q')", arr_q),
                *np_impl("numpy", lambda d: np.array(d, dtype=np.int64), lambda d: d),
            ],
        )
    )
    cases.append(
        Case(
            "construct: from range",
            lambda n, rng: n,
            [
                Impl("list", lambda n: list(range(n))),
                Impl("List", lambda n: List(range(n))),
                Impl("arange", lambda n: rs.arange(n)),
                Impl("array('q')", lambda n: arr_q(range(n))),
                *np_impl("numpy", lambda n: np.arange(n, dtype=np.int64), lambda n: n),
            ],
        )
    )
    cases.append(
        Case(
            "construct: from generator",
            ints,
            [
                Impl("list", list, prepare=lambda d: (x for x in d)),
                Impl("List", List, prepare=lambda d: (x for x in d)),
                Impl("List[int]", List[int], prepare=lambda d: (x for x in d)),
                Impl("array('q')", arr_q, prepare=lambda d: (x for x in d)),
            ],
        )
    )
    cases.append(
        Case(
            "construct: from list[float]",
            floats,
            [
                Impl("list", list),
                Impl("List", List),
                Impl("List[float]", List[float]),
                *np_impl("numpy", lambda d: np.array(d, dtype=np.float64), lambda d: d),
            ],
        )
    )
    cases.append(
        Case(
            "construct: from list[str]",
            strs,
            [Impl("list", list), Impl("List", List), Impl("List[str]", List[str])],
        )
    )

    # ---- element access ----------------------------------------------------------------
    cases.append(
        Case(
            "index: x[i] (100k random)",
            ints,
            [
                Impl("list", index_sum, lambda d: (list(d), with_indices(d)[1])),
                Impl("List", index_sum, lambda d: (List(d), with_indices(d)[1])),
                Impl("array('q')", index_sum, lambda d: (arr_q(d), with_indices(d)[1])),
                *np_impl("numpy", index_sum, lambda d: (np.array(d), with_indices(d)[1])),
            ],
        )
    )
    cases.append(
        Case(
            "iterate: for x in c",
            ints,
            [
                Impl("list", iterate_sum, list),
                Impl("List", iterate_sum, List),
                Impl("array('q')", iterate_sum, arr_q),
                *np_impl("numpy", lambda a: int(iterate_sum(a)), lambda d: np.array(d)),
            ],
        )
    )
    cases.append(
        Case(
            "append: n x c.append(v)",
            ints,
            [
                Impl("list", append_all, lambda d: ([], d)),
                Impl("List", append_all, lambda d: (List(), d)),
                Impl("List[int]", append_all, lambda d: (List[int](), d)),
                Impl("array('q')", append_all, lambda d: (array.array("q"), d)),
            ],
        )
    )
    cases.append(
        Case(
            "contains: v in c",
            ints,
            [
                Impl("list", contains_count, lambda d: with_probes(list(d))),
                Impl("List", contains_count, lambda d: (List(d), with_probes(d)[1])),
                Impl("array('q')", contains_count, lambda d: (arr_q(d), with_probes(d)[1])),
            ],
        )
    )

    # ---- reductions ----------------------------------------------------------------------
    for label, fn, meth in (("sum", sum, "sum"), ("min", min, "min"), ("max", max, "max")):
        cases.append(
            Case(
                f"{label}: int",
                ints,
                [
                    Impl("list", fn, list),
                    Impl(f"List.{meth}()", lambda c, m=meth: getattr(c, m)(), List),
                    Impl(f"{label}(List)", fn, List),
                    Impl("array('q')", fn, arr_q),
                    *np_impl(f"numpy.{meth}", lambda a, m=meth: getattr(a, m)().item(), lambda d: np.array(d)),
                ],
            )
        )
    cases.append(
        Case(
            "sum: float (compensated)",
            floats,
            [
                Impl("list", sum, list),
                Impl("List.sum()", lambda c: c.sum(), List),
            ],
            note="Python's float sum is compensated (Neumaier); List.sum() must return the identical "
            "value. numpy.sum (pairwise) and math.fsum (exact) differ in the last bits, so they are "
            "not listed.",
        )
    )

    # ---- sorting and slicing -------------------------------------------------------------
    cases.append(
        Case(
            "sort: int (in place)",
            ints,
            [
                Impl("list", sort_inplace, list),
                Impl("List", sort_inplace, List),
                Impl("List[int]", sort_inplace, List[int]),
                *np_impl("numpy", sort_inplace, lambda d: np.array(d)),
            ],
        )
    )
    cases.append(
        Case(
            "sort: float (in place)",
            floats,
            [
                Impl("list", sort_inplace, list),
                Impl("List", sort_inplace, List),
                *np_impl("numpy", sort_inplace, lambda d: np.array(d)),
            ],
        )
    )
    cases.append(
        Case(
            "sort: str (in place)",
            strs,
            [Impl("list", sort_inplace, list), Impl("List", sort_inplace, List)],
        )
    )
    cases.append(
        Case(
            "slice: c[n//4 : 3n//4]",
            ints,
            [
                Impl("list", lambda c: c[len(c) // 4 : 3 * len(c) // 4], list),
                Impl("List", lambda c: c[len(c) // 4 : 3 * len(c) // 4], List),
                Impl("array('q')", lambda c: c[len(c) // 4 : 3 * len(c) // 4], arr_q),
                *np_impl("numpy (copy)", lambda c: c[len(c) // 4 : 3 * len(c) // 4].copy(), lambda d: np.array(d)),
            ],
        )
    )
    cases.append(
        Case(
            "slice: c[::3]",
            ints,
            [
                Impl("list", lambda c: c[::3], list),
                Impl("List", lambda c: c[::3], List),
                *np_impl("numpy (copy)", lambda c: c[::3].copy(), lambda d: np.array(d)),
            ],
        )
    )

    # ---- vectorised expressions ----------------------------------------------------------
    # Expressions are built in the (untimed) prepare step, once per repetition.
    cases.append(
        Case(
            "map: x*2+1",
            ints,
            [
                Impl("list comp", lambda c: [x * 2 + 1 for x in c], list),
                Impl("List.map(X*2+1)", lambda p: p[0].map(p[1]), lambda d: (List(d), X * 2 + 1)),
                Impl("List.map(lambda)", lambda c: c.map(lambda x: x * 2 + 1), List),
                *np_impl("numpy", lambda a: a * 2 + 1, lambda d: np.array(d)),
            ],
        )
    )
    cases.append(
        Case(
            "filter: x%3==0",
            ints,
            [
                Impl("list comp", lambda c: [x for x in c if x % 3 == 0], list),
                Impl("List.filter(X%3==0)", lambda p: p[0].filter(p[1]), lambda d: (List(d), X % 3 == 0)),
                Impl("List.filter(lambda)", lambda c: c.filter(lambda x: x % 3 == 0), List),
                *np_impl("numpy", lambda a: a[a % 3 == 0], lambda d: np.array(d)),
            ],
        )
    )
    cases.append(
        Case(
            "map: float x*0.5-1",
            floats,
            [
                Impl("list comp", lambda c: [x * 0.5 - 1 for x in c], list),
                Impl("List.map(X*0.5-1)", lambda p: p[0].map(p[1]), lambda d: (List(d), X * 0.5 - 1)),
                *np_impl("numpy", lambda a: a * 0.5 - 1, lambda d: np.array(d)),
            ],
        )
    )

    # ---- dict ------------------------------------------------------------------------------
    def kv_int(n: int, rng: random.Random) -> list[tuple[int, int]]:
        keys = rng.sample(range(10 * n), n)
        return [(k, k * 3) for k in keys]

    def kv_str(n: int, rng: random.Random) -> list[tuple[str, int]]:
        return [(f"k{i:09d}{rng.randrange(1000)}", i) for i in range(n)]

    for label, gen in (("int keys", kv_int), ("str keys", kv_str)):
        cases.append(
            Case(
                f"dict build: {label}",
                gen,
                [Impl("dict", dict), Impl("Dict", Dict)],
            )
        )
        cases.append(
            Case(
                f"dict lookup: {label} (100k)",
                gen,
                [
                    Impl("dict", lookup_sum, lambda kv: (dict(kv), [k for k, _ in kv[: 100_000]])),
                    Impl("Dict", lookup_sum, lambda kv: (Dict(kv), [k for k, _ in kv[: 100_000]])),
                ],
            )
        )

    # ---- set ---------------------------------------------------------------------------------
    def two_sets(n: int, rng: random.Random) -> tuple[list[int], list[int]]:
        return [rng.randrange(2 * n) for _ in range(n)], [rng.randrange(2 * n) for _ in range(n)]

    for op_name, op in (("|", lambda a, b: a | b), ("&", lambda a, b: a & b), ("-", lambda a, b: a - b), ("^", lambda a, b: a ^ b)):
        cases.append(
            Case(
                f"set {op_name}",
                two_sets,
                [
                    Impl("set", lambda p, op=op: op(*p), lambda ab: (set(ab[0]), set(ab[1]))),
                    Impl("Set", lambda p, op=op: op(*p), lambda ab: (Set(ab[0]), Set(ab[1]))),
                ],
            )
        )
    cases.append(
        Case(
            "set build: int",
            ints,
            [Impl("set", set), Impl("Set", Set)],
        )
    )
    return cases


# --------------------------------------------------------------------------------------
# Runner
# --------------------------------------------------------------------------------------


@dataclass
class Measurement:
    case: str
    n: int
    impl: str
    times_ns: list[int] = field(default_factory=list)
    error: str | None = None

    @property
    def best(self) -> float:
        return min(self.times_ns) / 1e9 if self.times_ns else float("nan")

    @property
    def median(self) -> float:
        return statistics.median(self.times_ns) / 1e9 if self.times_ns else float("nan")


def time_one(impl: Impl, data: Any) -> tuple[int, Any]:
    inp = impl.prepare(data)
    gc.collect()
    gc.disable()
    try:
        t0 = time.perf_counter_ns()
        result = impl.run(inp)
        t1 = time.perf_counter_ns()
    finally:
        gc.enable()
    return t1 - t0, result


def run_case(case: Case, n: int, repeat: int, seed: int) -> list[Measurement]:
    ms = {impl.name: Measurement(case.name, n, impl.name) for impl in case.impls}
    reference = case.impls[0]
    for rep in range(repeat):
        rng = random.Random(seed * 1_000_003 + rep)
        data = case.make_data(n, rng)
        # Reference result for this repetition's data (computed on its own fresh input).
        _, ref_result = time_one(reference, data)
        expected = reference.normalize(ref_result)
        order = case.impls[rep % len(case.impls) :] + case.impls[: rep % len(case.impls)]
        for impl in order:
            m = ms[impl.name]
            if m.error:
                continue
            try:
                dt, result = time_one(impl, data)
                got = impl.normalize(result)
            except Exception as e:  # recorded and reported, never swallowed
                m.error = f"{type(e).__name__}: {e}"
                continue
            if not same(got, expected):
                m.error = f"WRONG RESULT: {summarize(got)} != {summarize(expected)}"
                continue
            m.times_ns.append(dt)
    return list(ms.values())


def summarize(x: Any) -> str:
    r = repr(x)
    return r if len(r) < 80 else r[:77] + "..."


def fmt_time(s: float) -> str:
    if s != s:
        return "-"
    if s < 1e-3:
        return f"{s * 1e6:.1f} µs"
    if s < 1:
        return f"{s * 1e3:.2f} ms"
    return f"{s:.3f} s"


def markdown(results: list[Measurement], cases: list[Case]) -> str:
    lines = [
        "| case | n | implementation | best | median | ratio vs builtin |",
        "|---|---:|---|---:|---:|---:|",
    ]
    by_case: dict[tuple[str, int], list[Measurement]] = {}
    for m in results:
        by_case.setdefault((m.case, m.n), []).append(m)
    notes = {c.name: c.note for c in cases if c.note}
    for (case_name, n), ms in by_case.items():
        case = case_name.replace("|", "\\|")  # e.g. "set |" must not split the table cell
        base = ms[0].best
        for m in ms:
            if m.error:
                lines.append(f"| {case} | {n:,} | {m.impl} | **FAILED** | | {m.error} |")
                continue
            ratio = m.best / base if base and base == base else float("nan")
            ratio_s = "1.00 (ref)" if m is ms[0] else f"{ratio:.2f}"
            lines.append(f"| {case} | {n:,} | {m.impl} | {fmt_time(m.best)} | {fmt_time(m.median)} | {ratio_s} |")
    out = "\n".join(lines)
    if notes:
        out += "\n\nNotes:\n" + "\n".join(f"- {k}: {v}" for k, v in notes.items())
    return out


def environment() -> dict[str, Any]:
    try:
        rev = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, check=False).stdout.strip()
    except OSError:
        rev = ""
    return {
        "python": sys.version,
        "implementation": platform.python_implementation(),
        "gil_enabled": getattr(sys, "_is_gil_enabled", lambda: True)(),
        "platform": platform.platform(),
        "cpu_count": os.cpu_count(),
        "rustique": rs.__version__,
        "numpy": None if np is None else np.__version__,
        "git": rev,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--quick", action="store_true", help="sizes 1e3 and 1e5, 3 repetitions")
    ap.add_argument("--sizes", help="comma-separated sizes, e.g. 1e3,1e5,1e6")
    ap.add_argument("--repeat", type=int, help="repetitions per measurement (default 7, quick 3)")
    ap.add_argument("--filter", default="", help="only cases whose name contains this substring")
    ap.add_argument("--json", metavar="PATH", help="also write raw results as JSON")
    ap.add_argument("--seed", type=int, default=12345)
    args = ap.parse_args(argv)

    sizes = [int(float(s)) for s in args.sizes.split(",")] if args.sizes else ([10**3, 10**5] if args.quick else [10**3, 10**5, 10**6])
    repeat = args.repeat or (3 if args.quick else 7)
    cases = [c for c in build_cases() if args.filter.lower() in c.name.lower()]
    env = environment()
    print(f"# Rustique benchmarks\n\n`{env['python'].split()[0]}` GIL={'on' if env['gil_enabled'] else 'off'}, "
          f"rustique {env['rustique']}, numpy {env['numpy']}, {env['cpu_count']} CPUs, git {env['git']}; "
          f"best of {repeat}, fresh inputs per repetition, results verified.\n", flush=True)

    results: list[Measurement] = []
    for case in cases:
        for n in sizes:
            if n > case.max_n:
                continue
            ms = run_case(case, n, repeat, args.seed)
            results.extend(ms)
            print(f"  {case.name} n={n:,}: " + ", ".join(f"{m.impl}={'ERR' if m.error else fmt_time(m.best)}" for m in ms), file=sys.stderr, flush=True)
    print(markdown(results, cases))
    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(
                {
                    "environment": env,
                    "repeat": repeat,
                    "results": [
                        {"case": m.case, "n": m.n, "impl": m.impl, "times_ns": m.times_ns, "best_s": m.best, "median_s": m.median, "error": m.error}
                        for m in results
                    ],
                },
                f,
                indent=2,
            )
    failures = [m for m in results if m.error]
    if failures:
        print(f"\n{len(failures)} measurement(s) FAILED (see table).", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
