import json, sys, datetime, _strptime, zoneinfo
from rustique._rustique import _accel
rows = _accel.fingerprint_report(only=["datetime"])
print(json.dumps({"v": list(sys.version_info[:2]), "ft": not sys._is_gil_enabled() if hasattr(sys, "_is_gil_enabled") else False, "rows": rows}))
