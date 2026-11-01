"""Native tier of the takeover (docs/TAKEOVER.md §5): the source rewrite, the import hook,
the runner and the decorator.

The rewrite must preserve CPython semantics of everything except the *type* of the
containers that displays, comprehensions and constructor calls produce: evaluation order,
starred forms, duplicate dict keys, comprehension scoping, `__class__` cells, tracebacks
and positions, annotations, and shadowed names.
"""

from __future__ import annotations

import ast
import asyncio
import dis
import importlib
import os
import subprocess
import sys
import textwrap
import traceback

import pytest

import rustique as rs
from rustique._takeover import _hook, _rewrite

L, D, S = rs.List, rs.Dict, rs.Set


def run(src: str, *, native: bool = True, filename: str = "<rewrite-test>") -> dict:
    """Execute `src` as a module body, rewritten or not; return its namespace."""
    src = textwrap.dedent(src)
    ns: dict = {"__name__": "rewrite_test"}
    if native:
        ns.update(_rewrite.helpers())
        code = _rewrite.compile_rewritten(src, filename)
    else:
        code = compile(src, filename, "exec", dont_inherit=True)
    exec(code, ns)
    return ns


def both(src: str, *names: str):
    """(plain, native) values of `names` after running `src` both ways."""
    a, b = run(src, native=False), run(src)
    return [a[n] for n in names], [b[n] for n in names]


def builtin(v):
    """Normalise Rustique containers to builtins, recursively, for comparisons."""
    if isinstance(v, L):
        return [builtin(x) for x in v]
    if isinstance(v, D):
        return {builtin(k): builtin(x) for k, x in v.items()}
    if isinstance(v, S):
        return {builtin(x) for x in v}
    if isinstance(v, list):
        return [builtin(x) for x in v]
    if isinstance(v, tuple):
        return tuple(builtin(x) for x in v)
    if isinstance(v, dict):
        return {builtin(k): builtin(x) for k, x in v.items()}
    return v


# --- what gets rewritten ------------------------------------------------------------------


def test_displays_comprehensions_and_calls_build_rustique_types():
    ns = run("""
        a, b = 1, 2
        l = [a, b]; d = {a: b}; s = {a, b}
        lc = [x for x in range(3)]; dc = {x: x for x in range(3)}; sc = {x for x in range(3)}
        cl = list(range(3)); cd = dict(k=1); cs = set([1, 1]); ce = list(); de = dict(); se = set()
        nested = [[1], {2: [3]}, {4}]
    """)
    for name, t in [("l", L), ("d", D), ("s", S), ("lc", L), ("dc", D), ("sc", S), ("cl", L), ("cd", D),
                    ("cs", S), ("ce", L), ("de", D), ("se", S), ("nested", L)]:
        assert type(ns[name]) is t, name
    assert type(ns["nested"][0]) is L and type(ns["nested"][1]) is D and type(ns["nested"][1][2]) is L
    assert builtin(ns["nested"]) == [[1], {2: [3]}, {4}]
    assert builtin(ns["cs"]) == {1}


def test_untouched_positions_keep_cpython_code():
    src = """
        def f(x, y):
            for v in [1, 2, 3]:
                pass
            g(*[x, y], **{"k": x})
            return x in [1, 2, 3], x not in {1, 2}, [a for a in (1, 2)]
        def g(*a, **k):
            return a, k
    """
    tree = _rewrite.rewrite_source(textwrap.dedent(src))
    text = ast.unparse(tree)
    assert "x in [1, 2, 3]" in text and "x not in {1, 2}" in text and "for v in [1, 2, 3]" in text
    assert "g(*[x, y], **{'k': x})" in text
    assert "__rs_list__([a for a in (1, 2)])" in text
    ns = run(src)
    f = ns["f"]
    consts = [c for c in f.__code__.co_consts if isinstance(c, (tuple, frozenset))]
    assert (1, 2, 3) in consts and frozenset({1, 2}) in consts  # CPython's constant folding kept
    assert builtin(f(1, 2)) == (True, False, [1, 2])


