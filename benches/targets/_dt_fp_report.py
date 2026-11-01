"""Dev helper: current fingerprints the datetime group checks (to maintain
crates/rustique-py/src/targets/datetime/fingerprints.rs)."""
import json, sys, datetime, _strptime, zoneinfo
from rustique._rustique import _accel
print(json.dumps({"v": list(sys.version_info[:2]), "c": _accel._datetime_cfingerprints(),
                  "fast": _accel._datetime_fingerprints(_strptime)}))
