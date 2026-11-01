"""Dev helper: fingerprints the datetime group checks, via the registry's record mode
(`python -m rustique._takeover._dev report --only datetime --import datetime,_strptime,time`
prints the same rows)."""
import json
import sys

import _strptime  # noqa: F401  (record mode lists its functions when it is imported)
import datetime  # noqa: F401
import zoneinfo  # noqa: F401

from rustique._rustique import _accel

print(json.dumps({"v": list(sys.version_info[:2]), "rows": _accel.fingerprint_report(only=["datetime"])}))
