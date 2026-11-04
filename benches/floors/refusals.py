"""Which targets refuse on the running interpreter, and why (one line per target).

    python benches/floors/refusals.py        # enable(), import every target module, report
"""
import collections, importlib, platform, sys
import rustique
rustique.enable()
for m in ["json", "copy", "dataclasses", "datetime", "_strptime", "time", "posixpath", "fnmatch", "pathlib",
          "logging", "random", "textwrap", "urllib.parse", "statistics"]:
    importlib.import_module(m)
st = rustique.status()["targets"]
c = collections.Counter(t["state"] for t in st)
ft = "t" if not getattr(sys, "_is_gil_enabled", lambda: True)() else ""
print(f"{platform.python_version()}{ft}: {dict(c)}")
for t in st:
    if t["state"] != "installed" and "not validated" not in (t["reason"] or ""):
        print(f"  {t['name']}: {t['reason']}")
