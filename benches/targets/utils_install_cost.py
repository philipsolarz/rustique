"""Install cost of the utils groups (budget, REGISTRY_ANNOUNCE item 13): Ir of
`enable(only=[group])` when the group's module is not imported (lazy: nothing to do), and
of `import <module>` plain vs with the group enabled (the lazy install runs in the import).
Each number from a fresh process (callgrind, region-instrumented).

    python benches/targets/utils_install_cost.py [groups...]
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

MODULES = {"random": "random", "textwrap": "textwrap", "urllib": "urllib.parse", "statistics": "statistics",
           "ipaddress": "ipaddress"}
# The harness (`_callgrind` -> tempfile) imports `random` in the worker: drop the target
# module from sys.modules so both sides import it afresh (its dependencies stay imported).
PRE = "import rustique, sys; [sys.modules.pop(k) for k in list(sys.modules) if k == {m!r} or k.startswith({m!r} + '.')]"

groups = sys.argv[1:] or ["random", "textwrap", "urllib", "statistics"]
for g in groups:
    m = MODULES[g]
    pre = PRE.format(m=m)
    en = region(f"rustique.enable(only=[{g!r}])", setup=pre)
    plain = region(f"import {m}", setup=pre)
    lazy = region(f"import {m}", setup=f"{pre}; rustique.enable(only=[{g!r}])")
    print(f"{g:11s} enable()={en:>9,} Ir   import {m}: plain={plain:>11,}  enabled={lazy:>11,}  "
          f"overhead={lazy - plain:>+10,} ({(lazy - plain) / plain:+.1%})", flush=True)
