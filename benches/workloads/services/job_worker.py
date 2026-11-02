"""A long-running background job worker: JSON messages, retries, caching, structured logs.

Stands for the worker side of every Python web product (Celery/RQ/Dramatiq/arq-style
workers): a pool of 4 worker threads, started once and kept alive across runs, consumes
JSON job envelopes from a queue, verifies their HMAC signature, dispatches by task name
through a registry, runs each task under a tenacity ``Retrying`` policy (transient
failures are retried; deterministic 1-in-7 first-attempt failures), memoizes reference
data lookups in a thread-safe ``cachetools`` LRU cache, stores results in a ``TTLCache``
results backend, and writes one structured JSON log line per job state change through a
``LoggerAdapter`` to a file handler. Tasks are the bread and butter of background jobs:
rendering an invoice (``string.Template``, ``Decimal``), aggregating metrics
(``statistics``), normalizing user records (``unicodedata``, ``re``, ``hashlib``),
enriching orders (``datetime.fromisoformat``, ``zoneinfo``, cached lookups) and a flaky
sync call. Each run enqueues 6,000 jobs and waits for the queue to drain
(``Queue.join``); the heap, caches and GC generations persist across runs. Peak RSS and
heap growth are recorded by ``services/_steady.py``.
"""

REQUIRES = ["tenacity==9.2.1", "cachetools==7.2.8"]

N_JOBS = 6000
SECRET = b"worker-signing-key-0123456789"


