"""Customer statements and management reports from an in-house template engine.

Stands for the reporting, invoicing, notification and document-generation code every
business application grows (and for the shape of Jinja2/Django template rendering): a
small template language (``{{ value.path | filter(arg) }}``, ``{% for %}``, ``{% if %}``)
tokenized with ``re`` and compiled to a tree of Python closures; dotted lookups that try
``getattr`` then ``__getitem__`` (exceptions on the miss path); a decorator-registered
filter library; ``collections.ChainMap`` scopes and a ``loop`` helper with properties;
domain value objects with ``__format__`` (custom format specs), ``__str__`` and
``__repr__``; message templates through ``str.format_map`` with a ``__missing__`` dict
and attribute/format-spec fields; fixed-width text tables with ``ljust``/``rjust``/
``center`` and nested f-string specs; ``html.escape`` for the HTML variant; and
Markdown/CSV exports built with ``str.join``.

Run: 2,200 customer statements (text and HTML, 0-40 lines each), their notification
e-mails, a Markdown management summary and a CSV export; ~9 MB of output text.
"""

import enum
import html
import re
from collections import ChainMap, defaultdict
from dataclasses import dataclass, field
from datetime import date, timedelta
from decimal import ROUND_HALF_UP, Decimal

REQUIRES: list[str] = []

N_CUSTOMERS = 2_200


# --------------------------------------------------------------------------------------
# value objects with custom formatting
# --------------------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Money:
    amount: Decimal
    currency: str

    SYMBOLS = {"EUR": "€", "USD": "$", "GBP": "£", "SEK": "kr"}

    def __add__(self, other: "Money") -> "Money":
        return Money(self.amount + other.amount, self.currency)

    def __sub__(self, other: "Money") -> "Money":
        return Money(self.amount - other.amount, self.currency)

    def __mul__(self, k) -> "Money":
        return Money((self.amount * k).quantize(Decimal("0.01"), ROUND_HALF_UP), self.currency)

    def __neg__(self) -> "Money":
        return Money(-self.amount, self.currency)

    def __lt__(self, other: "Money") -> bool:
        return self.amount < other.amount

    def __format__(self, spec: str) -> str:
        # "sym" -> €1,234.50 ; "code" -> 1,234.50 EUR ; anything else: a Decimal spec
        if spec in ("", "sym"):
            sign = "-" if self.amount < 0 else ""
            return f"{sign}{self.SYMBOLS.get(self.currency, '')}{abs(self.amount):,.2f}"
        if spec == "code":
            return f"{self.amount:,.2f} {self.currency}"
        return format(self.amount, spec)

    def __str__(self) -> str:
        return format(self, "code")

    def __repr__(self) -> str:
        return f"Money({str(self.amount)!r}, {self.currency!r})"


@dataclass(frozen=True, slots=True)
class Percent:
    value: Decimal

    def __format__(self, spec: str) -> str:
        digits = int(spec) if spec.isdigit() else 1
        return f"{self.value * 100:.{digits}f}%"


@dataclass(frozen=True, slots=True)
class Duration:
    seconds: int

    def __format__(self, spec: str) -> str:
        h, rest = divmod(self.seconds, 3600)
        m, s = divmod(rest, 60)
        if spec == "short":
            return f"{h}h{m:02d}" if h else f"{m}m{s:02d}"
        return f"{h:02d}:{m:02d}:{s:02d}"


class Tier(enum.Enum):
    BRONZE = "bronze"
    SILVER = "silver"
    GOLD = "gold"
    PLATINUM = "platinum"

    def __str__(self) -> str:
        return self.value.title()


@dataclass(slots=True)
class Line:
    day: date
    description: str
    quantity: int
    unit: Money
    usage: Duration | None = None

    @property
    def total(self) -> Money:
        return self.unit * self.quantity


