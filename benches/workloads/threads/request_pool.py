"""A thread-pool request handler: the per-request work of a threaded web worker.

Stands for services that handle requests on a pool of OS threads -- gunicorn's `gthread`
worker, `ThreadingHTTPServer`, Django/Flask dev servers, `concurrent.futures` job APIs --
which are exactly the programs free-threaded CPython promises to speed up. One run
handles 12,000 generated requests on a `ThreadPoolExecutor` (`WL_THREADS` workers,
default 8): URL splitting and query parsing (`urllib.parse`), `json.loads` of the body,
hand validation with compiled regexes shared by every thread, `datetime.fromisoformat` /
`strptime`, building `@dataclass` domain objects, lookups in a read-only product catalog
shared by every thread, a `copy.deepcopy` of a response template, and `json.dumps` of the
response. Results come back in submission order (`Executor.map`), so the digest does not
depend on scheduling. Total work is fixed: more threads = same work, less wall time.
"""

import os

REQUIRES: list[str] = []

N_REQUESTS = 12_000


def setup():
    import json
    import random

    rng = random.Random(11)
    first = ["Ana", "Bo", "Chen", "Dara", "Eli", "Femi", "Gus", "Hana", "Ivo", "Jun", "Kai", "Lea"]
    cities = ["Berlin", "São Paulo", "Kraków", "Zürich", "Austin", "Nairobi", "Seoul", "Lyon"]
    catalog = {}
    for i in range(500):
        sku = f"SKU-{i:05d}"
        catalog[sku] = {
            "sku": sku,
            "title": f"{rng.choice(['Red', 'Blue', 'Green', 'Black'])} {rng.choice(['mug', 'lamp', 'desk', 'chair', 'cable'])} {i}",
            "price_cents": rng.randint(150, 40000),
            "tags": rng.sample(["new", "sale", "eco", "gift", "bulk", "import"], rng.randint(0, 3)),
            "stock": {"eu": rng.randint(0, 50), "us": rng.randint(0, 50)},
        }
    skus = sorted(catalog)
    requests = []
    for i in range(N_REQUESTS):
        kind = i % 4
        if kind in (0, 1):
            body = {
                "customer": {"name": f"{rng.choice(first)} {rng.choice(first)}son",
                             "email": f"{rng.choice(first).lower()}.{i}@example.com"},
                "ship_to": {"city": rng.choice(cities), "zip": f"{rng.randint(10000, 99999)}",
                            "country": rng.choice(["DE", "BR", "PL", "CH", "US"])},
                "items": [{"sku": rng.choice(skus), "qty": rng.randint(1, 5)}
                          for _ in range(rng.randint(1, 6))],
                "placed_at": f"2026-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}T"
                             f"{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:{rng.randint(0, 59):02d}+00:00",
                "coupon": rng.choice([None, None, "WELCOME10", "VIP"]),
            }
            if i % 37 == 0:
                body["customer"]["email"] = "not-an-email"
            url = f"/api/v2/orders?dry_run={rng.choice(['0', '1'])}&region={rng.choice(['eu', 'us'])}&trace=req-{i}"
            requests.append(("POST", url, {"Content-Type": "application/json", "X-Request-Id": f"r{i}"},
                             json.dumps(body)))
        elif kind == 2:
            q = "&".join(f"sku={rng.choice(skus)}" for _ in range(rng.randint(2, 8)))
            requests.append(("GET", f"/api/v2/products?{q}&fields=title,price_cents,stock&lang=de-DE",
                             {"Accept": "application/json", "X-Request-Id": f"r{i}"}, ""))
        else:
            since = f"{rng.randint(1, 28):02d}/{rng.choice(['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun'])}/2026:" \
                    f"{rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}:00"
            requests.append(("GET", f"/api/v2/reports/sales?since={since}&group=city&top={rng.randint(3, 9)}",
                             {"Accept": "application/json", "X-Request-Id": f"r{i}"}, ""))
    threads = int(os.environ.get("WL_THREADS", "8"))
    return {"catalog": catalog, "requests": requests, "threads": threads}


