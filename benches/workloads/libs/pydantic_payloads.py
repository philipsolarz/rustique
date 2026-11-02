"""pydantic v2 validating and serializing webhook payloads at volume.

Stands for the most common direct use of pydantic (top-20 PyPI package, the validation
layer of FastAPI, LangChain, the OpenAI/Anthropic SDKs and countless services): ingest JSON
events, validate them into typed models, transform, and serialize them back. A webhook
ingestion pipeline: a discriminated union of five event types (``Field(discriminator=)``),
nested models with ``Decimal`` money, ``datetime``, ``UUID``, ``HttpUrl``, ``Literal`` and
enum fields, constrained strings and lists, camelCase aliases (``alias_generator``),
Python-level ``field_validator``s (before/after), a ``model_validator(mode="before")``
that reshapes legacy input, ``computed_field`` and ``field_serializer``; batches validated
with ``TypeAdapter(list[...]).validate_json``, single events with ``model_validate``,
``model_copy(update=)``, ``model_dump(mode="json", by_alias=True)`` and
``model_dump_json``, plus invalid payloads whose ``ValidationError.errors()`` are
collected. pydantic-core (Rust) does the core work; the Python callbacks, model glue and
object creation around it are what the profile separates. 38,400 events per run.
"""

REQUIRES = ["pydantic==2.14.2", "pydantic-core==2.50.2", "typing-extensions==4.16.0",
            "annotated-types==0.8.0", "typing-inspection==0.4.4"]


