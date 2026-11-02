"""A small scripting language: regex tokenizer, recursive-descent parser, tree-walking interpreter.

Stands for the in-house DSLs, rule/formula languages, template expression evaluators,
query languages and teaching interpreters that teams write in Python (and for the
structure of tools such as ``asteval``, ``simpleeval``, Lark-built evaluators, Jinja's
compiler front end): a master ``re`` pattern with named groups driven by ``finditer`` and
``m.lastgroup``; tokens as a ``typing.NamedTuple``; a recursive-descent parser with
precedence climbing building AST node classes with ``__slots__``; passes written as visitors
dispatching through ``getattr(self, "visit_" + type(node).__name__)``; a constant-folding
pass; lexical environments as linked dict scopes; first-class closures; ``return``/``break``
as exceptions; builtin functions, lists and maps; runtime errors with line numbers.

Run: tokenize, parse, fold and execute 9 programs (recursive fib, closures and
higher-order functions, quicksort, sieve, word counting, n-queens, a bank-account object
model built from closures and maps, a Mandelbrot plot, string building), ~4M AST node
evaluations.
"""

import re
from typing import NamedTuple

REQUIRES: list[str] = []

SCALE = 1


# --------------------------------------------------------------------------------------
# lexer
# --------------------------------------------------------------------------------------


class Token(NamedTuple):
    kind: str
    value: str
    line: int
    col: int


KEYWORDS = frozenset({"let", "fn", "if", "else", "while", "for", "in", "return", "break",
                      "continue", "true", "false", "nil", "and", "or", "not"})

TOKEN_SPEC = [
    ("NUMBER", r"\d+(?:\.\d+)?"),
    ("STRING", r'"(?:[^"\\]|\\.)*"'),
    ("NAME", r"[A-Za-z_][A-Za-z0-9_]*"),
    ("OP", r"==|!=|<=|>=|&&|\|\||=>|[-+*/%<>=!(){}\[\],;:.]"),
    ("NEWLINE", r"\n"),
    ("SKIP", r"[ \t\r]+"),
    ("COMMENT", r"#[^\n]*"),
    ("MISMATCH", r"."),
]
MASTER = re.compile("|".join(f"(?P<{name}>{pattern})" for name, pattern in TOKEN_SPEC))
ESCAPES = {"n": "\n", "t": "\t", '"': '"', "\\": "\\"}


class LangError(Exception):
    def __init__(self, message: str, line: int) -> None:
        super().__init__(f"line {line}: {message}")
        self.line = line


def tokenize(source: str) -> list[Token]:
    tokens = []
    line, line_start = 1, 0
    for m in MASTER.finditer(source):
        kind = m.lastgroup
        value = m.group()
        col = m.start() - line_start
        if kind == "NEWLINE":
            line += 1
            line_start = m.end()
            continue
        if kind in ("SKIP", "COMMENT"):
            continue
        if kind == "MISMATCH":
            raise LangError(f"unexpected character {value!r}", line)
        if kind == "NAME" and value in KEYWORDS:
            kind = value.upper()
        elif kind == "STRING":
            value = re.sub(r"\\(.)", lambda e: ESCAPES.get(e.group(1), e.group(1)), value[1:-1])
        tokens.append(Token(kind, value, line, col))
    tokens.append(Token("EOF", "", line, 0))
    return tokens


# --------------------------------------------------------------------------------------
# AST
# --------------------------------------------------------------------------------------


class Node:
    __slots__ = ("line",)
    fields: tuple[str, ...] = ()

    def __repr__(self) -> str:
        args = ", ".join(f"{f}={getattr(self, f)!r}" for f in self.fields)
        return f"{type(self).__name__}({args})"

    def children(self):
        for f in self.fields:
            value = getattr(self, f)
            if isinstance(value, Node):
                yield value
            elif isinstance(value, list):
                for item in value:
                    if isinstance(item, Node):
                        yield item
                    elif isinstance(item, tuple):
                        yield from (x for x in item if isinstance(x, Node))


class Literal(Node):
    __slots__ = ("value",)
    fields = ("value",)

    def __init__(self, value, line: int) -> None:
        self.value = value
        self.line = line