@dataclass
class Customer:
    number: str
    name: str
    email: str
    tier: Tier
    currency: str
    address: dict
    lines: list[Line] = field(default_factory=list)
    credits: list[Money] = field(default_factory=list)
    discount: Percent = Percent(Decimal("0"))

    @property
    def subtotal(self) -> Money:
        total = Money(Decimal("0"), self.currency)
        for line in self.lines:
            total = total + line.total
        return total

    @property
    def discount_amount(self) -> Money:
        return self.subtotal * self.discount.value

    @property
    def credit_total(self) -> Money:
        total = Money(Decimal("0"), self.currency)
        for c in self.credits:
            total = total + c
        return total

    @property
    def total(self) -> Money:
        return self.subtotal - self.discount_amount - self.credit_total

    @property
    def usage(self) -> Duration:
        return Duration(sum(ln.usage.seconds for ln in self.lines if ln.usage))

    def __repr__(self) -> str:
        return f"<Customer {self.number} {self.name!r} {self.tier.name}>"


# --------------------------------------------------------------------------------------
# template engine
# --------------------------------------------------------------------------------------


class TemplateError(Exception):
    pass


class Undefined:
    __slots__ = ("name",)

    def __init__(self, name: str) -> None:
        self.name = name

    def __str__(self) -> str:
        return ""

    def __bool__(self) -> bool:
        return False

    def __iter__(self):
        return iter(())

    def __format__(self, spec: str) -> str:
        return ""


FILTERS: dict = {}


def template_filter(name: str):
    def register(fn):
        FILTERS[name] = fn
        return fn
    return register


@template_filter("upper")
def _upper(v):
    return str(v).upper()


@template_filter("title")
def _title(v):
    return str(v).title()


@template_filter("default")
def _default(v, fallback=""):
    return v if v or v == 0 else fallback


@template_filter("format")
def _format(v, spec=""):
    return format(v, spec)


@template_filter("truncate")
def _truncate(v, n=40):
    s = str(v)
    return s if len(s) <= n else s[: n - 1] + "…"


@template_filter("ljust")
def _ljust(v, n):
    return str(v).ljust(n)


@template_filter("rjust")
def _rjust(v, n):
    return str(v).rjust(n)


@template_filter("date")
def _date(v, fmt="%Y-%m-%d"):
    return v.strftime(fmt) if v else ""


@template_filter("escape")
def _escape(v):
    return html.escape(str(v))


@template_filter("join")
def _join(v, sep=", "):
    return sep.join(str(x) for x in v)


@template_filter("length")
def _length(v):
    return len(v)


class Loop:
    __slots__ = ("index0", "length")

    def __init__(self, length: int) -> None:
        self.index0 = 0
        self.length = length

    @property
    def index(self) -> int:
        return self.index0 + 1

    @property
    def first(self) -> bool:
        return self.index0 == 0

    @property
    def last(self) -> bool:
        return self.index0 == self.length - 1

    @property
    def odd(self) -> bool:
        return self.index0 % 2 == 1


TOKEN = re.compile(r"({{.*?}}|{%.*?%})", re.S)
FILTER_CALL = re.compile(r"^(\w+)(?:\((.*)\))?$")
LITERAL = re.compile(r"""^(?:"([^"]*)"|'([^']*)'|(-?\d+))$""")


def lookup(scope, path: list[str]):
    name = path[0]
    try:
        value = scope[name]
    except KeyError:
        return Undefined(name)
    for part in path[1:]:
        try:
            value = getattr(value, part)
        except AttributeError:
            try:
                value = value[part]
            except (KeyError, TypeError, IndexError):
                return Undefined(".".join(path))
    return value


def compile_literal(text: str):
    m = LITERAL.match(text.strip())
    if m is None:
        path = text.strip().split(".")
        return lambda scope: lookup(scope, path)
    s1, s2, num = m.groups()
    value = int(num) if num is not None else (s1 if s1 is not None else s2)
    return lambda scope: value


def compile_expression(source: str):
    parts = [p.strip() for p in source.split("|")]
    getter = compile_literal(parts[0])
    pipeline = []
    for part in parts[1:]:
        m = FILTER_CALL.match(part)
        if m is None or m.group(1) not in FILTERS:
            raise TemplateError(f"unknown filter {part!r}")
        args = [compile_literal(a) for a in m.group(2).split(",")] if m.group(2) else []
        pipeline.append((FILTERS[m.group(1)], args))

    def evaluate(scope):
        value = getter(scope)
        for fn, args in pipeline:
            value = fn(value, *[a(scope) for a in args])
        return value
    return evaluate


