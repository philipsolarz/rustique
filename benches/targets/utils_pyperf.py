"""Wall-time cells (pyperf) for the utils groups' large inputs: each statement plain and
with `rustique.enable(only=[group])`, in fresh pyperf worker processes.

    python benches/targets/utils_pyperf.py random [-o out.json] [--fast]

Only ratios >= 1.5x are claims on the shared box (docs/ACCELERATION.md §7.4, brief); the
lead re-measures on a quiet box.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile

CELLS = {
    "random": (
        "import random; rng = random.Random(1); L = list(range(1000)); W = [i % 7 + 1 for i in range(1000)]",
        {
            "shuffle(1000)": "rng.shuffle(L)",
            "sample(range(10**6), 1000)": "rng.sample(range(1000000), 1000)",
            "sample(L, 500)": "rng.sample(L, 500)",
            "choices(L, k=1000)": "rng.choices(L, k=1000)",
            "choices(L, W, k=1000)": "rng.choices(L, W, k=1000)",
            "[randint(1, 6) x1000]": "[rng.randint(1, 6) for _ in range(1000)]",
        },
    ),
    "textwrap": (
        "import textwrap; T = ('The quick brown fox jumps over the lazy dog; ' * 12)[:540]; "
        "S = '\\n'.join('    line %d of some indented source text' % i for i in range(500)); "
        "B = 'word-wrapped hyphen-ated text, with punctuation! ' * 2000",
        {
            "wrap(540 chars, 70)": "textwrap.wrap(T)",
            "fill(540 chars, 40, indents)": "textwrap.fill(T, 40, initial_indent='> ', subsequent_indent='  ')",
            "dedent(500 lines)": "textwrap.dedent(S)",
            "indent(500 lines)": "textwrap.indent(S, '  ')",
            "fill(100 KB)": "textwrap.fill(B, 100)",
        },
    ),
    "statistics": (
        "import statistics as S; F = [x / 7 for x in range(10000)]; I = list(range(10000))",
        {
            "mean(10k floats)": "S.mean(F)",
            "mean(10k ints)": "S.mean(I)",
            "variance(10k floats)": "S.variance(F)",
            "stdev(10k floats)": "S.stdev(F)",
            "pstdev(10k ints)": "S.pstdev(I)",
        },
    ),
    "urllib": (
        "import urllib.parse as up; P = 'path with spaces/and ünicode/' * 300; "
        "Q = '&'.join('key%d=value+%d%%20x' % (i, i) for i in range(500)); "
        "U = ['http://host%d.example.com:8080/p/a/t/h?q=%d#f' % (i, i) for i in range(1000)]",
        {
            "quote(9 KB)": "up.quote(P)",
            "unquote(9 KB)": "up.unquote(up.quote(P))",
            "parse_qsl(500 fields)": "up.parse_qsl(Q)",
            "parse_qs(500 fields)": "up.parse_qs(Q)",
            "urlparse x1000 (uncached)": "[up.urlparse(u) for u in U]",
        },
    ),
}


def run(setup: str, stmt: str, enable: str | None, fast: bool) -> float:
    s = setup if enable is None else f"import rustique; rustique.enable(only=[{enable!r}]); {setup}"
    with tempfile.TemporaryDirectory() as d:
        out = d + "/r.json"  # pyperf refuses to overwrite an existing file
        cmd = [sys.executable, "-m", "pyperf", "timeit", "-q", "-o", out, "-s", s, stmt]
        if fast:
            cmd.insert(4, "--fast")
        subprocess.run(cmd, check=True, capture_output=True)
        data = json.load(open(out))
    vals = [v for r in data["benchmarks"][0]["runs"] for v in r.get("values", [])]
    vals.sort()
    return vals[len(vals) // 2]


def main() -> None:
    group = sys.argv[1]
    fast = "--fast" in sys.argv
    out = sys.argv[sys.argv.index("-o") + 1] if "-o" in sys.argv else None
    setup, cells = CELLS[group]
    rows = {}
    for name, stmt in cells.items():
        a = run(setup, stmt, None, fast)
        b = run(setup, stmt, group, fast)
        rows[name] = {"plain_s": a, "rustique_s": b, "ratio": b / a}
        print(f"{name:34s} {a * 1e6:12.1f} us {b * 1e6:12.1f} us {b / a:7.3f}", flush=True)
    if out:
        json.dump({"python": sys.version, "group": group, "rows": rows}, open(out, "w"), indent=1)


if __name__ == "__main__":
    main()
