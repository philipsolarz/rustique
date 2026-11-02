"""attrs data models structured and unstructured with cattrs: code generated for the program.

Stands for the attrs family (attrs is a top-20 PyPI package; dataclasses were modelled on
it) and cattrs, its (de)serialization companion used by botocore-adjacent tooling, the
Hypothesis/pytest ecosystem, Home Assistant-style apps and many API clients. Both
libraries *generate code*: attrs writes ``__init__``/``__eq__``/``__hash__``/``__lt__``/
``__repr__``/``__setattr__`` for every class with ``exec`` (filenames
``<attrs generated ...>``), and cattrs generates and compiles a structuring and an
unstructuring function per class (``<cattrs generated ...>``) -- so this workload's own
modelling code runs mostly in generated code. An inventory service: frozen slotted value
objects (``Sku``, ``Money``, ``Location``) with validators and converters, mutable
aggregates with ``factory`` defaults, ``order=True`` comparisons, a tagged union of
movement events (``configure_tagged_union``), enums, ``datetime``/``Decimal`` hooks, and
the json pre-configured converter. Per run: 2,000 warehouse documents (``json.loads`` ->
``structure``), stock movements applied with ``attrs.evolve``, sorting, set/dict use of
hashed value objects, then ``unstructure`` -> ``json.dumps`` and ``attrs.asdict``
reports (~115 attrs instances per warehouse).
"""

REQUIRES = ["attrs==26.1.0", "cattrs==26.2.1", "typing-extensions==4.16.0"]


def setup():
    import datetime as dt
    import enum
    import json
    import random
    from decimal import Decimal

    import attrs
    from attrs import define, evolve, field, frozen, validators
    from cattrs.preconf.json import make_converter
    from cattrs.strategies import configure_tagged_union

    class Unit(enum.Enum):
        PIECE = "pc"
        KILOGRAM = "kg"
        LITRE = "l"
        BOX = "box"

    def _upper(s):
        return s.strip().upper()

    @frozen(order=True)
    class Sku:
        family: str = field(converter=_upper, validator=validators.matches_re(r"^[A-Z]{2,4}$"))
        number: int = field(validator=[validators.instance_of(int), validators.ge(0), validators.lt(10 ** 7)])

        def __str__(self):
            return f"{self.family}-{self.number:07d}"

    @frozen
    class Money:
        amount: Decimal = field(converter=Decimal)
        currency: str = field(default="EUR", validator=validators.in_({"EUR", "USD", "GBP", "PLN"}))

        def __add__(self, other):
            if other.currency != self.currency:
                raise ValueError("currency mismatch")
            return Money(self.amount + other.amount, self.currency)

        def times(self, q):
            return Money((self.amount * q).quantize(Decimal("0.01")), self.currency)

    @frozen(order=True)
    class Location:
        aisle: str
        rack: int
        level: int = 0

    @define
    class StockItem:
        sku: Sku
        name: str
        unit: Unit
        quantity: Decimal = field(converter=Decimal, validator=validators.ge(Decimal(0)))
        unit_cost: Money
        location: Location
        tags: tuple[str, ...] = field(factory=tuple, converter=lambda xs: tuple(sorted(set(xs))))
        last_counted: dt.date | None = None

        def value(self):
            return self.unit_cost.times(self.quantity)

    @frozen(kw_only=True)
    class Receipt:
        at: dt.datetime
        sku: Sku
        quantity: Decimal
        supplier: str
        unit_cost: Money

    @frozen(kw_only=True)
    class Pick:
        at: dt.datetime
        sku: Sku
        quantity: Decimal
        order_id: str

    @frozen(kw_only=True)
    class Transfer:
        at: dt.datetime
        sku: Sku
        to: Location

    @frozen(kw_only=True)
    class Adjustment:
        at: dt.datetime
        sku: Sku
        delta: Decimal
        reason: str = "count"

    Movement = Receipt | Pick | Transfer | Adjustment

    @define
    class Warehouse:
        code: str
        name: str
        opened: dt.date
        items: list[StockItem] = field(factory=list)
        movements: list[Movement] = field(factory=list)
        attributes: dict[str, str] = field(factory=dict)

    conv = make_converter()
    conv.register_structure_hook(Decimal, lambda v, _: Decimal(v))
    conv.register_unstructure_hook(Decimal, str)
    conv.register_structure_hook(Sku, lambda v, _: Sku(*v.split("-")[:1], int(v.split("-")[1])))
    conv.register_unstructure_hook(Sku, str)
    configure_tagged_union(Movement, conv, tag_name="kind")

    rng = random.Random(8)
    fams = ["AB", "XYZ", "QRST", "MN", "OPQ"]
    def sku_s():
        return f"{rng.choice(fams)}-{rng.randint(0, 9999999):07d}"

    docs = []
    for w in range(2000):
        items = []
        skus = []
        for i in range(rng.randint(8, 20)):
            s = sku_s()
            skus.append(s)
            items.append({"sku": s, "name": f"item {i}", "unit": rng.choice(["pc", "kg", "l", "box"]),
                          "quantity": str(rng.randint(0, 500)), "unit_cost": {"amount": f"{rng.randint(10, 9999) / 100:.2f}",
                                                                          "currency": "EUR"},
                          "location": {"aisle": rng.choice("ABCDEFG"), "rack": rng.randint(1, 40), "level": rng.randint(0, 5)},
                          "tags": rng.sample(["fragile", "cold", "hazmat", "bulk", "fast"], rng.randint(0, 2)),
                          "last_counted": None if rng.random() < 0.3 else f"2025-0{1 + i % 9}-1{i % 9}"})
        moves = []
        for m in range(rng.randint(5, 25)):
            s = rng.choice(skus)
            at = f"2025-03-{1 + m % 28:02d}T{m % 24:02d}:15:00"
            k = m % 4
            if k == 0:
                moves.append({"kind": "Receipt", "at": at, "sku": s, "quantity": str(rng.randint(1, 50)),
                              "supplier": f"sup-{m}", "unit_cost": {"amount": "3.10", "currency": "EUR"}})
            elif k == 1:
                moves.append({"kind": "Pick", "at": at, "sku": s, "quantity": str(rng.randint(1, 5)), "order_id": f"o{m}"})
            elif k == 2:
                moves.append({"kind": "Transfer", "at": at, "sku": s,
                              "to": {"aisle": rng.choice("HJK"), "rack": rng.randint(1, 9), "level": 0}})
            else:
                moves.append({"kind": "Adjustment", "at": at, "sku": s, "delta": str(rng.randint(-3, 3))})
        docs.append(json.dumps({"code": f"WH{w:04d}", "name": f"Warehouse {w}", "opened": f"20{10 + w % 15}-01-01",
                                "items": items, "movements": moves, "attributes": {"region": rng.choice(["eu", "us"])}}))
    return {"docs": docs, "conv": conv, "Warehouse": Warehouse, "types": (Receipt, Pick, Transfer, Adjustment),
            "evolve": evolve, "asdict": attrs.asdict, "Decimal": Decimal}


