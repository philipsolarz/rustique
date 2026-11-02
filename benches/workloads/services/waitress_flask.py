"""A long-running waitress + Flask service with a pool of keep-alive client threads.

Stands for the classic threaded WSGI deployment: a Flask app served by waitress (the
pure-Python production WSGI server recommended by Flask's and Pyramid's docs and the
default on Windows; one asyncore-style I/O thread plus a pool of worker threads). The
server starts once in setup and keeps running across runs (its channels, worker threads,
Jinja2 template cache, heap and GC generations persist). Each run, 8 client threads
drive 2,400 requests over 8 persistent HTTP/1.1 keep-alive connections on 127.0.0.1 and
read every response; clients are thin (pre-encoded requests, raw sockets,
Content-Length framing) so the time measured is the server's: waitress's request parsing
and task dispatch to its 4 worker threads, Werkzeug's WSGI request/response objects and
routing, Flask's ``before_request``/``after_request`` hooks, Jinja2 page rendering with
inheritance and loops, ``jsonify``, form POST parsing and validation, per-request app
logging to a file handler, and the GIL hand-offs between the I/O thread, the workers and
the clients. Peak RSS and heap growth are recorded by ``services/_steady.py``.
"""

REQUIRES = ["waitress==3.0.2", "Flask==3.1.3", "Werkzeug==3.1.9", "Jinja2==3.1.6", "MarkupSafe==3.0.4",
            "itsdangerous==2.2.0", "click==8.5.0", "blinker==1.9.0"]

N_CONN = 8
N_REQ = 2400

TEMPLATES = {
    "base.html": """<!doctype html><html><head><title>{% block title %}Status{% endblock %} - Ops</title></head>
<body><nav>{% for name, url in nav %}<a href="{{ url }}"{% if name == active %} class="on"{% endif %}>{{ name }}</a>{% endfor %}</nav>
<main>{% block body %}{% endblock %}</main><footer>{{ user or "anonymous" }} &middot; build {{ build }}</footer></body></html>""",
    "services.html": """{% extends "base.html" %}{% block title %}Services{% endblock %}{% block body %}
<h1>{{ services|length }} services, {{ down }} degraded</h1><table>
{% for s in services %}<tr class="{{ 'bad' if s.errors > 5 else 'ok' }}"><td><a href="/services/{{ s.name }}">{{ s.name }}</a></td>
<td>{{ s.owner|title }}</td><td>{{ "%.1f"|format(s.latency) }} ms</td><td>{{ s.errors }}</td>
<td>{% for t in s.tags %}<span>{{ t }}</span>{% endfor %}</td></tr>{% endfor %}</table>{% endblock %}""",
    "service.html": """{% extends "base.html" %}{% block title %}{{ s.name }}{% endblock %}{% block body %}
<h1>{{ s.name }} <small>{{ s.owner }}</small></h1><dl>{% for k, v in s.meta.items() %}<dt>{{ k }}</dt><dd>{{ v }}</dd>{% endfor %}</dl>
<ol>{% for e in s.events[-15:]|reverse %}<li>{{ e.at }} &ndash; {{ e.msg|e }}</li>{% endfor %}</ol>
<form method="post" action="/services/{{ s.name }}/notes"><textarea name="note"></textarea><button>Add</button></form>{% endblock %}""",
}


