"""Callgrind profile of one group's lazy install (`import <module>` with the group enabled),
optionally warm (another group with an imported vectorcall target enabled first).

    valgrind --tool=callgrind --instr-atstart=no python benches/targets/_profile_install.py GROUP MODULE [WARM_GROUP WARM_MODULE]
"""

import gc
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from utils_cg import helper  # noqa: E402

h = helper()
import rustique  # noqa: E402

g, m = sys.argv[1], sys.argv[2]
if len(sys.argv) > 4:
    __import__(sys.argv[4])
    rustique.enable(only=[sys.argv[3]])
for k in [k for k in sys.modules if k == m]:
    del sys.modules[k]
if g != "-":  # `-`: the plain import (for a per-function diff)
    rustique.enable(only=[g])
gc.collect()
h.start()
__import__(m)
h.dump("x")
h.stop()
