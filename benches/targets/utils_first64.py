"""Cost of the first 64 fallback calls (before the registry suspends a target that keeps
falling back): Ir of `for _ in range(64): <call>` in a fresh process, plain vs enabled,
per call."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _callgrind import region  # noqa: E402

OWN = '''import random
class OwnBits(random.Random):
    def getrandbits(self, k):
        return super().getrandbits(k)
class OwnRandom(random.Random):
    def random(self):
        return super().random()
ob = OwnBits(9); orr = OwnRandom(3); sysr = random.SystemRandom(); L6 = list('abcdef')'''

CELLS = [
    ("random", "import random, collections; r = random.Random(1); L = collections.UserList([1, 2, 3])", "r.choice(L)"),
    ("random", OWN, "ob.randint(1, 6)"),
    ("random", OWN, "ob.choice(L6)"),
    ("random", OWN, "ob.shuffle(L6)"),
    ("random", OWN, "orr.randint(1, 6)"),
    ("random", OWN, "orr.choices(L6)"),
    ("random", OWN, "sysr.randint(1, 6)"),
    ("random", OWN, "sysr.choice(L6)"),
    ("random", "import random; from fractions import Fraction; r = random.Random(1); W = [Fraction(1)] * 3",
     "r.choices('abc', W)"),
    ("textwrap", "import textwrap\nclass S(str): pass\nT = S('a b c')", "textwrap.dedent(T)"),
    ("urllib", "import urllib.parse as up\nclass S(str): pass\nT = S('a b')", "up.quote(T)"),
    ("statistics", "import statistics as st; from decimal import Decimal; D = [Decimal(1), Decimal(2)]", "st.mean(D)"),
]

# `--n 63`: the same calls without the 64th, which suspends the target (its one-time cost).
args = sys.argv[1:]
N = 64
if "--n" in args:
    i = args.index("--n")
    N = int(args[i + 1])
    del args[i : i + 2]
only = args
for group, setup, stmt in CELLS:
    if only and group not in only:
        continue
    body = f"for _ in range({N}):\n    {stmt}"
    plain = region(body, setup=setup)
    en = region(body, setup=setup, enable=[group])
    print(f"{group:10s} {stmt:24s} plain={plain / N:9.0f} Ir/call  enabled={en / N:9.0f}  "
          f"ratio={en / plain:.3f}  (+{(en - plain) / N:.0f} Ir/call)", flush=True)
