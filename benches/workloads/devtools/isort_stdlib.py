"""isort sorting the imports of all 152 top-level standard-library modules (~135k lines).

Stands for the import sorter run on save / in pre-commit (also what ruff's `I` rules
replace). isort scans every line of each file (`str` methods, regexes, a small parser
for import statements), then rewrites the import blocks with the default (`black`-
incompatible) profile and section placement against its bundled stdlib list. Note: isort
9 ships **mypyc-compiled** cp315 wheels, so on 3.15 most of isort's own code is native
(C extension calling the C API directly); the pure-Python build (`--no-binary isort`) is
profiled separately in docs/atlas/workloads-devtools.md.
"""

import hashlib
import os
import sysconfig

REQUIRES = ["isort==9.0.2"]

def stdlib_sources():
    root = sysconfig.get_path("stdlib")
    out = []
    for rel in sorted(os.listdir(root)):
        if rel.endswith(".py"):
            with open(os.path.join(root, rel), encoding="utf-8") as f:
                out.append((rel, f.read()))
    return out


def setup():
    import isort

    # A fixed config: no filesystem lookups for settings files, stdlib known.
    config = isort.Config(settings_path="/", profile="", py_version="315", quiet=True)
    return isort, config, stdlib_sources()


def run(state):
    isort, config, files = state
    return [isort.code(src, config=config, file_path=None) for _, src in files]


def digest(result):
    h = hashlib.sha256()
    for text in result:
        h.update(text.encode())
    return h.hexdigest()
