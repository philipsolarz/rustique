"""Instructions per call of the builtin functions, original vs ``rustique._builtins``.

Measures small calls where the replacement either takes a fast path or falls back to the
original C function, to bound the fallback overhead (target: <= 1.05x). Each case runs as a
statement in a fresh function (locals for the inputs, the builtin looked up as a global, so
the interpreter specialises the call as usual); the cost of a baseline statement that only
loads the same locals is subtracted. Counts come from callgrind client requests around the
measured loop (two loop counts, so fixed costs cancel).

    valgrind --tool=callgrind --instr-atstart=no --callgrind-out-file=/tmp/cg/cg.out \\
        python benches/builtins_overhead.py --cg-dir /tmp/cg

Uses the client-request extension that ``benches/scoreboard.py instr`` builds. Without
valgrind (``--time``) it reports wall-clock nanoseconds per call instead.
"""

from __future__ import annotations

import argparse
import gc
import glob
import json
import os
import sys
import time
from typing import Any, Callable

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import builtins  # noqa: E402

from rustique import _builtins  # noqa: E402

CASES: list[tuple[str, str, str]] = [
    # (name, statement, baseline statement loading the same locals)
    ("sum([1, 2, 3])", "sum(x3)", "x3"),
    ("sum(t3)", "sum(t3)", "t3"),
    ("sum(gen, start)", "sum((v for v in x3), 10)", "(v for v in x3); 10"),
    ("sum(x100 ints)", "sum(x100)", "x100"),
    ("sum(x100 floats)", "sum(f100)", "f100"),
    ("min(a, b)", "min(a, b)", "a; b"),
    ("max(a, b)", "max(a, b)", "a; b"),
    ("min(fa, fb)", "min(fa, fb)", "fa; fb"),
    ("min(x8)", "min(x8)", "x8"),
    ("max(x8, key=f)", "max(x8, key=f)", "x8; f"),
    ("max(x8, default=0)", "max(x8, default=0)", "x8"),
    ("min(s8 str)", "min(s8)", "s8"),
    ("min(o8 tuples)", "min(o8)", "o8"),
    ("any(genexpr)", "any(v for v in z8)", "(v for v in z8)"),
    ("any(z8)", "any(z8)", "z8"),
    ("all(())", "all(())", "()"),
    ("all(x8)", "all(x8)", "x8"),
    ("any(r8 range)", "any(r8)", "r8"),
    ("sorted(x8)", "sorted(x8)", "x8"),
    ("sorted(x8, reverse=True)", "sorted(x8, reverse=True)", "x8"),
    ("sum(set8)", "sum(set8)", "set8"),
    ("min(set8)", "min(set8)", "set8"),
    ("max(d8)", "max(d8)", "d8"),
]


def make_env() -> dict[str, Any]:
    return {
        "x3": [1, 2, 3],
        "t3": (1, 2, 3),
        "x100": list(range(-50, 50)),
        "f100": [v + 0.25 for v in range(100)],
        "a": 3,
        "b": 5,
        "fa": 1.5,
        "fb": 0.5,
        "x8": [5, 3, 8, 1, 9, 2, 7, 4],
        "s8": ["e", "c", "h", "a", "i", "b", "g", "d"],
        "o8": [(5, "a"), (3, "b"), (8, "c"), (1, "d"), (9, "e"), (2, "f"), (7, "g"), (4, "h")],
        "z8": [0] * 8,
        "r8": range(8),
        "f": lambda v: -v,
        "set8": {5, 3, 8, 1, 9, 2, 7, 4},
        "d8": dict.fromkeys([5, 3, 8, 1, 9, 2, 7, 4]),
    }


def build(stmt: str, env: dict[str, Any]) -> Callable[[int, Callable[[], int], Callable[[int], int]], int]:
    names = sorted(env)
    src = (
        f"def run(loops, start, stop):\n"
        + "".join(f"    {n} = env[{n!r}]\n" for n in names)
        + "    t0 = start()\n    for _ in range(loops):\n"
        + "".join(f"        {s.strip()}\n" for s in stmt.split(";"))
        + "    return stop(t0)\n"
    )
    ns: dict[str, Any] = {"env": env}
    exec(compile(src, f"<{stmt}>", "exec"), ns)
    return ns["run"]  # type: ignore[no-any-return]


class Counter:
    def __init__(self, cg_dir: str | None) -> None:
        self.cg_dir = cg_dir
        if cg_dir:
            cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-scoreboard")
            sys.path.insert(0, cache)
            import _icount_v2  # type: ignore[import-not-found]

            self.ic = _icount_v2

    def start(self) -> int:
        if self.cg_dir:
            self.ic.zero()
            return 0
        return time.perf_counter_ns()

    def stop(self, t0: int) -> int:
        if not self.cg_dir:
            return time.perf_counter_ns() - t0
        self.ic.dump()
        latest = max(glob.glob(os.path.join(self.cg_dir, "cg.out.*")), key=os.path.getmtime)
        with open(latest) as f:
            total = next(int(line.split()[1]) for line in f if line.startswith("totals:"))
        os.remove(latest)
        return total


def per_call(stmt: str, counter: Counter, loops: int) -> float:
    fn = build(stmt, make_env())
    gc.collect()
    gc.disable()
    try:
        fn(max(2, loops // 4), lambda: 0, lambda t: 0)  # warm up / specialise
        if counter.cg_dir:
            a = fn(loops, counter.start, counter.stop)
            b = fn(2 * loops, counter.start, counter.stop)
            return (b - a) / loops
        return min(fn(loops, counter.start, counter.stop) for _ in range(5)) / loops
    finally:
        gc.enable()


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cg-dir", help="callgrind output directory (run under valgrind)")
    p.add_argument("--loops", type=int, default=200)
    p.add_argument("-o", "--output")
    args = p.parse_args()
    counter = Counter(args.cg_dir)
    loops = args.loops if args.cg_dir else 20_000
    rows = []
    for name, stmt, base in CASES:
        baseline = per_call(base, counter, loops)
        orig = per_call(stmt, counter, loops) - baseline
        _builtins.install()
        try:
            repl = per_call(stmt, counter, loops) - baseline
        finally:
            _builtins.uninstall()
        rows.append({"case": name, "original": orig, "rustique": repl, "ratio": repl / orig if orig else None})
        print(f"{name:28s} {orig:10.1f} {repl:10.1f}  {repl / orig:6.3f}", file=sys.stderr, flush=True)
    meta = {"python": sys.version, "metric": "instr" if args.cg_dir else "ns", "gil": getattr(sys, "_is_gil_enabled", lambda: True)()}
    if args.output:
        with open(args.output, "w") as f:
            json.dump({"meta": meta, "rows": rows}, f, indent=1)
    assert all(getattr(builtins, n) is not None for n in _builtins.REPLACED)


if __name__ == "__main__":
    main()
