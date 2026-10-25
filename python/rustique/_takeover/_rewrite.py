"""Source rewriting for the opt-in native tier (docs/TAKEOVER.md §5).

A rewritten module builds Rustique containers where its source builds builtin ones:

=====================================  ==========================================
source                                 rewritten
=====================================  ==========================================
``[a, *b]``, ``[e for e in it]``       ``__rs_list__([a, *b])``, ``__rs_list__([...])``
``{k: v, **d}``, ``{k: v for ...}``    ``__rs_dict__({k: v, **d})``, ...
``{a, *b}``, ``{e for e in it}``       ``__rs_set__({a, *b})``, ...
``list(x)``, ``dict(a=1)``, ``set()``  ``(__rs_List__ if list is __rs_builtin_list__
                                       else list)(x)``, ... (runtime-checked)
=====================================  ==========================================

Design rules, each pinned by ``tests/test_takeover_rewrite.py``:

* The builtin display is still built first and handed to the target, so CPython defines
  evaluation order, starred unpacking, duplicate keys (first key object, last value) and
  every error raised while building it. Rewritten nodes carry the source positions of the
  original node, so tracebacks, line numbers and PEP 657 carets are unchanged.
* Only expression values change. Assignment/``del``/``for`` targets, ``match`` patterns,
  annotations (PEP 649/749 evaluates them lazily: they must stay as written), type
  aliases and type parameters are left alone.
* Displays whose object can never be observed keep CPython's code (and its constant
  folding): the right operand of ``in``/``not in``, the iterable of a ``for`` loop or
  comprehension, ``f(*[...])`` and ``f(**{...})``.
* ``list(...)``/``dict(...)``/``set(...)`` calls are rewritten only in modules that never
  bind those names (assignment, ``def``, ``import ... as``, parameters, ...), and the
  rewritten callee still checks at run time that the name *is* the builtin, so shadowing
  from outside keeps working. The check is an expression, not a helper function: no extra
  traceback frame, and the callee is evaluated before the arguments, as in CPython.
* Helper names are dunder names, so class-private name mangling never applies, and they
  are global names, so no function gains or loses a cell (``__class__``/``super()`` work).

The helpers are bound per module by :func:`helpers` (the import hook and the decorator put
them into the module's globals); the generated code only names them, so cached bytecode
does not depend on the targets. ``tuple``/``frozenset`` slots exist for the Rustique
``Tuple``/``FrozenSet`` types; tuple *displays* are not rewritten (tuples are required by
``except``, ``isinstance``, ``%``-formatting, ``__match_args__``, ...).
"""

from __future__ import annotations

import ast
import builtins
from collections.abc import Callable, Iterable
from typing import Any

#: Bump whenever the generated code changes (it is part of the bytecode cache tag).
VERSION = 1

#: Builtin name -> display helper name.
DISPLAY_HELPERS = {"list": "__rs_list__", "dict": "__rs_dict__", "set": "__rs_set__"}
#: Builtin name -> (name bound to the builtin, name bound to the target type).
CALL_NAMES = {k: (f"__rs_builtin_{k}__", f"__rs_{k.capitalize()}__") for k in ("list", "dict", "set", "tuple", "frozenset")}

#: Kinds rewritten by default. ``tuple``/``frozenset`` are only rewritten as calls, and only
#: once a target type exists (see :func:`default_targets`).
DEFAULT_KINDS = frozenset({"list", "dict", "set"})

_DISPLAYS = {ast.List: "list", ast.ListComp: "list", ast.Dict: "dict", ast.DictComp: "dict",
             ast.Set: "set", ast.SetComp: "set"}


def default_targets() -> dict[str, Any]:
    """The mapping builtin name -> Rustique type used by the native tier."""
    import rustique as rs

    targets: dict[str, Any] = {"list": rs.List, "dict": rs.Dict, "set": rs.Set}
    for name, attr in (("tuple", "Tuple"), ("frozenset", "FrozenSet")):
        t = getattr(rs, attr, None)
        if t is not None:
            targets[name] = t
    return targets


