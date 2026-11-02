"""Wall time (pyperf) of the `float` and `json` targets on large inputs, plain vs enabled.

Each (case, variant) is a `pyperf timeit` run in its own worker processes; the report is
the ratio of medians (enabled / plain). The dev box is shared: use these numbers for
>= 1.5x claims only (callgrind cells in `json_cg.py` carry the small-input claims).

    python benches/targets/json_pyperf.py [--only SUBSTR] [--processes 6] [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile

SETUP = r"""
import json, random
rng = random.Random(7)
record = {"id": 12345, "name": "Ada Lovelace", "email": "ada@example.com", "active": True,
          "score": 98.5, "tags": ["admin", "math"], "age": 36, "ratio": 0.3333333333333333,
          "manager": None, "city": "London"}
records = [dict(record, id=i, score=i / 7, name=f"user{i}") for i in range(1000)]
floats = [rng.random() * 10 ** rng.randint(-5, 8) for _ in range(10000)]
ints = [rng.randint(-10**12, 10**12) for _ in range(10000)]
texts = ["".join(rng.choice("abc\xe9€\U0001F600 \"\\\n") for _ in range(40)) for _ in range(2000)]
records_text = json.dumps(records)
records_bytes = records_text.encode()
floats_text = json.dumps(floats)
texts_text = json.dumps(texts, ensure_ascii=False)
"""

ENABLE = "import rustique; rustique.enable(only=['json', 'float'])\n"

CASES = {
    "dumps 1000 records (190 KB)": "json.dumps(records)",
    "dumps 1000 records indent=2": "json.dumps(records, indent=2)",
    "dumps 1000 records sort_keys": "json.dumps(records, sort_keys=True)",
    "dumps 10k floats": "json.dumps(floats)",
    "dumps 10k ints": "json.dumps(ints)",
    "dumps 2000 unicode strings": "json.dumps(texts)",
    "dumps 2000 unicode strings ensure_ascii=False": "json.dumps(texts, ensure_ascii=False)",
    "loads 1000 records": "json.loads(records_text)",
    "loads 1000 records (bytes)": "json.loads(records_bytes)",
    "loads 10k floats": "json.loads(floats_text)",
    "loads 2000 unicode strings": "json.loads(texts_text)",
    "repr of 10k floats": "list(map(repr, floats))",
}


def run(stmt: str, setup: str, processes: int) -> float:
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "r.json")
        cmd = [sys.executable, "-m", "pyperf", "timeit", "-q", "--processes", str(processes), "--values", "5",
               "-s", setup, stmt, "-o", out]
        subprocess.run(cmd, check=True, capture_output=True, text=True)
        import pyperf

        return pyperf.Benchmark.load(out).median()


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--only", default="")
    p.add_argument("--processes", type=int, default=6)
    p.add_argument("-o")
    a = p.parse_args()
    rows = {}
    for name, stmt in CASES.items():
        if a.only and a.only not in name:
            continue
        plain = run(stmt, SETUP, a.processes)
        on = run(stmt, ENABLE + SETUP, a.processes)
        rows[name] = {"plain_s": plain, "on_s": on, "ratio": on / plain}
        print(f"{name:48s} {plain * 1e3:9.3f} ms {on * 1e3:9.3f} ms {on / plain:7.3f}", flush=True)
    if a.o:
        with open(a.o, "w") as f:
            json.dump({"python": sys.version, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main()
