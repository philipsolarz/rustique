"""A home-grown ORM (data mapper + unit of work) over ``sqlite3``, driving a helpdesk app.

Stands for the "ORM-lite" layers teams build when SQLAlchemy is too much (and for the
core mechanics of peewee, Django's model layer, SQLModel-style mappers): typed field
descriptors (``__set_name__``, ``__get__``/``__set__`` with validation and dirty tracking),
a model metaclass plus ``__init_subclass__`` collecting fields into table metadata, a query
builder where class attribute access returns column expressions and Python operators
(``==``, ``>=``, ``&``, ``|``, ``~``) build an expression tree compiled to parameterised SQL,
immutable query cloning (``copy.copy``), row hydration through an identity map
(``weakref.WeakValueDictionary``), lazy foreign-key proxies built on ``__getattr__``, a
unit of work that batches INSERTs and per-column UPDATEs, model ``__eq__``/``__hash__``/
``__repr__``, and enum/date/Decimal conversions at the DB boundary.

Run: create the schema, insert 2,000 customers / 60 agents / 9,000 tickets / 22,000
comments through the session (one validated model instance and INSERT each), then a
working day of the helpdesk: 700 agent sessions, ~5,500 SQL statements (queue views,
searches, per-customer histories, comment threads), triage updates with dirty tracking,
lazy relationship walks, and an SLA report; ~51k rows hydrated into model instances.
"""

import copy
import enum
import sqlite3
import weakref
from collections import Counter, defaultdict
from datetime import date, timedelta
from decimal import Decimal

REQUIRES: list[str] = []

N_CUSTOMERS = 2_000
N_AGENTS = 60
N_TICKETS = 9_000
N_COMMENTS = 22_000

_MISSING = object()


# --------------------------------------------------------------------------------------
# expressions
# --------------------------------------------------------------------------------------


class Expr:
    __slots__ = ()

    def __and__(self, other: "Expr") -> "Expr":
        return BoolOp("AND", (self, other))

    def __or__(self, other: "Expr") -> "Expr":
        return BoolOp("OR", (self, other))

    def __invert__(self) -> "Expr":
        return NotOp(self)

    def compile(self, params: list) -> str:
        raise NotImplementedError


class Param(Expr):
    __slots__ = ("value",)

    def __init__(self, value) -> None:
        self.value = value

    def compile(self, params: list) -> str:
        params.append(self.value)
        return "?"


def _wrap(field: "Field", value) -> Expr:
    if isinstance(value, Expr):
        return value
    return Param(field.to_db(value))


class Column(Expr):
    """What ``Model.attr`` evaluates to on the class: a column in expressions."""

    __slots__ = ("field", "model")

    def __init__(self, field: "Field", model: type) -> None:
        self.field = field
        self.model = model

    def compile(self, params: list) -> str:
        return f"{self.model._meta.table}.{self.field.column}"

    def _bin(self, op: str, value) -> "Compare":
        return Compare(self, op, _wrap(self.field, value))

    def __eq__(self, value) -> "Compare":  # type: ignore[override]
        if value is None:
            return Compare(self, "IS", Raw("NULL"))
        return self._bin("=", value)

    def __ne__(self, value) -> "Compare":  # type: ignore[override]
        if value is None:
            return Compare(self, "IS NOT", Raw("NULL"))
        return self._bin("!=", value)

    def __lt__(self, value) -> "Compare":
        return self._bin("<", value)

    def __le__(self, value) -> "Compare":
        return self._bin("<=", value)

    def __gt__(self, value) -> "Compare":
        return self._bin(">", value)

    def __ge__(self, value) -> "Compare":
        return self._bin(">=", value)

    __hash__ = None  # type: ignore[assignment]

    def in_(self, values) -> "Expr":
        return InList(self, [self.field.to_db(v) for v in values])

    def like(self, pattern: str) -> "Compare":
        return Compare(self, "LIKE", Param(pattern))

    def asc(self) -> "Ordering":
        return Ordering(self, "ASC")

    def desc(self) -> "Ordering":
        return Ordering(self, "DESC")