def test_targets_patterns_and_starred_assignment_untouched():
    ns = run("""
        [a, b] = 1, 2
        (c, [d, e]) = 3, (4, 5)
        for [f, g] in [(6, 7)]:
            pass
        h, *rest = [1, 2, 3]
        del [a]
        match [1, 2]:
            case [p, q]:
                matched = (p, q)
        match {"k": 9}:
            case {"k": v, **others}:
                mapped = (v, others)
    """)
    assert (ns["b"], ns["c"], ns["d"], ns["e"], ns["f"], ns["g"]) == (2, 3, 4, 5, 6, 7)
    assert "a" not in ns
    # Starred assignment targets are built by UNPACK_EX: always builtin lists.
    assert type(ns["rest"]) is list and ns["rest"] == [2, 3]
    assert ns["matched"] == (1, 2)
    assert ns["mapped"][0] == 9 and type(ns["mapped"][1]) is dict  # `**others` is built by CPython


# --- evaluation order and display semantics ---------------------------------------------


ORDER = """
log = []
def t(x):
    log.append(x)
    return x
r_list = [t(1), *[t(2), t(3)], t(4)]
r_dict = {t("k1"): t("v1"), **{t("k2"): t("v2")}, t("k3"): t("v3")}
r_set = {t(5), *(t(6), t(7))}
r_comp = [t(i) for i in range(3) if t(-i - 1)]
r_dcomp = {t(i): t(i * 10) for i in range(2)}
r_call = list(t([8, 9]))
r_kw = dict(t({"a": 1}), b=t(2))
"""


def test_evaluation_order_is_cpython_order():
    plain, native = both(ORDER, "log", "r_list", "r_dict", "r_set", "r_comp", "r_dcomp", "r_call", "r_kw")
    assert plain[0] == native[0]
    assert [builtin(v) for v in plain[1:]] == [builtin(v) for v in native[1:]]


def test_errors_while_building_propagate_and_stop_evaluation():
    src = """
        log = []
        def t(x):
            log.append(x)
            return x
        try:
            r = [t(1), 1 // 0, t(2)]
        except ZeroDivisionError as e:
            err = repr(e)
        try:
            r = {t("a"): [], t("b"): 2}[t("c")]
        except KeyError as e:
            err2 = repr(e)
        try:
            r = {[]}
        except TypeError as e:
            err3 = str(e)
        try:
            r = list(t(5))
        except TypeError as e:
            err4 = str(e)
    """
    plain, native = both(src, "log", "err", "err2", "err3", "err4")
    assert builtin(plain[:3]) == builtin(native[:3]) and plain[4] == native[4]
    # Documented deviation: the inner `[]` is itself rewritten, so the message names it.
    assert plain[3] == "cannot use 'list' as a set element (unhashable type: 'list')"
    assert native[3] == "cannot use 'rustique.List' as a set element (unhashable type: 'List')"


def test_duplicate_keys_keep_first_key_object_and_last_value():
    ns = run("""
        class K:
            def __init__(self, tag):
                self.tag = tag
            def __hash__(self):
                return 1
            def __eq__(self, other):
                return isinstance(other, K)
        k1, k2 = K("first"), K("second")
        d = {k1: "a", k2: "b"}
        nums = {1: "int", 1.0: "float", True: "bool"}
        comp = {k: i for i, k in enumerate([1, 1.0, True])}
        merged = {**{k1: 1}, k2: 2}
    """)
    d = ns["d"]
    assert type(d) is D and len(d) == 1
    (key,) = list(d.keys())
    assert key is ns["k1"] and d[ns["k2"]] == "b"
    (nk,) = list(ns["nums"].keys())
    assert type(nk) is int and ns["nums"][1] == "bool"
    (ck,) = list(ns["comp"].keys())
    assert type(ck) is int and ns["comp"][1] == 2
    (mk,) = list(ns["merged"].keys())
    assert mk is ns["k1"] and ns["merged"][mk] == 2


def test_starred_forms():
    src = """
        a, b = [1, 2], (3, 4)
        x = [*a, *b, *range(2)]
        y = {*a, *b}
        z = {**{"p": 1}, **dict(q=2), "r": 3}
        w = [*()]
        def f(*args, **kw):
            return args, kw
        call = f(*[1, 2], *a, **{"k": 1}, **z)
    """
    plain, native = both(src, "x", "y", "z", "w", "call")
    assert [builtin(v) for v in plain] == [builtin(v) for v in native]
    ns = run(src)
    assert type(ns["x"]) is L and type(ns["y"]) is S and type(ns["z"]) is D and type(ns["w"]) is L


# --- scoping ------------------------------------------------------------------------------


