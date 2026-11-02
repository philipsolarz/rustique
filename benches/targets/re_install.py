"""Install cost of the `re` group (ACCELERATION §3.12, T9): whole-process callgrind
instructions (PYTHONHASHSEED=0, fresh process each, median of 3):

    R  import rustique
    A  import rustique; import re
    B  import rustique; rustique.enable(only=["re"])
    C  import rustique; rustique.enable(only=["re"]); import re      (lazy install at import)
    D  import rustique; import re; rustique.enable(only=["re"])      (eager install)
    E  import rustique; import re; rustique.enable(only=["json"])    (an enable selecting no
                                                                     installable target: D's base)

lazy install = C - B - (A - R), against `import re` itself (A - R).

    python benches/targets/re_install.py
"""

from __future__ import annotations

import os
import statistics
import subprocess
import sys
import tempfile


def ir(code: str) -> int:
    vals = []
    for _ in range(3):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "cg.out")
            env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE")}
            env["PYTHONHASHSEED"] = "0"
            subprocess.run(["valgrind", "--tool=callgrind", "-q", f"--callgrind-out-file={out}", "--dump-instr=no",
                            sys.executable, "-c", code], env=env, check=True, capture_output=True)
            with open(out) as f:
                for line in f:
                    if line.startswith(("summary:", "totals:")):
                        vals.append(int(line.split()[1]))
                        break
    return int(statistics.median(vals))


def main() -> None:
    r = ir("import rustique")
    a = ir("import rustique; import re")
    b = ir("import rustique; rustique.enable(only=['re'])")
    c = ir("import rustique; rustique.enable(only=['re']); import re")
    d = ir("import rustique; import re; rustique.enable(only=['re'])")
    e = ir("import rustique; import re; rustique.enable(only=['json'])")
    imp = a - r
    lazy = c - b - imp
    print(f"import re itself                       {imp:12,d}")
    print(f"lazy install at the end of import re   {lazy:12,d}  ({100 * lazy / imp:.1f}% of import re)")
    print(f"eager install (re already imported)    {d - e:12,d}  (vs an enable selecting no re target)")


if __name__ == "__main__":
    main()
