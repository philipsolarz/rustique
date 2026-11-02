"""Install cost of the `float` and `json` groups (budget: <= 0.1M Ir per group at
`enable()`, lazy install <= 5% of the module's own import), callgrind-region measured.

Each cell is a fresh process under callgrind; only the region is instrumented:
  enable float            `rustique.enable(only=["float"])`
  enable json (pending)   `rustique.enable(only=["json"])` before `json` is imported
  enable json (installed) `rustique.enable(only=["json"])` after `import json`
  import json plain       `import json` without Rustique enabled
  import json lazy        `import json` with the json targets pending (installs them)

    python benches/targets/json_install_cost.py [-o out.json]
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import sysconfig
import tempfile

# No `json` import at module level: the worker measures `import json` itself.

CELLS = {
    "enable float": ("import rustique", "rustique.enable(only=['float'])"),
    "enable json (pending)": ("import rustique", "rustique.enable(only=['json'])"),
    "enable json (installed)": ("import rustique, json", "rustique.enable(only=['json'])"),
    "import json plain": ("import rustique, re, codecs", "import json"),
    "import json lazy": ("import rustique, re, codecs; rustique.enable(only=['json'])", "import json"),
}


def helper():
    """The callgrind client-request module of `json_cg.py` (built there, cached per ABI)."""
    cache = os.path.join(os.path.expanduser("~"), ".cache", "rustique-cg", sysconfig.get_config_var("SOABI"))
    if cache not in sys.path:
        sys.path.insert(0, cache)
    import _rq_cg_v1  # type: ignore[import-not-found]

    return _rq_cg_v1


def worker(name: str) -> None:
    h = helper()
    setup, stmt = CELLS[name]
    ns: dict = {}
    exec(setup, ns)
    code = compile(stmt, "<cell>", "exec")
    h.start()
    exec(code, ns)
    h.dump("x")
    h.stop()
    if "json lazy" in name or "installed" in name:
        import rustique

        st = {d["name"]: d["state"] for d in rustique.status()["targets"]}
        assert st["json.dumps"] == st["json.loads"] == "installed", st


def run(name: str) -> int:
    with tempfile.TemporaryDirectory() as d:
        out = os.path.join(d, "cg.out")
        subprocess.run(["valgrind", "--tool=callgrind", "--instr-atstart=no", "-q", f"--callgrind-out-file={out}",
                        sys.executable, os.path.abspath(__file__), "_worker", name], check=True,
                       env={**os.environ, "PYTHONHASHSEED": "0"})
        import glob

        for fn in glob.glob(out + "*"):
            with open(fn) as f:
                tag = None
                for line in f:
                    if line.startswith("desc: Trigger: Client Request: "):
                        tag = line.split("Client Request: ", 1)[1].strip()
                    elif line.startswith(("totals:", "summary:")) and tag == "x":
                        return int(line.split()[1])
    raise RuntimeError("no region")


def main() -> None:
    if len(sys.argv) > 2 and sys.argv[1] == "_worker":
        worker(sys.argv[2])
        return
    import json as _json

    subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), "json_cg.py"), "--only",
                    "float repr(1.23)", "-j", "1"], check=True, capture_output=True)  # builds the helper
    p = argparse.ArgumentParser()
    p.add_argument("-o")
    a = p.parse_args()
    rows = {name: run(name) for name in CELLS}
    for name, ir in rows.items():
        print(f"{name:28s} {ir:12,d} Ir")
    lazy = rows["import json lazy"] - rows["import json plain"]
    print(f"lazy install on `import json`: +{lazy:,d} Ir ({lazy / rows['import json plain']:.1%} of the import)")
    if a.o:
        with open(a.o, "w") as f:
            _json.dump({"python": sys.version, "rows": rows, "lazy_install": lazy}, f, indent=1)


if __name__ == "__main__":
    main()