class Raw(Expr):
    __slots__ = ("sql",)

    def __init__(self, sql: str) -> None:
        self.sql = sql

    def compile(self, params: list) -> str:
        return self.sql


class Compare(Expr):
    __slots__ = ("left", "op", "right")

    def __init__(self, left: Expr, op: str, right: Expr) -> None:
        self.left, self.op, self.right = left, op, right

    def compile(self, params: list) -> str:
        return f"{self.left.compile(params)} {self.op} {self.right.compile(params)}"


class InList(Expr):
    __slots__ = ("column", "values")

    def __init__(self, column: Column, values: list) -> None:
        self.column, self.values = column, values

    def compile(self, params: list) -> str:
        params.extend(self.values)
        return f"{self.column.compile(params)} IN ({', '.join('?' * len(self.values))})"


class BoolOp(Expr):
    __slots__ = ("op", "parts")

    def __init__(self, op: str, parts: tuple) -> None:
        flat = []
        for p in parts:
            if isinstance(p, BoolOp) and p.op == op:
                flat.extend(p.parts)
            else:
                flat.append(p)
        self.op, self.parts = op, tuple(flat)

    def compile(self, params: list) -> str:
        return "(" + f" {self.op} ".join(p.compile(params) for p in self.parts) + ")"


class NotOp(Expr):
    __slots__ = ("inner",)

    def __init__(self, inner: Expr) -> None:
        self.inner = inner

    def compile(self, params: list) -> str:
        return f"NOT ({self.inner.compile(params)})"


class Ordering:
    __slots__ = ("column", "direction")

    def __init__(self, column: Column, direction: str) -> None:
        self.column, self.direction = column, direction

    def compile(self) -> str:
        return f"{self.column.compile([])} {self.direction}"


# --------------------------------------------------------------------------------------
# fields (descriptors)
# --------------------------------------------------------------------------------------


class ValidationError(ValueError):
    pass


class Field:
    sql_type = "TEXT"

    def __init__(self, *, null: bool = False, default=_MISSING, index: bool = False) -> None:
        self.null = null
        self.default = default
        self.index = index
        self.name = ""
        self.column = ""
        self.model: type | None = None

    def __set_name__(self, owner: type, name: str) -> None:
        self.name = name
        self.column = name
        self.model = owner

    def __get__(self, obj, objtype=None):
        if obj is None:
            return Column(self, objtype)
        try:
            return obj.__dict__[self.name]
        except KeyError:
            raise AttributeError(f"{type(obj).__name__}.{self.name} is not loaded") from None

    def __set__(self, obj, value) -> None:
        if value is None:
            if not self.null:
                raise ValidationError(f"{type(obj).__name__}.{self.name} may not be null")
        else:
            value = self.validate(value)
        state = obj.__dict__
        old = state.get(self.name, _MISSING)
        if old is _MISSING or old != value:
            state[self.name] = value
            if old is not _MISSING:
                obj._dirty[self.name] = None

    def validate(self, value):
        return value

    def to_db(self, value):
        return value

    def from_db(self, value):
        return value

    def ddl(self) -> str:
        return f"{self.column} {self.sql_type}{'' if self.null else ' NOT NULL'}"


class IntegerField(Field):
    sql_type = "INTEGER"

    def __init__(self, *, min_value: int | None = None, max_value: int | None = None, **kw):
        super().__init__(**kw)
        self.min_value, self.max_value = min_value, max_value

    def validate(self, value):
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValidationError(f"{self.name}: expected int, got {type(value).__name__}")
        if self.min_value is not None and value < self.min_value:
            raise ValidationError(f"{self.name}: {value} < {self.min_value}")
        if self.max_value is not None and value > self.max_value:
            raise ValidationError(f"{self.name}: {value} > {self.max_value}")
        return value


class PrimaryKey(IntegerField):
    def ddl(self) -> str:
        return f"{self.column} INTEGER PRIMARY KEY"

    def __set__(self, obj, value) -> None:
        if value is not None:
            value = self.validate(value)
        obj.__dict__[self.name] = value


