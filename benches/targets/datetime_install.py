"""datetime group: install costs (budget, ACCELERATION item 13: enable() <= 0.1M Ir; a lazy
install <= 5% of the triggering import). One-shot regions, each in a fresh process.

    python benches/targets/datetime_install.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

RQ = "import rustique._takeover as rq"
CELLS = [
    # enable() with every trigger module already imported (all 10 targets install now)
    ("enable(only=datetime), modules imported", "rq.enable(only=['datetime'])",
     f"import datetime, _strptime, time, zoneinfo; {RQ}", None),
    ("enable(only=datetime), nothing imported (pending)", "rq.enable(only=['datetime'])", f"{RQ}", None),
    # lazy install when `_datetime` is imported after enable()
    ("import datetime", "import datetime", f"{RQ}", f"{RQ}; rq.enable(only=['datetime'])"),
    # first strptime call: imports _strptime (the kernel falls back; nothing captured yet)
    ("first datetime.strptime (imports _strptime)", "datetime.datetime.strptime('2024', '%Y')",
     f"import datetime; {RQ}", f"import datetime; {RQ}; rq.enable(only=['datetime'])"),
    # second call: the kernel fingerprints and snapshots `_strptime` once (lazy capture)
    ("second datetime.strptime (captures _strptime)", "datetime.datetime.strptime('2024', '%Y')",
     f"import datetime; {RQ}; datetime.datetime.strptime('2024', '%Y')",
     f"import datetime; {RQ}; rq.enable(only=['datetime']); datetime.datetime.strptime('2024', '%Y')"),
]


def main():
    for name, stmt, plain_setup, enabled_setup in CELLS:
        plain = region(stmt, setup=plain_setup)
        if enabled_setup is None:
            print(f"{name:52s} {plain:>12,.0f} Ir", flush=True)
        else:
            en = region(stmt, setup=enabled_setup)
            print(f"{name:52s} plain {plain:>12,.0f}  enabled {en:>12,.0f}  +{en - plain:,.0f} ({en / plain - 1:+.1%})",
                  flush=True)


if __name__ == "__main__":
    main()
