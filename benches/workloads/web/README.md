# Workloads: `web` family (web apps & services)

Module format and rules: `~/rustique-research/PHASE1_BRIEF.md` (shared). Install with
`uv pip install -r requirements.txt` into a CPython 3.15.1 (or 3.15.1t) environment. Profiles
and analysis: [`docs/atlas/workloads-web.md`](../../../docs/atlas/workloads-web.md).

- `django_blog` — Django 6.1 content site via the test client: middleware, URL resolving, ORM over sqlite, cached templates, ModelForm POST, JsonResponse (196 requests).
- `flask_jinja` — Flask 3.1 storefront via the test client: Werkzeug routing/forms/sessions, Jinja2 inheritance/macros/filters, MarkupSafe escaping, jsonify (600 requests).
- `fastapi_pydantic` — FastAPI/Starlette CRUD JSON API with dependency injection and nested pydantic v2 models, driven by httpx's ASGI transport on one asyncio loop (800 requests).
- `sqlalchemy_orm` — SQLAlchemy 2.1 ORM data-access layer over sqlite: selects with joins/eager loading, aggregates, unit-of-work inserts/updates, rolled back each run (180 units).
- `requests_local` — `requests.Session` API client against a stdlib `ThreadingHTTPServer` in the same process: JSON paging, POSTs, CSV download (540 requests over 127.0.0.1).
- `asyncio_http` — pure-stdlib asyncio streams HTTP/1.1 JSON server plus 16 concurrent keep-alive clients (5,120 requests over 127.0.0.1).
- `aiohttp_app` — aiohttp web server with middleware and JSON handlers plus a pooled aiohttp `ClientSession` with 16 workers (1,600 requests over 127.0.0.1).
- `logging_service` — stdlib `logging` of a busy service: hierarchy, level filtering, adapter, filter, text/stream/JSON-lines handlers, exceptions (6,000 requests, ~21k records).
- `json_api` — stdlib JSON REST core: `json.loads`, hand validation, dataclasses, `datetime`/`uuid`/`Decimal`, `asdict` + `json.dumps(default=)` (9,600 calls).
- `email_mime` — transactional e-mail: templated `EmailMessage` with HTML alternative and attachments, `as_bytes`, and `BytesParser(policy.default)` parsing (240 messages).

`_profile.py` profiles one workload (perf matrix, cProfile, argument sizes); see its
docstring.