class TextField(Field):
    def __init__(self, *, max_length: int = 255, **kw) -> None:
        super().__init__(**kw)
        self.max_length = max_length

    def validate(self, value):
        if not isinstance(value, str):
            raise ValidationError(f"{self.name}: expected str")
        if len(value) > self.max_length:
            raise ValidationError(f"{self.name}: longer than {self.max_length}")
        return value


class BoolField(Field):
    sql_type = "INTEGER"

    def validate(self, value):
        return bool(value)

    def to_db(self, value):
        return int(value)

    def from_db(self, value):
        return bool(value)


class DateField(Field):
    def validate(self, value):
        if not isinstance(value, date):
            raise ValidationError(f"{self.name}: expected date")
        return value

    def to_db(self, value):
        return value.isoformat()

    def from_db(self, value):
        return date.fromisoformat(value) if value is not None else None


class DecimalField(Field):
    def __init__(self, *, places: int = 2, **kw) -> None:
        super().__init__(**kw)
        self.quantum = Decimal(1).scaleb(-places)

    def validate(self, value):
        return Decimal(value).quantize(self.quantum)

    def to_db(self, value):
        return str(value)

    def from_db(self, value):
        return Decimal(value) if value is not None else None


class EnumField(Field):
    def __init__(self, enum_type: type[enum.Enum], **kw) -> None:
        super().__init__(**kw)
        self.enum_type = enum_type

    def validate(self, value):
        return self.enum_type(value)

    def to_db(self, value):
        return value.value

    def from_db(self, value):
        return self.enum_type(value)


class LazyRef:
    """Proxy for a related row: loads it on first attribute access."""

    __slots__ = ("_model", "_pk", "_session", "_target")

    def __init__(self, model: type, pk: int, session: "Session") -> None:
        object.__setattr__(self, "_model", model)
        object.__setattr__(self, "_pk", pk)
        object.__setattr__(self, "_session", session)
        object.__setattr__(self, "_target", None)

    def _resolve(self):
        target = self._target
        if target is None:
            target = self._session.get(self._model, self._pk)
            object.__setattr__(self, "_target", target)
        return target

    def __getattr__(self, name: str):
        if name == "id":
            return self._pk
        return getattr(self._resolve(), name)

    def __setattr__(self, name: str, value) -> None:
        setattr(self._resolve(), name, value)

    def __eq__(self, other) -> bool:
        if isinstance(other, LazyRef):
            return self._model is other._model and self._pk == other._pk
        if isinstance(other, Model):
            return type(other) is self._model and other.id == self._pk
        return NotImplemented

    def __hash__(self) -> int:
        return hash((self._model.__name__, self._pk))

    def __repr__(self) -> str:
        return f"<LazyRef {self._model.__name__}#{self._pk}>"


class ForeignKey(Field):
    sql_type = "INTEGER"

    def __init__(self, target: str, **kw) -> None:
        super().__init__(**kw)
        self.target_name = target

    def __set_name__(self, owner: type, name: str) -> None:
        super().__set_name__(owner, name)
        self.column = name + "_id"

    @property
    def target(self) -> type:
        return REGISTRY[self.target_name]

    def validate(self, value):
        if isinstance(value, (Model, LazyRef)):
            return value
        raise ValidationError(f"{self.name}: expected a {self.target_name}")

    def to_db(self, value):
        if value is None or isinstance(value, int):
            return value
        return value.id


# --------------------------------------------------------------------------------------
# models
# --------------------------------------------------------------------------------------

REGISTRY: dict[str, type] = {}


class Options:
    def __init__(self, model: type, table: str, fields: list[Field]) -> None:
        self.model = model
        self.table = table
        self.fields = fields
        self.by_column = {f.column: f for f in fields}
        self.pk = next(f for f in fields if isinstance(f, PrimaryKey))
        self.columns = [f.column for f in fields]
        self.select_list = ", ".join(f"{table}.{c}" for c in self.columns)


