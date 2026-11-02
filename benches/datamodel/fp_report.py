"""Print the fingerprints the datamodel groups check (record mode), as JSON."""
import collections, contextlib, dataclasses, enum, functools, json, sys
from rustique._rustique import _accel
rows = _accel.fingerprint_report(only=["enum", "generated", "contextlib", "functools"])
print(json.dumps({"v": list(sys.version_info[:2]), "rows": rows}, indent=1))
