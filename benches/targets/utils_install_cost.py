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

# Warm: another group with an imported vectorcall target is enabled first, so the
# framework's one-time costs (activation, monitoring calibration) are not charged to the
# measured group (REGISTRY_ANNOUNCE_2 item 16).
WARM = {"random": "import textwrap; rustique.enable(only=['textwrap'])"}
WARM_DEFAULT = "import random; rustique.enable(only=['random'])"

# Every setup ends with a full collection, so a region is not charged with a collection
# that the other side's setup happened to absorb.
GC = "; import gc; gc.collect()"

groups = sys.argv[1:] or ["random", "textwrap", "urllib", "statistics"]
for g in groups:
    m = MODULES[g]
    pre = PRE.format(m=m)
    warm = f"{pre}; {WARM.get(g, WARM_DEFAULT)}"
    en = region(f"rustique.enable(only=[{g!r}])", setup=pre + GC)
    plain = region(f"import {m}", setup=pre + GC)
    lazy = region(f"import {m}", setup=f"{pre}; rustique.enable(only=[{g!r}])" + GC)
    plain_w = region(f"import {m}", setup=warm + GC)
    lazy_w = region(f"import {m}", setup=f"{warm}; rustique.enable(only=[{g!r}])" + GC)
    print(f"{g:11s} enable()={en:>9,} Ir   import {m}: plain={plain:>11,}  enabled={lazy:>11,}  "
          f"overhead={lazy - plain:>+10,} ({(lazy - plain) / plain:+.1%})   "
          f"warm: plain={plain_w:>11,}  enabled={lazy_w:>11,}  overhead={lazy_w - plain_w:>+10,} "
          f"({(lazy_w - plain_w) / plain_w:+.1%})", flush=True)