def setup():
    import hashlib
    import hmac
    import json
    import logging
    import os
    import queue
    import random
    import re
    import statistics
    import string
    import threading
    import unicodedata
    from datetime import datetime, timezone
    from decimal import ROUND_HALF_EVEN, Decimal
    from zoneinfo import ZoneInfo

    import cachetools
    from tenacity import Retrying, retry_if_exception_type, stop_after_attempt, wait_exponential

    class TransientError(Exception):
        pass

    class JsonFormatter(logging.Formatter):
        def format(self, record):
            return json.dumps({"ts": self.formatTime(record), "lvl": record.levelname, "logger": record.name,
                               "msg": record.getMessage(), "job": getattr(record, "job", None),
                               "task": getattr(record, "task", None), "thread": record.threadName})

    log = logging.getLogger("worker")
    log.setLevel(logging.INFO)
    log.propagate = False
    h = logging.FileHandler(os.devnull)
    h.setFormatter(JsonFormatter())
    log.addHandler(h)

    rng = random.Random(10)
    products = {f"P{i:04d}": {"name": f"Product {i}", "price": Decimal(rng.randint(100, 50000)) / 100,
                              "vat": Decimal(rng.choice(["0.23", "0.19", "0.07", "0.00"]))} for i in range(800)}
    zones = ["Europe/Warsaw", "America/New_York", "Asia/Tokyo", "Europe/Lisbon", "Australia/Sydney"]
    lookup_lock = threading.Lock()

    @cachetools.cached(cache=cachetools.LRUCache(maxsize=256), lock=lookup_lock)
    def product(code):
        p = products[code]
        return {"code": code, **p}

    @cachetools.cached(cache=cachetools.LRUCache(maxsize=16), lock=lookup_lock)
    def zone(name):
        return ZoneInfo(name)

    INVOICE = string.Template("INVOICE $number\nCustomer: $customer\n$lines\nNet: $net\nVAT: $vat\nTotal: $total $currency\n")
    cent = Decimal("0.01")

    def render_invoice(number, customer, lines, currency="EUR"):
        out, net, vat = [], Decimal(0), Decimal(0)
        for code, qty in lines:
            p = product(code)
            amount = (p["price"] * qty).quantize(cent, ROUND_HALF_EVEN)
            net += amount
            vat += (amount * p["vat"]).quantize(cent, ROUND_HALF_EVEN)
            out.append(f"{qty:>3} x {p['name']:<20} {amount:>10}")
        text = INVOICE.substitute(number=number, customer=customer, lines="\n".join(out), net=net, vat=vat,
                                  total=net + vat, currency=currency)
        return {"number": number, "total": str(net + vat), "digest": hashlib.sha1(text.encode()).hexdigest()}

    def aggregate_metrics(series, name):
        return {"name": name, "n": len(series), "mean": round(statistics.fmean(series), 6),
                "median": statistics.median(series), "p95": sorted(series)[int(len(series) * 0.95) - 1],
                "stdev": round(statistics.pstdev(series), 6), "max": max(series)}

    slug_re = re.compile(r"[^a-z0-9]+")
    email_re = re.compile(r"^[^@\s]+@[^@\s]+\.[a-z]{2,}$")

    def normalize_user(name, email, city):
        n = unicodedata.normalize("NFKC", name).strip()
        e = email.strip().lower()
        if not email_re.match(e):
            raise ValueError(f"bad email {email!r}")
        ascii_name = unicodedata.normalize("NFKD", n).encode("ascii", "ignore").decode()
        return {"name": n.title(), "email": e, "slug": slug_re.sub("-", ascii_name.lower()).strip("-"),
                "avatar": hashlib.md5(e.encode()).hexdigest(), "city": city.title()}

    def enrich_order(order_id, placed_at, tz, items):
        when = datetime.fromisoformat(placed_at).astimezone(zone(tz))
        total = sum((product(c)["price"] * q for c, q in items), Decimal(0))
        return {"order": order_id, "local": when.isoformat(), "weekday": when.strftime("%A"),
                "items": len(items), "total": str(total.quantize(cent))}

    attempts = {}
    attempts_lock = threading.Lock()

    def flaky_sync(job_id, payload):
        with attempts_lock:
            n = attempts[job_id] = attempts.get(job_id, 0) + 1
        if job_id % 7 == 0 and n == 1:
            raise TransientError("upstream timeout")
        return {"synced": job_id, "bytes": len(json.dumps(payload))}

    registry = {"invoice.render": render_invoice, "metrics.aggregate": aggregate_metrics,
                "user.normalize": normalize_user, "order.enrich": enrich_order, "crm.sync": flaky_sync}
    results_backend = cachetools.TTLCache(maxsize=20000, ttl=3600)
    backend_lock = threading.Lock()
    q = queue.Queue(maxsize=1000)
    state = {"results": {}}

    def handle(raw):
        env = json.loads(raw)
        body = env["body"]
        sig = hmac.new(SECRET, body.encode(), hashlib.sha256).hexdigest()
        if not hmac.compare_digest(sig, env["headers"]["signature"]):
            raise PermissionError("bad signature")
        msg = json.loads(body)
        jl = logging.LoggerAdapter(log, {"job": msg["id"], "task": msg["task"]})
        jl.info("received")
        fn = registry[msg["task"]]
        retrying = Retrying(stop=stop_after_attempt(3), retry=retry_if_exception_type(TransientError),
                            wait=wait_exponential(multiplier=0.001, max=0.01), sleep=lambda s: None, reraise=True)
        try:
            for attempt in retrying:
                with attempt:
                    if attempt.retry_state.attempt_number > 1:
                        jl.warning("retry %d", attempt.retry_state.attempt_number)
                    res = fn(*msg.get("args", []), **msg.get("kwargs", {}))
            status = "SUCCESS"
        except Exception as e:  # noqa: BLE001 - a worker records every failure
            res, status = {"error": type(e).__name__, "detail": str(e)}, "FAILURE"
            jl.error("failed: %s", e)
        jl.info("done %s", status)
        with backend_lock:
            results_backend[f"result:{msg['id']}"] = json.dumps({"status": status, "result": res}, default=str)
        state["results"][msg["id"]] = (status, json.dumps(res, sort_keys=True, default=str))

    def worker():
        while True:
            raw = q.get()
            try:
                handle(raw)
            finally:
                q.task_done()

    for i in range(4):
        threading.Thread(target=worker, name=f"worker-{i}", daemon=True).start()

    names = ["Zoë Ångström", "José Núñez", "Ｆｕｌｌ Ｗｉｄｔｈ", "Łukasz Żółw", "Ana María", "Björk Guðmundsdóttir"]
    codes = sorted(products)
    jobs = []
    for i in range(N_JOBS):
        k = i % 5
        if k == 0:
            task, kwargs = "invoice.render", {"number": f"INV-{i:06d}", "customer": names[i % 6],
                                               "lines": [[codes[(i * j) % 800], 1 + j % 4] for j in range(1, 2 + i % 9)]}
        elif k == 1:
            task, kwargs = "metrics.aggregate", {"name": f"latency.{i % 20}", "series": [round(rng.lognormvariate(3, 0.6), 3) for _ in range(60)]}
        elif k == 2:
            task, kwargs = "user.normalize", {"name": f"  {names[i % 6]} {i} ", "email": f" User.{i}@Example.ORG " if i % 13 else "broken@",
                                               "city": rng.choice(["kraków", "são paulo", "zürich", "malmö"])}
        elif k == 3:
            task, kwargs = "order.enrich", {"order_id": f"O{i}", "placed_at": f"2025-03-{1 + i % 28:02d}T{i % 24:02d}:{i % 60:02d}:00+00:00",
                                             "tz": zones[i % 5], "items": [[codes[(i + j * 31) % 800], 1 + j] for j in range(1 + i % 5)]}
        else:
            task, kwargs = "crm.sync", {"job_id": i, "payload": {"contact": names[i % 6], "score": i % 100}}
        body = json.dumps({"id": i, "task": task, "kwargs": kwargs, "eta": None, "retries": 0})
        jobs.append(json.dumps({"headers": {"signature": hmac.new(SECRET, body.encode(), hashlib.sha256).hexdigest(),
                                            "content-type": "application/json", "origin": "web-1"}, "body": body}))
    return {"q": q, "jobs": jobs, "state": state, "attempts": attempts}


def run(state):
    import hashlib

    q, st = state["q"], state["state"]
    st["results"] = {}
    state["attempts"].clear()
    for raw in state["jobs"]:
        q.put(raw)
    q.join()
    res = st["results"]
    h = hashlib.sha256()
    counts = {}
    for k in sorted(res):
        status, payload = res[k]
        counts[status] = counts.get(status, 0) + 1
        h.update(f"{k}:{status}:{payload}\n".encode())
    return sorted(counts.items()), h.hexdigest()


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
