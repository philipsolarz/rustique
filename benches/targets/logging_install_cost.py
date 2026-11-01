"""Install cost of the logging group (contract budget: <= 0.1M Ir at enable(), lazy
install <= 5% of the module's own import). callgrind, one region per fresh process.

    python benches/targets/logging_install_cost.py
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

rows = {
    "import logging (plain)": region("import logging", "import rustique"),
    "import logging (logging group pending: lazy install)": region(
        "import logging", "import rustique; rustique.enable(only=['logging'])"),
    "rustique.enable(only=['logging']) with logging imported": region(
        "rustique.enable(only=['logging'])", "import rustique, logging"),
    "rustique.enable(only=['logging']) before import (pending)": region(
        "rustique.enable(only=['logging'])", "import rustique"),
    "_accel.enable(['logging']) (quiet registry call) with logging imported": region(
        "A.enable(['logging'])", "import rustique, logging; from rustique._rustique import _accel as A"),
    "_accel.enable(['logging']) again after disable (warm: process-wide calibrations done)": region(
        "A.enable(['logging'])",
        "import rustique, logging; from rustique._rustique import _accel as A; A.enable(['logging']); A.disable(['logging'])"),
}
base = rows["import logging (plain)"]
for k, v in rows.items():
    extra = f"  (+{(v - base) / base:.1%} of the import)" if "lazy" in k else ""
    print(f"{k:60s} {v:>12,}{extra}")
