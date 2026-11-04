"""Differential check of the CALL_KW instantiation override: prints results, error messages
and tracebacks for keyword instantiation shapes; run plain and with the override and diff.

    python kwtest.py            # plain
    python kwtest.py MOD        # with MOD's CALL_KW_NON_PY swapped in
"""
import sys, traceback, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
if len(sys.argv) > 1:
    import rqpatch
    rqpatch.swap("/work/w2-interp/rqeval", sys.argv[1], ["CALL_KW_NON_PY"])


class P:
    def __init__(self, a, b=0, *args, c=3, **kw):
        self.a, self.b, self.args, self.c, self.kw = a, b, args, c, kw
    def __repr__(self):
        return f"P({self.a!r}, {self.b!r}, {self.args!r}, {self.c!r}, {self.kw!r})"


class Bad:
    def __init__(self, a, b=0):
        return 5


class Raises:
    def __init__(self, a, b=0):
        raise ValueError(f"bad {a} {b}")


class Slots:
    __slots__ = ("a", "b")
    def __init__(self, a, b=0):
        self.a = a; self.b = b
    def __repr__(self):
        return f"Slots({self.a}, {self.b})"


class Sub(P):
    pass


def deep(n):
    return deep(n + 1) if n < 10 ** 6 else P(1, b=n)


def show(label, f):
    try:
        r = f()
        print(label, "->", repr(r))
    except BaseException as e:
        tb = traceback.format_exception(e)
        print(label, "!!", type(e).__name__, str(e)[:200], "| frames:",
              [l.strip().split(",")[-1] for l in tb if l.strip().startswith("File")])


for _ in range(3):   # warm the specializer
    show("kw", lambda: P(1, b=2))
    show("kw extra", lambda: P(1, b=2, z=9))
    show("kw only", lambda: P(a=1, c=4))
    show("kw varargs", lambda: P(1, 2, 3, c=4))
    show("bad return", lambda: Bad(1, b=2))
    show("raises", lambda: Raises(1, b=2))
    show("unexpected kw", lambda: Slots(1, zz=2))
    show("missing", lambda: Slots(b=2))
    show("dup", lambda: Slots(1, a=2))
    show("slots", lambda: Slots(1, b=2))
    show("subclass", lambda: Sub(1, b=2))
    show("recursion", lambda: deep(0))
print("recursion limit", sys.getrecursionlimit())
