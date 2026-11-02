"""A service's shared in-process state under threads: caches, locks, metrics and logging.

Stands for the shared parts of a multi-threaded service that every request touches: a
read-through cache (`dict` + `threading.Lock` on miss, lock-free reads on hit),
`functools.lru_cache` on a pure helper, a read-mostly settings dict, per-thread state in
`threading.local`, a metrics registry (`collections.Counter` under a lock), and `logging`
to a shared handler (one `Handler` lock, a `Formatter` with `%(asctime)s`,
`%(threadName)s`, `LoggerAdapter` extra fields). One run serves 36,000 pricing requests
on `WL_THREADS` (default 8) threads, each request formatting a quote with f-strings,
`str.format`, `format(Decimal)`, `datetime.isoformat` and `json.dumps`. The cache is
cleared at the start of each run, so misses and hits recur. Responses are kept per
request index and the metrics are sorted; the log stream's order and timestamps are not
part of the digest (only the record count), so the digest does not depend on scheduling.
"""

import os

REQUIRES: list[str] = []

N_REQUESTS = 36_000


def setup():
    import random

    rng = random.Random(59)
    regions = ["eu-west", "eu-central", "us-east", "us-west", "ap-south", "sa-east"]
    tiers = ["free", "basic", "pro", "enterprise"]
    reqs = []
    for i in range(N_REQUESTS):
        reqs.append({"id": i, "sku": f"P{rng.randint(0, 1499):04d}", "qty": rng.randint(1, 40),
                     "region": rng.choice(regions), "tier": rng.choice(tiers),
                     "day": f"2026-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}",
                     "user": f"user{rng.randint(1, 900)}"})
    products = {f"P{i:04d}": {"name": f"Product {i}", "base": f"{rng.randint(100, 99999) / 100:.2f}",
                              "category": rng.choice(["hw", "sw", "svc", "sub"]),
                              "weight_g": rng.randint(10, 9000)} for i in range(1500)}
    settings = {"currency": {"eu-west": "EUR", "eu-central": "EUR", "us-east": "USD", "us-west": "USD",
                             "ap-south": "INR", "sa-east": "BRL"},
                "fx": {"EUR": "1", "USD": "1.08", "INR": "90.1", "BRL": "5.92"},
                "tier_discount": {"free": 0, "basic": 5, "pro": 12, "enterprise": 20},
                "vat": {"eu-west": 21, "eu-central": 19, "us-east": 0, "us-west": 0, "ap-south": 18, "sa-east": 17}}
    threads = int(os.environ.get("WL_THREADS", "8"))
    return {"reqs": reqs, "products": products, "settings": settings, "threads": threads}


def run(state):
    import functools
    import io
    import json
    import logging
    import threading
    from collections import Counter
    from datetime import date, datetime, timedelta
    from decimal import ROUND_HALF_EVEN, Decimal

    products, settings, reqs = state["products"], state["settings"], state["reqs"]
    n = state["threads"]

    stream = io.StringIO()
    handler = logging.StreamHandler(stream)
    handler.setFormatter(logging.Formatter(
        "%(asctime)s %(levelname)-7s [%(threadName)s] %(name)s: %(message)s req=%(req)s"))
    log = logging.getLogger("wl.pricing")
    log.propagate = False
    log.setLevel(logging.INFO)
    log.addHandler(handler)

    cache = {}
    cache_lock = threading.Lock()
    metrics = Counter()
    metrics_lock = threading.Lock()
    local = threading.local()
    cent = Decimal("0.01")

    def load_product(sku):  # the "slow" backend behind the cache
        p = products[sku]
        return {"sku": sku, "name": p["name"], "base": Decimal(p["base"]), "category": p["category"],
                "ship": Decimal(p["weight_g"]) / 1000 * Decimal("0.85")}

    def get_product(sku):
        p = cache.get(sku)
        if p is not None:
            return p, True
        with cache_lock:
            p = cache.get(sku)
            if p is None:
                p = cache[sku] = load_product(sku)
        return p, False

    @functools.lru_cache(maxsize=256)
    def fx_vat(region):
        cur = settings["currency"][region]
        return cur, Decimal(settings["fx"][cur]), Decimal(100 + settings["vat"][region]) / 100

    def quote(r):
        if not hasattr(local, "served"):
            local.served = 0
        local.served += 1
        adapter = logging.LoggerAdapter(log, {"req": r["id"]})
        p, hit = get_product(r["sku"])
        cur, fx, vat = fx_vat(r["region"])
        disc = settings["tier_discount"][r["tier"]]
        net = (p["base"] * r["qty"] * (100 - disc) / 100 + p["ship"]) * fx
        gross = (net * vat).quantize(cent, rounding=ROUND_HALF_EVEN)
        day = date.fromisoformat(r["day"])
        valid = datetime(day.year, day.month, day.day, 12, 0) + timedelta(days=14)
        line = f"{r['qty']} x {p['name']} ({p['category']})"
        doc = {"id": r["id"], "user": r["user"], "line": line, "currency": cur,
               "net": format(net.quantize(cent), "f"), "gross": "{:,.2f}".format(gross),
               "valid_until": valid.isoformat(timespec="minutes")}
        if gross > 5000:
            adapter.warning("large quote %s %s for %s", gross, cur, r["user"])
        elif r["id"] % 4 == 0:
            adapter.info("quote %s %s", r["sku"], cur)
        else:
            adapter.debug("quote detail %r", doc)  # below the level: filtered
        with metrics_lock:
            metrics[(r["region"], r["tier"])] += 1
            metrics["cache_hit" if hit else "cache_miss"] += 1  # schedule-dependent: not digested
        return json.dumps(doc, sort_keys=True)

    out = [None] * len(reqs)

    def worker(k):
        for i in range(k, len(reqs), n):
            out[i] = quote(reqs[i])

    ts = [threading.Thread(target=worker, args=(k,), name=f"worker-{k}") for k in range(n)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    log.removeHandler(handler)
    records = stream.getvalue().count("\n")
    return {"responses": out, "metrics": sorted((k, v) for k, v in metrics.items() if isinstance(k, tuple)),
            "products_loaded": len(cache), "log_records": records}


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