class ModelMeta(type):
    def __new__(mcls, name, bases, namespace, **kwargs):
        cls = super().__new__(mcls, name, bases, namespace, **kwargs)
        fields: list[Field] = []
        for klass in reversed(cls.__mro__):
            for value in vars(klass).values():
                if isinstance(value, Field) and value not in fields:
                    fields.append(value)
        if namespace.get("__abstract__"):
            cls._meta = None
            return cls
        cls._meta = Options(cls, getattr(cls, "__table__", name.lower() + "s"), fields)
        REGISTRY[name] = cls
        return cls

    def __repr__(cls) -> str:
        return f"<model {cls.__name__}>"


class Model(metaclass=ModelMeta):
    __abstract__ = True
    id = PrimaryKey(null=True)

    def __init_subclass__(cls, table: str | None = None, **kwargs) -> None:
        super().__init_subclass__(**kwargs)
        if table is not None:
            cls.__table__ = table

    def __init__(self, **values) -> None:
        self.__dict__["_dirty"] = {}
        for f in self._meta.fields:
            value = values.pop(f.name, _MISSING)
            if value is _MISSING:
                value = f.default() if callable(f.default) else f.default
                if value is _MISSING:
                    if f is self._meta.pk:
                        value = None
                    else:
                        raise TypeError(f"{type(self).__name__}() missing {f.name!r}")
            setattr(self, f.name, value)
        if values:
            raise TypeError(f"unexpected fields {sorted(values)}")

    @classmethod
    def _from_row(cls, row, session: "Session") -> "Model":
        obj = cls.__new__(cls)
        state = obj.__dict__
        state["_dirty"] = {}
        for f, raw in zip(cls._meta.fields, row):
            if isinstance(f, ForeignKey):
                state[f.name] = LazyRef(f.target, raw, session) if raw is not None else None
            else:
                state[f.name] = f.from_db(raw) if raw is not None else None
        return obj

    def __eq__(self, other) -> bool:
        if type(other) is not type(self):
            return NotImplemented
        if self.id is None or other.id is None:
            return self is other
        return self.id == other.id

    def __hash__(self) -> int:
        return hash((type(self).__name__, self.id))

    def __repr__(self) -> str:
        shown = ", ".join(f"{f.name}={self.__dict__.get(f.name)!r}"
                          for f in self._meta.fields[:3])
        return f"{type(self).__name__}({shown})"


# --------------------------------------------------------------------------------------
# query and session
# --------------------------------------------------------------------------------------


class Query:
    def __init__(self, session: "Session", model: type) -> None:
        self.session = session
        self.model = model
        self._where: list[Expr] = []
        self._order: list[Ordering] = []
        self._limit: int | None = None
        self._offset = 0

    def _clone(self) -> "Query":
        q = copy.copy(self)
        q._where = list(self._where)
        q._order = list(self._order)
        return q

    def filter(self, *exprs: Expr) -> "Query":
        q = self._clone()
        q._where.extend(exprs)
        return q

    def exclude(self, expr: Expr) -> "Query":
        return self.filter(~expr)

    def order_by(self, *orderings) -> "Query":
        q = self._clone()
        q._order.extend(o if isinstance(o, Ordering) else o.asc() for o in orderings)
        return q

    def limit(self, n: int, offset: int = 0) -> "Query":
        q = self._clone()
        q._limit, q._offset = n, offset
        return q

    def compile(self, select: str | None = None) -> tuple[str, list]:
        meta = self.model._meta
        params: list = []
        sql = f"SELECT {select or meta.select_list} FROM {meta.table}"
        if self._where:
            sql += " WHERE " + BoolOp("AND", tuple(self._where)).compile(params)
        if self._order and select is None:
            sql += " ORDER BY " + ", ".join(o.compile() for o in self._order)
        if self._limit is not None:
            sql += f" LIMIT {self._limit} OFFSET {self._offset}"
        return sql, params

    def __iter__(self):
        sql, params = self.compile()
        return iter(self.session.hydrate(self.model, self.session.execute(sql, params)))

    def all(self) -> list:
        return list(self)

    def first(self):
        for obj in self.limit(1):
            return obj
        return None

    def count(self) -> int:
        sql, params = self.compile("COUNT(*)")
        return self.session.execute(sql, params)[0][0]