def test_comprehension_scoping():
    src = """
        i = "outer"
        squares = [i * i for i in range(4)]
        after = i
        def f():
            vals = [(last := v) for v in range(3)]
            return vals, last
        walrus = f()
        class C:
            base = [1, 2]
            doubled = [v * 2 for v in base]        # outermost iterable: class scope
            try:
                hidden = [base for _ in range(1)]  # class names are not visible inside
            except NameError as e:
                err = str(e)
        nested = [[x * y for y in range(2)] for x in range(3)]
    """
    plain, native = both(src, "squares", "after", "walrus", "C", "nested")
    assert plain[1] == native[1] == "outer"
    assert builtin(plain[0]) == builtin(native[0])
    assert builtin(plain[2]) == builtin(native[2])
    assert builtin(plain[3].doubled) == builtin(native[3].doubled)
    assert plain[3].err == native[3].err
    assert builtin(plain[4]) == builtin(native[4])


def test_class_cell_and_super():
    ns = run("""
        class Base:
            def items(self):
                return ["base"]
        class Child(Base):
            def items(self):
                return super().items() + ["child"]
            def cls(self):
                return __class__
            @classmethod
            def make(cls):
                return [cls.__name__]
    """)
    c = ns["Child"]()
    assert builtin(c.items()) == ["base", "child"]
    assert c.cls() is ns["Child"]
    assert builtin(ns["Child"].make()) == ["Child"]


def test_shadowed_names_are_respected():
    src = """
        def local_shadow(x):
            list = tuple
            return list(x)
        def param_shadow(dict):
            return dict([("a", 1)])
        def plain(x):
            return list(x)
    """
    ns = run(src)
    assert ns["local_shadow"]([1, 2]) == (1, 2)
    from collections import OrderedDict
    assert type(ns["param_shadow"](OrderedDict)) is OrderedDict
    # The module binds `list`/`dict` somewhere, so constructor calls are not rewritten at all.
    assert type(ns["plain"]([1])) is list
    # A module that never binds the names gets rewritten calls ...
    ns = run("""
        def f(x):
            return list(x), set(x)
    """)
    assert type(ns["f"]([1])[0]) is L and type(ns["f"]([1])[1]) is S
    # ... which still honour shadowing introduced from outside at run time.
    ns["list"] = tuple
    assert ns["f"]([1, 2])[0] == (1, 2)
    del ns["list"]
    import builtins as b
    orig = b.set
    try:
        b.set = frozenset
        assert ns["f"]([1])[1] == frozenset({1})
    finally:
        b.set = orig


def test_module_level_bindings_disable_call_rewrite():
    for binding in ("list = tuple", "def list(x): return x", "from builtins import list",
                    "import json as list", "for list in [1]: pass", "with open(__file__) as list: pass"):
        names = _rewrite._bound_names(ast.parse(binding))
        assert "list" in names, binding


# --- positions, tracebacks, annotations ---------------------------------------------------


def test_tracebacks_and_positions_unchanged(tmp_path):
    src = textwrap.dedent("""
        def boom(n):
            data = [
                1,
                10 // n,
            ]
            return {k: 1 // n for k in data}

        def call(x):
            return list(
                x
            )
    """)
    path = str(tmp_path / "tb_mod.py")
    out = []
    for native in (False, True):
        ns = {"__name__": "tb_mod"}
        if native:
            ns.update(_rewrite.helpers())
            code = _rewrite.compile_rewritten(src, path)
        else:
            code = compile(src, path, "exec")
        exec(code, ns)
        for fn, arg in ((ns["boom"], 0), (ns["call"], 5)):
            try:
                fn(arg)
            except Exception as e:  # noqa: BLE001
                out.append((native, "".join(traceback.format_exception(e)[1:])))
        lines = sorted({ln for _, _, ln in ns["boom"].__code__.co_lines() if ln})
        out.append((native, lines))
    plain = [v for n, v in out if not n]
    native = [v for n, v in out if n]
    assert plain == native


def test_annotations_untouched():
    ns = run("""
        def f(a: [int] = [], *, b: {str: int} = {}) -> [list]:
            return a
        class C:
            x: [int] = [1]
            y: {1} = set()
        type Alias = [int]
    """)
    import annotationlib

    ann = annotationlib.get_annotations(ns["f"])
    assert type(ann["a"]) is list and type(ann["b"]) is dict and type(ann["return"]) is list
    cann = annotationlib.get_annotations(ns["C"])
    assert type(cann["x"]) is list and type(cann["y"]) is set
    assert type(ns["Alias"].__value__) is list
    # Defaults and class attribute values are values: they are rewritten.
    assert type(ns["f"].__defaults__[0]) is L and type(ns["f"].__kwdefaults__["b"]) is D
    assert type(ns["C"].x) is L and type(ns["C"].y) is S


