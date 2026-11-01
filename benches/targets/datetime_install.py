"""datetime group: install costs (budget, ACCELERATION item 13: enable() <= 0.1M Ir; a lazy
install <= 5% of the triggering import). One-shot regions, each in a fresh process.

    python benches/targets/datetime_install.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

RQ = "from rustique._rustique import _accel"
CELLS = [
    # enable() with every trigger module already imported (all 10 targets install now)
    ("enable(only=datetime), modules imported", "_accel.enable(['datetime'])",
     f"import datetime, _strptime, time, zoneinfo; {RQ}", None),
    ("enable(only=datetime), nothing imported (pending)", "_accel.enable(['datetime'])", f"{RQ}", None),
    # lazy install when `_datetime` is imported after enable()
    ("import datetime", "import datetime", f"{RQ}", f"{RQ}; _accel.enable(['datetime'])"),
    # first strptime call: imports _strptime (the kernel falls back; nothing captured yet)
    ("first datetime.strptime (imports _strptime)", "datetime.datetime.strptime('2024', '%Y')",
     f"import datetime; {RQ}", f"import datetime; {RQ}; _accel.enable(['datetime'])"),
    # second call: the kernel fingerprints and snapshots `_strptime` once (lazy capture)
    ("second datetime.strptime (captures _strptime)", "datetime.datetime.strptime('2024', '%Y')",
     f"import datetime; {RQ}; datetime.datetime.strptime('2024', '%Y')",
     f"import datetime; {RQ}; _accel.enable(['datetime']); datetime.datetime.strptime('2024', '%Y')"),
]


BASE = [
    ("registry baseline: enable(only=_selftest.meth), modules imported", "_accel.enable(['_selftest.meth'])",
     f"import datetime, _strptime, time, zoneinfo; {RQ}", None),
    ("registry baseline: enable(only=_selftest.meth)", "_accel.enable(['_selftest.meth'])", f"{RQ}", None),
]


def main():
    for name, stmt, plain_setup, enabled_setup in BASE + CELLS:
        plain = region(stmt, setup=plain_setup)
        if enabled_setup is None:
            print(f"{name:52s} {plain:>12,.0f} Ir", flush=True)
        else:
            en = region(stmt, setup=enabled_setup)
            print(f"{name:52s} plain {plain:>12,.0f}  enabled {en:>12,.0f}  +{en - plain:,.0f} ({en / plain - 1:+.1%})",
                  flush=True)


if __name__ == "__main__":
    main()