class Name(Node):
    __slots__ = ("name",)
    fields = ("name",)

    def __init__(self, name: str, line: int) -> None:
        self.name = name
        self.line = line


class Unary(Node):
    __slots__ = ("op", "operand")
    fields = ("op", "operand")

    def __init__(self, op: str, operand: Node, line: int) -> None:
        self.op = op
        self.operand = operand
        self.line = line


class Binary(Node):
    __slots__ = ("op", "left", "right")
    fields = ("op", "left", "right")

    def __init__(self, op: str, left: Node, right: Node, line: int) -> None:
        self.op = op
        self.left = left
        self.right = right
        self.line = line


class Logical(Binary):
    __slots__ = ()


class Call(Node):
    __slots__ = ("callee", "args")
    fields = ("callee", "args")

    def __init__(self, callee: Node, args: list[Node], line: int) -> None:
        self.callee = callee
        self.args = args
        self.line = line


class Index(Node):
    __slots__ = ("target", "index")
    fields = ("target", "index")

    def __init__(self, target: Node, index: Node, line: int) -> None:
        self.target = target
        self.index = index
        self.line = line


class Attribute(Node):
    __slots__ = ("target", "name")
    fields = ("target", "name")

    def __init__(self, target: Node, name: str, line: int) -> None:
        self.target = target
        self.name = name
        self.line = line


class ListExpr(Node):
    __slots__ = ("items",)
    fields = ("items",)

    def __init__(self, items: list[Node], line: int) -> None:
        self.items = items
        self.line = line


class MapExpr(Node):
    __slots__ = ("pairs",)
    fields = ("pairs",)

    def __init__(self, pairs: list[tuple[Node, Node]], line: int) -> None:
        self.pairs = pairs
        self.line = line


class FunctionExpr(Node):
    __slots__ = ("name", "params", "body")
    fields = ("name", "params", "body")

    def __init__(self, name: str, params: list[str], body: list[Node], line: int) -> None:
        self.name = name
        self.params = params
        self.body = body
        self.line = line


class Let(Node):
    __slots__ = ("name", "value")
    fields = ("name", "value")

    def __init__(self, name: str, value: Node, line: int) -> None:
        self.name = name
        self.value = value
        self.line = line


class Assign(Node):
    __slots__ = ("target", "value")
    fields = ("target", "value")

    def __init__(self, target: Node, value: Node, line: int) -> None:
        self.target = target
        self.value = value
        self.line = line


class ExprStmt(Node):
    __slots__ = ("expr",)
    fields = ("expr",)

    def __init__(self, expr: Node, line: int) -> None:
        self.expr = expr
        self.line = line


class If(Node):
    __slots__ = ("test", "body", "orelse")
    fields = ("test", "body", "orelse")

    def __init__(self, test: Node, body: list[Node], orelse: list[Node], line: int) -> None:
        self.test = test
        self.body = body
        self.orelse = orelse
        self.line = line


class While(Node):
    __slots__ = ("test", "body")
    fields = ("test", "body")

    def __init__(self, test: Node, body: list[Node], line: int) -> None:
        self.test = test
        self.body = body
        self.line = line


class For(Node):
    __slots__ = ("var", "iterable", "body")
    fields = ("var", "iterable", "body")

    def __init__(self, var: str, iterable: Node, body: list[Node], line: int) -> None:
        self.var = var
        self.iterable = iterable
        self.body = body
        self.line = line


class Return(Node):
    __slots__ = ("value",)
    fields = ("value",)

    def __init__(self, value: Node | None, line: int) -> None:
        self.value = value
        self.line = line


class Break(Node):
    __slots__ = ()

    def __init__(self, line: int) -> None:
        self.line = line


class Continue(Node):
    __slots__ = ()

    def __init__(self, line: int) -> None:
        self.line = line


# --------------------------------------------------------------------------------------
# parser
# --------------------------------------------------------------------------------------

PRECEDENCE = {"OR": 1, "AND": 2, "==": 3, "!=": 3, "<": 4, "<=": 4, ">": 4, ">=": 4,
              "+": 5, "-": 5, "*": 6, "/": 6, "%": 6}


