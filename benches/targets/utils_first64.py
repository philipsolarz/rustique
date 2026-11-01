"""Cost of the first 64 fallback calls (before the registry suspends a target that keeps
falling back): Ir of `for _ in range(64): <call>` in a fresh process, plain vs enabled,
per call."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

CELLS = [
    ("random", "import random, collections; r = random.Random(1); L = collections.UserList([1, 2, 3])", "r.choice(L)"),
    ("random", "import random; from fractions import Fraction; r = random.Random(1); W = [Fraction(1)] * 3",
     "r.choices('abc', W)"),
    ("textwrap", "import textwrap\nclass S(str): pass\nT = S('a b c')", "textwrap.dedent(T)"),
    ("urllib", "import urllib.parse as up\nclass S(str): pass\nT = S('a b')", "up.quote(T)"),
    ("statistics", "import statistics as st; from decimal import Decimal; D = [Decimal(1), Decimal(2)]", "st.mean(D)"),
]

for group, setup, stmt in CELLS:
    body = f"for _ in range(64):\n    {stmt}"
    plain = region(body, setup=setup)
    en = region(body, setup=setup, enable=[group])
    print(f"{group:10s} {stmt:24s} plain={plain / 64:9.0f} Ir/call  enabled={en / 64:9.0f}  "
          f"ratio={en / plain:.3f}  (+{(en - plain) / 64:.0f} Ir/call)", flush=True)