def helpers(targets: dict[str, Any] | None = None) -> dict[str, Any]:
    """Global names a rewritten module needs, bound to ``targets``."""
    targets = default_targets() if targets is None else targets
    out: dict[str, Any] = {}
    for kind, target in targets.items():
        if kind in DISPLAY_HELPERS:
            out[DISPLAY_HELPERS[kind]] = _adopter(target)
        builtin_name, target_name = CALL_NAMES[kind]
        out[builtin_name] = getattr(builtins, kind)
        out[target_name] = target
    return out


def _adopter(target: Any) -> Any:
    """The fastest way to turn a fresh builtin temporary into ``target``: an O(1) adoption
    hook when the type offers one (requested from the List/Dict/Set workstreams as
    ``_from_builtin``), else the constructor."""
    return getattr(target, "_from_builtin", target)


def _bound_names(tree: ast.AST) -> set[str]:
    """Every name the module binds anywhere (any scope)."""
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Name) and isinstance(node.ctx, (ast.Store, ast.Del)):
            names.add(node.id)
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(node.name)
        elif isinstance(node, ast.arg):
            names.add(node.arg)
        elif isinstance(node, ast.alias):
            names.add((node.asname or node.name).split(".")[0])
        elif isinstance(node, (ast.Global, ast.Nonlocal)):
            names.update(node.names)
        elif isinstance(node, ast.ExceptHandler) and node.name:
            names.add(node.name)
        elif isinstance(node, (ast.MatchAs, ast.MatchStar)) and node.name:
            names.add(node.name)
        elif isinstance(node, ast.MatchMapping) and node.rest:
            names.add(node.rest)
        elif isinstance(node, (ast.TypeVar, ast.ParamSpec, ast.TypeVarTuple)):
            names.add(node.name)
        elif isinstance(node, ast.TypeAlias) and isinstance(node.name, ast.Name):
            names.add(node.name.id)
    return names