class Parser:
    def __init__(self, tokens: list[Token]) -> None:
        self.tokens = tokens
        self.pos = 0

    # -- helpers -------------------------------------------------------------------------
    @property
    def current(self) -> Token:
        return self.tokens[self.pos]

    def peek_is(self, *kinds: str) -> bool:
        tok = self.tokens[self.pos]
        return tok.kind in kinds or (tok.kind == "OP" and tok.value in kinds)

    def advance(self) -> Token:
        tok = self.tokens[self.pos]
        self.pos += 1
        return tok

    def expect(self, kind: str) -> Token:
        tok = self.current
        if tok.kind == kind or (tok.kind == "OP" and tok.value == kind):
            self.pos += 1
            return tok
        raise LangError(f"expected {kind}, found {tok.value or tok.kind!r}", tok.line)

    def match(self, kind: str) -> bool:
        if self.peek_is(kind):
            self.pos += 1
            return True
        return False

    # -- statements ----------------------------------------------------------------------
    def program(self) -> list[Node]:
        stmts = []
        while not self.peek_is("EOF"):
            stmts.append(self.statement())
        return stmts

    def block(self) -> list[Node]:
        self.expect("{")
        stmts = []
        while not self.peek_is("}"):
            stmts.append(self.statement())
        self.expect("}")
        return stmts

    def statement(self) -> Node:
        tok = self.current
        kind = tok.kind
        if kind == "LET":
            self.advance()
            name = self.expect("NAME").value
            self.expect("=")
            value = self.expression()
            self.match(";")
            return Let(name, value, tok.line)
        if kind == "FN" and self.tokens[self.pos + 1].kind == "NAME":
            self.advance()
            name = self.advance().value
            fn = self.function_rest(name, tok.line)
            self.match(";")
            return Let(name, fn, tok.line)
        if kind == "IF":
            return self.if_statement()
        if kind == "WHILE":
            self.advance()
            test = self.expression()
            return While(test, self.block(), tok.line)
        if kind == "FOR":
            self.advance()
            var = self.expect("NAME").value
            self.expect("IN")
            iterable = self.expression()
            return For(var, iterable, self.block(), tok.line)
        if kind == "RETURN":
            self.advance()
            value = None if self.peek_is(";", "}") else self.expression()
            self.match(";")
            return Return(value, tok.line)
        if kind == "BREAK":
            self.advance()
            self.match(";")
            return Break(tok.line)
        if kind == "CONTINUE":
            self.advance()
            self.match(";")
            return Continue(tok.line)
        expr = self.expression()
        if self.match("="):
            if not isinstance(expr, (Name, Index, Attribute)):
                raise LangError("invalid assignment target", tok.line)
            value = self.expression()
            self.match(";")
            return Assign(expr, value, tok.line)
        self.match(";")
        return ExprStmt(expr, tok.line)

    def if_statement(self) -> If:
        tok = self.expect("IF")
        test = self.expression()
        body = self.block()
        orelse: list[Node] = []
        if self.match("ELSE"):
            orelse = [self.if_statement()] if self.peek_is("IF") else self.block()
        return If(test, body, orelse, tok.line)

    def function_rest(self, name: str, line: int) -> FunctionExpr:
        self.expect("(")
        params = []
        if not self.peek_is(")"):
            params.append(self.expect("NAME").value)
            while self.match(","):
                params.append(self.expect("NAME").value)
        self.expect(")")
        if self.match("=>"):
            expr = self.expression()
            return FunctionExpr(name, params, [Return(expr, line)], line)
        return FunctionExpr(name, params, self.block(), line)

    # -- expressions (precedence climbing) -----------------------------------------------
    def expression(self, min_prec: int = 1) -> Node:
        left = self.unary()
        while True:
            tok = self.current
            op = tok.value if tok.kind == "OP" else tok.kind
            prec = PRECEDENCE.get(op)
            if prec is None or prec < min_prec:
                return left
            self.advance()
            right = self.expression(prec + 1)
            if op in ("AND", "OR"):
                left = Logical(op.lower(), left, right, tok.line)
            else:
                left = Binary(op, left, right, tok.line)

    def unary(self) -> Node:
        tok = self.current
        if self.peek_is("-", "!", "NOT"):
            self.advance()
            op = "not" if tok.value in ("!", "not") else tok.value
            return Unary(op, self.unary(), tok.line)
        return self.postfix(self.primary())

    def postfix(self, expr: Node) -> Node:
        while True:
            tok = self.current
            if self.match("("):
                args = []
                if not self.peek_is(")"):
                    args.append(self.expression())
                    while self.match(","):
                        args.append(self.expression())
                self.expect(")")
                expr = Call(expr, args, tok.line)
            elif self.match("["):
                index = self.expression()
                self.expect("]")
                expr = Index(expr, index, tok.line)
            elif self.match("."):
                expr = Attribute(expr, self.expect("NAME").value, tok.line)
            else:
                return expr

    def primary(self) -> Node:
        tok = self.advance()
        kind = tok.kind
        if kind == "NUMBER":
            value = float(tok.value) if "." in tok.value else int(tok.value)
            return Literal(value, tok.line)
        if kind == "STRING":
            return Literal(tok.value, tok.line)
        if kind == "TRUE":
            return Literal(True, tok.line)
        if kind == "FALSE":
            return Literal(False, tok.line)
        if kind == "NIL":
            return Literal(None, tok.line)
        if kind == "NAME":
            return Name(tok.value, tok.line)
        if kind == "FN":
            return self.function_rest("<lambda>", tok.line)
        if tok.value == "(":
            expr = self.expression()
            self.expect(")")
            return expr
        if tok.value == "[":
            items = []
            if not self.peek_is("]"):
                items.append(self.expression())
                while self.match(","):
                    items.append(self.expression())
            self.expect("]")
            return ListExpr(items, tok.line)
        if tok.value == "{":
            pairs = []
            if not self.peek_is("}"):
                while True:
                    key = self.expression()
                    if isinstance(key, Name):  # {name: ...} means the string key "name"
                        key = Literal(key.name, key.line)
                    self.expect(":")
                    pairs.append((key, self.expression()))
                    if not self.match(","):
                        break
            self.expect("}")
            return MapExpr(pairs, tok.line)
        raise LangError(f"unexpected {tok.value or tok.kind!r}", tok.line)


