"""Install cost of groups `copy` and `dataclasses` (ACCELERATION budget: enable() of a group
≤ 0.1M Ir; lazy install on import ≤ 5% of the module's own import). Callgrind regions,
fresh processes (`benches/targets/_callgrind.py`).

`_accel.enable(only=[...])` is the quiet native call (no status dict). "warm": after an
imported vectorcall target of another group (`_selftest.shared_a`) took the process's
one-time costs, as the budget is stated (REGISTRY_ANNOUNCE_2 item 16).

    python benches/targets/copy_install.py
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

A = "import rustique._rustique as r; "
# Warm-up (docs/targets/framework.md): an imported vectorcall target of another group takes
# the one-time monitoring calibration, so the measured enable is the group's own cost.
WARM = (
    "import sys, types; m = types.ModuleType('_rustique_selftest'); "
    "exec('SEP = 1\\ndef same_a(x):\\n    return x\\n', m.__dict__); "
    "sys.modules['_rustique_selftest'] = m; r._accel.enable(['_selftest.shared_a']); "
)
CELLS = [
    # (name, stmt, setup)
    ("enable(['copy']) cold (first enable of the process)", "r._accel.enable(['copy'])", A + "import copy"),
    ("enable(['copy']) warm", "r._accel.enable(['copy'])", A + WARM + "import copy"),
    ("enable(['dataclasses']) warm", "r._accel.enable(['dataclasses'])", A + WARM + "import copy, dataclasses"),
    ("enable(['copy', 'dataclasses']) warm", "r._accel.enable(['copy', 'dataclasses'])",
     A + WARM + "import copy, dataclasses"),
]
IMPORTS = [
    # (name, stmt, groups, pre-imports)
    ("import copy", "import copy", ["copy"], ""),
    ("import dataclasses (copy imported, group dataclasses)", "import dataclasses", ["dataclasses"], "import copy"),
    ("import dataclasses (copy imported, both groups)", "import dataclasses", ["copy", "dataclasses"],
     "import copy"),
]


def main() -> None:
    for name, stmt, setup in CELLS:
        ir = region(stmt, setup=setup) - region("pass", setup=setup)
        print(f"{name:52s} {ir:10,d} Ir", flush=True)
    for name, stmt, groups, pre in IMPORTS:
        setup = A + WARM + (pre or "pass")
        plain = region(stmt, setup=setup)
        lazy = region(stmt, setup=setup + f"; r._accel.enable({groups!r})")
        print(f"{name:55s} plain {plain:12,d}  lazy install (warm) {lazy:12,d}  overhead {lazy - plain:10,d} "
              f"({lazy / plain - 1:+.1%})", flush=True)


if __name__ == "__main__":
    main()
