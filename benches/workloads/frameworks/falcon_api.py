"""A Falcon REST API: a lean, performance-oriented WSGI framework with little magic.

Stands for the "minimal framework" end of Python web services: Falcon (used for
high-throughput APIs and as the HTTP layer of many internal services) does routing,
request/response objects, media (JSON) handling, hooks, middleware and HTTP errors, and
leaves the rest to the application -- so its profile shows what a request costs when
the framework is thin. An IoT device registry: compiled URI templates with ``uuid``/
``int`` converters, class-based resources with ``on_get``/``on_post``/``on_patch``/
``on_delete`` responders, ``@falcon.before`` validation hooks, auth and request-id
middleware (``process_request``/``process_resource``/``process_response``), query
parameters parsed with ``get_param_as_int/bool/list/datetime``, ETags with conditional
GETs (304), ``falcon.HTTPError`` subclasses and a custom error handler, CORS, JSON media
in and out. Driven the way a WSGI server drives it: per request a WSGI environ is built
(``falcon.testing.create_environ``) and the app is called directly. (Falcon's
``TestClient`` is not used: it wraps every call in ``wsgiref.validate`` and runs
``inspect.getfullargspec`` on the app, 39% of the samples in a first profile, which no
production server does.) 11,000 requests per run; the in-memory store is rebuilt from the seed at the
start of each run. Data: 1,000 devices, 20,000 readings. Falcon installs as pure Python
on 3.15 (no compiled wheel), which is how it runs there.
"""

REQUIRES = ["falcon==4.4.0"]


