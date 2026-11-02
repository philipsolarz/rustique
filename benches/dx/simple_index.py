"""Serve wheels/sdists as a PEP 503 simple index that carries Requires-Python, like PyPI
(docs/targets/dx.md §5: what pip/uv say on interpreters without a wheel).

    python benches/dx/simple_index.py DIST_DIR [--port 8765] [--requires-python ">=3.14,<3.16"]
"""

import argparse
import functools
import html
import http.server
import os
import shutil
import tempfile

ap = argparse.ArgumentParser()
ap.add_argument("dist", nargs="+")
ap.add_argument("--port", type=int, default=8765)
ap.add_argument("--requires-python", default=">=3.14,<3.16")
args = ap.parse_args()
root = tempfile.mkdtemp(prefix="simple-index-")
pkg = os.path.join(root, "simple", "rustique")
os.makedirs(pkg)
for d in args.dist:
    for f in os.listdir(d):
        if f.endswith((".whl", ".tar.gz")):
            shutil.copy(os.path.join(d, f), pkg)
rp = html.escape(args.requires_python)
links = "".join(f'<a href="{f}" data-requires-python="{rp}">{f}</a><br>\n' for f in sorted(os.listdir(pkg)))
with open(os.path.join(pkg, "index.html"), "w") as fh:
    fh.write(f"<!DOCTYPE html><html><body>\n{links}</body></html>\n")
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=root)
print(f"serving {len(os.listdir(pkg)) - 1} files at http://127.0.0.1:{args.port}/simple/", flush=True)
http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler).serve_forever()
