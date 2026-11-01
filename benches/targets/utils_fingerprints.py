"""Print the code fingerprints of the utils groups' originals on this interpreter (to record
them per minor version in the targets' `fp` tables)."""

import importlib
import sys

from rustique._rustique import _t_random as kit

TARGETS = {
    "random": ("random", ["Random.randint", "Random.randrange", "Random.choice", "Random.shuffle",
                          "Random.sample", "Random.choices"]),
}


def resolve(mod, path):
    obj = mod
    for part in path.split("."):
        obj = vars(obj)[part]
    return obj


for group, (modname, paths) in TARGETS.items():
    if len(sys.argv) > 1 and group not in sys.argv[1:]:
        continue
    mod = importlib.import_module(modname)
    for p in paths:
        print(f"{group:10s} {p:40s} {kit.fingerprint(resolve(mod, p)):#018x}")
