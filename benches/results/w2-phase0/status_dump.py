"""Target status of one build after `rustique.enable()`, with the target modules imported.

    python status_dump.py OUT.json        (in the build's venv)

Imports the stdlib modules the target groups patch, calls `rustique.enable()`, then imports
the module of any target still `pending-import` (longest importable dotted prefix of its
name) and reads `rustique.status()` again. Writes the full status plus a summary
(installed / refused with reason and category / pending, per target) and prints the summary.
A patch release whose stdlib code differs from the per-minor fingerprints shows up here as
`refused` targets.
"""

import importlib
import json
import sys
import sysconfig

BASE_MODULES = ("json", "copy", "dataclasses", "datetime", "_strptime", "time", "posixpath", "os.path",
                "fnmatch", "glob", "pathlib", "logging", "random", "textwrap", "urllib.parse", "statistics")

imported, failed = [], {}
for m in BASE_MODULES:
    try:
        importlib.import_module(m)
        imported.append(m)
    except Exception as e:  # noqa: BLE001 - recorded
        failed[m] = repr(e)

import rustique  # noqa: E402

rustique.enable()
st = rustique.status()
for t in st["targets"]:
    if t["state"] != "pending-import":
        continue
    parts = t["name"].split(".")
    for k in range(len(parts) - 1, 0, -1):
        mod = ".".join(parts[:k])
        if mod in sys.modules:
            break
        try:
            importlib.import_module(mod)
            imported.append(mod)
            break
        except Exception:  # noqa: BLE001 - not a module name; try a shorter prefix
            continue
st = rustique.status()

by_state: dict[str, list] = {}
for t in st["targets"]:
    by_state.setdefault(t["state"], []).append(t)
summary = {
    "python": sys.version,
    "free_threaded": bool(sysconfig.get_config_var("Py_GIL_DISABLED")),
    "rustique": rustique.__version__,
    "targets": len(st["targets"]),
    "groups": sorted({t["group"] for t in st["targets"]}),
    "counts": {k: len(v) for k, v in sorted(by_state.items())},
    "refused": [{"name": t["name"], "group": t["group"], "refusal": t.get("refusal"), "reason": t.get("reason")}
                for t in by_state.get("refused", [])],
    "pending": [t["name"] for t in by_state.get("pending-import", [])],
    "disabled": [t["name"] for t in by_state.get("disabled", [])],
    "imported": imported,
    "import_failed": failed,
}
with open(sys.argv[1], "w") as f:
    json.dump({"summary": summary, "status": st}, f, indent=1, default=str)
print(json.dumps(summary, indent=1, default=str))
