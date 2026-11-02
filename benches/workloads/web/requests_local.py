"""A service calling a JSON/HTTP API with ``requests``, against a local ``http.server``.

Stands for the most common outbound-HTTP code in Python (``requests`` is the most
downloaded package): an API client using a pooled ``requests.Session`` (urllib3 connection
pool, keep-alive) to page through a REST API, fetch single resources, POST JSON events and
download a CSV export, with query params, custom headers, ``raise_for_status`` and
``resp.json()``. The server side is the stdlib's ``http.server`` (``ThreadingHTTPServer``,
HTTP/1.1 keep-alive) in a background thread of the same process, so both ends of a typical
internal-service exchange are profiled: request line/header parsing
(``http.client.parse_headers`` -> ``email.parser``), JSON encode/decode, socket I/O.
540 requests per run over 127.0.0.1.
"""

REQUIRES = ["requests==2.34.2", "urllib3==2.8.0", "charset-normalizer==3.5.2", "idna==3.20",
            "certifi==2026.10.23"]


def _make_server():
    import json
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from urllib.parse import parse_qs, urlsplit

    users = [{"id": i, "username": f"user{i:04d}", "email": f"user{i}@example.com",
              "name": f"Test User {i}", "active": i % 7 != 0, "roles": ["reader"] + (["admin"] if i % 50 == 0 else []),
              "profile": {"city": ("Berlin", "Austin", "Kyoto", "Lagos")[i % 4], "score": (i * 37) % 1000 / 10}}
             for i in range(1000)]
    csv_body = ("id,username,email,score\n" + "".join(
        f"{u['id']},{u['username']},{u['email']},{u['profile']['score']}\n" for u in users)).encode()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "bench/1.0"
        disable_nagle_algorithm = True  # TCP_NODELAY, as production servers set it

        def log_message(self, fmt, *args):
            pass

        def _send(self, code, body, ctype="application/json"):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            parts = urlsplit(self.path)
            q = parse_qs(parts.query)
            if self.headers.get("Authorization") != "Bearer t0ken":
                return self._send(401, b'{"error": "unauthorized"}')
            if parts.path == "/api/users":
                page = int(q.get("page", ["1"])[0])
                per = int(q.get("per_page", ["25"])[0])
                rows = users[(page - 1) * per:page * per]
                body = json.dumps({"page": page, "per_page": per, "total": len(users), "data": rows}).encode()
                return self._send(200, body)
            if parts.path.startswith("/api/users/"):
                uid = int(parts.path.rsplit("/", 1)[1])
                if 0 <= uid < len(users):
                    return self._send(200, json.dumps(users[uid]).encode())
                return self._send(404, b'{"error": "not found"}')
            if parts.path == "/export.csv":
                return self._send(200, csv_body, "text/csv; charset=utf-8")
            return self._send(404, b'{"error": "no route"}')

        def do_POST(self):
            n = int(self.headers.get("Content-Length", "0"))
            payload = json.loads(self.rfile.read(n))
            events = payload.get("events", [])
            body = json.dumps({"accepted": len(events), "first": events[0]["type"] if events else None}).encode()
            self._send(201, body)

    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    srv.daemon_threads = True
    t = threading.Thread(target=srv.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    t.start()
    return srv


def setup():
    import requests  # noqa: F401  (import cost outside the measured region)

    srv = _make_server()
    return {"server": srv, "base": f"http://127.0.0.1:{srv.server_address[1]}"}


def run(state):
    import hashlib

    import requests

    base = state["base"]
    out = []
    with requests.Session() as s:
        s.headers.update({"Authorization": "Bearer t0ken", "User-Agent": "bench-client/1.0",
                          "Accept": "application/json"})
        for i in range(540):
            k = i % 9
            if k in (0, 4):
                r = s.get(f"{base}/api/users", params={"page": 1 + (i // 9) % 40, "per_page": 25}, timeout=10)
                r.raise_for_status()
                data = r.json()
                out.append((r.status_code, data["page"], sum(u["profile"]["score"] for u in data["data"])))
            elif k in (1, 2, 5, 6):
                r = s.get(f"{base}/api/users/{(i * 97) % 1010}", timeout=10)
                out.append((r.status_code, r.json().get("username"), r.headers.get("Content-Type")))
            elif k in (3, 7):
                events = [{"type": ("click", "view", "buy")[j % 3], "user": (i + j) % 1000,
                           "ts": 1700000000 + i * 60 + j, "props": {"path": f"/p/{j}", "ref": "home"}}
                          for j in range(20)]
                r = s.post(f"{base}/api/events", json={"events": events}, timeout=10)
                out.append((r.status_code, r.json()["accepted"]))
            else:
                r = s.get(f"{base}/export.csv", timeout=10)
                text = r.text
                out.append((r.status_code, len(text), hashlib.sha256(text.encode()).hexdigest()[:16],
                            r.encoding))
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