def _handlers():
    import copy
    import json
    import re
    from dataclasses import asdict, dataclass, field
    from datetime import datetime, timezone
    from urllib.parse import parse_qs, urlsplit

    email_re = re.compile(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$")
    zip_re = re.compile(r"^\d{5}$")
    sku_re = re.compile(r"^SKU-(\d{5})$")

    @dataclass
    class Line:
        sku: str
        qty: int
        unit_cents: int
        total_cents: int = 0

    @dataclass
    class Order:
        customer: str
        email: str
        city: str
        country: str
        placed_at: datetime
        lines: list = field(default_factory=list)
        coupon: str | None = None
        total_cents: int = 0

    template = {"status": "ok", "api": "v2", "meta": {"server": "pool", "region": None, "warnings": []},
                "data": None, "links": {"self": None, "docs": "https://example.com/docs/v2"}}

    def respond(status, data, region, url, warnings=()):
        doc = copy.deepcopy(template)
        doc["status"] = status
        doc["data"] = data
        doc["meta"]["region"] = region
        doc["meta"]["warnings"].extend(warnings)
        doc["links"]["self"] = url
        return json.dumps(doc, separators=(",", ":"))

    def post_order(catalog, url, qs, headers, body):
        region = qs.get("region", ["eu"])[0]
        doc = json.loads(body)
        cust = doc["customer"]
        if not email_re.match(cust["email"]):
            return respond("error", {"field": "customer.email", "message": "invalid e-mail"}, region, url)
        ship = doc["ship_to"]
        if not zip_re.match(ship["zip"]):
            return respond("error", {"field": "ship_to.zip"}, region, url)
        order = Order(cust["name"].strip().title(), cust["email"].lower(), ship["city"], ship["country"],
                      datetime.fromisoformat(doc["placed_at"]), coupon=doc.get("coupon"))
        warnings = []
        for item in doc["items"]:
            m = sku_re.match(item["sku"])
            product = catalog.get(item["sku"])
            if m is None or product is None:
                warnings.append(f"unknown sku {item['sku']}")
                continue
            if product["stock"][region] < item["qty"]:
                warnings.append(f"backorder {item['sku']}")
            line = Line(item["sku"], item["qty"], product["price_cents"])
            line.total_cents = line.qty * line.unit_cents
            order.lines.append(line)
        total = sum(line.total_cents for line in order.lines)
        if order.coupon == "WELCOME10":
            total = total * 90 // 100
        elif order.coupon == "VIP":
            total = total * 80 // 100
        order.total_cents = total
        data = asdict(order)
        data["placed_at"] = order.placed_at.astimezone(timezone.utc).isoformat()
        data["dry_run"] = qs.get("dry_run", ["0"])[0] == "1"
        return respond("created", data, region, url, warnings)

    def get_products(catalog, url, qs, headers, body):
        fields = qs.get("fields", ["title"])[0].split(",")
        out = []
        for sku in sorted(set(qs.get("sku", []))):
            p = catalog.get(sku)
            if p is not None:
                out.append({f: p[f] for f in fields if f in p} | {"sku": sku})
        return respond("ok", {"items": out, "count": len(out)}, qs.get("lang", ["en"])[0], url)

    def sales_report(catalog, url, qs, headers, body):
        since = datetime.strptime(qs["since"][0], "%d/%b/%Y:%H:%M:%S")
        top = int(qs.get("top", ["5"])[0])
        # A deterministic pseudo-aggregate over the catalog (stands for a DB query result).
        rows = []
        for sku in list(catalog)[since.day * 7: since.day * 7 + 60]:
            p = catalog[sku]
            rows.append((p["price_cents"] * (p["stock"]["eu"] + 1) % 9973, sku, p["title"]))
        rows.sort(reverse=True)
        data = {"since": since.isoformat(), "top": [{"sku": s, "title": t, "score": v} for v, s, t in rows[:top]]}
        return respond("ok", data, "eu", url)

    routes = {("POST", "/api/v2/orders"): post_order, ("GET", "/api/v2/products"): get_products,
              ("GET", "/api/v2/reports/sales"): sales_report}

    def handle(catalog, request):
        method, url, headers, body = request
        parts = urlsplit(url)
        qs = parse_qs(parts.query)
        handler = routes.get((method, parts.path))
        if handler is None:
            return respond("error", {"message": "not found"}, None, url)
        return handler(catalog, url, qs, headers, body)

    return handle


def run(state):
    from concurrent.futures import ThreadPoolExecutor
    from functools import partial

    handle = partial(_handlers(), state["catalog"])
    with ThreadPoolExecutor(max_workers=state["threads"]) as pool:
        return list(pool.map(handle, state["requests"], chunksize=1))


def digest(result):
    import hashlib

    h = hashlib.sha256()
    for r in result:
        h.update(r.encode())
        h.update(b"\n")
    return h.hexdigest()
