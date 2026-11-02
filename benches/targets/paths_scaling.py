"""Thread scaling of the paths kernels (posixpath, fnmatch, pathlib): the 8-thread rule
(DECISIONS §10.9: CPU time per call at 8 threads ≤ 1.05× CPython's original on 3.15.1t).

Runs `benches/ft/kernel_scaling.py` with its paths cells plus the ones below:

    python benches/targets/paths_scaling.py run [--enable] -t 1,8 -o out.json [--cells a,b]
    python benches/targets/paths_scaling.py table plain.json enabled.json

`run` without `--cells` runs only the paths cells (the shared ones and these).
"""

from __future__ import annotations

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "ft"))

import kernel_scaling as K  # noqa: E402

NAMES = "['main.py', 'utils_3.txt', 'config.json', 'models.pyc', 'README', 'index.html', 'a.tmp', 'b.bak'] * 4"
PATS = "['*.pyc', '*.tmp', '*.bak', '*~', '.DS_Store', '*.log', 'Thumbs.db', '*.swp', '*.egg-info', '*.o', '*.so', 'coverage.*']"

EXTRA = {
    "posixpath.relpath": ("import posixpath", "posixpath.relpath('/srv/app/static/css/site.css', '/srv/app')"),
    "posixpath.relpath(rel)": ("import posixpath", "posixpath.relpath('static/css/site.css')"),
    "posixpath.abspath": ("import posixpath", "posixpath.abspath('/srv/app/./static//css/site.css')"),
    "posixpath.splitext": ("import posixpath", "posixpath.splitext('/srv/app/static/site.css')"),
    "fnmatch.filter": (f"import fnmatch; names = {NAMES}", "fnmatch.filter(names, '*.py')"),
    "fnmatch 12 patterns": (f"import fnmatch; pats = {PATS}", "any(fnmatch.fnmatch('test_api_12.py', p) for p in pats)"),
    "pathlib.relative_to": ("import pathlib; p = pathlib.PurePosixPath('/srv/app/static/css/site.css')",
                            "p.relative_to('/srv/app')"),
}
K.CELLS.update(EXTRA)
PATHS_CELLS = ["posixpath.join", "posixpath.join_fb", "fnmatch.fnmatch", "pathlib.join", *EXTRA]

if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "run" and "--cells" not in sys.argv:
        sys.argv += ["--cells", ",".join(PATHS_CELLS)]
    if len(sys.argv) > 1 and sys.argv[1] == "run":
        del sys.argv[1]  # kernel_scaling's default command is `run`
    K.main()
