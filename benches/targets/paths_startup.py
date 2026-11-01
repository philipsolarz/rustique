"""One-shot costs of the paths groups (callgrind regions, each in a fresh process):
`enable()` with the target modules already imported, and the lazy install when a target
module is imported after `enable()`.

    python benches/targets/paths_startup.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

CELLS = [
    ("enable(only=['posixpath'])", "rustique.enable(only=['posixpath'])", "import rustique, posixpath"),
    ("enable(only=['fnmatch']) (fnmatch imported)", "rustique.enable(only=['fnmatch'])", "import rustique, fnmatch"),
    ("enable(only=['pathlib']) (pathlib imported)", "rustique.enable(only=['pathlib'])", "import rustique, pathlib"),
    ("enable(only=['posixpath','fnmatch','pathlib'])", "rustique.enable(only=['posixpath', 'fnmatch', 'pathlib'])",
     "import rustique, posixpath, fnmatch, pathlib"),
    ("import pathlib, plain", "import pathlib", "import rustique"),
    ("import pathlib, pathlib group pending", "import pathlib", "import rustique; rustique.enable(only=['pathlib'])"),
]

for name, stmt, setup in CELLS:
    print(f"{name:48s} {region(stmt, setup):>12,} Ir", flush=True)