def compile_condition(source: str):
    source = source.strip()
    if source.startswith("not "):
        inner = compile_condition(source[4:])
        return lambda scope: not inner(scope)
    for op in (" >= ", " <= ", " == ", " != ", " > ", " < "):
        if op in source:
            left, right = source.split(op, 1)
            lf, rf = compile_expression(left), compile_expression(right)
            fn = {" >= ": lambda a, b: a >= b, " <= ": lambda a, b: a <= b,
                  " == ": lambda a, b: a == b, " != ": lambda a, b: a != b,
                  " > ": lambda a, b: a > b, " < ": lambda a, b: a < b}[op]
            return lambda scope: fn(lf(scope), rf(scope))
    expr = compile_expression(source)
    return lambda scope: bool(expr(scope))


def compile_template(source: str):
    """Compile to a render(scope, out) closure tree."""
    tokens = [t for t in TOKEN.split(source) if t]
    pos = 0

    def parse_block(stop: tuple[str, ...]):
        nonlocal pos
        nodes = []
        while pos < len(tokens):
            tok = tokens[pos]
            if tok.startswith("{%"):
                stmt = tok[2:-2].strip()
                word = stmt.split()[0]
                if word in stop:
                    return nodes, stmt
                pos += 1
                if word == "for":
                    m = re.match(r"for (\w+) in (.+)", stmt)
                    var, iterable = m.group(1), compile_expression(m.group(2))
                    body, _ = parse_block(("endfor",))
                    pos += 1
                    nodes.append(make_for(var, iterable, body))
                elif word == "if":
                    branches = []
                    cond = compile_condition(stmt[3:])
                    while True:
                        body, end = parse_block(("elif", "else", "endif"))
                        pos += 1
                        branches.append((cond, body))
                        if end.startswith("elif"):
                            cond = compile_condition(end[5:])
                        elif end == "else":
                            cond = None
                        else:
                            break
                    nodes.append(make_if(branches))
                else:
                    raise TemplateError(f"unknown tag {word!r}")
            elif tok.startswith("{{"):
                pos += 1
                nodes.append(make_var(compile_expression(tok[2:-2])))
            else:
                pos += 1
                nodes.append(make_text(tok))
        if stop:
            raise TemplateError(f"missing {stop[-1]}")
        return nodes, ""

    def make_text(text: str):
        def render(scope, out):
            out.append(text)
        return render

    def make_var(expr):
        def render(scope, out):
            value = expr(scope)
            out.append(value if isinstance(value, str) else format(value, ""))
        return render

    def make_for(var: str, iterable, body):
        def render(scope, out):
            items = list(iterable(scope))
            loop = Loop(len(items))
            inner = scope.new_child({"loop": loop})
            for item in items:
                inner[var] = item
                for node in body:
                    node(inner, out)
                loop.index0 += 1
        return render

    def make_if(branches):
        def render(scope, out):
            for cond, body in branches:
                if cond is None or cond(scope):
                    for node in body:
                        node(scope, out)
                    return
        return render

    nodes, _ = parse_block(())

    def render_template(**context) -> str:
        out: list[str] = []
        scope = ChainMap(context, GLOBALS)
        for node in nodes:
            node(scope, out)
        return "".join(out)
    return render_template


GLOBALS = {"company": {"name": "Northwind Analytics AB", "vat": "SE556677889901",
                       "support": "billing@northwind.example"}}


class SafeFormat(dict):
    def __missing__(self, key: str) -> str:
        return "{" + key + "}"


# --------------------------------------------------------------------------------------
# templates
# --------------------------------------------------------------------------------------

