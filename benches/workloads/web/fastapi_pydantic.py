"""A FastAPI JSON service: Starlette routing, dependency injection and pydantic v2 models.

Stands for the fastest-growing Python API stack: an async FastAPI CRUD service for orders.
Each request goes through Starlette's ASGI routing and middleware, FastAPI's dependency
injection (an async auth-header dependency, a sync store dependency that FastAPI runs in
its worker-thread pool as it does for the common ``def get_db()``, pagination query
parameters), request-body validation into nested pydantic v2 models (enums, constrained
fields, ``datetime``, ``Decimal``, nested lists, a ``field_validator``), and
response-model serialization of lists of nested models; plus 404/422 error paths. Driven in-process with
``httpx.AsyncClient(transport=ASGITransport(app))`` on one asyncio event loop (FastAPI's
documented async test client; same app-side code path as under uvicorn, without the
TestClient's thread portal; ``sniffio`` installed, as in most environments -- without it
httpx's ASGI transport retries a failing ``import sniffio`` on every request), 800 requests
per run against an in-memory store of 300 orders.
pydantic-core (Rust) does the validation/serialization work; its share is reported
separately.
"""

REQUIRES = ["fastapi==0.162.0", "starlette==1.10.0", "pydantic==2.14.2", "pydantic-core==2.50.2",
            "httpx==0.28.1", "httpcore==1.0.9", "anyio==4.15.1", "h11==0.16.0", "idna==3.20",
            "certifi==2026.10.23", "typing-extensions==4.16.0", "annotated-types==0.8.0",
            "typing-inspection==0.4.4", "annotated-doc==0.0.5", "opentelemetry-api==1.45.2",
            "sniffio==1.3.1"]


