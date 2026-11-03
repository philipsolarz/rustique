"""T9 check: what `import fnmatch` / `import urllib.parse` cost with Rustique enabled (first-call
validation), and how much of it is the `re` group's lazy install (callgrind regions)."""
import sys
sys.path.insert(0, "benches/targets")
from _callgrind import region

E = {"RUSTIQUE_VALIDATE": "first-call"}
for mod in ("fnmatch", "urllib.parse", "re"):
    plain = region(f"import {mod}", "import rustique", env=E)
    allg = region(f"import {mod}", "import rustique; rustique.enable()", env=E)
    nore = region(f"import {mod}", "import rustique; rustique.enable(exclude=['re'])", env=E)
    print(f"import {mod:13s} plain {plain:>12,}  enabled {allg:>12,} ({allg / plain:.3f}, {allg - plain:+,})  "
          f"enabled without re {nore:>12,} ({nore / plain:.3f}, {nore - plain:+,})", flush=True)