class Session:
    def __init__(self, conn: sqlite3.Connection) -> None:
        self.conn = conn
        self.identity: weakref.WeakValueDictionary = weakref.WeakValueDictionary()
        self.new: list[Model] = []
        self.tracked: dict[tuple[str, int], Model] = {}
        self.stats: Counter[str] = Counter()

    def query(self, model: type) -> Query:
        return Query(self, model)

    def execute(self, sql: str, params=()) -> list:
        self.stats["statements"] += 1
        return self.conn.execute(sql, params).fetchall()

    def hydrate(self, model: type, rows: list) -> list:
        out = []
        name = model.__name__
        for row in rows:
            key = (name, row[0])
            obj = self.identity.get(key)
            if obj is None:
                obj = model._from_row(row, self)
                self.identity[key] = obj
                self.stats["hydrated"] += 1
            out.append(obj)
            self.tracked[key] = obj
        return out

    def get(self, model: type, pk: int):
        obj = self.identity.get((model.__name__, pk))
        if obj is not None:
            return obj
        return self.query(model).filter(model.id == pk).first()

    def add(self, obj: Model) -> None:
        self.new.append(obj)

    def create_all(self) -> None:
        for model in REGISTRY.values():
            meta = model._meta
            cols = ", ".join(f.ddl() for f in meta.fields)
            self.conn.execute(f"CREATE TABLE {meta.table} ({cols})")
            for f in meta.fields:
                if f.index:
                    self.conn.execute(f"CREATE INDEX ix_{meta.table}_{f.column} "
                                      f"ON {meta.table} ({f.column})")

    def flush(self) -> None:
        by_model: defaultdict[type, list[Model]] = defaultdict(list)
        for obj in self.new:
            by_model[type(obj)].append(obj)
        for model, objs in by_model.items():
            meta = model._meta
            fields = [f for f in meta.fields if f is not meta.pk]
            sql = (f"INSERT INTO {meta.table} ({', '.join(f.column for f in fields)}) "
                   f"VALUES ({', '.join('?' * len(fields))})")
            for obj in objs:
                state = obj.__dict__
                row = [None if state[f.name] is None else f.to_db(state[f.name]) for f in fields]
                obj.__dict__["id"] = self.conn.execute(sql, row).lastrowid
                key = (model.__name__, obj.id)
                self.identity[key] = obj
                self.tracked[key] = obj
                self.stats["inserted"] += 1
        self.new.clear()
        for obj in self.tracked.values():
            if not obj._dirty:
                continue
            meta = obj._meta
            changed = [meta.by_column[f.column] for f in meta.fields if f.name in obj._dirty]
            sets = ", ".join(f"{f.column} = ?" for f in changed)
            values = [None if obj.__dict__[f.name] is None else f.to_db(obj.__dict__[f.name])
                      for f in changed]
            self.conn.execute(f"UPDATE {meta.table} SET {sets} WHERE id = ?", values + [obj.id])
            obj._dirty.clear()
            self.stats["updated"] += 1

    def commit(self) -> None:
        self.flush()
        self.conn.commit()
        self.tracked.clear()


# --------------------------------------------------------------------------------------
# the application's models
# --------------------------------------------------------------------------------------


class Status(enum.Enum):
    NEW = "new"
    OPEN = "open"
    PENDING = "pending"
    SOLVED = "solved"
    CLOSED = "closed"


class Channel(enum.Enum):
    EMAIL = "email"
    CHAT = "chat"
    PHONE = "phone"
    WEB = "web"


class TimestampedModel(Model):
    __abstract__ = True
    created = DateField(index=True)


class Customer(TimestampedModel, table="customers"):
    name = TextField(max_length=80)
    email = TextField(max_length=120, index=True)
    plan = TextField(max_length=20, default="free")
    mrr = DecimalField(places=2, default=Decimal("0"))
    vip = BoolField(default=False)


class Agent(Model, table="agents"):
    name = TextField(max_length=80)
    team = TextField(max_length=40, index=True)
    active = BoolField(default=True)
    capacity = IntegerField(min_value=1, max_value=60, default=25)