def run(state):
    import hashlib
    import json

    conv, Warehouse, evolve, asdict, Decimal = (state["conv"], state["Warehouse"], state["evolve"],
                                                state["asdict"], state["Decimal"])
    Receipt, Pick, Transfer, Adjustment = state["types"]
    h = hashlib.sha256()
    totals = []
    all_skus = set()
    for raw in state["docs"]:
        wh = conv.structure(json.loads(raw), Warehouse)
        by_sku = {it.sku: idx for idx, it in enumerate(wh.items)}
        for mv in sorted(wh.movements, key=lambda m: (m.at, str(m.sku))):
            idx = by_sku.get(mv.sku)
            if idx is None:
                continue
            it = wh.items[idx]
            if isinstance(mv, Receipt):
                wh.items[idx] = evolve(it, quantity=it.quantity + mv.quantity, unit_cost=mv.unit_cost)
            elif isinstance(mv, Pick):
                wh.items[idx] = evolve(it, quantity=max(Decimal(0), it.quantity - mv.quantity))
            elif isinstance(mv, Transfer):
                wh.items[idx] = evolve(it, location=mv.to)
            else:
                wh.items[idx] = evolve(it, quantity=max(Decimal(0), it.quantity + mv.delta))
        wh.items.sort(key=lambda it: (it.location, it.sku))
        all_skus.update(it.sku for it in wh.items)
        value = sum((it.value().amount for it in wh.items), Decimal(0))
        totals.append((wh.code, str(value), wh.items[0] == wh.items[0], repr(wh.items[-1].location)))
        h.update(json.dumps(conv.unstructure(wh), sort_keys=True).encode())
        if wh.code.endswith("7"):
            h.update(repr(asdict(wh.items[0], recurse=True)).encode())
    return h.hexdigest(), len(all_skus), min(all_skus) < max(all_skus), totals


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
