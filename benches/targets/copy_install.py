"""Install cost of groups `copy` and `dataclasses` (ACCELERATION budget: enable() of a group
≤ 0.1M Ir; lazy install on import ≤ 5% of the module's own import). Callgrind regions,
fresh processes (`benches/targets/_callgrind.py`).

`_accel.enable(only=[...])` is the quiet native call (no status dict); the group's own cost
is the difference to enabling the same group while its module is not imported (the
framework's pass and the lazy-install hook, without the group's prepare()).

    python benches/targets/copy_install.py
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

A = "import rustique._rustique as r; "
CELLS = [
    # (name, stmt, setup with the module imported, setup without it)
    ("enable(only=['copy'])", "r._accel.enable(['copy'])", A + "import copy", A + "pass"),
    ("enable(only=['dataclasses'])", "r._accel.enable(['dataclasses'])", A + "import copy, dataclasses",
     A + "import copy"),
    ("enable(only=['copy','dataclasses'])", "r._accel.enable(['copy', 'dataclasses'])",
     A + "import copy, dataclasses", A + "pass"),
]
IMPORTS = [
    ("import copy", "import copy", ["copy"], ""),
    ("import dataclasses (copy imported, group dataclasses)", "import dataclasses", ["dataclasses"], "import copy"),
    ("import dataclasses (copy imported, both groups)", "import dataclasses", ["copy", "dataclasses"],
     "import copy"),
]


def main() -> None:
    for name, stmt, setup, setup_absent in CELLS:
        base = region("pass", setup=setup)
        ir = region(stmt, setup=setup) - base
        absent = region(stmt, setup=setup_absent) - region("pass", setup=setup_absent)
        print(f"{name:40s} {ir:10,d} Ir   module not imported {absent:10,d}   group's own {ir - absent:10,d}",
              flush=True)
    for name, stmt, groups, pre in IMPORTS:
        setup = A + (pre or "pass")
        plain = region(stmt, setup=setup)
        lazy = region(stmt, setup=setup + f"; r._accel.enable({groups!r})")
        print(f"{name:55s} plain {plain:12,d}  lazy install {lazy:12,d}  overhead {lazy - plain:10,d} "
              f"({lazy / plain - 1:+.1%})", flush=True)


if __name__ == "__main__":
    main()