def test_generators_async_and_lambdas():
    ns = run("""
        def gen():
            got = [(yield 1), (yield 2)]
            return got
        async def agen_items():
            for i in range(3):
                yield i
        async def co():
            async def one():
                return 1
            return [await one(), *[x async for x in agen_items()]], {x async for x in agen_items()}
        lam = lambda n=[]: [n, [n]]
    """)
    g = ns["gen"]()
    assert next(g) == 1 and g.send("a") == 2
    with pytest.raises(StopIteration) as stop:
        g.send("b")
    assert type(stop.value.value) is L and builtin(stop.value.value) == ["a", "b"]
    lst, st = asyncio.run(ns["co"]())
    assert type(lst) is L and builtin(lst) == [1, 0, 1, 2] and type(st) is S
    assert type(ns["lam"]()) is L and type(ns["lam"].__defaults__[0]) is L


def test_future_import_and_docstrings_survive():
    ns = run('''
        """Module doc."""
        from __future__ import annotations
        def f() -> [int]:
            """Function doc."""
            return [1]
    ''')
    assert ns["__doc__"] == "Module doc." and ns["f"].__doc__ == "Function doc."
    assert ns["f"].__annotations__ == {"return": "[int]"}
    assert type(ns["f"]()) is L


# --- import hook ---------------------------------------------------------------------------


@pytest.fixture
def pkg(tmp_path, monkeypatch):
    root = tmp_path / "pkgs"
    (root / "opted").mkdir(parents=True)
    (root / "opted" / "__init__.py").write_text("")
    (root / "opted" / "mod.py").write_text(textwrap.dedent("""
        import plainmod
        def make():
            return [1, 2], {"a": 1}, {1}, [i for i in range(2)], list((1,))
        def handoff():
            return plainmod.describe([1, 2])
    """))
    (root / "plainmod.py").write_text(textwrap.dedent("""
        def make():
            return [1, 2], {"a": 1}, {1}
        def describe(x):
            return type(x).__name__, isinstance(x, list), [type(v).__name__ for v in x]
    """))
    monkeypatch.syspath_prepend(str(root))
    yield root
    _hook.uninstall()
    for name in ("opted", "opted.mod", "plainmod"):
        sys.modules.pop(name, None)


def test_import_hook_rewrites_only_matching_modules(pkg):
    _hook.install(("opted.*",))
    mod = importlib.import_module("opted.mod")
    plain = importlib.import_module("plainmod")
    assert [type(v) for v in mod.make()] == [L, D, S, L, L]
    assert [type(v) for v in plain.make()] == [list, dict, set]
    # A native value handed to non-opted code is a Rustique container there (documented).
    assert mod.handoff() == ("List", False, ["int", "int"])
    cached = os.listdir(pkg / "opted" / "__pycache__")
    assert any(f".opt-{_hook.CACHE_TAG}.pyc" in f for f in cached)


def test_rewritten_bytecode_cache_is_separate(pkg):
    _hook.install(("opted.*",))
    importlib.import_module("opted.mod")
    _hook.uninstall()
    # A fresh interpreter without the hook gets builtins, from its own cache entry.
    code = "import opted.mod as m; print([type(v).__name__ for v in m.make()])"
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True,
                         env={**os.environ, "PYTHONPATH": str(pkg)})
    assert out.stdout.strip() == "['list', 'dict', 'set', 'list', 'list']"
    _hook.install(("opted.*",))
    sys.modules.pop("opted.mod", None)
    assert type(importlib.import_module("opted.mod").make()[0]) is L


def test_enable_native_and_disable(pkg):
    st = rs.enable(native="opted.*")
    try:
        assert st["process"]["native"] == ("opted.*",) and st["process"]["enabled"]
        assert type(importlib.import_module("opted.mod").make()[0]) is L
    finally:
        rs.disable()
    from rustique._takeover import status

    assert status()["process"]["native"] == () and not status()["process"]["enabled"]


# --- runner ----------------------------------------------------------------------------------


def _runner(*args, cwd):
    return subprocess.run([sys.executable, "-m", "rustique", *args], capture_output=True, text=True, cwd=cwd,
                          env={**os.environ, "PYTHONPATH": str(cwd)})


