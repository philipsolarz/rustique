# Workloads: `frameworks` family (framework apps, wave 2)

Framework applications beyond wave 1's `web` family (`django_blog`, `flask_jinja`,
`fastapi_pydantic`, `sqlalchemy_orm`, `aiohttp_app`): the API and back-office shapes people
build most on each major framework. Module format and rules:
`~/rustique-research/w2/PHASE1_BRIEF.md`. Install with
`uv pip install -r requirements.txt` into CPython 3.15.1 or 3.15.1t (all six run on both with
identical digests). Profiles and analysis:
[`docs/atlas/w2-wl-frameworks.md`](../../../docs/atlas/w2-wl-frameworks.md).

- `drf_api` — Django REST framework API: router + viewsets, nested `ModelSerializer`s, `HyperlinkedIdentityField`, token auth, object permissions, django-filter + search + ordering, pagination, ORM over sqlite (400 requests).
- `django_admin` — the Django admin of a shop: changelists with filters/search/date hierarchy/sorting, change forms with fieldsets and a `TabularInline`, saves with inline formsets, a bulk action (108 requests, template-heavy).
- `flask_marshmallow` — Flask + Flask-SQLAlchemy + marshmallow REST API: blueprint, bearer auth hook, `db.paginate`, nested schemas, validators, error handlers, aggregates (630 requests).
- `fastapi_sqlalchemy` — FastAPI on SQLAlchemy's async ORM (greenlet + aiosqlite): per-request `AsyncSession` dependency, auth and pagination dependencies, pydantic `from_attributes` models, middleware, 404/409/422 paths (450 requests).
- `strawberry_graphql` — Strawberry/graphql-core GraphQL API: parse + validate + execute per operation, nested resolvers through `DataLoader`s, fragments, unions, interfaces, mutations (330 operations).
- `falcon_api` — Falcon WSGI API (pure Python on 3.15), called directly as a WSGI server would: URI templates with converters, resources, hooks, middleware, ETag/304, CORS preflight, HTTP errors (11,000 requests).

Each run is rolled back or restored from a snapshot (sqlite backup API, in-memory store
rebuild) so it repeats exactly in one process.

`_layers.py` profiles one workload of the `frameworks`, `libs` or `services` family in the
six layers of the wave-2 brief, with third-party attribution and the CPython entry points
each distribution calls (plain perf + re-weighted `-X perf` + cProfile per code object +
`perf stat` plain/enabled); `_rollup.py` turns the profiles into the atlas tables. See their
docstrings.
