#!/usr/bin/env python3
"""Differential check of the w2-machinery prototype kernels: run the same cases plain and
with `proto_enable("all")` in fresh processes and compare every observation (results,
types, identities, exceptions and messages, instance dicts, tracebacks' function names).

    python benches/machinery/check_proto.py            # compare (exit 1 on a difference)
    python benches/machinery/check_proto.py --side proto|plain   # print one side
"""

import json
import subprocess
import sys
import traceback


def observe():
    out = []

    def rec(label, fn):
        try:
            v = fn()
            out.append([label, "ok", type(v).__name__, repr(v)])
        except BaseException as e:  # noqa: BLE001
            tb = [f.name for f in traceback.extract_tb(e.__traceback__)]
            out.append([label, "exc", type(e).__name__, str(e), tb[1:]])

    import collections
    import contextlib
    import dataclasses
    import enum
    import typing
    from dataclasses import dataclass, field

    class Color(enum.Enum):
        RED = 1
        GREEN = 2
        BLUE = "b"

    class Perm(enum.Flag):
        R = 4
        W = 2
        X = 1

    class IP(enum.IntFlag):
        A = 1
        B = 2

    class Weird(enum.Enum):
        A = (1, 2)
        B = 1.5

    class Odd(enum.Enum):
        A = 1

        @property
        def label(self):
            return "L" + self.name

    class HashKey:
        def __init__(self, v):
            self.v = v

        def __hash__(self):
            out.append(["side-effect", "hash", self.v])
            return hash(self.v)

        def __eq__(self, o):
            out.append(["side-effect", "eq", self.v])
            return self.v == getattr(o, "v", o)

    class Obj(enum.Enum):
        K = HashKey(1)

    class MyProp(enum.property):
        pass

    class Meta(enum.EnumType):
        pass

    class Choice(enum.Enum, metaclass=Meta):
        A = 1
        B = 2

    class WithProp(enum.Enum):
        A = 1

        @MyProp
        def info(self):
            return "i" + str(self._value_)

    rec("meta call", lambda: Choice(2))
    rec("meta call miss", lambda: Choice(5))
    rec("myprop", lambda: WithProp.A.info)
    rec("myprop value", lambda: WithProp.A.value)

    m = Color.GREEN
    rec("value", lambda: m.value)
    rec("name", lambda: m.name)
    rec("value str", lambda: Color.BLUE.value)
    rec("label", lambda: Odd.A.label)
    rec("class name attr", lambda: Color.name)
    rec("call int", lambda: Color(2) is Color.GREEN)
    rec("call str", lambda: Color("b"))
    rec("call member", lambda: Color(Color.RED))
    rec("call miss", lambda: Color(7))
    rec("call unhashable", lambda: Color([1]))
    rec("call bool", lambda: Color(True))
    rec("call float", lambda: Color(1.0))
    rec("weird tuple", lambda: Weird((1, 2)))
    rec("weird float", lambda: Weird(1.5))
    rec("obj key", lambda: Obj(HashKey(1)).name)
    rec("obj key2", lambda: Obj(1).name)
    rec("hash", lambda: hash(Color.RED) == hash("RED"))
    rec("dict", lambda: {Color.RED: 1, Color.GREEN: 2}[m])
    rec("or", lambda: Perm.R | Perm.W)
    rec("or again", lambda: (Perm.R | Perm.W) is (Perm.R | Perm.W))
    rec("and", lambda: (Perm.R | Perm.W) & Perm.W)
    rec("and zero", lambda: Perm.R & Perm.W)
    rec("xor", lambda: Perm.R ^ Perm.W ^ Perm.R)
    rec("contains", lambda: Perm.R in (Perm.R | Perm.W))
    rec("contains no", lambda: Perm.X in (Perm.R | Perm.W))
    rec("contains bad", lambda: 1 in Perm.R)
    rec("invert", lambda: ~Perm.R)
    rec("invert2", lambda: ~Perm.R)
    rec("invert3", lambda: ~~Perm.R is Perm.R)
    rec("invert rw", lambda: ~(Perm.R | Perm.W))
    rec("prop explicit", lambda: type(Color).__dict__ and Color.__dict__["value"].__get__(Color.RED, Color))
    rec("prop on class", lambda: Color.value)
    rec("or int", lambda: Perm.R | 2)
    rec("intflag or", lambda: IP.A | IP.B)
    rec("intflag or int", lambda: IP.A | 4)
    rec("intflag and", lambda: (IP.A | IP.B) & IP.B)

    @dataclass
    class P:
        x: int
        y: int
        z: int = 0

    @dataclass(frozen=True, slots=True, kw_only=True)
    class F:
        x: int
        y: list = field(default_factory=list)

    @dataclass
    class L:
        x: int
        tags: list = field(default_factory=list)
        d: dict = field(default_factory=dict)
        e: object = field(default_factory=lambda: "lam")

    @dataclass
    class Sub(P):
        w: int = 9

    def dump(o):
        try:
            return (type(o).__name__, list(vars(o).items()))
        except TypeError:
            return (type(o).__name__, [(s, getattr(o, s)) for s in o.__slots__])

    rec("P pos", lambda: dump(P(1, 2)))
    rec("P kw", lambda: dump(P(y=2, x=1)))
    rec("P mix", lambda: dump(P(1, z=5, y=3)))
    rec("P missing", lambda: P(1))
    rec("P extra", lambda: P(1, 2, 3, 4))
    rec("P dup", lambda: P(1, x=2))
    rec("P badkw", lambda: P(1, 2, q=3))
    rec("P **", lambda: dump(P(**{"x": 1, "y": 2})))
    rec("F kw", lambda: dump(F(x=1)))
    rec("F pos", lambda: F(1))
    rec("F fresh list", lambda: F(x=1).y is not F(x=1).y)
    rec("F set", lambda: setattr(F(x=1), "x", 3))
    rec("L", lambda: dump(L(1)))
    rec("L given", lambda: dump(L(1, [5], {}, "e")))
    rec("Sub", lambda: dump(Sub(1, 2, 3, 4)))
    P.__init__.__defaults__ = (42,)
    rec("P new default", lambda: dump(P(1, 2)))

    NT = collections.namedtuple("NT", "x y z", defaults=(3,))

    class TN(typing.NamedTuple):
        a: int
        b: str = "q"

    class NTSub(NT):
        pass

    rec("NT", lambda: (NT(1, 2), type(NT(1, 2)).__name__))
    rec("NT kw", lambda: NT(y=2, x=1, z=0))
    rec("NT missing", lambda: NT())
    rec("NT extra", lambda: NT(1, 2, 3, 4))
    rec("NT hash", lambda: hash(NT(1, 2)) == hash((1, 2, 3)))
    rec("TN", lambda: TN(1))
    rec("NTSub", lambda: (NTSub(1, 2), type(NTSub(1, 2)).__name__))
    rec("NT make", lambda: NT._make([1, 2, 3]))
    rec("NT make tuple", lambda: NT._make((1, 2, 3)))
    rec("NT make short", lambda: NT._make((1, 2)))
    rec("NT make iter", lambda: NT._make(iter([1, 2, 3])))
    rec("TN make", lambda: TN._make(["x", "y"]))
    rec("NT make sub", lambda: type(NTSub._make([1, 2, 3])).__name__)

    class NTNew(NT):
        def __new__(cls, *a):
            return super().__new__(cls, *a)

    rec("NTNew", lambda: (NTNew(1, 2), type(NTNew(1, 2)).__name__))
    rec("tuple.__new__(NT)", lambda: tuple.__new__(NT, (1, 2, 3)))
    rec("NT.__new__(int)", lambda: NT.__new__(int, 1, 2, 3))
    rec("NT.__new__(NTNew)", lambda: type(NT.__new__(NTNew, 1, 2, 3)).__name__)
    rec("NT replace", lambda: NT(1, 2)._replace(x=9))
    rec("NT replace none", lambda: NT(1, 2)._replace())
    rec("NT replace bad", lambda: NT(1, 2)._replace(q=9))
    rec("NT replace two", lambda: NT(1, 2)._replace(z=7, y=8))
    rec("NTSub replace", lambda: (NTSub(1, 2)._replace(x=5), type(NTSub(1, 2)._replace(x=5)).__name__))

    @dataclass
    class Own:
        x: int

        def __init__(self, x):
            self.x = x * 2

    rec("own init", lambda: dump(Own(3)))
    rec("own init again", lambda: dump(Own(x=4)))

    @contextlib.contextmanager
    def cm(a, b=2):
        """doc of cm"""
        out.append(["side-effect", "enter", a, b])
        yield a + b
        out.append(["side-effect", "exit"])

    @contextlib.contextmanager
    def nodoc():
        yield 1

    def run_cm():
        c = cm(1, b=5)
        r = [type(c).__name__, c.__doc__, c.args, c.kwds, c.func is cm.__wrapped__,
             sorted(vars(c))]
        with c as v:
            r.append(v)
        return r

    rec("cm", run_cm)
    rec("cm nodoc", lambda: nodoc().__doc__)
    rec("cm badargs", lambda: cm())
    rec("cm order", lambda: list(vars(cm(1)).keys()))

    def raising():
        with cm(1):
            raise ValueError("x")

    rec("cm raise", raising)

    import abc
    import functools

    @functools.singledispatch
    def fmt(v):
        return "obj"

    @fmt.register
    def _(v: int):
        return "int"

    @fmt.register(list)
    def _(v):
        out.append(["side-effect", "list impl"])
        return "list"

    class Base(abc.ABC):
        pass

    class Impl:
        pass

    @fmt.register
    def _(v: Base):
        return "Base"

    class SDH:
        @functools.singledispatchmethod
        def handle(self, v):
            return ("obj", type(self).__name__)

        @handle.register
        def _(self, v: str):
            return ("str", v)

        @handle.register
        @classmethod
        def _(cls, v: float):
            return ("float-cm", cls.__name__)

        @handle.register
        @staticmethod
        def _(v: bytes):
            return ("bytes-sm", v)

    sh = SDH()
    for _ in range(2):
        rec("sd int", lambda: fmt(1))
        rec("sd bool", lambda: fmt(True))
        rec("sd str", lambda: fmt("x"))
        rec("sd list", lambda: fmt([]))
        rec("sd dispatch", lambda: fmt.dispatch(int).__name__)
        rec("sd none", lambda: fmt())
        rec("sdm str", lambda: sh.handle("a"))
        rec("sdm obj", lambda: sh.handle(3))
        rec("sdm float", lambda: sh.handle(1.5))
        rec("sdm bytes", lambda: sh.handle(b"z"))
        rec("sdm class access", lambda: SDH.handle(sh, "q"))
        rec("sdm attrs", lambda: (type(sh.handle).__name__, sh.handle.__module__, sh.handle.__doc__,
                                  sh.handle.__qualname__, sorted(vars(sh.handle))))
    Base.register(Impl)  # changes the ABC cache token
    rec("sd abc registered", lambda: fmt(Impl()))
    rec("sd abc registered again", lambda: fmt(Impl()))

    import re
    import warnings

    s1 = "Hello World 123 abc"
    pat = re.compile(r"\w+")
    for _ in range(2):  # second round: cache hits
        rec("re.match", lambda: re.match(r"\w+", s1).span())
        rec("re.match miss", lambda: re.match(r"\d+", s1))
        rec("re.search I", lambda: re.search(r"world", s1, re.I).group())
        rec("re.search kwflags", lambda: re.search(r"WORLD", s1, flags=re.IGNORECASE).span())
        rec("re.search intflags", lambda: re.search(r"WORLD", s1, 2).span())
        rec("re.fullmatch", lambda: re.fullmatch(r"[\w ]+", s1).group())
        rec("re.findall", lambda: re.findall(r"\d", s1))
        rec("re.finditer", lambda: [m.span() for m in re.finditer(r"o", s1)])
        rec("re.match compiled", lambda: re.match(pat, s1).group())
        rec("re.match compiled+flags", lambda: re.match(pat, s1, re.I))
        rec("re.match bytes", lambda: re.match(rb"\w+", b"ab cd").group())
        rec("re.match bad", lambda: re.match(3, s1))
        rec("re.sub", lambda: re.sub(r"o", "0", s1))
        rec("re.sub backref", lambda: re.sub(r"(o)", r"<\1>", s1))
        rec("re.sub callable", lambda: re.sub(r"o", lambda m: "#", s1))
        rec("re.sub count", lambda: re.sub(r"o", "0", s1, count=1))
        rec("re.subn", lambda: re.subn(r"l", "L", s1))
        rec("re.split", lambda: re.split(r"\s+", s1))
        rec("re.split max", lambda: re.split(r"\s+", s1, maxsplit=1))
        rec("re.split flags", lambda: re.split(r"O", s1, flags=re.I))

        def poscount():
            with warnings.catch_warnings(record=True) as w:
                warnings.simplefilter("always")
                r = re.sub(r"o", "0", s1, 1)
                return r, [str(x.message) for x in w]

        rec("re.sub positional count", poscount)
    re.purge()
    rec("re after purge", lambda: re.match(r"\w+", s1).span())
    return out


def main():
    if "--side" in sys.argv:
        side = sys.argv[sys.argv.index("--side") + 1]
        if side == "proto":
            import rustique._rustique as R

            R.proto_enable("all")
        print(json.dumps(observe()))
        return
    res = {}
    for side in ("plain", "proto"):
        r = subprocess.run([sys.executable, __file__, "--side", side], capture_output=True, text=True)
        if r.returncode:
            print(side, "FAILED", r.stderr[-3000:])
            sys.exit(2)
        res[side] = json.loads(r.stdout)
    a, b = res["plain"], res["proto"]
    bad = 0
    for x, y in zip(a, b):
        if x != y:
            bad += 1
            print("DIFF plain:", x, "\n     proto:", y)
    if len(a) != len(b):
        bad += 1
        print("DIFF lengths", len(a), len(b))
    print(f"{len(a)} observations, {bad} differences")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