STATEMENT_TXT = """\
{{ company.name | upper }}                                   VAT {{ company.vat }}
Statement {{ customer.number }} for {{ customer.name | title }} ({{ customer.tier }})
{{ customer.address.street }}, {{ customer.address.postcode }} {{ customer.address.city | upper }}
Period: {{ period.start | date }} - {{ period.end | date("%d %b %Y") }}
{% if customer.lines %}
{{ header }}
{% for line in customer.lines %}{{ loop.index | rjust(3) }} {{ line.day | date("%d.%m") }} {{ line.description | truncate(34) | ljust(34) }} {{ line.quantity | rjust(5) }} {{ line.unit | format("sym") | rjust(12) }} {{ line.total | format("sym") | rjust(13) }}{% if line.usage %} [{{ line.usage | format("short") }}]{% endif %}
{% endfor %}{{ rule }}
{{ "Subtotal" | ljust(58) }}{{ customer.subtotal | format("sym") | rjust(15) }}
{% if customer.discount.value > 0 %}{{ "Discount" | ljust(44) }}{{ customer.discount | format("1") | rjust(12) }}  {{ customer.discount_amount | format("sym") | rjust(15) }}
{% endif %}{% for credit in customer.credits %}{{ "Credit note" | ljust(58) }}{{ credit | format("sym") | rjust(15) }}
{% endfor %}{{ "TOTAL DUE" | ljust(58) }}{{ customer.total | format("code") | rjust(15) }}
Usage this period: {{ customer.usage }}
{% else %}
No activity this period.
{% endif %}
Questions? {{ company.support }} - reference {{ customer.number }}{{ missing.field | default("") }}
"""

STATEMENT_HTML = """\
<article class="statement tier-{{ customer.tier.value }}">
<h1>{{ customer.name | escape }}</h1>
<p>{{ customer.address.street | escape }}<br>{{ customer.address.city | escape }}</p>
<table>
{% for line in customer.lines %}<tr class="{% if loop.odd %}odd{% else %}even{% endif %}"><td>{{ line.day | date }}</td><td>{{ line.description | escape }}</td><td class="n">{{ line.quantity }}</td><td class="n">{{ line.total | format("sym") | escape }}</td></tr>
{% endfor %}</table>
<p class="total">{{ customer.total | format("sym") | escape }}</p>
{% if customer.total.amount < 0 %}<p class="note">You have a credit balance.</p>{% endif %}
</article>
"""

EMAIL_SUBJECT = "[{company}] Statement {number} for {customer.name}: {total:sym} due {due:%d %B}"
EMAIL_BODY = ("Dear {customer.name},\n\nyour {tier} statement {number} totals {total:code} "
              "({lines} lines, discount {discount:2}). Payment is due on {due:%A %d %B %Y}.\n"
              "Usage: {usage:short}. Account manager: {manager}.\n")


# --------------------------------------------------------------------------------------
# workload
# --------------------------------------------------------------------------------------

PRODUCTS = [("Analytics seat", "19.00"), ("Data warehouse storage (GB)", "0.023"),
            ("Query compute (credit)", "2.40"), ("Premium support", "450.00"),
            ("Dashboard embed", "79.00"), ("API calls (1k)", "0.35"),
            ("Training session <onsite>", "1200.00"), ("Connector: Salesforce & HubSpot", "99.00")]
CITIES = ["Stockholm", "Göteborg", "Malmö", "Uppsala", "Berlin", "London", "Austin"]


def setup():
    import random

    rng = random.Random(55_555)
    customers = []
    for i in range(N_CUSTOMERS):
        cur = rng.choice(["EUR", "EUR", "SEK", "USD", "GBP"])
        lines = []
        for _ in range(rng.choice([0, 4, 6, 8, 12, 18, 25, 40])):
            prod, price = rng.choice(PRODUCTS)
            lines.append((rng.randint(1, 30), prod, rng.randint(1, 400), price,
                          rng.randint(60, 40_000) if rng.random() < 0.4 else None))
        customers.append({
            "number": f"ST-2026-10-{i:05d}",
            "name": rng.choice(["acme", "globex", "initech", "umbrella", "hooli", "stark",
                                "wayne", "tyrell"]) + f" {rng.choice(['ab', 'gmbh', 'ltd', 'inc'])} {i}",
            "email": f"billing{i}@example.com",
            "tier": rng.choice(list(Tier)).value,
            "currency": cur,
            "address": {"street": f"{rng.randint(1, 120)} Storgatan", "postcode": f"{rng.randint(10000, 99999)}",
                        "city": rng.choice(CITIES)},
            "lines": lines,
            "credits": [f"{rng.uniform(5, 300):.2f}" for _ in range(rng.choice([0, 0, 0, 1, 2]))],
            "discount": rng.choice(["0", "0", "0.05", "0.1", "0.15"]),
            "manager": rng.choice(["Anna", "Björn", "Chloé", "Dmitri", None]),
        })
    return {"customers": customers}


