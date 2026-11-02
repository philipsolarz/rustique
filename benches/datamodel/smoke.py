import collections, contextlib, dataclasses, enum, sys, typing
import rustique
from rustique._rustique import _accel
rustique.stats(True)
rustique.enable(only=["enum", "generated", "contextlib"])
st = rustique.status()
for t in st["targets"]:
    if t["group"] in ("enum", "generated", "contextlib"):
        print(t["name"], t["state"], t.get("reason"))

class Color(enum.Enum):
    RED = 1
    GREEN = 2
class Perm(enum.Flag):
    R = 4
    W = 2
    X = 1
class S(enum.StrEnum):
    A = "a"
print(Color(2), Color(Color.RED), Color.RED.value, Color.RED.name, hash(Color.RED) == hash("RED"))
print(Perm.R | Perm.W, (Perm.R | Perm.W) & Perm.R, Perm.R in (Perm.R | Perm.W), ~Perm.R, S("a"), S.A.value)
NT = collections.namedtuple("NT", "a b c", defaults=[9])
n = NT(1, 2)
print(n, NT._make([4, 5, 6]), n._replace(b=7), NT(a=1, b=2, c=3))
TI = typing.NamedTuple("TI", [("x", int), ("y", int)])
print(TI(1, 2))
@dataclasses.dataclass(frozen=True)
class F:
    x: int
    y: list = dataclasses.field(default_factory=list)
    z: int = 3
print(F(1), F(x=1, z=5), F(1, [2]))
@dataclasses.dataclass(frozen=True, slots=True, kw_only=True)
class E:
    a: int
    b: str = "s"
print(E(a=1), E(a=2, b="t"))
@contextlib.contextmanager
def cm(x, y=2):
    yield x + y
with cm(1) as v:
    print("cm", v)
print(cm(1).__doc__, cm.__wrapped__)
print(_accel.datamodel_dynamic_sites())
s = rustique.stats()
for k, v in s.items() if isinstance(s, dict) else []:
    if any(g in k for g in ("enum", "namedtuple", "dataclasses.__init__", "contextmanager")):
        print(k, v)
