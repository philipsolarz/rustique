"""Callgrind cells for the `random` group (run with benches/targets/utils_cg.py random_cells)."""

COMMON = """
import random, secrets
rng = random.Random(12345)
class Sub(random.Random):
    pass
sub = Sub(7)
class OwnRandom(random.Random):
    def random(self):
        return super().random()
own = OwnRandom(3)
sysr = random.SystemRandom()
L6 = ['payments', 'search', 'platform', 'mobile', 'data', 'growth']
T4 = ('low', 'medium', 'high', 'critical')
W = 'alpha beta gamma delta epsilon release deploy rollback latency throughput customer invoice backlog sprint incident outage database cache queue worker migration feature flag rollout canary regression benchmark memory leak timeout retry checkout payment search index'.split()
L100 = list(range(100))
L1000 = list(range(1000))
from decimal import Decimal
from fractions import Fraction
randint = random.randint
import collections
deque6 = collections.deque(L6)
class Sh(random.Random):
    pass
shadow = Sh(5)
shadow.getrandbits = shadow.getrandbits
class OwnBits(random.Random):
    def getrandbits(self, k):
        return super().getrandbits(k)
own_bits = OwnBits(9)
FW = [Fraction(1)] * 6
LD = collections.UserList(L6)
"""

INSTALL = """
import rustique
rustique.enable(only=["random"])
"""

CELLS = {
    # fast paths, small realistic inputs
    "randint(1, 6) [module]": "randint(1, 6)",
    "rng.randint(3, 7)": "rng.randint(3, 7)",
    "rng.randint(0, 2**40)": "rng.randint(0, 1099511627776)",
    "rng.randrange(10)": "rng.randrange(10)",
    "rng.randrange(0, 100, 5)": "rng.randrange(0, 100, 5)",
    "rng.choice(L6)": "rng.choice(L6)",
    "rng.choice(T4)": "rng.choice(T4)",
    "rng.choices(W, k=40)": "rng.choices(W, k=40)",
    "rng.choices(T4, weights=[50,30,15,5])": "rng.choices(T4, weights=[50, 30, 15, 5])",
    "rng.choices(L6)": "rng.choices(L6)",
    "rng.sample(W, 5)": "rng.sample(W, 5)",
    "rng.sample(range(10**6), 20)": "rng.sample(range(1000000), 20)",
    "rng.shuffle(L100)": "rng.shuffle(L100)",
    "sub.randint(1, 6) [subclass]": "sub.randint(1, 6)",
    "sub.choice(L6) [subclass]": "sub.choice(L6)",
    # generic paths / fallbacks
    "own.randint(1, 6) [random() overridden]": "own.randint(1, 6)",
    "own.choice(L6) [random() overridden]": "own.choice(L6)",
    "own_bits.randint(1, 6) [Python getrandbits]": "own_bits.randint(1, 6)",
    "own_bits.choice(L6) [Python getrandbits]": "own_bits.choice(L6)",
    "own_bits.shuffle(L6) [Python getrandbits]": "own_bits.shuffle(L6)",
    "own_bits.choices(L6) [Python getrandbits]": "own_bits.choices(L6)",
    "own.choices(L6) [Python random]": "own.choices(L6)",
    "own.sample(L100, 2) [Python random]": "own.sample(L100, 2)",
    "rng.randint(True, 6) [bool]": "rng.randint(True, 6)",
    "rng.randint(1, 2**70) [big]": "rng.randint(1, 1180591620717411303424)",
    "rng.choice('abcdef') [str]": "rng.choice('abcdef')",
    "rng.choice(range(10))": "rng.choice(range(10))",
    "rng.randint(a=1, b=6) [kwargs]": "rng.randint(a=1, b=6)",
    "rng.randint(0, 2**64)": "rng.randint(0, 18446744073709551616)",
    "rng.choice(deque6) [C sequence]": "rng.choice(deque6)",
    "shadow.choice(L6) [instance attr shadows getrandbits]": "shadow.choice(L6)",
    "own_bits.sample(L6, 2) [Python getrandbits]": "own_bits.sample(L6, 2)",
    "rng.choices(L6, k=3) [Fraction weights: fallback]": "rng.choices(L6, FW, k=3)",
    "rng.choice(UserList) [Python sequence: fallback]": "rng.choice(LD)",
    "rng.sample(L6, k=2, counts=[1]*6) [fallback]": "rng.sample(L6, k=2, counts=[1, 1, 1, 1, 1, 1])",
    "rng.choices(L6, cum_weights=[1,2,3,4,5,6])": "rng.choices(L6, cum_weights=[1, 2, 3, 4, 5, 6])",
}

N = {"rng.shuffle(L100)": 20, "rng.sample(range(10**6), 20)": 40, "rng.choices(W, k=40)": 40}
