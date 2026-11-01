"""The JSON (de)serialization core of a REST service, in plain stdlib Python.

Stands for the request/response handling every JSON API does regardless of framework:
``json.loads`` of request bodies, hand-written validation (``isinstance`` checks, string
normalisation, ``datetime.fromisoformat``, ``uuid.UUID``, ``decimal.Decimal``), conversion
into ``@dataclass`` domain objects, business logic (totals, filtering, sorting), and
response building with ``dataclasses.asdict`` + ``json.dumps(default=...)`` for
``datetime``/``UUID``/``Decimal``/``Enum`` values, pagination envelopes, and error
documents. 9,600 API calls per run (create order, list orders page, get order, update
customer, bulk import of 25 events) over generated payloads of realistic shape.
"""

REQUIRES: list[str] = []


def setup():
    import json
    import random
    import uuid

    rng = random.Random(5)
    first = ["Ana", "Bo", "Chen", "Dara", "Eli", "Femi", "Gus", "Hana", "Ivo", "Jun", "Kai", "Lea"]
    cities = ["Berlin", "São Paulo", "Kraków", "Zürich", "Austin", "Nairobi", "Seoul", "Lyon"]
    skus = [f"SKU-{i:05d}" for i in range(400)]
    bodies = []
    for i in range(9600):
        k = i % 5
        if k == 0:
            body = {"customer": {"id": str(uuid.UUID(int=rng.getrandbits(128))), "name": f"{rng.choice(first)} {rng.choice(first)}son",
                                 "email": f"{rng.choice(first).lower()}{i}@example.com"},
                    "shipping": {"line1": f"{rng.randint(1, 999)} Main St", "city": rng.choice(cities),
                                 "zip": f"{rng.randint(10000, 99999)}", "country": rng.choice(["DE", "BR", "PL", "CH", "US"])},
                    "currency": rng.choice(["EUR", "USD", "PLN"]),
                    "items": [{"sku": rng.choice(skus), "qty": rng.randint(1, 4),
                               "price": f"{rng.randint(100, 20000) / 100:.2f}"} for _ in range(rng.randint(1, 8))],
                    "placed_at": f"2025-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}T{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:00+00:00",
                    "coupon": rng.choice([None, None, "WELCOME10", "VIP"]), "notes": rng.choice(["", "ring twice", "fragile – handle with care"])}
            if i % 40 == 0:
                body["items"][0]["qty"] = "three"
            bodies.append(("create", json.dumps(body)))
        elif k == 1:
            bodies.append(("list", json.dumps({"page": 1 + i % 12, "per_page": 25, "status": rng.choice([None, "new", "paid"]),
                                               "sort": rng.choice(["-placed_at", "total", "placed_at"])})))
        elif k == 2:
            bodies.append(("get", json.dumps({"id": i % 300})))
        elif k == 3:
            bodies.append(("update", json.dumps({"id": i % 300, "patch": {"email": f"  NEW{i}@Example.COM ", "name": rng.choice(first),
                                                                         "marketing_opt_in": bool(i % 2)}})))
        else:
            bodies.append(("events", json.dumps({"events": [
                {"id": str(uuid.UUID(int=rng.getrandbits(128))), "type": rng.choice(["view", "click", "add_to_cart", "purchase"]),
                 "ts": f"2025-06-{rng.randint(1, 30):02d}T{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:{rng.randint(0, 59):02d}+00:00",
                 "props": {"path": f"/p/{rng.randint(1, 500)}", "ms": rng.randint(5, 3000), "ab": rng.choice(["A", "B"])}}
                for _ in range(25)]})))
    return {"bodies": bodies}