class Ticket(TimestampedModel, table="tickets"):
    customer = ForeignKey("Customer", index=True)
    agent = ForeignKey("Agent", null=True, default=None, index=True)
    subject = TextField(max_length=200)
    status = EnumField(Status, default=Status.NEW)
    channel = EnumField(Channel, default=Channel.EMAIL)
    priority = IntegerField(min_value=1, max_value=4, default=2)
    due = DateField(null=True, default=None)


class Comment(TimestampedModel, table="comments"):
    ticket = ForeignKey("Ticket", index=True)
    author = ForeignKey("Agent", null=True, default=None)
    body = TextField(max_length=2_000)
    public = BoolField(default=True)


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------

SUBJECTS = ["cannot log in", "invoice question", "refund request", "bug report: export",
            "feature request", "password reset", "API rate limit", "data import failed",
            "slow dashboard", "cancel subscription", "upgrade plan", "SSO setup"]
TEAMS = ["billing", "tier1", "tier2", "integrations", "enterprise"]


def setup():
    import random

    rng = random.Random(1234)
    start = date(2026, 1, 1)
    customers = [(f"Customer {i}", f"c{i}@example{i % 37}.com",
                  rng.choice(["free", "free", "pro", "team", "enterprise"]),
                  f"{rng.choice([0, 0, 29, 99, 499, 2_400]) * rng.uniform(0.9, 1.1):.2f}",
                  rng.random() < 0.05, rng.randrange(300)) for i in range(N_CUSTOMERS)]
    agents = [(f"Agent {i}", TEAMS[i % len(TEAMS)], rng.random() > 0.1, rng.randint(10, 40))
              for i in range(N_AGENTS)]
    tickets = [(rng.randrange(N_CUSTOMERS) + 1, rng.choice([None, None] + list(range(1, N_AGENTS))),
                f"{rng.choice(SUBJECTS)} #{i}", rng.choice(["new", "open", "open", "pending",
                                                             "solved", "closed", "closed"]),
                rng.choice(["email", "chat", "phone", "web"]), rng.choice([1, 2, 2, 2, 3, 4]),
                300 + rng.randrange(60)) for i in range(N_TICKETS)]
    comments = [(rng.randrange(N_TICKETS) + 1, rng.choice([None] + list(range(1, N_AGENTS))),
                 " ".join(rng.choice(["thanks", "please", "check", "logs", "attached", "update",
                                      "we", "fixed", "the", "issue", "again", "soon"])
                          for _ in range(rng.randint(4, 40))), rng.random() < 0.8,
                 300 + rng.randrange(60)) for _ in range(N_COMMENTS)]
    work = [(rng.randrange(N_AGENTS) + 1, rng.choice(TEAMS), rng.randrange(N_CUSTOMERS) + 1,
             rng.choice(SUBJECTS).split()[0]) for _ in range(700)]
    return {"start": start, "customers": customers, "agents": agents, "tickets": tickets,
            "comments": comments, "work": work}


