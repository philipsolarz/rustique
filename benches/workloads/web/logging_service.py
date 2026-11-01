"""The logging layer of a busy web service: stdlib ``logging`` to a file, a stream and JSON.

Stands for what every Python service does on every request: emit log records through
``logging`` -- a logger hierarchy (``app.http``, ``app.db``, ``app.auth``, ``app.jobs``)
with level filtering (DEBUG calls that are dropped by ``isEnabledFor``), a
``LoggerAdapter`` adding request context, %-style lazy arguments, ``extra=`` fields, a
``Filter`` injecting a request id, three handlers (a ``FileHandler`` with a classic
``asctime/levelname/name`` text format, a ``StreamHandler`` to an in-memory stream with a
``{``-style format, and a ``FileHandler`` with a JSON-lines formatter as used for log
shipping), and ``logger.exception`` with formatted tracebacks for ~1% of requests.
Simulates 6,000 requests (~30,000 logger calls, ~21,000 emitted records) per run; output
goes to temp files truncated at the start of each run.
"""

REQUIRES: list[str] = []

N_REQUESTS = 6000


def setup():
    import io
    import json
    import logging
    import os
    import tempfile

    tmp = tempfile.TemporaryDirectory(prefix="rq-logging-")

    class JsonFormatter(logging.Formatter):
        """Typical structured-logging formatter (cf. python-json-logger)."""

        RESERVED = set(vars(logging.LogRecord("x", 0, "x", 0, "x", None, None))) | {"message", "asctime"}

        def format(self, record):
            payload = {
                "ts": self.formatTime(record, "%Y-%m-%dT%H:%M:%S"),
                "level": record.levelname,
                "logger": record.name,
                "msg": record.getMessage(),
                "module": record.module,
                "line": record.lineno,
            }
            for k, v in record.__dict__.items():
                if k not in self.RESERVED and not k.startswith("_"):
                    payload[k] = v
            if record.exc_info:
                payload["exc"] = self.formatException(record.exc_info)
            return json.dumps(payload, default=str)

    class RequestIdFilter(logging.Filter):
        def __init__(self):
            super().__init__()
            self.current = "-"

        def filter(self, record):
            record.request_id = self.current
            return True

    root = logging.getLogger("app")
    root.setLevel(logging.INFO)
    root.propagate = False
    for h in list(root.handlers):
        root.removeHandler(h)
    rid = RequestIdFilter()
    text_path = os.path.join(tmp.name, "app.log")
    json_path = os.path.join(tmp.name, "app.jsonl")
    fh = logging.FileHandler(text_path, encoding="utf-8")
    fh.setFormatter(logging.Formatter("%(asctime)s %(levelname)-8s [%(name)s] [%(request_id)s] %(message)s"))
    stream = io.StringIO()
    sh = logging.StreamHandler(stream)
    sh.setLevel(logging.WARNING)
    sh.setFormatter(logging.Formatter("{asctime} {levelname} {name}:{lineno} {message}", style="{"))
    jh = logging.FileHandler(json_path, encoding="utf-8")
    jh.setFormatter(JsonFormatter())
    for h in (fh, sh, jh):
        h.addFilter(rid)
        root.addHandler(h)
    loggers = {n: logging.getLogger(f"app.{n}") for n in ("http", "db", "auth", "jobs")}
    loggers["db"].setLevel(logging.WARNING)
    return {"tmp": tmp, "loggers": loggers, "rid": rid, "handlers": (fh, sh, jh), "stream": stream,
            "paths": (text_path, json_path)}


def _handle(i, log, db, auth, jobs):
    """One simulated request's worth of logging."""
    path = ("/api/orders", "/api/users/%d" % (i % 500), "/health", "/api/search", "/login")[i % 5]
    log.debug("parsing headers for %s", path)
    log.info("request started method=%s path=%s", "GET" if i % 3 else "POST", path)
    db.debug("SELECT * FROM orders WHERE user_id = %s LIMIT %s", i % 500, 20)
    db.info("query took %.2f ms", (i * 7) % 300 / 10)
    if i % 5 == 4:
        auth.info("login attempt", extra={"user": "user%d" % (i % 97), "ip": "10.0.%d.%d" % (i % 256, i % 13)})
        if i % 35 == 4:
            auth.warning("failed login for %r from %s", "user%d" % (i % 97), "10.0.0.%d" % (i % 250))
    if i % 50 == 7:
        db.warning("slow query (%d ms): %s", 1200 + i % 300, "SELECT ... FROM order_line JOIN product ...")
    if i % 97 == 13:
        try:
            {"a": 1}["missing-%d" % i]
        except KeyError:
            log.exception("unhandled error while processing %s", path)
    if i % 10 == 0:
        jobs.info("enqueued job %s", {"job": "email", "to": "user%d@example.com" % (i % 500), "attempt": 1})
    log.info("request finished status=%d bytes=%d duration=%.3fs", 200 if i % 97 != 13 else 500,
             (i * 131) % 20000, ((i * 17) % 900) / 1000)


def run(state):
    import logging

    fh, sh, jh = state["handlers"]
    for h in (fh, jh):
        h.acquire()
        try:
            h.stream.seek(0)
            h.stream.truncate()
        finally:
            h.release()
    state["stream"].seek(0)
    state["stream"].truncate()
    lg = state["loggers"]
    rid = state["rid"]
    http = logging.LoggerAdapter(lg["http"], {"service": "shop"})
    for i in range(N_REQUESTS):
        rid.current = "req-%06d" % i
        _handle(i, http, lg["db"], lg["auth"], lg["jobs"])
    fh.flush()
    jh.flush()
    parts = []
    for p in state["paths"]:
        with open(p, encoding="utf-8") as f:
            parts.append(f.read())
    parts.append(state["stream"].getvalue())
    return parts


def digest(result):
    """Hash of the log output with timestamps and source paths normalized."""
    import hashlib
    import re

    ts = re.compile(r"\d{4}-\d\d-\d\d[ T]\d\d:\d\d:\d\d(,\d+)?")
    paths = re.compile(r'(File \\?")[^"\\]*/')
    norm = [paths.sub(r"\1", ts.sub("TS", t)) for t in result]
    return hashlib.sha256(repr([(len(t.splitlines()), hashlib.sha256(t.encode()).hexdigest())
                                for t in norm]).encode()).hexdigest()
