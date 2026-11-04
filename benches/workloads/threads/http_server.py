"""A threaded HTTP/1.1 JSON service with in-process clients over 127.0.0.1.

Stands for the classic thread-per-connection server (`http.server.ThreadingHTTPServer`,
the model behind `wsgiref`/`socketserver`-based services, internal tools and test
servers) under concurrent keep-alive clients (`http.client`, what `urllib.request` and
many SDKs use underneath). `setup()` starts the server on an ephemeral port; each run
opens `WL_THREADS` (default 8) client threads, each with one persistent connection, and
sends 3,600 requests in total: JSON GETs with query strings, JSON POSTs that the handler
parses, validates and stores in a shared dict under a `threading.Lock`, and HEAD/404
paths. The server parses request lines and headers (`email.parser` via
`http.client.parse_headers`), routes with `urllib.parse`, and writes JSON responses.
Every client's requests are fixed in advance and the digest covers response status and
body per request (no `Date` header), so it does not depend on scheduling.
"""

import os

REQUIRES: list[str] = []

N_REQUESTS = 3600


def _make_server():
    import json
    import threading
    from datetime import datetime, timezone
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from urllib.parse import parse_qs, urlsplit

    store = {}
    store_lock = threading.Lock()
    catalog = {f"item-{i}": {"id": f"item-{i}", "name": f"Widget {i}", "price": round(1.5 + i * 0.37, 2),
                             "tags": ["a", "b", "c"][: i % 4]} for i in range(400)}

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "wl/1.0"
        disable_nagle_algorithm = True  # TCP_NODELAY, as every production server sets

        def log_message(self, format, *args):  # quiet, as in production behind a proxy
            pass

        def _send(self, status, doc):
            body = json.dumps(doc, sort_keys=True).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def do_GET(self):
            parts = urlsplit(self.path)
            qs = parse_qs(parts.query)
            if parts.path == "/items":
                ids = qs.get("id", [])
                items = [catalog[i] for i in ids if i in catalog]
                limit = int(qs.get("limit", ["50"])[0])
                self._send(200, {"items": items[:limit], "missing": sorted(set(ids) - set(catalog))})
            elif parts.path.startswith("/orders/"):
                oid = parts.path.rsplit("/", 1)[1]
                with store_lock:
                    order = store.get(oid)
                if order is None:
                    self._send(404, {"error": "no such order", "id": oid})
                else:
                    self._send(200, order)
            else:
                self._send(404, {"error": "not found", "path": parts.path})

        do_HEAD = do_GET

        def do_POST(self):
            n = int(self.headers.get("Content-Length", "0"))
            doc = json.loads(self.rfile.read(n))
            lines = []
            total = 0.0
            for it in doc.get("lines", []):
                p = catalog.get(it["id"])
                if p is None:
                    self._send(422, {"error": "unknown item", "id": it["id"]})
                    return
                amount = round(p["price"] * it["qty"], 2)
                total += amount
                lines.append({"id": it["id"], "qty": it["qty"], "amount": amount})
            ts = datetime.fromisoformat(doc["at"]).astimezone(timezone.utc)
            order = {"id": doc["id"], "customer": doc["customer"].strip().lower(), "lines": lines,
                     "total": round(total, 2), "at": ts.isoformat(), "day": ts.strftime("%A")}
            with store_lock:
                store[doc["id"]] = order
            self._send(201, order)

    ThreadingHTTPServer.daemon_threads = True
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.request_queue_size = 64
    t = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    t.start()
    return server


def setup():
    import json
    import random

    rng = random.Random(31)
    threads = int(os.environ.get("WL_THREADS", "8"))
    reqs = []
    for i in range(N_REQUESTS):
        k = i % 6
        if k in (0, 1):
            ids = "&".join(f"id=item-{rng.randint(0, 450)}" for _ in range(rng.randint(1, 10)))
            reqs.append(("GET", f"/items?{ids}&limit={rng.randint(3, 12)}", None))
        elif k in (2, 3):
            oid = f"o{i}"
            body = {"id": oid, "customer": f"  Customer{rng.randint(1, 300)}@Example.COM ",
                    "lines": [{"id": f"item-{rng.randint(0, 399)}", "qty": rng.randint(1, 9)}
                              for _ in range(rng.randint(1, 7))],
                    "at": f"2026-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}T{rng.randint(0, 23):02d}:"
                          f"{rng.randint(0, 59):02d}:00+0{rng.randint(0, 5)}:00"}
            reqs.append(("POST", "/orders", json.dumps(body)))
        elif k == 4:
            # One of the two orders posted earlier in the same group (same client, same
            # connection, so already stored whatever the scheduling).
            reqs.append(("GET", f"/orders/o{i - rng.choice([1, 2])}", None))
        else:
            reqs.append((rng.choice(["HEAD", "GET"]), f"/nope/{i}", None))
    # Requests are split round-robin per group of 6, so a client's GET /orders/<id> refers
    # to an order the same client posted before (keeps the result independent of scheduling).
    groups = [reqs[i:i + 6] for i in range(0, len(reqs), 6)]
    return {"server": _make_server(), "groups": groups, "threads": threads}


def run(state):
    import http.client
    import threading

    server = state["server"]
    host, port = server.server_address[:2]
    n = state["threads"]
    groups = state["groups"]
    results = [None] * len(groups)

    def client(k):
        conn = http.client.HTTPConnection(host, port, timeout=30)
        for gi in range(k, len(groups), n):
            out = []
            for method, path, body in groups[gi]:
                headers = {"Accept": "application/json"}
                if body is not None:
                    headers["Content-Type"] = "application/json"
                conn.request(method, path, body=body, headers=headers)
                r = conn.getresponse()
                data = r.read()
                out.append((r.status, r.getheader("Content-Type"), data))
            results[gi] = out
        conn.close()

    ts = [threading.Thread(target=client, args=(k,)) for k in range(n)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    return results


def digest(result):
    import hashlib

    h = hashlib.sha256()
    for group in result:
        for status, ctype, data in group:
            h.update(f"{status} {ctype} ".encode())
            h.update(data)
            h.update(b"\n")
    return h.hexdigest()
