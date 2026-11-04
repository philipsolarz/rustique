"""A producer/consumer log pipeline over `queue.Queue`: the shape of log shippers and ETL stages.

Stands for threaded pipelines (log forwarders, ingestion workers, `queue.Queue`-based
stage graphs as in Celery/Kombu consumers, scrapers and ETL scripts): one producer thread
reads access-log lines in batches of 25 into a bounded queue, `WL_THREADS` (default 8)
parser threads each take a batch, parse every line (a compiled regex shared by all
threads, `datetime.strptime`, `urllib.parse.urlsplit`/`unquote`/`parse_qsl`, `ipaddress`
classification, `str` methods) into records, and put the batch's partial aggregates on an
output queue; one aggregator thread merges them (`collections.Counter`, per-minute
buckets, latency lists) and the result is sorted. 36,000 lines per run. Aggregation is
commutative and the result is sorted, so the digest does not depend on scheduling.
"""

import os

REQUIRES: list[str] = []

N_LINES = 36_000
BATCH = 25


def setup():
    import random

    rng = random.Random(23)
    paths = ["/", "/index.html", "/api/v1/items", "/api/v1/items/{id}", "/static/app.{h}.js",
             "/search", "/login", "/cart/add", "/img/{id}.png", "/docs/guide/{s}.html"]
    agents = ["Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/131.0",
              "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 Safari/605.1.15",
              "curl/8.9.1", "python-requests/2.34.2", "Googlebot/2.1 (+http://www.google.com/bot.html)"]
    months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    lines = []
    for i in range(N_LINES):
        if rng.random() < 0.3:
            ip = f"10.{rng.randint(0, 255)}.{rng.randint(0, 255)}.{rng.randint(1, 254)}"
        elif rng.random() < 0.1:
            ip = f"2001:db8:{rng.randint(0, 0xffff):x}::{rng.randint(1, 0xffff):x}"
        else:
            ip = f"{rng.randint(11, 223)}.{rng.randint(0, 255)}.{rng.randint(0, 255)}.{rng.randint(1, 254)}"
        p = rng.choice(paths).format(id=rng.randint(1, 5000), h=f"{rng.getrandbits(32):08x}",
                                     s=rng.choice(["intro", "api", "faq%20and%20tips"]))
        if p == "/search":
            p += f"?q={rng.choice(['red+mug', 'lamp', 'caf%C3%A9', 'desk%2Fchair'])}&page={rng.randint(1, 9)}"
        ts = (f"{rng.randint(1, 28):02d}/{rng.choice(months)}/2026:{rng.randint(0, 23):02d}:"
              f"{rng.randint(0, 59):02d}:{rng.randint(0, 59):02d} +0000")
        status = rng.choice([200] * 12 + [301, 304, 404, 404, 500])
        line = (f'{ip} - - [{ts}] "{rng.choice(["GET"] * 6 + ["POST", "HEAD"])} {p} HTTP/1.1" '
                f'{status} {rng.randint(0, 90000)} "-" "{rng.choice(agents)}" rt={rng.randint(1, 4000) / 1000:.3f}')
        if i % 997 == 0:
            line = "garbage " + line[:40]
        lines.append(line)
    batches = [lines[i:i + BATCH] for i in range(0, len(lines), BATCH)]
    threads = int(os.environ.get("WL_THREADS", "8"))
    return {"batches": batches, "threads": threads}


def _parser():
    import ipaddress
    import re
    from collections import Counter
    from datetime import datetime
    from urllib.parse import parse_qsl, unquote, urlsplit

    line_re = re.compile(
        r'(?P<ip>\S+) \S+ \S+ \[(?P<ts>[^\]]+)\] "(?P<method>[A-Z]+) (?P<target>\S+) HTTP/[\d.]+" '
        r'(?P<status>\d{3}) (?P<size>\d+) "[^"]*" "(?P<agent>[^"]*)" rt=(?P<rt>[\d.]+)$')

    def parse_batch(batch):
        hits = Counter()
        statuses = Counter()
        minutes = Counter()
        terms = Counter()
        bad = 0
        private = 0
        bytes_out = 0
        slow = []
        for line in batch:
            m = line_re.match(line)
            if m is None:
                bad += 1
                continue
            ts = datetime.strptime(m["ts"], "%d/%b/%Y:%H:%M:%S %z")
            parts = urlsplit(m["target"])
            path = unquote(parts.path)
            if path.startswith("/api/v1/items/"):
                path = "/api/v1/items/:id"
            elif path.startswith("/img/"):
                path = "/img/:id"
            for k, v in parse_qsl(parts.query):
                if k == "q":
                    terms[v.lower()] += 1
            ip = ipaddress.ip_address(m["ip"])
            if ip.is_private:
                private += 1
            hits[(m["method"], path)] += 1
            statuses[int(m["status"])] += 1
            minutes[ts.strftime("%Y-%m-%dT%H:%M")] += 1
            bytes_out += int(m["size"])
            rt = float(m["rt"])
            if rt > 3.5:
                slow.append((ts.isoformat(), path, rt))
        return hits, statuses, minutes, terms, bad, private, bytes_out, slow

    return parse_batch


def run(state):
    import queue
    import threading
    from collections import Counter

    parse_batch = _parser()
    n = state["threads"]
    inq = queue.Queue(maxsize=4 * n)
    outq = queue.Queue()
    done = object()

    def producer():
        for b in state["batches"]:
            inq.put(b)
        for _ in range(n):
            inq.put(done)

    def worker():
        while True:
            b = inq.get()
            if b is done:
                outq.put(done)
                return
            outq.put(parse_batch(b))

    agg = {"hits": Counter(), "statuses": Counter(), "minutes": Counter(), "terms": Counter(),
           "bad": 0, "private": 0, "bytes": 0, "slow": []}

    def aggregator():
        finished = 0
        while finished < n:
            r = outq.get()
            if r is done:
                finished += 1
                continue
            hits, statuses, minutes, terms, bad, private, bytes_out, slow = r
            agg["hits"].update(hits)
            agg["statuses"].update(statuses)
            agg["minutes"].update(minutes)
            agg["terms"].update(terms)
            agg["bad"] += bad
            agg["private"] += private
            agg["bytes"] += bytes_out
            agg["slow"].extend(slow)

    threads = [threading.Thread(target=producer), threading.Thread(target=aggregator)]
    threads += [threading.Thread(target=worker) for _ in range(n)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return {
        "hits": sorted(agg["hits"].items()),
        "statuses": sorted(agg["statuses"].items()),
        "busiest": sorted(agg["minutes"].items(), key=lambda kv: (-kv[1], kv[0]))[:20],
        "terms": sorted(agg["terms"].items()),
        "bad": agg["bad"], "private": agg["private"], "bytes": agg["bytes"],
        "slow": sorted(agg["slow"]),
    }


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