# --------------------------------------------------------------------------------------
# passes
# --------------------------------------------------------------------------------------


class Visitor:
    def visit(self, node: Node):
        method = getattr(self, "visit_" + type(node).__name__, None)
        if method is None:
            return self.generic_visit(node)
        return method(node)

    def generic_visit(self, node: Node):
        for child in node.children():
            self.visit(child)
        return node


BINOPS = {
    "+": lambda a, b: a + b, "-": lambda a, b: a - b, "*": lambda a, b: a * b,
    "/": lambda a, b: a / b if isinstance(a, float) or isinstance(b, float) or a % b else a // b,
    "%": lambda a, b: a % b, "==": lambda a, b: a == b, "!=": lambda a, b: a != b,
    "<": lambda a, b: a < b, "<=": lambda a, b: a <= b, ">": lambda a, b: a > b,
    ">=": lambda a, b: a >= b,
}


class ConstantFolder(Visitor):
    """Folds literal arithmetic, in place, and counts what it saw."""

    def __init__(self) -> None:
        self.folded = 0
        self.nodes = 0

    def visit(self, node: Node):
        self.nodes += 1
        return super().visit(node)

    def fold_list(self, nodes: list[Node]) -> list[Node]:
        return [self.visit(n) for n in nodes]

    def visit_Binary(self, node: Binary) -> Node:
        node.left = self.visit(node.left)
        node.right = self.visit(node.right)
        if isinstance(node.left, Literal) and isinstance(node.right, Literal):
            a, b = node.left.value, node.right.value
            if type(a) in (int, float) and type(b) in (int, float) and not (
                    node.op in ("/", "%") and b == 0):
                self.folded += 1
                return Literal(BINOPS[node.op](a, b), node.line)
        return node

    def visit_Unary(self, node: Unary) -> Node:
        node.operand = self.visit(node.operand)
        if node.op == "-" and isinstance(node.operand, Literal) and \
                type(node.operand.value) in (int, float):
            self.folded += 1
            return Literal(-node.operand.value, node.line)
        return node

    def generic_visit(self, node: Node) -> Node:
        for f in node.fields:
            value = getattr(node, f)
            if isinstance(value, Node):
                setattr(node, f, self.visit(value))
            elif isinstance(value, list):
                setattr(node, f, [self.visit(v) if isinstance(v, Node) else
                                  tuple(self.visit(x) for x in v) if isinstance(v, tuple) else v
                                  for v in value])
        return node


