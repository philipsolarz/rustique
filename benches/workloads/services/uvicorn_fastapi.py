"""A long-running uvicorn + FastAPI service under keep-alive load over real sockets.

Stands for how FastAPI is deployed: ``pip install "fastapi[standard]"`` brings
``uvicorn[standard]``, which runs the app on uvloop with the httptools HTTP parser (both
C extensions) and access logging on. The server is started once in setup on a uvloop
event loop and keeps running across runs (connections, caches, the heap and the GC's
generations persist, as in a server that has been up for hours); each run sends a fixed
batch of 3,200 requests from 16 keep-alive connections over 127.0.0.1 and waits for every
response. The clients are deliberately thin (pre-encoded HTTP/1.1 requests, a
Content-Length read on asyncio streams in the same loop) so that the measured time is the
server's: uvicorn's httptools protocol, ASGI cycle and access log, Starlette
routing and middleware (``GZipMiddleware`` threshold not reached, a timing function
middleware), FastAPI dependency injection, pydantic validation of query/path/body and
response-model serialization, ``JSONResponse`` rendering. Endpoints: catalogue item
lookups, filtered searches, cart POSTs with nested bodies, a 404 and a 422 path.
Peak RSS and heap growth over many iterations are recorded by ``services/_steady.py``.
"""

REQUIRES = ["uvicorn==0.54.0", "uvloop==0.23.0", "httptools==0.9.0", "click==8.5.0", "h11==0.16.0",
            "python-dotenv==1.2.4", "PyYAML==6.0.3", "watchfiles==1.3.0", "websockets==17.4",
            "fastapi==0.162.0", "starlette==1.10.0", "pydantic==2.14.2", "pydantic-core==2.50.2",
            "anyio==4.15.1", "idna==3.20", "sniffio==1.3.1", "typing-extensions==4.16.0",
            "annotated-types==0.8.0", "typing-inspection==0.4.4", "annotated-doc==0.0.5",
            "opentelemetry-api==1.45.2"]

N_CONN = 16
N_REQ = 3200


