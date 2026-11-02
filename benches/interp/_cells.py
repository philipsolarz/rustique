"""Micro cells of the interp tier's overrides: each function runs one operation shape a fixed
number of times. Used for PGO training (pgo_train.py) and callgrind cells (cells_cg.py).
Fast paths, the fallbacks of each override, and the specialized LOAD_ATTR forms (our verbatim
copies run instead of the core's when the family is swapped)."""
import abc
import dataclasses
import functools


class A:
    cls_attr = 7

    def __init__(self, x):
        self.x = x

    def m(self):
        return self.x


class B(A):
    pass


class C(A):
    pass


class Meta(type):
    pass


class MA(metaclass=Meta):
    pass


class Other:
    pass


class WithGetattr:
    def __getattr__(self, name):
        return 1


class Abc(abc.ABC):
    pass


class Slot:
    __slots__ = ("args",)

    def __init__(self, a):
        self.args = a


class Slot2(Slot):
    __slots__ = ()


class Slot3(Slot):
    __slots__ = ()


class Cached:
    @functools.cached_property
    def v(self):
        return 3


class Desc:
    def __get__(self, obj, owner=None):
        return 5

    def __set__(self, obj, value):
        pass


class WithDesc:
    d = Desc()


@dataclasses.dataclass(frozen=True)
class Frozen:
    a: int
    b: str
    c: float


class P:
    def __init__(self, a, b=0):
        self.a = a
        self.b = b


poly = [A(1), B(2), C(3), A(4)]
slots = [Slot(1), Slot2(2), Slot3(3), Slot(4)]
mat = A(1)
mat.__dict__  # materialized dict
cached = Cached()
cached.v
wd = WithDesc()
o_a, o_m, o_o, o_g = A(1), MA(), Other(), WithGetattr()


def poly_attr(n=100):
    s = 0
    for _ in range(n):
        for o in poly:
            s += o.x
    return s


def poly_method(n=100):
    s = 0
    for _ in range(n):
        for o in poly:
            s += o.m()
    return s


def poly_slots(n=100):
    s = 0
    for _ in range(n):
        for o in slots:
            s += o.args
    return s


def mat_dict(n=400):
    s = 0
    for _ in range(n):
        s += mat.x
    return s


def cached_prop(n=400):
    s = 0
    for _ in range(n):
        s += cached.v
    return s


def mono_attr(n=400):
    s = 0
    a = o_a
    for _ in range(n):
        s += a.x + a.m() + a.cls_attr
    return s


def data_descr(n=400):
    s = 0
    for _ in range(n):
        s += wd.d
    return s


def attr_error(n=50):
    s = 0
    for _ in range(n):
        try:
            o_o.missing
        except AttributeError:
            s += 1
    return s


def isinst(n=200):
    s = 0
    for _ in range(n):
        s += isinstance(o_a, A) + isinstance(o_a, B) + isinstance(o_m, MA) + isinstance(o_a, MA)
        s += isinstance(1, A) + isinstance(o_a, (Other, A)) + isinstance(o_g, Other)
        s += isinstance(o_a, Abc)
    return s


def frozen(n=100):
    s = 0
    for i in range(n):
        s += Frozen(i, "x", 2.0).a
    return s


def setattr_other(n=200):
    d = {}
    for i in range(n):
        dict.__setitem__(d, "a", i)
        object.__setattr__(o_a, "x", i)
    return d["a"]


def kw_init(n=200):
    s = 0
    for i in range(n):
        s += P(i, b=1).b + P(i).a
    return s


ALL = [poly_attr, poly_method, poly_slots, mat_dict, cached_prop, mono_attr, data_descr,
       attr_error, isinst, frozen, setattr_other, kw_init]