def setup():
    import datetime as dt
    import enum
    import random
    from decimal import Decimal
    from typing import Annotated

    from fastapi import Depends, FastAPI, Header, HTTPException, Query
    from pydantic import BaseModel, ConfigDict, Field, field_validator

    class Status(str, enum.Enum):
        pending = "pending"
        paid = "paid"
        shipped = "shipped"
        cancelled = "cancelled"

    class Address(BaseModel):
        street: str = Field(min_length=1, max_length=120)
        city: str
        postcode: str = Field(pattern=r"^[0-9A-Z -]{3,10}$")
        country: str = Field(min_length=2, max_length=2)

    class Item(BaseModel):
        sku: str = Field(pattern=r"^[A-Z]{3}-\d{4}$")
        name: str
        quantity: int = Field(gt=0, le=1000)
        unit_price: Decimal = Field(max_digits=10, decimal_places=2)

    class OrderIn(BaseModel):
        customer_email: str
        shipping: Address
        items: list[Item] = Field(min_length=1)
        note: str | None = None
        tags: list[str] = []

        @field_validator("customer_email")
        @classmethod
        def check_email(cls, v):
            if "@" not in v or v.startswith("@") or v.endswith("@"):
                raise ValueError("invalid email")
            return v.lower()

    class Order(OrderIn):
        model_config = ConfigDict(from_attributes=True)
        id: int
        status: Status
        created_at: dt.datetime
        total: Decimal

    class OrderSummary(BaseModel):
        id: int
        customer_email: str
        status: Status
        total: Decimal
        n_items: int
        created_at: dt.datetime

    class Page(BaseModel):
        total: int
        offset: int
        limit: int
        results: list[OrderSummary]

    rng = random.Random(11)
    words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima".split()
    cities = ["Berlin", "Warsaw", "Lisbon", "Oslo", "Austin", "Osaka", "Lyon", "Gdansk"]

    def make_payload(i):
        return {
            "customer_email": f"Customer{i}@Example.com",
            "shipping": {"street": f"{rng.randint(1, 300)} {rng.choice(words).title()} Street",
                         "city": rng.choice(cities), "postcode": f"{rng.randint(10000, 99999)}",
                         "country": rng.choice(["DE", "PL", "PT", "NO", "US", "JP", "FR"])},
            "items": [{"sku": f"{rng.choice(['ABC', 'XYZ', 'QRS', 'LMN'])}-{rng.randint(0, 9999):04d}",
                       "name": " ".join(rng.choice(words) for _ in range(3)),
                       "quantity": rng.randint(1, 5), "unit_price": f"{rng.randint(100, 50000) / 100:.2f}"}
                      for _ in range(rng.randint(1, 6))],
            "note": rng.choice([None, "leave at door", "gift wrap please"]),
            "tags": rng.sample(words, rng.randint(0, 3)),
        }

    base = dt.datetime(2025, 3, 1, 9, 0, tzinfo=dt.timezone.utc)
    seed_orders = []
    for i in range(300):
        p = OrderIn.model_validate(make_payload(i))
        total = sum((it.unit_price * it.quantity for it in p.items), Decimal("0"))
        seed_orders.append(Order(**p.model_dump(), id=i + 1, status=list(Status)[i % 4],
                                 created_at=base + dt.timedelta(minutes=17 * i), total=total))

    app = FastAPI(title="orders")

    def get_store():
        return app.state.store

    async def require_token(x_api_key: Annotated[str, Header()]):
        if x_api_key != "secret-key":
            raise HTTPException(status_code=401, detail="bad key")
        return x_api_key

    Store = Annotated[dict, Depends(get_store)]
    Auth = Annotated[str, Depends(require_token)]

    @app.get("/orders", response_model=Page)
    async def list_orders(store: Store, _: Auth, status: Status | None = None,
                          offset: Annotated[int, Query(ge=0)] = 0,
                          limit: Annotated[int, Query(ge=1, le=100)] = 20):
        rows = [o for o in store["orders"].values() if status is None or o.status == status]
        page = rows[offset:offset + limit]
        return Page(total=len(rows), offset=offset, limit=limit, results=[
            OrderSummary(id=o.id, customer_email=o.customer_email, status=o.status, total=o.total,
                         n_items=len(o.items), created_at=o.created_at) for o in page])

    @app.get("/orders/{order_id}", response_model=Order)
    async def get_order(order_id: int, store: Store, _: Auth):
        o = store["orders"].get(order_id)
        if o is None:
            raise HTTPException(status_code=404, detail="order not found")
        return o

    @app.post("/orders", response_model=Order, status_code=201)
    async def create_order(body: OrderIn, store: Store, _: Auth):
        store["next_id"] += 1
        total = sum((it.unit_price * it.quantity for it in body.items), Decimal("0"))
        o = Order(**body.model_dump(), id=store["next_id"], status=Status.pending,
                  created_at=base + dt.timedelta(days=30, seconds=store["next_id"]), total=total)
        store["orders"][o.id] = o
        return o

    @app.patch("/orders/{order_id}/status", response_model=OrderSummary)
    async def set_status(order_id: int, status: Status, store: Store, _: Auth):
        o = store["orders"].get(order_id)
        if o is None:
            raise HTTPException(status_code=404, detail="order not found")
        o.status = status
        return OrderSummary(id=o.id, customer_email=o.customer_email, status=o.status, total=o.total,
                            n_items=len(o.items), created_at=o.created_at)

    plan = []
    for i in range(800):
        k = i % 10
        if k in (0, 5):
            st = ["", "&status=paid", "&status=pending", "&status=shipped"][i % 4]
            plan.append(("GET", f"/orders?offset={(i * 7) % 250}&limit=25{st}", None))
        elif k in (1, 3, 6, 8):
            plan.append(("GET", f"/orders/{1 + (i * 37) % 320}", None))
        elif k in (2, 7):
            plan.append(("POST", "/orders", make_payload(1000 + i)))
        elif k == 4:
            bad = make_payload(2000 + i)
            bad["items"][0]["quantity"] = 0
            bad["shipping"]["postcode"] = "bad!"
            plan.append(("POST", "/orders", bad))
        else:
            plan.append(("PATCH", f"/orders/{1 + (i * 13) % 300}/status?status=shipped", None))
    return {"app": app, "seed": seed_orders, "plan": plan}


def run(state):
    import asyncio
    import hashlib

    import httpx

    app = state["app"]
    app.state.store = {"orders": {o.id: o.model_copy(deep=True) for o in state["seed"]},
                       "next_id": len(state["seed"])}

    async def main():
        out = []
        transport = httpx.ASGITransport(app=app)
        async with httpx.AsyncClient(transport=transport, base_url="http://api.test",
                                     headers={"x-api-key": "secret-key"}) as client:
            for method, url, body in state["plan"]:
                r = await client.request(method, url, json=body)
                out.append((method, url, r.status_code, hashlib.sha256(r.content).hexdigest()))
        return out

    return asyncio.run(main())


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
