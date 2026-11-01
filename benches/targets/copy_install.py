"""Install cost of groups `copy` and `dataclasses` (ACCELERATION budget: enable() of a group
≤ 0.1M Ir; lazy install on import ≤ 5% of the module's own import). Callgrind regions,
fresh processes (`benches/targets/_callgrind.py`).

    python benches/targets/copy_install.py
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

CELLS = [
    ("enable(only=['copy']), copy imported", "rustique.enable(only=['copy'])", "import copy, rustique"),
    ("enable(only=['dataclasses']), imported", "rustique.enable(only=['dataclasses'])", "import copy, dataclasses, rustique"),
    ("enable(only=['copy','dataclasses']), imported", "rustique.enable(only=['copy', 'dataclasses'])",
     "import copy, dataclasses, rustique"),
]
IMPORTS = [
    ("import copy", "import copy", ["copy"]),
    ("import dataclasses (copy imported)", "import dataclasses", ["dataclasses"]),
    ("import dataclasses (both groups)", "import dataclasses", ["copy", "dataclasses"]),
]


def main() -> None:
    for name, stmt, setup in CELLS:
        base = region("pass", setup=setup)
        ir = region(stmt, setup=setup) - base
        print(f"{name:50s} {ir:12,d} Ir", flush=True)
    for name, stmt, groups in IMPORTS:
        pre = "import rustique" + ("; import copy" if "copy imported" in name else "")
        plain = region(stmt, setup=pre)
        lazy = region(stmt, setup=pre + f"; rustique.enable(only={groups!r})")
        print(f"{name:50s} plain {plain:12,d}  with lazy install {lazy:12,d}  overhead {lazy / plain - 1:+.1%}",
              flush=True)


if __name__ == "__main__":
    main()