def run(state):
    import dataclasses
    import datetime as dt
    import enum
    import hashlib
    import json
    import uuid
    from decimal import Decimal

    class Status(enum.Enum):
        NEW = "new"
        PAID = "paid"

    @dataclasses.dataclass
    class Customer:
        id: uuid.UUID
        name: str
        email: str
        marketing_opt_in: bool = False

    @dataclasses.dataclass
    class Line:
        sku: str
        qty: int
        price: Decimal

        @property
        def total(self):
            return self.price * self.qty

    @dataclasses.dataclass
    class Order:
        id: int
        customer: Customer
        city: str
        country: str
        currency: str
        lines: list
        placed_at: dt.datetime
        status: Status
        total: Decimal
        coupon: str | None = None

    class ValidationError(Exception):
        def __init__(self, errors):
            self.errors = errors

    def encode(o):
        if isinstance(o, (dt.datetime, dt.date)):
            return o.isoformat()
        if isinstance(o, (uuid.UUID, Decimal)):
            return str(o)
        if isinstance(o, enum.Enum):
            return o.value
        raise TypeError(type(o).__name__)

    def respond(status, obj):
        return status, json.dumps(obj, default=encode, ensure_ascii=False, separators=(",", ":"))

    def parse_order(d, oid):
        errors = []
        if not isinstance(d.get("customer"), dict):
            errors.append({"loc": "customer", "msg": "required"})
        items = d.get("items")
        if not isinstance(items, list) or not items:
            errors.append({"loc": "items", "msg": "must be a non-empty list"})
            items = []
        lines = []
        for n, it in enumerate(items):
            if not isinstance(it.get("qty"), int) or isinstance(it.get("qty"), bool) or it["qty"] < 1:
                errors.append({"loc": f"items.{n}.qty", "msg": "must be a positive integer"})
                continue
            sku = str(it.get("sku", "")).strip().upper()
            if not sku.startswith("SKU-"):
                errors.append({"loc": f"items.{n}.sku", "msg": "bad sku"})
            lines.append(Line(sku=sku, qty=it["qty"], price=Decimal(it["price"])))
        if errors:
            raise ValidationError(errors)
        c = d["customer"]
        cust = Customer(id=uuid.UUID(c["id"]), name=c["name"].strip(), email=c["email"].strip().lower())
        ship = d["shipping"]
        total = sum((l.total for l in lines), Decimal(0))
        if d.get("coupon") == "WELCOME10":
            total = (total * Decimal("0.9")).quantize(Decimal("0.01"))
        return Order(id=oid, customer=cust, city=ship["city"], country=ship["country"].upper(),
                     currency=d["currency"], lines=lines, placed_at=dt.datetime.fromisoformat(d["placed_at"]),
                     status=Status.NEW, total=total, coupon=d.get("coupon"))

    orders = {}
    customers = {}
    out = []
    for kind, raw in state["bodies"]:
        req = json.loads(raw)
        if kind == "create":
            try:
                o = parse_order(req, len(orders))
            except ValidationError as e:
                out.append(respond(422, {"detail": e.errors}))
                continue
            orders[o.id] = o
            customers[o.id % 300] = o.customer
            out.append(respond(201, dataclasses.asdict(o)))
        elif kind == "list":
            recent = (orders[j] for j in range(max(0, len(orders) - 200), len(orders)))  # "last 200" window
            rows = [o for o in recent if req["status"] is None or o.status.value == req["status"]]
            key = req["sort"].lstrip("-")
            rows.sort(key=lambda o: getattr(o, key), reverse=req["sort"].startswith("-"))
            per = req["per_page"]
            page = rows[(req["page"] - 1) * per:req["page"] * per]
            out.append(respond(200, {"page": req["page"], "total": len(rows), "results": [
                {"id": o.id, "customer": o.customer.name, "total": o.total, "currency": o.currency,
                 "placed_at": o.placed_at, "n_lines": len(o.lines), "status": o.status} for o in page]}))
        elif kind == "get":
            o = orders.get(req["id"])
            out.append(respond(200, dataclasses.asdict(o)) if o else respond(404, {"detail": "not found"}))
        elif kind == "update":
            c = customers.get(req["id"])
            if c is None:
                out.append(respond(404, {"detail": "no customer"}))
                continue
            patch = req["patch"]
            c = dataclasses.replace(c, email=patch["email"].strip().lower(), name=patch["name"],
                                    marketing_opt_in=bool(patch["marketing_opt_in"]))
            customers[req["id"]] = c
            out.append(respond(200, dataclasses.asdict(c)))
        else:
            counts = {}
            latest = None
            for ev in req["events"]:
                ts = dt.datetime.fromisoformat(ev["ts"])
                uuid.UUID(ev["id"])
                counts[ev["type"]] = counts.get(ev["type"], 0) + 1
                if latest is None or ts > latest:
                    latest = ts
            out.append(respond(202, {"accepted": len(req["events"]), "by_type": dict(sorted(counts.items())),
                                     "latest": latest}))
    return [(s, hashlib.sha256(b.encode()).hexdigest()[:16]) for s, b in out]


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
