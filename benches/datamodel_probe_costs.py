import timeit, dataclasses, collections, sys
src_init = "def __create_fn__(__dataclass_builtins_object__,__dataclass_dflt_z__):\n def __init__(self,x,y,z=__dataclass_dflt_z__):\n  __dataclass_builtins_object__.__setattr__(self,'x',x)\n  __dataclass_builtins_object__.__setattr__(self,'y',y)\n  __dataclass_builtins_object__.__setattr__(self,'z',z)\n return (__init__,)"
def mk():
    @dataclasses.dataclass(frozen=True)
    class P:
        x: int
        y: int
        z: int = 0
    return P
def mk_plain():
    @dataclasses.dataclass
    class P:
        x: int
        y: int
        z: int = 0
    return P
P = mk()
n = 2000
print("dataclass frozen create us", timeit.timeit(mk, number=n)/n*1e6)
print("dataclass plain create us", timeit.timeit(mk_plain, number=n)/n*1e6)
print("compile init src us", timeit.timeit(lambda: compile(src_init, "<string>", "exec"), number=n)/n*1e6)
print("namedtuple create us", timeit.timeit(lambda: collections.namedtuple("NT", "a b c"), number=n)/n*1e6)
print("compile nt lambda us", timeit.timeit(lambda: compile("lambda _cls, a, b, c: _tuple_new(_cls, (a, b, c))", "<string>", "eval"), number=n)/n*1e6)
print("P(1,2,3) us", timeit.timeit(lambda: P(1, 2, 3), number=200000)/200000*1e6)
print("P(x=1,y=2,z=3) us", timeit.timeit(lambda: P(x=1, y=2, z=3), number=200000)/200000*1e6)
print(sys.version)