class Rewriter(ast.NodeTransformer):
    """Wrap container displays/comprehensions and constructor calls (see module doc)."""

    def __init__(self, kinds: Iterable[str] = DEFAULT_KINDS, *, calls: Iterable[str] = ()) -> None:
        self.kinds = frozenset(kinds)
        self.calls = frozenset(calls)
        self.changed = 0

    # -- helpers -------------------------------------------------------------------------
    def _wrap(self, helper: str, node: ast.expr) -> ast.expr:
        self.changed += 1
        func = ast.copy_location(ast.Name(id=helper, ctx=ast.Load()), node)
        return ast.copy_location(ast.Call(func=func, args=[node], keywords=[]), node)

    def _keep(self, node: ast.expr) -> ast.expr:
        """Rewrite inside a display whose own object is unobservable, but not the display."""
        if type(node) in _DISPLAYS or isinstance(node, ast.Tuple):
            self.generic_visit(node)
            return node
        return self.visit(node)

    def _display(self, node: ast.expr) -> ast.expr:
        self.generic_visit(node)
        kind = _DISPLAYS[type(node)]
        if kind not in self.kinds:
            return node
        if isinstance(node, ast.List) and not isinstance(node.ctx, ast.Load):
            return node
        return self._wrap(DISPLAY_HELPERS[kind], node)

    visit_List = visit_ListComp = visit_Dict = visit_DictComp = visit_Set = visit_SetComp = _display

    # -- unobservable positions ------------------------------------------------------------
    def visit_Compare(self, node: ast.Compare) -> ast.expr:
        node.left = self.visit(node.left)
        node.comparators = [
            self._keep(c) if isinstance(op, (ast.In, ast.NotIn)) else self.visit(c)
            for op, c in zip(node.ops, node.comparators, strict=True)
        ]
        return node

    def _loop(self, node: ast.For | ast.AsyncFor) -> ast.stmt:
        node.target = self.visit(node.target)
        node.iter = self._keep(node.iter)
        node.body = [self.visit(s) for s in node.body]
        node.orelse = [self.visit(s) for s in node.orelse]
        return node

    visit_For = visit_AsyncFor = _loop

    def visit_comprehension(self, node: ast.comprehension) -> ast.comprehension:
        node.target = self.visit(node.target)
        node.iter = self._keep(node.iter)
        node.ifs = [self.visit(e) for e in node.ifs]
        return node

    def visit_Call(self, node: ast.Call) -> ast.expr:
        node.func = self.visit(node.func)
        node.args = [
            ast.copy_location(ast.Starred(value=self._keep(a.value), ctx=a.ctx), a) if isinstance(a, ast.Starred) else self.visit(a)
            for a in node.args
        ]
        for kw in node.keywords:
            kw.value = self._keep(kw.value) if kw.arg is None else self.visit(kw.value)
        f = node.func
        if isinstance(f, ast.Name) and f.id in self.calls:
            # (__rs_List__ if list is __rs_builtin_list__ else list)(...)
            self.changed += 1
            builtin_name, target_name = CALL_NAMES[f.id]

            def name(n: str) -> ast.Name:
                return ast.copy_location(ast.Name(id=n, ctx=ast.Load()), f)

            test = ast.copy_location(ast.Compare(left=name(f.id), ops=[ast.Is()], comparators=[name(builtin_name)]), f)
            node.func = ast.copy_location(ast.IfExp(test=test, body=name(target_name), orelse=f), f)
        return node

    # -- never rewritten: annotations, type parameters, aliases ----------------------------
    def visit_arg(self, node: ast.arg) -> ast.arg:
        return node  # only the annotation lives here

    def _function(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> ast.stmt:
        node.decorator_list = [self.visit(d) for d in node.decorator_list]
        a = node.args
        a.defaults = [self.visit(d) for d in a.defaults]
        a.kw_defaults = [self.visit(d) if d is not None else None for d in a.kw_defaults]
        node.body = [self.visit(s) for s in node.body]
        return node  # returns / arg annotations / type_params untouched

    visit_FunctionDef = visit_AsyncFunctionDef = _function

    def visit_Lambda(self, node: ast.Lambda) -> ast.expr:
        node.args.defaults = [self.visit(d) for d in node.args.defaults]
        node.args.kw_defaults = [self.visit(d) if d is not None else None for d in node.args.kw_defaults]
        node.body = self.visit(node.body)
        return node

    def visit_ClassDef(self, node: ast.ClassDef) -> ast.stmt:
        node.decorator_list = [self.visit(d) for d in node.decorator_list]
        node.bases = [self.visit(b) for b in node.bases]
        for kw in node.keywords:
            kw.value = self.visit(kw.value)
        node.body = [self.visit(s) for s in node.body]
        return node

    def visit_AnnAssign(self, node: ast.AnnAssign) -> ast.stmt:
        node.target = self.visit(node.target)
        if node.value is not None:
            node.value = self.visit(node.value)
        return node

    def visit_TypeAlias(self, node: ast.TypeAlias) -> ast.stmt:
        return node


def rewrite_tree(tree: ast.Module, kinds: Iterable[str] = DEFAULT_KINDS) -> tuple[ast.Module, int]:
    """Rewrite a module AST in place; returns it and the number of rewritten nodes."""
    kinds = frozenset(kinds)
    shadowed = _bound_names(tree)
    rw = Rewriter(kinds, calls={k for k in kinds if k not in shadowed})
    tree = rw.visit(tree)
    ast.fix_missing_locations(tree)
    return tree, rw.changed


def rewrite_source(source: str | bytes, filename: str = "<string>", kinds: Iterable[str] = DEFAULT_KINDS) -> ast.Module:
    tree = ast.parse(source, filename)
    return rewrite_tree(tree, kinds)[0]


def compile_rewritten(source: str | bytes, filename: str, mode: str = "exec", *, kinds: Iterable[str] = DEFAULT_KINDS,
                      flags: int = 0, dont_inherit: bool = True, optimize: int = -1):
    """``compile()`` with the native-tier rewrite applied."""
    tree = ast.parse(source, filename, mode)
    if mode == "exec":
        tree = rewrite_tree(tree, kinds)[0]
    else:  # eval/single: rewrite the expression(s) the same way
        tree = Rewriter(kinds, calls=set(kinds) - _bound_names(tree)).visit(tree)
        ast.fix_missing_locations(tree)
    return compile(tree, filename, mode, flags=flags, dont_inherit=dont_inherit, optimize=optimize)


# -- code-object rewriting for already-compiled functions (the decorator) -----------------


def rewrite_function(fn: Callable[..., Any], *, kinds: Iterable[str] = DEFAULT_KINDS, targets: dict[str, Any] | None = None) -> Callable[..., Any]:
    """Replace ``fn.__code__`` with a rewritten compilation of its source, keeping its
    closure, defaults, globals, name and first line. Returns ``fn``."""
    import inspect

    code = fn.__code__
    try:
        lines, first = inspect.getsourcelines(fn)
    except (OSError, TypeError) as e:
        raise TypeError(f"rustique.native needs the source of {fn.__qualname__!r}: {e}") from None
    src = "".join(lines)
    # Parse an indented method in place (an `if 1:` block) instead of dedenting it, so that
    # column offsets and multi-line string literals stay exactly as written.
    indented = src[:1] in (" ", "\t")
    tree = ast.parse(("if 1:\n" + src) if indented else src, code.co_filename)
    fdef = tree.body[0].body[0] if indented else tree.body[0]  # type: ignore[attr-defined]
    if not isinstance(fdef, (ast.FunctionDef, ast.AsyncFunctionDef)) or fdef.name != fn.__name__:
        raise TypeError(f"rustique.native: cannot locate the definition of {fn.__qualname__!r} (lambda or generated code?)")
    # The decorators stay in the tree: the factory is compiled, never run, so they are not
    # applied again, and the code object's first line is the first decorator's, as before.
    kinds = frozenset(kinds)
    rw = Rewriter(kinds, calls={k for k in kinds if k not in _module_bound_names(fn)})
    fdef = rw.visit(fdef)
    # A factory whose parameters are the free variables: the inner code object then has
    # exactly the same co_freevars (incl. an implicit `__class__` cell for `super()`).
    factory = ast.FunctionDef(
        name="__rs_factory__",
        args=ast.arguments(posonlyargs=[], args=[ast.arg(arg=v) for v in code.co_freevars], vararg=None,
                           kwonlyargs=[], kw_defaults=[], kwarg=None, defaults=[]),
        body=[fdef, ast.Return(value=ast.Name(id=fdef.name, ctx=ast.Load()))],
        decorator_list=[], returns=None, type_params=[],
    )
    module = ast.Module(body=[factory], type_ignores=[])
    ast.increment_lineno(module, first - (2 if indented else 1))
    ast.copy_location(factory, fdef)
    ast.fix_missing_locations(module)
    compiled = compile(module, code.co_filename, "exec", dont_inherit=True)
    fcode = next(c for c in compiled.co_consts if hasattr(c, "co_code") and c.co_name == "__rs_factory__")
    inner = next(c for c in fcode.co_consts if hasattr(c, "co_code") and c.co_name == fn.__name__)
    if inner.co_freevars != code.co_freevars:
        raise TypeError(f"rustique.native: free variables of {fn.__qualname__!r} changed ({code.co_freevars} -> {inner.co_freevars})")
    if inner.co_firstlineno != code.co_firstlineno:
        raise TypeError(f"rustique.native: source of {fn.__qualname__!r} does not match its code object")
    fn.__code__ = inner.replace(co_qualname=code.co_qualname)
    for name, value in helpers(targets).items():
        fn.__globals__.setdefault(name, value)
    return fn


_BOUND_CACHE: dict[str, set[str]] = {}


def _module_bound_names(fn: Callable[..., Any]) -> set[str]:
    """Names bound anywhere in the module that defines ``fn`` (cached per file)."""
    import inspect

    path = fn.__code__.co_filename
    if path not in _BOUND_CACHE:
        module = inspect.getmodule(fn)
        try:
            _BOUND_CACHE[path] = _bound_names(ast.parse(inspect.getsource(module))) if module else set()
        except (OSError, TypeError, SyntaxError):
            _BOUND_CACHE[path] = set()
    return _BOUND_CACHE[path]


def rewrite_object(obj: Any) -> Any:
    """``@rustique.native``: rewrite a function, or every plain function defined in a class
    body (methods, static and class methods). Class-body statements already ran."""
    if isinstance(obj, type):
        for name, member in vars(obj).items():
            target = member.__func__ if isinstance(member, (staticmethod, classmethod)) else member
            if callable(target) and hasattr(target, "__code__") and target.__qualname__.startswith(obj.__qualname__ + "."):
                rewrite_function(target)
        return obj
    rewrite_function(obj)
    return obj
