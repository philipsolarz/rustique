"""CPython fact behind a framework-wide cost: after any sys.monitoring.free_tool_id (which
accel's monitoring-layout probe runs at the first vectorcall install), the global monitoring
version is nonzero, so every code object created afterwards pays _Py_Instrument at its
first execution. Region: compile+exec a module of 50 functions and call each once."""
import os, sys
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "targets"))
from _callgrind import region
SRC = "\n".join(f"def f{i}(x):\n    return x + {i}\n" for i in range(50)) + "\nfor i in range(50):\n    globals()[f'f{i}'](1)\n"
stmt = f"exec(compile({SRC!r}, 'm', 'exec'), {{}})"
plain = region(stmt, "")
probed = region(stmt, "import sys\nsys.monitoring.use_tool_id(5, 'p')\nsys.monitoring.free_tool_id(5)")
enabled = region(stmt, "import rustique\nrustique.enable()")
print(f"plain {plain:,}  after free_tool_id {probed:,} ({probed/plain:.3f})  after rustique.enable() {enabled:,} ({enabled/plain:.3f})")