def setup():
    import asyncio
    import copy
    import json
    import os
    import random
    import socket
    from decimal import Decimal
    from typing import Annotated

    import uvicorn
    import uvloop
    from fastapi import Depends, FastAPI, HTTPException, Query, Request
    from fastapi.middleware.gzip import GZipMiddleware
    from pydantic import BaseModel, Field
    from uvicorn.config import LOGGING_CONFIG

    class Item(BaseModel):
        id: int
        name: str
        category: str
        price: Decimal = Field(decimal_places=2)
        stock: int
        tags: list[str]

    class CartLine(BaseModel):
        item_id: int = Field(gt=0)
        quantity: int = Field(gt=0, le=20)

    class CartIn(BaseModel):
        customer: str = Field(min_length=3, max_length=40)
        lines: list[CartLine] = Field(min_length=1, max_length=30)
        coupon: str | None = Field(default=None, pattern=r"^[A-Z0-9]{6}$")

    class CartOut(BaseModel):
        customer: str
        lines: list[dict]
        subtotal: Decimal
        discount: Decimal
        total: Decimal

    class SearchOut(BaseModel):
        total: int
        items: list[Item]

    rng = random.Random(2)
    cats = ["books", "garden", "kitchen", "toys", "audio", "office"]
    words = "red blue small large classic smart eco steel wooden quiet portable".split()
    catalogue = {i: Item(id=i, name=f"{rng.choice(words)} {rng.choice(words)} {i}", category=cats[i % 6],
                         price=Decimal(rng.randint(100, 20000)) / 100, stock=rng.randint(0, 50),
                         tags=rng.sample(words, 2)) for i in range(1, 2001)}
    by_cat = {c: [it for it in catalogue.values() if it.category == c] for c in cats}

    app = FastAPI(title="catalogue")
    app.add_middleware(GZipMiddleware, minimum_size=50_000)

    @app.middleware("http")
    async def timing(request: Request, call_next):
        response = await call_next(request)
        response.headers["x-app"] = "catalogue"
        return response

    def get_catalogue():
        return catalogue

    Cat = Annotated[dict, Depends(get_catalogue)]

    @app.get("/health")
    async def health():
        return {"ok": True}

    @app.get("/items/{item_id}", response_model=Item)
    async def item(item_id: int, cat: Cat):
        it = cat.get(item_id)
        if it is None:
            raise HTTPException(404, "no such item")
        return it

    @app.get("/search", response_model=SearchOut)
    async def search(category: str, cat: Cat, max_price: Annotated[Decimal | None, Query(gt=0)] = None,
                     in_stock: bool = False, limit: Annotated[int, Query(ge=1, le=50)] = 20):
        rows = [it for it in by_cat.get(category, []) if (max_price is None or it.price <= max_price)
                and (not in_stock or it.stock > 0)]
        return SearchOut(total=len(rows), items=rows[:limit])

    @app.post("/cart", response_model=CartOut)
    async def cart(body: CartIn, cat: Cat):
        lines, subtotal = [], Decimal("0")
        for ln in body.lines:
            it = cat.get(ln.item_id)
            if it is None:
                raise HTTPException(422, f"unknown item {ln.item_id}")
            amount = it.price * ln.quantity
            subtotal += amount
            lines.append({"item": it.name, "quantity": ln.quantity, "amount": str(amount)})
        discount = (subtotal * Decimal("0.1")).quantize(Decimal("0.01")) if body.coupon else Decimal("0.00")
        return CartOut(customer=body.customer, lines=lines, subtotal=subtotal, discount=discount,
                       total=subtotal - discount)

    log_config = copy.deepcopy(LOGGING_CONFIG)
    for h in log_config["handlers"].values():  # access log on, written to /dev/null
        h.pop("stream", None)
        h["class"] = "logging.FileHandler"
        h["filename"] = os.devnull
    config = uvicorn.Config(app, loop="uvloop", http="httptools", lifespan="on", log_config=log_config,
                            access_log=True, timeout_keep_alive=3600, log_level="info")
    server = uvicorn.Server(config)
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    loop = uvloop.new_event_loop()
    asyncio.set_event_loop(loop)
    loop.create_task(server.serve(sockets=[sock]))

    async def wait_started():
        while not server.started:
            await asyncio.sleep(0.01)
        conns = []
        for _ in range(N_CONN):
            conns.append(await asyncio.open_connection("127.0.0.1", port))
        return conns

    conns = loop.run_until_complete(wait_started())

    def req(method, path, body=None):
        head = f"{method} {path} HTTP/1.1\r\nHost: api.local\r\nUser-Agent: bench/1.0\r\nAccept: application/json\r\n"
        if body is None:
            return (head + "\r\n").encode()
        data = json.dumps(body).encode()
        return (head + f"Content-Type: application/json\r\nContent-Length: {len(data)}\r\n\r\n").encode() + data

    plan = []
    for i in range(N_REQ):
        k = i % 10
        if k in (0, 3, 6, 8):
            plan.append(req("GET", f"/items/{1 + (i * 37) % 2000}"))
        elif k in (1, 7):
            plan.append(req("GET", f"/search?category={cats[i % 6]}&max_price={50 + i % 100}&in_stock={'true' if i % 2 else 'false'}&limit={5 + i % 20}"))
        elif k in (2, 5):
            plan.append(req("POST", "/cart", {"customer": f"cust-{i}", "coupon": "SAVE10" if i % 3 == 0 else None,
                                              "lines": [{"item_id": 1 + (i * j) % 2000, "quantity": 1 + j % 3} for j in range(1, 2 + i % 8)]}))
        elif k == 4:
            plan.append(req("GET", "/health") if i % 20 != 4 else req("GET", "/items/99999"))
        else:
            plan.append(req("POST", "/cart", {"customer": "x", "lines": []}))
    return {"loop": loop, "conns": conns, "plan": plan, "server": server}


async def _client(reader, writer, reqs, out, base):
    for j, r in enumerate(reqs):
        writer.write(r)
        head = await reader.readuntil(b"\r\n\r\n")
        status = int(head[9:12])
        n = 0
        for line in head.split(b"\r\n"):
            if line[:15].lower() == b"content-length:":
                n = int(line[15:])
                break
        body = await reader.readexactly(n)
        out[base + j] = (status, body)


def run(state):
    import asyncio
    import hashlib

    plan, conns = state["plan"], state["conns"]
    out = [None] * len(plan)
    per = len(plan) // len(conns)

    async def main():
        await asyncio.gather(*[_client(rd, wr, plan[c * per:(c + 1) * per], out, c * per)
                               for c, (rd, wr) in enumerate(conns)])

    state["loop"].run_until_complete(main())
    h = hashlib.sha256()
    statuses = {}
    for st, body in out:
        statuses[st] = statuses.get(st, 0) + 1
        h.update(body)
    return sorted(statuses.items()), h.hexdigest()


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