def setup():
    import datetime as dt
    import hashlib
    import json
    import random
    import uuid

    import falcon

    rng = random.Random(17)
    kinds = ["thermostat", "camera", "lock", "meter", "sensor", "gateway"]
    sites = [f"site-{i:02d}" for i in range(25)]
    seed_devices = []
    for i in range(1000):
        seed_devices.append((str(uuid.UUID(int=rng.getrandbits(128), version=4)), f"dev-{i:04d}",
                             kinds[i % 6], sites[rng.randrange(25)], rng.random() < 0.85,
                             f"{rng.randint(1, 4)}.{rng.randint(0, 9)}.{rng.randint(0, 20)}",
                             sorted(rng.sample(["indoor", "outdoor", "critical", "beta", "lab", "eu", "us"], rng.randint(0, 3)))))
    base = dt.datetime(2025, 3, 1, 0, 0, tzinfo=dt.timezone.utc)
    seed_readings = {}
    for d in seed_devices[:400]:
        seed_readings[d[0]] = [((base + dt.timedelta(minutes=15 * j)).isoformat(), round(rng.gauss(20, 5), 3))
                               for j in range(50)]
    tokens = {f"tok-{i:03d}": f"tenant-{i % 5}" for i in range(50)}

    class Store:
        def reset(self):
            self.devices = {d[0]: {"id": d[0], "name": d[1], "kind": d[2], "site": d[3], "active": d[4],
                                   "firmware": d[5], "labels": list(d[6]), "version": 1} for d in seed_devices}
            self.readings = {k: list(v) for k, v in seed_readings.items()}
            self.counter = 0

    store = Store()

    class AuthMiddleware:
        def process_request(self, req, resp):
            token = req.get_header("Authorization")
            if token is None or not token.startswith("Bearer "):
                raise falcon.HTTPUnauthorized(title="Auth required", description="Send a bearer token.",
                                              challenges=["Bearer"])
            tenant = tokens.get(token[7:])
            if tenant is None:
                raise falcon.HTTPForbidden(title="Bad token", description="The token is not valid.")
            req.context.tenant = tenant

    class RequestIDMiddleware:
        def process_resource(self, req, resp, resource, params):
            store.counter += 1
            req.context.request_id = f"req-{store.counter:06d}"

        def process_response(self, req, resp, resource, req_succeeded):
            resp.set_header("X-Request-ID", getattr(req.context, "request_id", "none"))
            resp.set_header("X-Tenant", getattr(req.context, "tenant", "-"))

    def etag_of(doc):
        return hashlib.md5(json.dumps(doc, sort_keys=True).encode()).hexdigest()

    def validate_device(req, resp, resource, params, partial=False):
        doc = req.get_media()
        if not isinstance(doc, dict):
            raise falcon.HTTPBadRequest(title="Invalid body", description="Expected a JSON object.")
        errors = {}
        if not partial or "name" in doc:
            name = doc.get("name")
            if not isinstance(name, str) or not 3 <= len(name) <= 40:
                errors["name"] = "3 to 40 characters"
        if not partial or "kind" in doc:
            if doc.get("kind") not in kinds:
                errors["kind"] = f"one of {', '.join(kinds)}"
        if "site" in doc and doc["site"] not in sites:
            errors["site"] = "unknown site"
        if "labels" in doc and (not isinstance(doc["labels"], list) or len(doc["labels"]) > 5):
            errors["labels"] = "a list of at most 5 labels"
        if errors:
            raise falcon.HTTPUnprocessableEntity(title="Validation failed", description=json.dumps(errors, sort_keys=True))

    def validate_patch(req, resp, resource, params):
        validate_device(req, resp, resource, params, partial=True)

    class DevicesResource:
        def on_get(self, req, resp):
            kind = req.get_param("kind")
            site = req.get_param("site")
            active = req.get_param_as_bool("active")
            labels = req.get_param_as_list("label") or []
            limit = req.get_param_as_int("limit", min_value=1, max_value=200, default=50)
            offset = req.get_param_as_int("offset", min_value=0, default=0)
            rows = [d for d in store.devices.values()
                    if (kind is None or d["kind"] == kind) and (site is None or d["site"] == site)
                    and (active is None or d["active"] == active) and all(lb in d["labels"] for lb in labels)]
            rows.sort(key=lambda d: d["name"])
            resp.media = {"total": len(rows), "offset": offset, "items": rows[offset:offset + limit]}

        @falcon.before(validate_device)
        def on_post(self, req, resp):
            doc = req.get_media()
            store.counter += 1
            did = str(uuid.UUID(int=store.counter * 7919 + 12345, version=4))
            dev = {"id": did, "name": doc["name"], "kind": doc["kind"], "site": doc.get("site", sites[0]),
                   "active": bool(doc.get("active", True)), "firmware": doc.get("firmware", "1.0.0"),
                   "labels": sorted(doc.get("labels", [])), "version": 1}
            store.devices[did] = dev
            resp.status = falcon.HTTP_201
            resp.location = f"/v1/devices/{did}"
            resp.media = dev

    class DeviceResource:
        def _get(self, device_id):
            dev = store.devices.get(str(device_id))
            if dev is None:
                raise falcon.HTTPNotFound(title="No such device", description=str(device_id))
            return dev

        def on_get(self, req, resp, device_id):
            dev = self._get(device_id)
            tag = etag_of(dev)
            if req.if_none_match and any(t == tag for t in req.if_none_match):
                resp.status = falcon.HTTP_304
                return
            resp.etag = tag
            resp.cache_control = ["private", "max-age=30"]
            resp.media = dev

        @falcon.before(validate_patch)
        def on_patch(self, req, resp, device_id):
            dev = self._get(device_id)
            for k, v in req.get_media().items():
                if k in ("name", "kind", "site", "active", "firmware", "labels"):
                    dev[k] = v
            dev["version"] += 1
            resp.media = dev

        def on_delete(self, req, resp, device_id):
            self._get(device_id)
            del store.devices[str(device_id)]
            store.readings.pop(str(device_id), None)
            resp.status = falcon.HTTP_204

    class ReadingsResource:
        def on_get(self, req, resp, device_id):
            if str(device_id) not in store.devices:
                raise falcon.HTTPNotFound()
            since = req.get_param_as_datetime("since", format_string="%Y-%m-%dT%H:%M:%SZ")
            rows = store.readings.get(str(device_id), [])
            if since is not None:
                cut = since.replace(tzinfo=dt.timezone.utc).isoformat()
                rows = [r for r in rows if r[0] >= cut]
            vals = [v for _, v in rows]
            resp.media = {"device": str(device_id), "count": len(rows),
                          "min": min(vals, default=None), "max": max(vals, default=None),
                          "mean": round(sum(vals) / len(vals), 4) if vals else None,
                          "readings": [{"at": a, "value": v} for a, v in rows[-20:]]}

        def on_post(self, req, resp, device_id):
            if str(device_id) not in store.devices:
                raise falcon.HTTPNotFound()
            batch = req.get_media()
            try:
                rows = [(dt.datetime.fromisoformat(r["at"]).isoformat(), float(r["value"])) for r in batch]
            except (KeyError, TypeError, ValueError) as e:
                raise ValueError(f"bad reading: {e}") from None
            store.readings.setdefault(str(device_id), []).extend(rows)
            resp.status = falcon.HTTP_202
            resp.media = {"accepted": len(rows)}

    class SiteStatsResource:
        def on_get(self, req, resp, site, page):
            devs = sorted((d for d in store.devices.values() if d["site"] == site), key=lambda d: d["name"])
            by_kind = {}
            for d in devs:
                by_kind[d["kind"]] = by_kind.get(d["kind"], 0) + 1
            resp.media = {"site": site, "page": page, "devices": len(devs), "by_kind": by_kind,
                          "inactive": [d["name"] for d in devs if not d["active"]][page * 10:(page + 1) * 10]}

    def on_value_error(req, resp, ex, params):
        raise falcon.HTTPUnprocessableEntity(title="Invalid data", description=str(ex))

    app = falcon.App(middleware=[AuthMiddleware(), RequestIDMiddleware()], cors_enable=True)
    app.add_route("/v1/devices", DevicesResource())
    app.add_route("/v1/devices/{device_id:uuid}", DeviceResource())
    app.add_route("/v1/devices/{device_id:uuid}/readings", ReadingsResource())
    app.add_route("/v1/sites/{site}/stats/{page:int(min=0)}", SiteStatsResource())
    app.add_error_handler(ValueError, on_value_error)

    ids = [d[0] for d in seed_devices]
    plan = []
    for i in range(10000):
        k = i % 20
        hdr = {"Authorization": f"Bearer tok-{i % 50:03d}"}
        did = ids[(i * 37) % 1000]
        if k in (0, 10):
            q = [f"kind={kinds[i % 6]}", f"site={sites[i % 25]}&active=true", "label=critical&label=eu",
                 f"limit=20&offset={i % 200}", "active=false&limit=100"][i % 5]
            plan.append(("GET", "/v1/devices", q, hdr, None))
        elif k in (1, 4, 7, 11, 14, 17):
            plan.append(("GET", f"/v1/devices/{did}", "", hdr, None))
        elif k == 2:
            plan.append(("GET", f"/v1/devices/{ids[i % 400]}/readings", "since=2025-03-01T06:00:00Z", hdr, None))
        elif k == 3:
            plan.append(("POST", "/v1/devices", "", hdr, {"name": f"new-{i}", "kind": kinds[i % 6],
                                                           "site": sites[i % 25], "labels": ["beta"]}))
        elif k == 5:
            plan.append(("PATCH", f"/v1/devices/{did}", "", hdr, {"firmware": f"5.{i % 10}.0", "active": i % 3 > 0}))
        elif k == 6:
            plan.append(("POST", f"/v1/devices/{ids[i % 400]}/readings", "", hdr,
                         [{"at": f"2025-04-0{1 + j % 9}T12:{j:02d}:00+00:00", "value": 20.5 + j} for j in range(10)]))
        elif k == 8:
            plan.append(("POST", "/v1/devices", "", hdr, {"name": "x", "kind": "toaster", "labels": "a"}))
        elif k == 9:
            plan.append(("GET", f"/v1/devices/{did}", "", {}, None))
            plan.append(("GET", f"/v1/devices/{did}", "", {"Authorization": "Bearer nope"}, None))
        elif k == 12:
            plan.append(("GET", f"/v1/sites/{sites[i % 25]}/stats/{i % 3}", "", hdr, None))
        elif k == 13:
            plan.append(("GET", "/v1/devices/00000000-0000-4000-8000-000000000000", "", hdr, None))
            plan.append(("GET", "/v1/devices/not-a-uuid", "", hdr, None))
        elif k == 15:
            plan.append(("POST", f"/v1/devices/{ids[i % 400]}/readings", "", hdr, [{"at": "yesterday", "value": 1}]))
        elif k == 16:
            plan.append(("DELETE", f"/v1/devices/{ids[999 - i // 20]}", "", hdr, None))
        elif k == 18:
            plan.append(("GET", f"/v1/devices/{did}", "", dict(hdr, **{"If-None-Match": "COND"}), None))
        else:
            plan.append(("OPTIONS", "/v1/devices", "", dict(hdr, Origin="https://ui.example.com",
                                                            **{"Access-Control-Request-Method": "POST"}), None))
    return {"app": app, "plan": plan, "store": store, "etag_of": etag_of}


def run(state):
    import hashlib
    import json

    from falcon.testing import create_environ

    app, store = state["app"], state["store"]
    store.reset()
    out = []
    resp = {}

    def start_response(status, headers, exc_info=None):
        resp["status"], resp["headers"] = status, headers

    for method, path, query, headers, body in state["plan"]:
        if headers.get("If-None-Match") == "COND":
            dev = store.devices.get(path.rsplit("/", 1)[1])
            headers = dict(headers, **{"If-None-Match": '"%s"' % state["etag_of"](dev)}) if dev else headers
        if body is not None:
            headers = dict(headers, **{"Content-Type": "application/json"})
        # what a WSGI server does per request: build the environ, call the app, drain the body
        env = create_environ(path=path, query_string=query, method=method, headers=headers,
                             body=json.dumps(body) if body is not None else "")
        chunks = app(env, start_response)
        data = b"".join(chunks)
        if hasattr(chunks, "close"):
            chunks.close()
        rid = next((v for k, v in resp["headers"] if k.lower() == "x-request-id"), None)
        out.append((method, path, int(resp["status"][:3]), rid, hashlib.sha256(data).hexdigest()))
    return out


def digest(result):
    import hashlib

    return hashlib.sha256(repr(result).encode()).hexdigest()