def test_runner_script_module_and_command(tmp_path):
    (tmp_path / "helper.py").write_text("def make():\n    return [1], {1: 2}\n")
    (tmp_path / "app.py").write_text(textwrap.dedent("""
        import sys, helper
        print(__name__, sys.argv[1:], type([]).__name__, " ".join(type(v).__name__ for v in helper.make()))
    """))
    out = _runner("app.py", "a", "--b", cwd=tmp_path)
    assert out.returncode == 0, out.stderr
    assert out.stdout.split("\n")[0] == "__main__ ['a', '--b'] list list dict"
    out = _runner("--native", "helper", "app.py", "x", cwd=tmp_path)
    assert out.stdout.split("\n")[0] == "__main__ ['x'] List List Dict", out.stderr
    out = _runner("--native-main", "app.py", cwd=tmp_path)
    assert out.stdout.split("\n")[0] == "__main__ [] List list dict", out.stderr
    out = _runner("--native", "app", "-m", "app", "q", cwd=tmp_path)
    assert out.stdout.split("\n")[0] == "__main__ ['q'] List list dict", out.stderr
    out = _runner("--native", "zzz", "-c", "print(type({}).__name__, sorted([2, 1]))", cwd=tmp_path)
    assert out.stdout.strip() == "Dict [1, 2]", out.stderr


def test_runner_traceback_points_at_user_code(tmp_path):
    (tmp_path / "bad.py").write_text("x = [\n    1,\n    1 // 0,\n]\n")
    for args in (("bad.py",), ("--native-main", "bad.py")):
        out = _runner(*args, cwd=tmp_path)
        assert out.returncode == 1
        assert 'bad.py", line 3, in <module>' in out.stderr and "ZeroDivisionError" in out.stderr


# --- decorator (code-object rewriting of already-compiled functions) ------------------------


def test_native_decorator_on_functions_closures_and_methods():
    factor = 3

    @rs.native
    def scaled(xs, extra=[]):
        return [x * factor for x in xs] + list(extra)

    assert type(scaled([1, 2])) is L and builtin(scaled([1, 2])) == [3, 6]
    assert type(scaled.__defaults__[0]) is list  # evaluated before rewriting
    assert scaled.__name__ == "scaled" and scaled.__code__.co_freevars == ("factor",)

    class Base:
        def get(self):
            return ["base"]

    class Child(Base):
        @rs.native
        def get(self):
            return super().get() + ["child"]

    assert builtin(Child().get()) == ["base", "child"]

    @rs.native
    class Bag:
        items = []  # class body already ran: stays a builtin list

        def fresh(self):
            return {"k": [1]}

        @staticmethod
        def s():
            return {1}

    assert type(Bag.items) is list and type(Bag().fresh()) is D and type(Bag.s()) is S


def test_native_decorator_keeps_line_numbers():
    @rs.native
    def fails():
        x = 1
        return [x // 0]

    with pytest.raises(ZeroDivisionError) as info:
        fails()
    tb = info.tb
    while tb.tb_next:
        tb = tb.tb_next
    assert tb.tb_lineno == fails.__code__.co_firstlineno + 3  # decorator, def, x = 1, return
    assert dis.Bytecode(fails).first_line == fails.__code__.co_firstlineno


def test_native_decorator_rejects_lambdas():
    with pytest.raises(TypeError):
        rs.native(lambda: [1])


# --- mapping designed for Tuple / FrozenSet -------------------------------------------------


def test_custom_targets_including_tuple_and_frozenset_slots():
    class MyList(list):
        pass

    class MyTuple(tuple):
        pass

    class MyFrozen(frozenset):
        pass

    targets = {"list": MyList, "dict": dict, "set": set, "tuple": MyTuple, "frozenset": MyFrozen}
    src = "a = [1]\nb = tuple([1, 2])\nc = frozenset({1})\nd = (1, 2)\n"
    tree, n = _rewrite.rewrite_tree(ast.parse(src), kinds={"list", "dict", "set", "tuple", "frozenset"})
    ns = dict(_rewrite.helpers(targets))
    exec(compile(tree, "<t>", "exec"), ns)
    assert type(ns["a"]) is MyList and type(ns["b"]) is MyTuple and type(ns["c"]) is MyFrozen
    assert type(ns["d"]) is tuple  # tuple displays are never rewritten
