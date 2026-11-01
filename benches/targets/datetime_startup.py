"""datetime group: whole-process cells (ACCELERATION item 13: what finally counts).

Total instructions of a whole `python -c ...` process under callgrind (deterministic), with
the startup switch: plain, `RUSTIQUE=1 RUSTIQUE_ONLY=datetime`, and the registry's own
baseline `RUSTIQUE=1 RUSTIQUE_ONLY=_selftest.meth` (extension loaded, one trivial target).

    python benches/targets/datetime_startup.py
"""

import glob
import os
import subprocess
import sys
import tempfile

CODES = {
    "pass": "pass",
    "import datetime": "import datetime",
    "import datetime + strftime + isoformat": "import datetime as D; d = D.datetime(2024, 3, 15); d.strftime('%Y-%m-%d'); d.isoformat()",
    "import datetime + 3 strptime": "import datetime as D; [D.datetime.strptime('2024-03-15', '%Y-%m-%d') for _ in range(3)]",
}
ENVS = {
    "plain": {},
    "registry baseline": {"RUSTIQUE": "1", "RUSTIQUE_ONLY": "_selftest.meth"},
    "datetime": {"RUSTIQUE": "1", "RUSTIQUE_ONLY": "datetime"},
}


def total(code: str, extra: dict) -> int:
    env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
    env.update(extra)
    env["PYTHONHASHSEED"] = "0"
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        subprocess.run(["valgrind", "--tool=callgrind", "-q", f"--callgrind-out-file={out}", sys.executable, "-c", code],
                       env=env, check=True, capture_output=True)
        for f in glob.glob(out + "*"):
            for line in open(f):
                if line.startswith(("totals:", "summary:")):
                    return int(line.split()[1])
    raise RuntimeError("no totals")


def main():
    for name, code in CODES.items():
        r = {k: total(code, e) for k, e in ENVS.items()}
        base, reg, dt = r["plain"], r["registry baseline"], r["datetime"]
        print(f"{name:42s} plain {base:>12,}  registry {reg:>12,} ({reg / base - 1:+.1%})  "
              f"datetime {dt:>12,} ({dt / base - 1:+.1%}; group +{dt - reg:,})", flush=True)


if __name__ == "__main__":
    main()