class ReturnSignal(Exception):
    def __init__(self, value) -> None:
        self.value = value


class BreakSignal(Exception):
    pass


class ContinueSignal(Exception):
    pass


class Environment:
    __slots__ = ("values", "parent")

    def __init__(self, parent: "Environment | None" = None) -> None:
        self.values: dict = {}
        self.parent = parent

    def lookup(self, name: str, line: int):
        env = self
        while env is not None:
            values = env.values
            if name in values:
                return values[name]
            env = env.parent
        raise LangError(f"undefined name {name!r}", line)

    def assign(self, name: str, value, line: int) -> None:
        env = self
        while env is not None:
            if name in env.values:
                env.values[name] = value
                return
            env = env.parent
        raise LangError(f"assignment to undeclared {name!r}", line)

    def define(self, name: str, value) -> None:
        self.values[name] = value


class Closure:
    __slots__ = ("decl", "env", "interp")

    def __init__(self, decl: FunctionExpr, env: Environment, interp: "Interpreter") -> None:
        self.decl = decl
        self.env = env
        self.interp = interp

    def __call__(self, *args):
        decl = self.decl
        if len(args) != len(decl.params):
            raise LangError(f"{decl.name}() takes {len(decl.params)} arguments, got {len(args)}",
                            decl.line)
        env = Environment(self.env)
        for name, value in zip(decl.params, args):
            env.values[name] = value
        try:
            self.interp.execute_block(decl.body, env)
        except ReturnSignal as ret:
            return ret.value
        return None

    def __repr__(self) -> str:
        return f"<fn {self.decl.name}/{len(self.decl.params)}>"