def run(state):
    start = state["start"]
    conn = sqlite3.connect(":memory:")
    session = Session(conn)
    session.create_all()
    errors = Counter()

    # -- load ------------------------------------------------------------------------------
    for name, email, plan, mrr, vip, day in state["customers"]:
        session.add(Customer(name=name, email=email, plan=plan, mrr=mrr, vip=vip,
                             created=start + timedelta(days=day)))
    for name, team, active, cap in state["agents"]:
        session.add(Agent(name=name, team=team, active=active, capacity=cap))
    session.commit()
    customers = {c.id: c for c in session.query(Customer)}
    agents = {a.id: a for a in session.query(Agent)}
    for cust, agent, subject, status, channel, prio, day in state["tickets"]:
        try:
            session.add(Ticket(customer=customers[cust], agent=agents.get(agent),
                               subject=subject, status=status, channel=channel, priority=prio,
                               created=start + timedelta(days=day),
                               due=start + timedelta(days=day + 5 - prio)))
        except ValidationError:
            errors["ticket"] += 1
    session.commit()
    for ticket_id, author, body, public, day in state["comments"]:
        try:
            session.add(Comment(ticket=LazyRef(Ticket, ticket_id, session),
                                author=agents.get(author), body=body, public=public,
                                created=start + timedelta(days=day)))
        except ValidationError:
            errors["comment"] += 1
    session.commit()

    # -- a working day ---------------------------------------------------------------------
    today = start + timedelta(days=345)
    report = Counter()
    queue_sizes = []
    for agent_id, team, customer_id, word in state["work"]:
        agent = session.get(Agent, agent_id)
        # my queue: open or pending tickets assigned to me, most urgent first
        mine = (session.query(Ticket)
                .filter(Ticket.agent == agent, Ticket.status.in_([Status.OPEN, Status.PENDING]))
                .order_by(Ticket.priority.desc(), Ticket.due.asc(), Ticket.id))
        queue = mine.limit(agent.capacity).all()
        queue_sizes.append(len(queue))
        for t in queue[:6]:
            if t.due is not None and t.due < today and t.priority < 4:
                t.priority += 1
                report["escalated"] += 1
            if t.customer.vip and t.status is Status.PENDING:
                t.status = Status.OPEN
                report["vip_reopened"] += 1
        # unassigned tickets for the team, oldest first: pick up a few
        unassigned = (session.query(Ticket)
                      .filter(Ticket.agent == None, Ticket.status == Status.NEW)  # noqa: E711
                      .order_by(Ticket.created, Ticket.id).limit(3))
        for t in unassigned:
            t.agent = agent
            t.status = Status.OPEN
            report["assigned"] += 1
        # customer history view
        history = (session.query(Ticket).filter(Ticket.customer == customer_id)
                   .order_by(Ticket.created.desc()).limit(20).all())
        report["history_rows"] += len(history)
        for t in history[:5]:
            comments = (session.query(Comment).filter(Comment.ticket == t.id,
                                                      Comment.public == True)  # noqa: E712
                        .order_by(Comment.created, Comment.id).all())
            report["comments_read"] += len(comments)
            for c in comments:
                if c.author is not None:
                    report[f"team:{c.author.team}"] += 1
        # search
        hits = (session.query(Ticket)
                .filter(Ticket.subject.like(f"{word}%"),
                        (Ticket.priority >= 3) | (Ticket.channel == Channel.PHONE))
                .exclude(Ticket.status.in_([Status.CLOSED]))
                .limit(30).all())
        report["search_hits"] += len(hits)
        # solve the oldest ticket in my queue
        if queue:
            t = queue[-1]
            t.status = Status.SOLVED
            session.add(Comment(ticket=t, author=agent, body=f"Solved by {agent.name}",
                                public=True, created=today))
            report["solved"] += 1
        session.commit()

    # -- SLA report --------------------------------------------------------------------------
    sla = defaultdict(lambda: [0, 0])
    overdue_vip = []
    for t in session.query(Ticket).filter(Ticket.status.in_([Status.NEW, Status.OPEN,
                                                             Status.PENDING])):
        bucket = sla[(t.priority, t.channel.value)]
        bucket[0] += 1
        if t.due is not None and t.due < today:
            bucket[1] += 1
            if t.customer.vip:
                overdue_vip.append(t.id)
    mrr_at_risk = sum((session.get(Customer, cid).mrr for cid in
                       {t.customer.id for t in session.query(Ticket).filter(
                           Ticket.priority == 4, Ticket.status != Status.CLOSED)}), Decimal("0"))
    counts = {s.value: session.query(Ticket).filter(Ticket.status == s).count() for s in Status}
    try:
        Ticket(customer=customers[1], subject="x" * 300, created=today)
    except ValidationError as exc:
        errors[str(exc)] += 1
    sample = repr(session.query(Ticket).order_by(Ticket.id).first())
    conn.close()
    return {
        "counts": counts,
        "report": dict(sorted(report.items())),
        "queue_sizes": [sum(queue_sizes), max(queue_sizes)],
        "sla": sorted([k[0], k[1], v[0], v[1]] for k, v in sla.items()),
        "overdue_vip": sorted(overdue_vip)[:50],
        "mrr_at_risk": str(mrr_at_risk),
        "stats": {k: session.stats[k] for k in ("statements", "inserted", "updated")},
        "errors": dict(sorted(errors.items())),
        "sample": sample,
    }


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
