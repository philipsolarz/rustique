"""A pure-stdlib asyncio HTTP/1.1 JSON service and its concurrent clients over localhost.

Stands for hand-rolled asyncio network services (internal APIs, proxies, gateways, bots)
and for the event-loop layer under every asyncio framework: ``asyncio.start_server`` with
``StreamReader``/``StreamWriter``, per-connection handler coroutines that parse request
lines and headers with ``readline``, read bodies with ``readexactly``, route, and write
JSON responses with ``drain``; and 16 concurrent keep-alive client coroutines
(``open_connection``) issuing GET/POST requests and parsing responses, gathered with
``asyncio.gather``. One event loop (``asyncio.run``) per run, 5,120 requests per run over
127.0.0.1. No third-party code.
"""

REQUIRES: list[str] = []

N_CLIENTS = 16
PER_CLIENT = 320


def setup():
    items = [{"id": i, "name": f"item-{i:05d}", "price": round((i * 7919) % 10000 / 100, 2),
              "tags": ["t%d" % (i % 5), "t%d" % (i % 11)], "stock": (i * 31) % 97}
             for i in range(2000)]
    return {"items": items}


def run(state):
    import asyncio
    import hashlib
    import json

    items = state["items"]
    reasons = {200: "OK", 201: "Created", 400: "Bad Request", 404: "Not Found"}

    def respond(code, obj):
        body = json.dumps(obj, separators=(",", ":")).encode()
        head = (f"HTTP/1.1 {code} {reasons[code]}\r\nContent-Type: application/json\r\n"
                f"Content-Length: {len(body)}\r\nConnection: keep-alive\r\n\r\n")
        return head.encode("latin-1") + body

    async def handle(reader, writer):
        try:
            while True:
                line = await reader.readline()
                if not line:
                    break
                method, target, _version = line.decode("latin-1").rstrip("\r\n").split(" ", 2)
                headers = {}
                while True:
                    h = await reader.readline()
                    if h in (b"\r\n", b"\n", b""):
                        break
                    name, _, value = h.decode("latin-1").partition(":")
                    headers[name.strip().lower()] = value.strip()
                body = b""
                n = int(headers.get("content-length", "0"))
                if n:
                    body = await reader.readexactly(n)
                path, _, query = target.partition("?")
                params = dict(p.split("=", 1) for p in query.split("&") if "=" in p)
                if method == "GET" and path == "/items":
                    off, lim = int(params.get("offset", 0)), int(params.get("limit", 20))
                    resp = respond(200, {"offset": off, "items": items[off:off + lim]})
                elif method == "GET" and path.startswith("/items/"):
                    i = int(path[7:])
                    resp = respond(200, items[i]) if 0 <= i < len(items) else respond(404, {"error": "no item"})
                elif method == "POST" and path == "/orders":
                    try:
                        order = json.loads(body)
                        total = sum(items[l["id"]]["price"] * l["qty"] for l in order["lines"])
                        resp = respond(201, {"ok": True, "total": round(total, 2), "lines": len(order["lines"])})
                    except (ValueError, KeyError, IndexError):
                        resp = respond(400, {"error": "bad order"})
                else:
                    resp = respond(404, {"error": "no route"})
                writer.write(resp)
                await writer.drain()
        finally:
            writer.close()

    async def client(port, cid):
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        results = []
        for j in range(PER_CLIENT):
            k = (cid + j) % 6
            if k in (0, 3):
                req = f"GET /items?offset={(cid * 101 + j * 13) % 1980}&limit=20 HTTP/1.1\r\nHost: x\r\n\r\n".encode()
            elif k in (1, 4):
                req = f"GET /items/{(cid * 997 + j * 61) % 2050} HTTP/1.1\r\nHost: x\r\nAccept: application/json\r\n\r\n".encode()
            elif k == 2:
                payload = json.dumps({"customer": cid, "lines": [{"id": (cid + j + x) % 2000, "qty": 1 + x % 3}
                                                                  for x in range(5)]}).encode()
                req = (f"POST /orders HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n"
                       f"Content-Length: {len(payload)}\r\n\r\n").encode() + payload
            else:
                req = b"POST /orders HTTP/1.1\r\nHost: x\r\nContent-Length: 9\r\n\r\nnot json!"
            writer.write(req)
            await writer.drain()
            status = int((await reader.readline()).split()[1])
            length = 0
            while True:
                h = await reader.readline()
                if h == b"\r\n":
                    break
                if h[:15].lower() == b"content-length:":
                    length = int(h[15:])
            raw = await reader.readexactly(length)
            data = json.loads(raw)
            results.append((status, len(data), hashlib.sha256(raw).hexdigest()[:12]))
        writer.close()
        await writer.wait_closed()
        return results

    async def main():
        server = await asyncio.start_server(handle, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        async with server:
            res = await asyncio.gather(*(client(port, c) for c in range(N_CLIENTS)))
            server.close()
        return res

    return asyncio.run(main())


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
