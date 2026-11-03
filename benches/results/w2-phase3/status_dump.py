"""Target status of one interpreter after `rustique.enable()`, every target module imported (T10).

    RUSTIQUE_VALIDATE=eager python status_dump.py OUT.json     (in a venv with Rustique)

Wave 2 version of Phase 0's status_dump.py. Imports the module of every target the
extension declares (`_accel.targets()`: name, group, module), calls `rustique.enable()`
(the default tier; `RUSTIQUE_TIERS` in the environment adds tiers), imports the module of any
target still `pending-import` (longest importable dotted prefix of its name), and records
`rustique.status()`. Run with `RUSTIQUE_VALIDATE=eager` so that every verdict is final
(first-call validation shows `armed` until a target's first call). Writes the full status
plus a summary: counts per state for the default selection (tier targets apart), refusals
with category and reason, the process gate's verdict.
"""

import collections
import importlib
import json
import os
import sys
import sysconfig

import rustique

try:
    import _rustique as ext
except ImportError:  # in-package layout without the shim
    from rustique import _rustique as ext

imported, failed = [], {}
for _name, group, mod in ext._accel.targets():
    if not mod or group.startswith("_") or mod in sys.modules:  # hidden groups: as reach.py
        continue
    try:
        importlib.import_module(mod)
        imported.append(mod)
    except Exception as e:  # noqa: BLE001 - recorded
        failed[mod] = repr(e)

res = rustique.enable()
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


def counts(ts):
    return dict(sorted(collections.Counter(t["state"] for t in ts).items()))


default = [t for t in st["targets"] if not t.get("tier")]
tiers = [t for t in st["targets"] if t.get("tier")]
summary = {
    "python": sys.version,
    "executable": sys.executable,
    "free_threaded": bool(sysconfig.get_config_var("Py_GIL_DISABLED")),
    "rustique": rustique.__version__,
    "extension": ext.__file__,
    "env": {k: v for k, v in os.environ.items() if k.startswith("RUSTIQUE")},
    "process": st.get("process"),
    "targets": len(st["targets"]),
    "default_targets": len(default),
    "groups": sorted({t["group"] for t in default}),
    "counts_default": counts(default),
    "installed_default": sum(t["state"] == "installed" for t in default),
    "refusal_categories_default": dict(collections.Counter(t.get("refusal") for t in default if t["state"] == "refused")),
    "counts_tiers": {tier: counts([t for t in tiers if t["tier"] == tier]) for tier in sorted({t["tier"] for t in tiers})},
    "refused": [{"name": t["name"], "group": t["group"], "tier": t.get("tier"), "refusal": t.get("refusal"),
                 "reason": t.get("reason")} for t in st["targets"] if t["state"] == "refused"],
    "pending": [t["name"] for t in st["targets"] if t["state"] == "pending-import"],
    "armed": [t["name"] for t in st["targets"] if t["state"] == "armed"],
    "imported": imported,
    "import_failed": failed,
}
with open(sys.argv[1], "w") as f:
    json.dump({"summary": summary, "status": st, "enable_result": res}, f, indent=1, default=str)
s = dict(summary)
s.pop("process", None)
print(json.dumps(s, indent=1, default=str))