def setup():
    import datetime as dt
    import enum
    import json
    import random
    import uuid
    from decimal import Decimal
    from typing import Annotated, Literal, Union

    from pydantic import (AliasChoices, BaseModel, ConfigDict, Field, HttpUrl, TypeAdapter,
                          computed_field, field_serializer, field_validator, model_validator)
    from pydantic.alias_generators import to_camel

    class Base(BaseModel):
        model_config = ConfigDict(alias_generator=to_camel, populate_by_name=True, str_strip_whitespace=True,
                                  frozen=False, extra="ignore")

    class Currency(str, enum.Enum):
        EUR = "EUR"
        USD = "USD"
        GBP = "GBP"
        PLN = "PLN"

    class Money(Base):
        amount: Annotated[Decimal, Field(max_digits=12, decimal_places=2, ge=0)]
        currency: Currency

        @field_serializer("amount")
        def ser_amount(self, v):
            return f"{v:.2f}"

    class Address(Base):
        line1: Annotated[str, Field(min_length=1, max_length=100)]
        line2: str | None = None
        city: str
        postal_code: Annotated[str, Field(pattern=r"^[A-Z0-9][A-Z0-9 -]{2,9}$")]
        country: Annotated[str, Field(min_length=2, max_length=2)]

        @field_validator("country", mode="before")
        @classmethod
        def upper_country(cls, v):
            return v.upper() if isinstance(v, str) else v

    class LineItem(Base):
        sku: Annotated[str, Field(pattern=r"^[A-Z]{2,4}-\d{3,6}$")]
        title: str
        quantity: Annotated[int, Field(gt=0, le=999)]
        unit_price: Money
        tags: list[str] = []

        @computed_field
        @property
        def subtotal(self) -> Decimal:
            return self.unit_price.amount * self.quantity

    class Customer(Base):
        id: uuid.UUID
        email: Annotated[str, Field(max_length=254)]
        name: str
        vip: bool = False

        @field_validator("email")
        @classmethod
        def check_email(cls, v):
            local, sep, domain = v.partition("@")
            if not sep or not local or "." not in domain:
                raise ValueError("not an e-mail address")
            return local + "@" + domain.lower()

    class EventBase(Base):
        id: uuid.UUID
        occurred_at: dt.datetime
        source: HttpUrl
        attempt: Annotated[int, Field(ge=1, le=20)] = 1

        @field_validator("occurred_at")
        @classmethod
        def must_be_aware(cls, v):
            if v.tzinfo is None:
                raise ValueError("timestamp must include a timezone")
            return v.astimezone(dt.timezone.utc)

        @model_validator(mode="before")
        @classmethod
        def legacy_shape(cls, data):
            # v1 senders nest everything under "payload" and use "ts"
            if isinstance(data, dict) and "payload" in data:
                data = {**data["payload"], **{k: v for k, v in data.items() if k != "payload"}}
                if "ts" in data:
                    data["occurredAt"] = data.pop("ts")
            return data

    class OrderCreated(EventBase):
        type: Literal["order.created"]
        order_id: Annotated[str, Field(pattern=r"^ORD-\d{8}$")]
        customer: Customer
        items: Annotated[list[LineItem], Field(min_length=1, max_length=50)]
        shipping: Address
        billing: Address | None = None
        total: Money

        @model_validator(mode="after")
        def check_total(self):
            s = sum((i.subtotal for i in self.items), Decimal("0"))
            if s != self.total.amount:
                raise ValueError(f"total {self.total.amount} != sum of items {s}")
            return self

    class OrderShipped(EventBase):
        type: Literal["order.shipped"]
        order_id: str
        carrier: Literal["dhl", "ups", "fedex", "inpost"]
        tracking: Annotated[str, Field(min_length=8, max_length=30)]
        shipped_items: list[str]

    class PaymentFailed(EventBase):
        type: Literal["payment.failed"]
        order_id: str
        amount: Money
        reason: str
        retry_at: dt.datetime | None = None

    class UserSignedUp(EventBase):
        type: Literal["user.signed_up"]
        customer: Customer
        referrer: HttpUrl | None = None
        marketing_opt_in: bool = Field(default=False, validation_alias=AliasChoices("marketingOptIn", "optIn"))

    class RefundIssued(EventBase):
        type: Literal["refund.issued"]
        order_id: str
        amount: Money
        lines: list[LineItem] = []

    Event = Annotated[Union[OrderCreated, OrderShipped, PaymentFailed, UserSignedUp, RefundIssued],
                      Field(discriminator="type")]
    batch_adapter = TypeAdapter(list[Event])
    one_adapter = TypeAdapter(Event)

    rng = random.Random(77)
    words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike".split()
    cities = [("Berlin", "10115", "de"), ("Paris", "75001", "FR"), ("Warsaw", "00-001", "pl"), ("Austin", "73301", "US")]

    def money(cents, cur):
        return {"amount": f"{cents / 100:.2f}", "currency": cur}

    def address():
        c = rng.choice(cities)
        return {"line1": f" {rng.randint(1, 200)} {rng.choice(words).title()} Street ", "city": c[0],
                "postalCode": c[1], "country": c[2]}

    def customer(i):
        return {"id": str(uuid.UUID(int=rng.getrandbits(128), version=4)), "email": f"User{i}@Example.COM",
                "name": f"{rng.choice(words).title()} {rng.choice(words).title()}", "vip": i % 11 == 0}

    def event(i):
        kind = i % 5
        base = {"id": str(uuid.UUID(int=rng.getrandbits(128), version=4)),
                "occurredAt": f"2025-03-{1 + i % 28:02d}T{i % 24:02d}:{i % 60:02d}:00+0{i % 3}:00",
                "source": f"https://shop{i % 7}.example.com/hooks?id={i}", "attempt": 1 + i % 3}
        cur = ["EUR", "USD", "GBP", "PLN"][i % 4]
        if kind == 0:
            items = [{"sku": f"{rng.choice(['AB', 'XYZ', 'QRST'])}-{rng.randint(100, 99999)}", "title": rng.choice(words),
                      "quantity": rng.randint(1, 4), "unitPrice": money(rng.randint(100, 20000), cur),
                      "tags": rng.sample(words, 2)} for _ in range(rng.randint(1, 6))]
            total = sum(int(round(float(it["unitPrice"]["amount"]) * 100)) * it["quantity"] for it in items)
            ev = dict(base, type="order.created", orderId=f"ORD-{i:08d}", customer=customer(i), items=items,
                      shipping=address(), billing=address() if i % 3 == 0 else None, total=money(total, cur))
        elif kind == 1:
            ev = dict(base, type="order.shipped", orderId=f"ORD-{i:08d}", carrier=["dhl", "ups", "fedex", "inpost"][i % 4],
                      tracking=f"TRK{i:012d}", shippedItems=[f"AB-{j:04d}" for j in range(rng.randint(1, 5))])
        elif kind == 2:
            ev = dict(base, type="payment.failed", orderId=f"ORD-{i:08d}", amount=money(rng.randint(500, 90000), cur),
                      reason=rng.choice(["card_declined", "insufficient_funds", "expired_card"]),
                      retryAt=None if i % 2 else f"2025-04-01T10:00:00Z")
        elif kind == 3:
            ev = dict(base, type="user.signed_up", customer=customer(i), referrer=f"https://ref.example.org/c/{i}",
                      optIn=i % 2 == 0)
        else:
            ev = dict(base, type="refund.issued", orderId=f"ORD-{i:08d}", amount=money(rng.randint(100, 5000), cur))
        if i % 10 == 7:  # legacy v1 shape
            ts = ev.pop("occurredAt")
            ev = {"type": ev.pop("type"), "id": ev.pop("id"), "ts": ts, "payload": ev}
        return ev

    batches = [json.dumps([event(b * 100 + j) for j in range(100)]).encode() for b in range(256)]
    singles = [event(100000 + j) for j in range(9600)]
    bad = []
    for j in range(3200):
        ev = event(200000 + j)
        m = j % 6
        if m == 0:
            ev["occurredAt" if "occurredAt" in ev else "ts"] = "2025-03-01T10:00:00"
        elif m == 1:
            ev["type"] = "order.deleted"
        elif m == 2:
            ev["attempt"] = 99
        elif m == 3 and ev.get("type") == "order.created":
            ev["total"]["amount"] = "0.01"
        elif m == 4:
            ev["source"] = "not a url"
        else:
            ev["id"] = "1234"
        bad.append(ev)
    return {"batches": batches, "singles": singles, "bad": bad, "batch_adapter": batch_adapter,
            "one_adapter": one_adapter, "ValidationError": __import__("pydantic").ValidationError}


def run(state):
    import hashlib

    ba, oa, VE = state["batch_adapter"], state["one_adapter"], state["ValidationError"]
    h = hashlib.sha256()
    counts = {}
    for raw in state["batches"]:
        events = ba.validate_json(raw)
        for ev in events:
            counts[ev.type] = counts.get(ev.type, 0) + 1
        h.update(ba.dump_json(events, by_alias=True))
    for d in state["singles"]:
        ev = oa.validate_python(d)
        ev2 = ev.model_copy(update={"attempt": ev.attempt + 1})
        h.update(repr(ev2.model_dump(mode="json", by_alias=True, exclude_none=True)).encode())
        h.update(ev.model_dump_json().encode())
    errors = []
    for d in state["bad"]:
        try:
            oa.validate_python(d)
            errors.append("ok")
        except VE as e:
            errors.append(tuple((err["type"], err["loc"]) for err in e.errors()))
    return h.hexdigest(), sorted(counts.items()), errors


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
