"""Hits and fallbacks of the data-model targets over one workload run():
python benches/datamodel/wl_stats.py custom/mini_interp [...]"""
import importlib.util
import os
import sys

import rustique

HERE = os.path.dirname(os.path.abspath(__file__))
WL = os.path.join(HERE, "..", "workloads")
rustique.stats(True)
rustique.enable()
for sel in sys.argv[1:]:
    path = os.path.join(WL, sel + ".py")
    sys.path.insert(0, os.path.dirname(path))
    spec = importlib.util.spec_from_file_location("wl_" + sel.replace("/", "_"), path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    st = mod.setup()
    mod.run(st)
    rustique.stats(True, reset=True)
    mod.run(st)
    print("==", sel)
    for name, v in sorted(rustique.stats().items()):
        if name.split(".")[0] in ("enum", "collections", "dataclasses", "contextlib", "functools"):
            fb = {k: n for k, n in v["fallback"].items() if n}
            if v["hits"] or fb:
                print(f"  {name:40s} hits {v['hits']:>9}  fallbacks {fb}")
