"""Code created after enable() costs what it costs in plain CPython (no monitoring tax).

Wave 1 calibrated the local-monitoring layout with a free `sys.monitoring` tool id; freeing it
bumped the interpreter's monitoring version, so every code object created afterwards paid
`_Py_Instrument` at its first RESUME (~930 Ir each). The cells compile, exec and first-call 50
fresh functions (and import a fresh 50-function module) after `enable()` of a vectorcall
target, against plain. Ratios must be 1.00 within noise.

    python benches/accel/monitoring_tax.py
"""

from __future__ import annotations

import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "targets"))
from _callgrind import region  # noqa: E402

SRC = "".join(f"def f{i}(x):\n    return x + {i}\n" for i in range(50))
EXEC = f"ns = {{}}; exec(compile({SRC!r}, '<m>', 'exec'), ns); [ns['f%d' % i](1) for i in range(50)]"
ENABLE = (
    "import posixpath; from rustique import _rustique; A = _rustique._accel; "
    "getattr(A, '_set_validation', lambda m: m)('eager'); A.enable(['posixpath.join']); "
    "assert 'posixpath.join' in A.installed()"
)


def main() -> None:
    d = tempfile.mkdtemp()
    with open(os.path.join(d, "_mtax_mod.py"), "w") as f:
        f.write(SRC)
    imp = f"import sys; sys.path.insert(0, {d!r})"
    rows = [
        ("compile+exec+first call, 50 functions", EXEC, ""),
        ("import a 50-function module (cached .pyc)", "import _mtax_mod; [getattr(_mtax_mod, 'f%d' % i)(1) for i in range(50)]", imp),
    ]
    # Warm the .pyc once.
    os.system(f'{sys.executable} -c "import sys; sys.path.insert(0, {d!r}); import _mtax_mod"')
    print(f"{'cell':48} {'plain Ir':>10} {'enabled Ir':>11} {'ratio':>6}")
    for name, stmt, setup in rows:
        plain = region(stmt, setup)
        enabled = region(stmt, setup + ("; " if setup else "") + ENABLE)
        print(f"{name:48} {plain:>10} {enabled:>11} {enabled / plain:>6.3f}")


if __name__ == "__main__":
    main()