def setup():
    import logging
    import os
    import random
    import socket
    import threading
    import time

    import waitress
    from flask import Flask, abort, g, jsonify, render_template, request
    from jinja2 import DictLoader

    rng = random.Random(3)
    owners = ["payments", "search", "identity", "growth", "platform"]
    services = {}
    for i in range(60):
        name = f"svc-{owners[i % 5]}-{i:02d}"
        services[name] = {"name": name, "owner": owners[i % 5], "latency": rng.uniform(2, 400), "errors": rng.randint(0, 12),
                          "tags": rng.sample(["db", "cache", "grpc", "http", "batch", "edge"], 2),
                          "meta": {"repo": f"git@example.com:{name}.git", "tier": str(1 + i % 3), "oncall": f"team-{i % 5}"},
                          "events": [{"at": f"2025-03-01T{h:02d}:00Z", "msg": f"deploy <{h}> ok & healthy"} for h in range(24)]}

    app = Flask("ops_status")
    app.jinja_loader = DictLoader(TEMPLATES)
    log = logging.getLogger("ops_status.requests")
    log.setLevel(logging.INFO)
    log.propagate = False
    handler = logging.FileHandler(os.devnull)
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s %(message)s"))
    log.addHandler(handler)
    logging.getLogger("waitress").addHandler(handler)  # server warnings ("Task queue depth") to the log file
    nav = [("Services", "/services"), ("API", "/api/services"), ("Health", "/healthz")]

    @app.before_request
    def who():
        g.user = request.headers.get("X-User")
        g.t0 = time.perf_counter()

    @app.after_request
    def secure(resp):
        resp.headers["X-Content-Type-Options"] = "nosniff"
        resp.headers["X-Frame-Options"] = "DENY"
        log.info("%s %s %s user=%s", request.method, request.path, resp.status_code, g.get("user"))
        return resp

    @app.get("/healthz")
    def healthz():
        return jsonify(status="ok", services=len(services))

    @app.get("/services")
    def list_services():
        owner = request.args.get("owner")
        rows = sorted((s for s in services.values() if owner is None or s["owner"] == owner), key=lambda s: s["name"])
        return render_template("services.html", services=rows, down=sum(s["errors"] > 5 for s in rows), nav=nav,
                               active="Services", user=g.user, build="2025.03.1")

    @app.get("/services/<name>")
    def service(name):
        s = services.get(name) or abort(404)
        return render_template("service.html", s=s, nav=nav, active="Services", user=g.user, build="2025.03.1")

    @app.get("/api/services")
    def api_services():
        limit = request.args.get("limit", 20, type=int)
        rows = sorted(services.values(), key=lambda s: (-s["errors"], s["name"]))[:limit]
        return jsonify([{k: s[k] for k in ("name", "owner", "latency", "errors", "tags")} for s in rows])

    @app.post("/services/<name>/notes")
    def add_note(name):
        s = services.get(name) or abort(404)
        note = request.form.get("note", "").strip()
        if len(note) < 3:
            return jsonify(error="note too short"), 400
        notes = s.setdefault("notes", [])  # not rendered: responses must not depend on thread interleaving
        notes.append({"at": "2025-03-02T00:00Z", "msg": note, "by": g.user})
        del notes[:-40]  # bounded history: steady state
        return jsonify(ok=True, service=name)

    server = waitress.create_server(app, host="127.0.0.1", port=0, threads=4, channel_timeout=3600,
                                    connection_limit=100, ident="ops")
    port = server.effective_port
    threading.Thread(target=server.run, name="waitress-io", daemon=True).start()
    conns = [socket.create_connection(("127.0.0.1", port)) for _ in range(N_CONN)]

    def req(method, path, form=None, user=None):
        head = f"{method} {path} HTTP/1.1\r\nHost: ops.local\r\nUser-Agent: bench/1.0\r\n"
        if user:
            head += f"X-User: {user}\r\n"
        if form is None:
            return (head + "\r\n").encode()
        from urllib.parse import urlencode
        data = urlencode(form).encode()
        return (head + f"Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {len(data)}\r\n\r\n").encode() + data

    names = sorted(services)
    plan = []
    for i in range(N_REQ):
        k = i % 8
        user = f"user{i % 7}" if i % 3 else None
        if k in (0, 4):
            plan.append(req("GET", f"/services/{names[(i * 7) % 60]}", user=user))
        elif k == 1:
            plan.append(req("GET", "/services" + (f"?owner={owners[i % 5]}" if i % 2 else ""), user=user))
        elif k in (2, 6):
            plan.append(req("GET", f"/api/services?limit={5 + i % 30}"))
        elif k == 3:
            plan.append(req("POST", f"/services/{names[i % 60]}/notes", form={"note": f"checked by bench #{i % 50}"}, user=user))
        elif k == 5:
            plan.append(req("GET", "/healthz"))
        else:
            plan.append(req("GET", "/services/nope") if i % 16 == 7 else req("POST", f"/services/{names[0]}/notes", form={"note": ""}))
    return {"conns": conns, "plan": plan, "server": server}


def _client(sock, reqs, out, base):
    buf = b""
    for j, r in enumerate(reqs):
        sock.sendall(r)
        while b"\r\n\r\n" not in buf:
            buf += sock.recv(65536)
        head, _, buf = buf.partition(b"\r\n\r\n")
        status = int(head[9:12])
        n = 0
        for line in head.split(b"\r\n"):
            if line[:15].lower() == b"content-length:":
                n = int(line[15:])
                break
        while len(buf) < n:
            buf += sock.recv(65536)
        out[base + j] = (status, buf[:n])
        buf = buf[n:]


def run(state):
    import hashlib
    import threading

    plan, conns = state["plan"], state["conns"]
    out = [None] * len(plan)
    per = len(plan) // len(conns)
    threads = [threading.Thread(target=_client, args=(s, plan[c * per:(c + 1) * per], out, c * per))
               for c, s in enumerate(conns)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    h = hashlib.sha256()
    statuses = {}
    for st, body in out:
        statuses[st] = statuses.get(st, 0) + 1
        h.update(body)
    return sorted(statuses.items()), h.hexdigest()


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