def run(state):
    period_start = date(2026, 10, 1)
    period = {"start": period_start, "end": date(2026, 10, 31)}
    txt = compile_template(STATEMENT_TXT)
    htm = compile_template(STATEMENT_HTML)
    header = (f"{'#':>3} {'Date':<5} {'Description':<34} {'Qty':>5} {'Unit':>12} {'Amount':>13}")
    rule = "-" * len(header)
    statements, pages, emails = [], [], []
    by_tier: defaultdict[Tier, list[Money]] = defaultdict(list)
    for raw in state["customers"]:
        cur = raw["currency"]
        customer = Customer(
            raw["number"], raw["name"], raw["email"], Tier(raw["tier"]), cur, raw["address"],
            [Line(period_start + timedelta(days=d - 1), desc, q, Money(Decimal(p), cur),
                  Duration(u) if u else None) for d, desc, q, p, u in raw["lines"]],
            [Money(Decimal(c), cur) for c in raw["credits"]],
            Percent(Decimal(raw["discount"])))
        customer.lines.sort(key=lambda ln: (ln.day, ln.description))
        statements.append(txt(customer=customer, period=period, header=header, rule=rule))
        pages.append(htm(customer=customer))
        due = period["end"] + timedelta(days=30 if customer.tier is not Tier.PLATINUM else 45)
        fields = SafeFormat(company=GLOBALS["company"]["name"], number=customer.number,
                            customer=customer, total=customer.total, due=due, tier=customer.tier,
                            lines=len(customer.lines), discount=customer.discount,
                            usage=customer.usage)
        if raw["manager"]:
            fields["manager"] = raw["manager"]
        emails.append(EMAIL_SUBJECT.format_map(fields) + "\n" + EMAIL_BODY.format_map(fields))
        by_tier[customer.tier].append(customer.total)

    # management summary (Markdown) and CSV export
    rows = []
    for tier in Tier:
        totals = sorted(by_tier[tier])
        if not totals:
            continue
        eur = sum((t.amount for t in totals if t.currency == "EUR"), Decimal("0"))
        rows.append((str(tier), str(len(totals)), f"{eur:,.2f}", f"{totals[0]:sym}",
                     f"{totals[len(totals) // 2]:sym}", f"{totals[-1]:sym}"))
    head = ("Tier", "Customers", "EUR total", "Min", "Median", "Max")
    widths = [max(len(r[i]) for r in rows + [head]) for i in range(len(head))]
    md = ["| " + " | ".join(h.ljust(w) for h, w in zip(head, widths)) + " |",
          "|" + "|".join("-" * (w + 2) for w in widths) + "|"]
    md += ["| " + " | ".join(c.rjust(w) if i else c.ljust(w) for i, (c, w) in
                              enumerate(zip(r, widths))) + " |" for r in rows]
    summary = "\n".join(md)
    csv_lines = [",".join(("number", "tier", "currency", "total"))]
    for raw, text in zip(state["customers"], statements):
        total_line = next((ln for ln in text.splitlines() if ln.startswith("TOTAL DUE")), "")
        csv_lines.append(",".join((raw["number"], raw["tier"], raw["currency"],
                                   '"' + total_line[58:].strip() + '"')))
    export = "\n".join(csv_lines)

    import hashlib

    h = hashlib.sha256()
    for doc in (*statements, *pages, *emails, summary, export):
        h.update(doc.encode())
    return {"bytes": sum(len(s) for s in (*statements, *pages, *emails)) + len(summary) + len(export),
            "sha": h.hexdigest(), "summary": summary, "first": statements[1][:600],
            "email": emails[3], "html": pages[2][:400]}


def digest(result):
    import hashlib
    import json

    return hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
