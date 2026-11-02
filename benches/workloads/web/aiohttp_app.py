"""An aiohttp web service and aiohttp client over localhost.

Stands for production async Python HTTP on the most popular asyncio-native framework
(aiohttp server + ``ClientSession``, also used by countless bots, scrapers and service
clients). The server has a router with dynamic segments, a middleware (request id + timing
header), JSON handlers (``json_response``, ``await request.json()``), query parsing via
yarl/multidict, and error responses; the client uses one ``ClientSession`` with a pooled
``TCPConnector`` and 16 concurrent worker coroutines doing keep-alive GET/POST requests,
reading JSON and text. Both in one event loop (``asyncio.run``) per run, 1,600 requests per
run over 127.0.0.1. aiohttp's C extensions (llhttp parser, multidict, yarl, propcache,
frozenlist) are active as installed from wheels; their share is reported separately.
"""

REQUIRES = ["aiohttp==3.14.5", "aiohappyeyeballs==2.7.1", "aiosignal==1.4.0", "attrs==26.1.0",
            "frozenlist==1.8.0", "multidict==7.2.1", "propcache==0.5.4", "yarl==1.25.1", "idna==3.20"]

N_WORKERS = 16
PER_WORKER = 100


def setup():
    import aiohttp  # noqa: F401

    articles = [{"id": i, "title": f"Article number {i}", "author": f"author{i % 37}",
                 "words": 200 + (i * 17) % 1800, "tags": [f"tag{i % 9}", f"tag{i % 4}"],
                 "summary": ("lorem ipsum dolor sit amet " * (1 + i % 6)).strip()} for i in range(1500)]
    return {"articles": articles}


def run(state):
    import asyncio
    import hashlib

    import aiohttp
    from aiohttp import web

    articles = state["articles"]

    @web.middleware
    async def stamp(request, handler):
        resp = await handler(request)
        resp.headers["X-Request-Id"] = request.headers.get("X-Request-Id", "-")
        resp.headers["X-Served-By"] = "bench"
        return resp

    async def list_articles(request):
        q = request.query
        off, lim = int(q.get("offset", "0")), int(q.get("limit", "20"))
        tag = q.get("tag")
        rows = [a for a in articles if tag is None or tag in a["tags"]][off:off + lim]
        return web.json_response({"offset": off, "count": len(rows), "items": rows})

    async def get_article(request):
        i = int(request.match_info["aid"])
        if not 0 <= i < len(articles):
            raise web.HTTPNotFound(text="no such article")
        return web.json_response(articles[i])

    async def post_comment(request):
        data = await request.json()
        text = data.get("text", "")
        if not text:
            return web.json_response({"error": "empty"}, status=400)
        return web.json_response({"article": int(request.match_info["aid"]), "len": len(text),
                                  "words": len(text.split())}, status=201)

    async def health(request):
        return web.Response(text="ok")

    async def main():
        app = web.Application(middlewares=[stamp])
        app.router.add_get("/articles", list_articles)
        app.router.add_get("/articles/{aid:\\d+}", get_article)
        app.router.add_post("/articles/{aid:\\d+}/comments", post_comment)
        app.router.add_get("/health", health)
        runner = web.AppRunner(app, access_log=None)
        await runner.setup()
        site = web.TCPSite(runner, "127.0.0.1", 0)
        await site.start()
        port = runner.addresses[0][1]
        base = f"http://127.0.0.1:{port}"
        try:
            conn = aiohttp.TCPConnector(limit=N_WORKERS)
            async with aiohttp.ClientSession(base, connector=conn) as session:
                async def worker(w):
                    res = []
                    for j in range(PER_WORKER):
                        k = (w + j) % 5
                        hdrs = {"X-Request-Id": f"{w}-{j}"}
                        if k in (0, 3):
                            params = {"offset": str((w * 31 + j * 7) % 1400), "limit": "20"}
                            if j % 3 == 0:
                                params["tag"] = f"tag{j % 9}"
                            async with session.get("/articles", params=params, headers=hdrs) as r:
                                d = await r.json()
                                res.append((r.status, d["count"], sum(a["words"] for a in d["items"])))
                        elif k in (1, 4):
                            async with session.get(f"/articles/{(w * 101 + j * 13) % 1520}", headers=hdrs) as r:
                                if r.status == 200:
                                    d = await r.json()
                                    res.append((r.status, d["title"], r.headers["X-Request-Id"]))
                                else:
                                    res.append((r.status, await r.text()))
                        else:
                            body = {"text": "nice article " * (j % 4), "user": w}
                            async with session.post(f"/articles/{j}/comments", json=body, headers=hdrs) as r:
                                res.append((r.status, await r.json()))
                    return res

                out = await asyncio.gather(*(worker(w) for w in range(N_WORKERS)))
        finally:
            await runner.cleanup()
        return out

    out = asyncio.run(main())
    return [hashlib.sha256(repr(r).encode()).hexdigest() for r in out]


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