class Interpreter(Visitor):
    def __init__(self) -> None:
        self.globals = Environment()
        self.env = self.globals
        self.output: list[str] = []
        self.calls = 0
        for name, fn in self.builtins().items():
            self.globals.define(name, fn)

    def builtins(self) -> dict:
        def to_str(v) -> str:
            if v is None:
                return "nil"
            if v is True:
                return "true"
            if v is False:
                return "false"
            if isinstance(v, float):
                return f"{v:.6g}"
            if isinstance(v, list):
                return "[" + ", ".join(to_str(x) for x in v) + "]"
            if isinstance(v, dict):
                return "{" + ", ".join(f"{to_str(k)}: {to_str(x)}" for k, x in v.items()) + "}"
            return str(v)

        return {
            "print": lambda *args: self.output.append(" ".join(to_str(a) for a in args)),
            "str": to_str,
            "len": len,
            "push": lambda lst, v: lst.append(v),
            "pop": lambda lst: lst.pop(),
            "range": lambda *a: list(range(*a)),
            "keys": lambda m: list(m),
            "has": lambda m, k: k in m,
            "split": lambda s, sep: s.split(sep),
            "join": lambda lst, sep: sep.join(lst),
            "lower": str.lower,
            "floor": lambda x: int(x // 1),
            "sorted_by": lambda lst, key: sorted(lst, key=key),
            "chr": chr,
            "ord": ord,
        }

    # -- statements ----------------------------------------------------------------------
    def execute_block(self, stmts: list[Node], env: Environment) -> None:
        previous = self.env
        self.env = env
        try:
            for stmt in stmts:
                self.visit(stmt)
        finally:
            self.env = previous

    def visit_Let(self, node: Let) -> None:
        self.env.define(node.name, self.visit(node.value))

    def visit_Assign(self, node: Assign) -> None:
        value = self.visit(node.value)
        target = node.target
        if isinstance(target, Name):
            self.env.assign(target.name, value, node.line)
        elif isinstance(target, Index):
            container = self.visit(target.target)
            container[self.visit(target.index)] = value
        else:
            self.visit(target.target)[target.name] = value

    def visit_ExprStmt(self, node: ExprStmt) -> None:
        self.visit(node.expr)

    def visit_If(self, node: If) -> None:
        if self.truthy(self.visit(node.test)):
            self.execute_block(node.body, Environment(self.env))
        elif node.orelse:
            self.execute_block(node.orelse, Environment(self.env))

    def visit_While(self, node: While) -> None:
        while self.truthy(self.visit(node.test)):
            try:
                self.execute_block(node.body, Environment(self.env))
            except BreakSignal:
                break
            except ContinueSignal:
                continue

    def visit_For(self, node: For) -> None:
        iterable = self.visit(node.iterable)
        if isinstance(iterable, dict):
            iterable = list(iterable)
        for item in iterable:
            env = Environment(self.env)
            env.values[node.var] = item
            try:
                self.execute_block(node.body, env)
            except BreakSignal:
                break
            except ContinueSignal:
                continue

    def visit_Return(self, node: Return) -> None:
        raise ReturnSignal(None if node.value is None else self.visit(node.value))

    def visit_Break(self, node: Break) -> None:
        raise BreakSignal()

    def visit_Continue(self, node: Continue) -> None:
        raise ContinueSignal()

    # -- expressions ---------------------------------------------------------------------
    @staticmethod
    def truthy(value) -> bool:
        return not (value is None or value is False)

    def visit_Literal(self, node: Literal):
        return node.value

    def visit_Name(self, node: Name):
        return self.env.lookup(node.name, node.line)

    def visit_Unary(self, node: Unary):
        value = self.visit(node.operand)
        if node.op == "-":
            if type(value) not in (int, float):
                raise LangError(f"cannot negate {type(value).__name__}", node.line)
            return -value
        return not self.truthy(value)

    def visit_Binary(self, node: Binary):
        left = self.visit(node.left)
        right = self.visit(node.right)
        op = node.op
        if op == "+" and (isinstance(left, str) or isinstance(right, str)):
            return f"{left}{right}" if isinstance(left, str) and isinstance(right, str) else \
                str(left) + str(right)
        try:
            return BINOPS[op](left, right)
        except ZeroDivisionError:
            raise LangError("division by zero", node.line) from None
        except TypeError:
            raise LangError(f"bad operands for {op}: {type(left).__name__}, "
                            f"{type(right).__name__}", node.line) from None

    def visit_Logical(self, node: Logical):
        left = self.visit(node.left)
        if node.op == "or":
            return left if self.truthy(left) else self.visit(node.right)
        return self.visit(node.right) if self.truthy(left) else left

    def visit_Call(self, node: Call):
        callee = self.visit(node.callee)
        args = [self.visit(a) for a in node.args]
        if not callable(callee):
            raise LangError(f"{callee!r} is not callable", node.line)
        self.calls += 1
        return callee(*args)

    def visit_Index(self, node: Index):
        target = self.visit(node.target)
        index = self.visit(node.index)
        try:
            return target[index]
        except (IndexError, KeyError):
            raise LangError(f"index {index!r} out of range", node.line) from None

    def visit_Attribute(self, node: Attribute):
        target = self.visit(node.target)
        if not isinstance(target, dict):
            raise LangError(f"no attribute {node.name!r}", node.line)
        return target.get(node.name)

    def visit_ListExpr(self, node: ListExpr):
        return [self.visit(item) for item in node.items]

    def visit_MapExpr(self, node: MapExpr):
        return {self.visit(k): self.visit(v) for k, v in node.pairs}

    def visit_FunctionExpr(self, node: FunctionExpr):
        return Closure(node, self.env, self)

    def run(self, program: list[Node]) -> None:
        for stmt in program:
            self.visit(stmt)


# --------------------------------------------------------------------------------------
# programs
# --------------------------------------------------------------------------------------

PROGRAMS = {
    "fib": """
fn fib(n) {
  if n < 2 { return n; }
  return fib(n - 1) + fib(n - 2);
}
let total = 0;
for i in range(@FIB@) { total = total + fib(i); }
print("fib", fib(@FIB@ - 1), total);
""",
    "closures": """
fn make_counter(step) {
  let count = 0;
  return fn() { count = count + step; return count; };
}
fn compose(f, g) => fn(x) => f(g(x));
fn map(f, xs) { let out = []; for x in xs { push(out, f(x)); } return out; }
fn filter(p, xs) { let out = []; for x in xs { if p(x) { push(out, x); } } return out; }
fn reduce(f, xs, acc) { for x in xs { acc = f(acc, x); } return acc; }
let counters = map(make_counter, range(1, 30));
let ticks = 0;
for round in range(@ROUNDS@) {
  for c in counters { ticks = ticks + c(); }
}
let inc = fn(x) => x + 1;
let dbl = fn(x) => x * 2;
let f = compose(inc, compose(dbl, inc));
let ys = map(f, range(@N@));
let evens = filter(fn(x) => x % 3 == 0, ys);
print("closures", ticks, len(evens), reduce(fn(a, b) => a + b, evens, 0));
""",
    "quicksort": """
fn quicksort(xs) {
  if len(xs) <= 1 { return xs; }
  let pivot = xs[floor(len(xs) / 2)];
  let less = []; let same = []; let more = [];
  for x in xs {
    if x < pivot { push(less, x); } else if x > pivot { push(more, x); } else { push(same, x); }
  }
  let out = quicksort(less);
  for x in same { push(out, x); }
  for x in quicksort(more) { push(out, x); }
  return out;
}
let seed = 12345;
fn rand() { seed = (seed * 1103515245 + 12345) % 2147483648; return seed; }
let data = [];
for i in range(@N@) { push(data, rand() % 100000); }
let sorted = quicksort(data);
let ok = true;
let i = 1;
while i < len(sorted) { if sorted[i - 1] > sorted[i] { ok = false; break; } i = i + 1; }
print("quicksort", len(sorted), sorted[0], sorted[len(sorted) - 1], ok);
""",
    "sieve": """
fn sieve(limit) {
  let flags = [];
  for i in range(limit + 1) { push(flags, true); }
  flags[0] = false; flags[1] = false;
  let p = 2;
  while p * p <= limit {
    if flags[p] {
      let m = p * p;
      while m <= limit { flags[m] = false; m = m + p; }
    }
    p = p + 1;
  }
  let primes = [];
  for i in range(limit + 1) { if flags[i] { push(primes, i); } }
  return primes;
}
let primes = sieve(@LIMIT@);
let twins = 0;
let k = 1;
while k < len(primes) { if primes[k] - primes[k - 1] == 2 { twins = twins + 1; } k = k + 1; }
print("sieve", len(primes), primes[len(primes) - 1], twins);
""",
    "words": """
let text = "@TEXT@";
let counts = {};
let longest = "";
for word in split(lower(text), " ") {
  if len(word) == 0 { continue; }
  if has(counts, word) { counts[word] = counts[word] + 1; } else { counts[word] = 1; }
  if len(word) > len(longest) { longest = word; }
}
let ranked = sorted_by(keys(counts), fn(w) => [0 - counts[w], w]);
let top = [];
for w in ranked { if len(top) >= 8 { break; } push(top, w + "=" + str(counts[w])); }
print("words", len(keys(counts)), longest, join(top, ","));
""",
    "queens": """
fn solve(n) {
  let cols = []; let count = 0;
  fn safe(r, c) {
    let i = 0;
    while i < r {
      let d = cols[i] - c;
      if d == 0 or d == r - i or d == i - r { return false; }
      i = i + 1;
    }
    return true;
  }
  fn place(r) {
    if r == n { count = count + 1; return nil; }
    for c in range(n) {
      if safe(r, c) { push(cols, c); place(r + 1); pop(cols); }
    }
  }
  place(0);
  return count;
}
print("queens", solve(@QUEENS@), solve(5));
""",
    "accounts": """
fn account(owner, balance) {
  let self = {owner: owner, balance: balance, history: []};
  self.deposit = fn(amount) {
    if amount <= 0 { return false; }
    self.balance = self.balance + amount; push(self.history, ["dep", amount]); return true;
  };
  self.withdraw = fn(amount) {
    if amount > self.balance { push(self.history, ["denied", amount]); return false; }
    self.balance = self.balance - amount; push(self.history, ["wd", amount]); return true;
  };
  return self;
}
let accts = [];
for i in range(40) { push(accts, account("user" + str(i), i * 10)); }
let seed = 7;
fn rnd(n) { seed = (seed * 48271) % 2147483647; return seed % n; }
let denied = 0;
for step in range(@STEPS@) {
  let a = accts[rnd(len(accts))];
  let amount = rnd(200) - 20;
  if rnd(3) == 0 { if !a.withdraw(amount) { denied = denied + 1; } } else { a.deposit(amount); }
}
let total = 0;
for a in accts { total = total + a.balance; }
print("accounts", total, denied, len(accts[0].history));
""",
    "mandelbrot": """
let rows = [];
let y = 0;
while y < @H@ {
  let line = "";
  let x = 0;
  while x < @W@ {
    let cr = x * 3.0 / @W@ - 2.1; let ci = y * 2.4 / @H@ - 1.2;
    let zr = 0.0; let zi = 0.0; let n = 0;
    while n < 40 and zr * zr + zi * zi < 4.0 {
      let t = zr * zr - zi * zi + cr;
      zi = 2.0 * zr * zi + ci; zr = t; n = n + 1;
    }
    if n == 40 { line = line + "#"; } else if n > 8 { line = line + "+"; } else { line = line + "."; }
    x = x + 1;
  }
  push(rows, line);
  y = y + 1;
}
let filled = 0;
for r in rows { for c in range(len(r)) { if r[c] == "#" { filled = filled + 1; } } }
print("mandelbrot", filled, len(rows), rows[floor(@H@ / 2)]);
""",
    "strings": """
fn pad(s, n) { while len(s) < n { s = " " + s; } return s; }
fn caesar(s, k) {
  let out = "";
  let i = 0;
  while i < len(s) {
    let c = ord(s[i]);
    if c >= 97 and c <= 122 { c = (c - 97 + k) % 26 + 97; }
    out = out + chr(c);
    i = i + 1;
  }
  return out;
}
let table = [];
for i in range(@ROWS@) {
  push(table, join([pad(str(i), 5), pad(str(i * i), 9), caesar("row number " + str(i), i % 26)], " | "));
}
print("strings", len(table), table[len(table) - 1]);
""",
}

WORDS = ("the quick brown fox jumps over the lazy dog while a rule engine evaluates "
         "policies against customer records and the interpreter walks the tree").split()


def setup():
    import random

    rng = random.Random(31337)
    text = " ".join(rng.choice(WORDS) + ("s" if rng.random() < 0.1 else "")
                    for _ in range(6_000 * SCALE))
    params = {"@FIB@": "18", "@ROUNDS@": str(60 * SCALE), "@N@": str(2_500 * SCALE),
              "@LIMIT@": str(30_000 * SCALE), "@TEXT@": text, "@QUEENS@": "7",
              "@STEPS@": str(6_000 * SCALE), "@H@": "24", "@W@": "70", "@ROWS@": str(400 * SCALE)}
    sources = {}
    for name, src in PROGRAMS.items():
        for key, value in params.items():
            src = src.replace(key, value)
        sources[name] = src
    return {"sources": sources}


def run(state):
    results = {}
    for name, source in state["sources"].items():
        tokens = tokenize(source)
        program = Parser(tokens).program()
        folder = ConstantFolder()
        program = folder.fold_list(program)
        interp = Interpreter()
        interp.run(program)
        results[name] = {"tokens": len(tokens), "nodes": folder.nodes, "folded": folder.folded,
                         "calls": interp.calls, "output": interp.output,
                         "ast_head": repr(program[0])[:200]}
    # error reporting paths
    errors = []
    for bad in ("let x = 1 +;", "let y = z + 1;", "print(1 / 0);", "let s = \"a\" - 1;",
                "fn f(a) { return a; } f(1, 2);", "let q = [1, 2][5];", "x = 3;", "let $ = 1;"):
        try:
            prog = Parser(tokenize(bad)).program()
            Interpreter().run(prog)
            errors.append("ok")
        except LangError as exc:
            errors.append(str(exc))
    results["errors"] = errors
    return results


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
