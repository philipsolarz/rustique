STATUS: wave-2 Phase 1 result (slice `w2-wl-frameworks`), 2026-11-02. Evidence for the
lead's wave-2 scoring in docs/IMPACT_ATLAS.md. Every number is **measured** (M: how and
where is stated) or **inferred** (I: from what). Raw profiles: `~/rustique-research/w2/wl-frameworks/`.

# Atlas: framework apps, third-party-heavy code and long-running services

Question: *where does the time go in the programs wave 1 did not move* — framework apps,
code dominated by third-party libraries, long-running services — *and which CPython-provided
entry points do those libraries spend it in?* Answered with 16 new workloads in three
families (`benches/workloads/{frameworks,libs,services}/`), profiled in the six layers of
the wave-2 brief, with every sample also attributed to the third-party distribution (or the
program) that called into CPython.

## Key findings

1. **The program's own code is 0–4% of framework apps and library-heavy code; third-party
   bytecode is the largest layer.** Mean over the 16 workloads (M): own 2.3%, generated
   2.4%, language 21.0%, stdlib 23.1%, thirdparty 35.5%, runtime 15.7%. Framework apps:
   thirdparty 35.6%, language 24.6%, stdlib 19.2%, runtime 16.4%. Library workloads:
   thirdparty 43.0%, language 23.1%, runtime 16.2%, stdlib 11.6%. Services: stdlib 46.2%,
   thirdparty 23.9%, runtime 14.0%, language 12.5%.
2. **What third-party code makes CPython do is mostly not an entry point.** Across all 16
   (M): allocation/deallocation 12.6% (7–18% in every workload), generic attribute access
   from unspecialized sites and `getattr`/`hasattr` 5.9% (16% in `drf_api`, 11% in
   `flask_marshmallow`), dict operations 3.4%, cyclic GC 2.9% (15% in `bs4_scrape`, 12% in
   `strawberry_graphql`). The attribute-access time is spread over hundreds of sites (the
   largest single Django site is 0.5%): interpreter and runtime work, not kernels.
3. **The stdlib entry points libraries do use are few and depend on program type** (M):
   json (3.3% mean, 19% of `falcon_api`), asyncio (4.0%, 39.5% of `cache_service`, 14% of
   `fastapi_sqlalchemy`), logging (2.4%, 27% of `job_worker`), sockets/select (C, mostly
   kernel), sqlite3 (C engine, 1.3%/8.6%), `html.parser` (13% of `bs4_scrape`),
   `_elementtree` (6.5% of `boto3_client`), copy (≤ 2.9%), urllib.parse (≤ 3.1%).
   Pure-Python stdlib — what Python-function kernels could own — is 12.3% of framework
   apps, 7.2% of library workloads and 35.6% of services (M): even if all of it became
   free, framework apps would gain at most 1.14× and library workloads 1.08× (I, Amdahl).
4. **Rustique today**: indicative instruction ratios (M) 0.65–0.67 (`job_worker`: logging,
   statistics, json), 0.81 (`falcon_api`: json, datetime), 0.87 (`attrs_cattrs`: json,
   datetime, sort), 0.97–1.00 on the other 13; geometric mean 0.946 (instructions) / 0.932
   (wall, loaded box); never slower in instructions; digests always
   identical (no Rustique bug found). Coverage gaps seen: `json.dumps(default=)` (every call
   of `job_worker`), `logging.Logger.level` (`deopt` on every call in all three logging
   services), `sorted(key=)` (14/16 workloads, tiny share), `deepcopy` with hooks (Django/
   DRF, ruling 15).
5. **Generated code matters only when a library generates it**: attrs + cattrs 21% of
   `attrs_cattrs`, attrs-generated `__init__` 6.4% of `jsonschema_validate`;
   CPython-generated dataclass/namedtuple/enum code ≤ 2% everywhere (M).
6. **Services in steady state** (60 iterations, M): no leaks (RSS +0–3.6 MB per 100
   iterations), Rustique adds a constant 4.1–5.0 MB RSS; GC 0–3% of their time (0 to 105
   young collections per iteration); 17–54% of their CPU is kernel time. On 3.15.1t the
   threaded WSGI server is 3.6–3.9× faster in wall time, the threaded worker uses ~1.7× the
   CPU, single-threaded async services are 1.0–1.5× slower, and plain framework/library
   workloads are 1.05–1.53× slower (`strawberry_graphql` worst).
7. **Surprises** (M): Falcon's `TestClient` (`wsgiref.validate` + `inspect.getfullargspec` per
   request) was 39% of a first profile; botocore parses every response timestamp with
   `dateutil` (28.5% of `boto3_client`); FastAPI runs `def` dependencies through anyio's
   thread pool (~10% of `uvicorn_fastapi` in `queue`/`threading`); bs4 calls `typing.cast`
   186 k times per run; pydantic-core spends 6.4% in libc string copies; attrs `order=True`
   comparisons rebuild tuples in a Python closure (`attrs_to_tuple`, 18% of `attrs_cattrs`).
8. **Recommendation** (§9): score interpreter (attribute sites) and runtime (allocation,
   GC policy) work against these 16 programs — they are the only levers for framework
   apps; for services, close the json `default=` and `Logger.level` gaps first; settle
   `copy` hooks and `html.parser` under ruling 15.

## 1. The workloads

| family | workload | stands for | why representative | reach: PyPI downloads last month (M) | run() s 3.15.1 | 3.15.1t |
|---|---|---|---|---|---:|---:|
| frameworks | `drf_api` | Django REST framework JSON API: router + viewsets, nested `ModelSerializer`s, hyperlinks, token auth, object permissions, django-filter + search + ordering, pagination, ORM over sqlite; 400 requests | the default Django API stack | djangorestframework 24.1 M, Django 41.1 M, django-filter 13.8 M | 1.40 | 1.56 (1.11×) |
| frameworks | `django_admin` | the Django admin of a shop: changelists (filters, search, date hierarchy, sorting, pagination), change forms with fieldsets and a `TabularInline`, inline-formset saves, a bulk action; 108 requests | every Django project's back office; Django's most template-heavy code | Django 41.1 M | 1.24 | 1.46 (1.18×) |
| frameworks | `flask_marshmallow` | Flask + Flask-SQLAlchemy + marshmallow REST API: blueprint, bearer auth hook, `db.paginate`, nested schemas, validators, error handlers, aggregates; 630 requests | the classic Flask service stack (tutorials, flask-smorest, webargs) | Flask 136.5 M, SQLAlchemy 329.8 M, marshmallow 84.1 M, Flask-SQLAlchemy 12.6 M | 0.89 | 0.95 (1.06×) |
| frameworks | `fastapi_sqlalchemy` | FastAPI on SQLAlchemy 2.1's async ORM (greenlet + aiosqlite): per-request `AsyncSession` dependency, auth/pagination dependencies, pydantic `from_attributes`, middleware, 404/409/422; 450 requests | the production FastAPI shape (wave 1's `fastapi_pydantic` had no database) | fastapi 381.6 M, SQLAlchemy 329.8 M, greenlet 361.1 M, aiosqlite 61.2 M | 1.40 | 1.58 (1.13×) |
| frameworks | `strawberry_graphql` | GraphQL API: parse + validate + execute per operation, nested resolvers via `DataLoader`s, fragments, unions, interfaces, mutations; 330 operations | GraphQL servers; graphql-core is the engine under Strawberry, Graphene and Ariadne | graphql-core 60.1 M (strawberry-graphql 5.8 M, graphene 27.0 M) | 1.22 | 1.87 (1.53×) |
| frameworks | `falcon_api` | Falcon WSGI API (pure Python on 3.15) called as a WSGI server calls it: URI templates with converters, resources, hooks, middleware, ETag/304, CORS, HTTP errors; 11,000 requests | the thin-framework end: what a request costs when the framework does little | falcon 1.4 M | 0.47 | 0.53 (1.14×) |
| libs | `boto3_client` | boto3/botocore S3 + DynamoDB (resource layer) + SQS calls through the full client path, canned `before-send` responses; 1,140 calls | the most-downloaded package on PyPI | boto3 2,446 M, botocore 844 M | 1.76 | 2.04 (1.16×) |
| libs | `pydantic_payloads` | pydantic v2 discriminated-union webhook events: `validate_json`/`validate_python`, Python validators, aliases, `model_copy`, `dump_json`, error reports; 38,400 events | direct pydantic use (services, SDKs, LLM tooling) | pydantic 842 M | 0.84 | 1.03 (1.23×) |
| libs | `jsonschema_validate` | jsonschema Draft 2020-12 with `$ref`, `oneOf`, `if/then`, formats; `is_valid`/`validate` and `iter_errors` + `best_match`; 2,500 documents | config/API validation (Jupyter, OpenAPI tooling, linters) | jsonschema 471 M | 1.60 | 1.68 (1.05×) |
| libs | `yaml_config` | PyYAML pure `SafeLoader`/`SafeDumper`: Kubernetes manifests, compose files with anchors/merge keys, app configs; 0.44 MB | config loading everywhere (`yaml.safe_load` is the pure-Python loader by default) | PyYAML 911 M | 1.22 | 1.44 (1.18×) |
| libs | `attrs_cattrs` | attrs value objects/aggregates (exec-generated methods) structured/unstructured by cattrs-generated functions, tagged unions, `evolve`; 2,000 documents | code generated by libraries on the program's behalf | attrs 693 M, cattrs 33.9 M | 0.87 | 0.95 (1.10×) |
| libs | `bs4_scrape` | BeautifulSoup on the stdlib `html.parser` + soupsieve CSS selectors: parse, query, edit, re-serialize 24 product pages (1.8 MB) | scraping / HTML processing | beautifulsoup4 290 M | 1.52 | 1.67 (1.10×) |
| services | `uvicorn_fastapi` | `uvicorn[standard]` (uvloop + httptools, access log on) serving FastAPI; 16 thin keep-alive clients over 127.0.0.1; server persists across runs; 3,200 requests | how FastAPI is deployed | uvicorn 499 M, uvloop 217 M, httptools 215 M | 1.66 | 1.84 (1.11×) |
| services | `waitress_flask` | waitress (pure-Python threaded WSGI server, 4 workers) serving Flask + Jinja2 pages and JSON; 8 client threads on keep-alive sockets; 2,400 requests | threaded WSGI deployment | waitress 6.5 M, Flask 136.5 M | 1.00 | 0.28 (0.28×) |
| services | `job_worker` | 4 persistent worker threads: signed JSON jobs, registry dispatch, tenacity retries, cachetools LRU/TTL caches, JSON log lines; 6,000 jobs | background job workers (Celery/RQ/Dramatiq shape) | tenacity 337 M, cachetools 233 M | 1.20 | 0.73 (0.61×) |
| services | `cache_service` | asyncio RESP cache server (cachetools LRU + logical-clock TTL) + 16 redis-py asyncio clients storing pickled objects; ~24,800 commands | the cache tier (Django/Flask cache backends, session stores) | redis 200 M, cachetools 233 M | 1.35 | 1.33 (0.99×) |

*Reach*: pypistats.org `recent` API, last month, fetched 2026-11-02 from the dev box (M).
*run() s*: median of 10 steady-state runs (2 fresh processes × 5) per build, plain CPython,
interleaved 3.15.1 / 3.15.1t, on the loaded shared dev box (M; ±15%).

All 16: deterministic digests, identical across runs in one process, across fresh processes,
across `PYTHONHASHSEED` values (spot-checked on `attrs_cattrs`), between 3.15.1 and 3.15.1t,
and between plain CPython and `rustique.enable()` (`run.py compare`, M, dev box). No digest
mismatch was found, so no Rustique bug to report. Each family's `requirements.txt` installs
with `uv pip install --no-deps -r` + `uv pip check` clean on 3.15.1 and 3.15.1t (M). Not
binary on 3.15: PyYAML 6.0.3 (no cp315/cp315t wheel; builds from the sdist as pure Python,
which is what 3.15 users get) and falcon 4.4.0 (pure-Python wheel, no Cython speedups);
redis runs without the optional hiredis parser (its default install).

Workload hygiene that changed results (M): Falcon's `TestClient` wraps every call in
`wsgiref.validate` and runs `inspect.getfullargspec` on the app; that was **39% of the
samples** of a first `falcon_api` profile, so the workload now calls the WSGI app directly
as a server does (run time 0.65 s -> 0.27 s for the same 5,500 requests). The Django/DRF,
Flask/Werkzeug and httpx-ASGI test clients have no such wrapper (checked in the profiles:
no `wsgiref`, `inspect` < 1%). In the services, clients are deliberately thin (raw sockets,
pre-encoded requests) so the time is the server's; `waitress_flask`'s 8 client threads
still show as 14% `own` (socket reads and response framing).

## 2. Method

Tool: `benches/workloads/frameworks/_layers.py` (this slice; it reuses wave 1's driver and
symbolizer from `benches/workloads/data/_profiler.py` and follows w2-wl-custom's
`_profile_custom.py` method so layer numbers are comparable across slices), rolled up by
`benches/workloads/frameworks/_rollup.py`. Per workload, on the dev box (c7a.8xlarge,
CPython 3.15.1 python-build-standalone PGO+LTO+BOLT), each step in a fresh process, after
`setup()` and one warm-up `run()`, sampling only steady-state `run()` calls (perf control
FIFO), all threads:

| step | what | used for |
|---|---|---|
| plain perf | `perf record -e cpu-clock -F 1999 -g`, frame pointers, ≥ 6 s, 10–16 k samples | the leaf native-component split (undistorted) |
| `-X perf` | the same with the perf trampoline, 10–15 k samples | *who*: innermost Python frame's owner, the calling distribution, the entered CPython function; samples re-weighted per leaf component so the component marginal equals the plain profile's (`-X perf` disables inlined Python-to-Python calls and inflates call/interpreter shares, w2-interp) |
| cProfile | one `run()`, read per code object (`Profile.getstats()`, not pstats: pstats merges exec-generated methods), with caller→callee edges | call counts of builtins and stdlib functions per calling distribution (main thread only) |
| `perf stat` | `instructions:u` over the gated loop, plain and after `rustique.enable()` | the noise-robust indicative Rustique ratio |

Layers (brief vocabulary): **runtime** = allocation/deallocation leaf, a cyclic-GC frame on
the stack, import, GIL/lock/QSBR machinery (any owner); **generated** = under generated code
objects (dataclass `__create_fn__`, namedtuple `<string>`, `enum.py`, `total_ordering`,
attrs/cattrs `<... generated ...>`, Jinja2 `<template>`); **stdlib** = under stdlib Python
frames, plus C stdlib modules (`_json`, `_sre`, `_pickle`, `_socket`, `select`, sqlite3,
`_elementtree`, `_decimal`, `_datetime`, `_functools`, …) entered from own or third-party
bytecode, including what they call; **thirdparty** = interpreter time under third-party
frames plus third-party native extensions (pydantic-core, uvloop, rpds-py, greenlet,
SQLAlchemy's Cython, MarkupSafe); **language** = native object-protocol/builtin components
entered from own or third-party bytecode (attribute lookup, calls, dict/str/list ops,
`isinstance`, `getattr`/`hasattr`, comparisons, exceptions); **own** = interpreter time in the
workload file (the app). Kernel samples (cpu-clock counts them: `perf_event_paranoid=-1`)
go to the layer of the C entry that made the syscall (socket/select/os → stdlib, uvloop →
thirdparty) and are also shown separately. Every sample additionally carries its *caller*
(the innermost own or third-party frame) and *entry* (the stdlib function that caller
entered, or the C entry point its bytecode reached) — §4 is built from that.

Caveats. The dev box was shared and loaded (load average 45–62 on 32 cores during these
runs): wall times are indicative (±15%); shares and instruction counts are not affected
much. Instruction counts of the threaded services (`waitress_flask`, `job_worker`) depend on
thread timing (poll loops, GIL hand-offs) and are less reliable than the others. cProfile
sees only the main thread (the services' worker threads are not in its call counts).

## 3. Where the time goes

### 3.1 Layers per workload (M: share of steady-state CPU samples, kernel included; plain-perf components, `-X perf` owners)

| workload | kernel (sys) | own | generated | language | stdlib | thirdparty | runtime (alloc / gc) | top distributions (all layers under their frames) | Rustique instr ratio |
|---|---:|---:|---:|---:|---:|---:|---|---|---:|
| `drf_api` | 0% | 0.1% | 0.1% | 29.9% | 17.7% | 35.0% | 17.2% (11% / 6%) | Django 75%, djangorestframework 19%, django-filter 4% | 0.983 |
| `django_admin` | 0% | 0.1% | 0.1% | 27.3% | 15.5% | 42.3% | 14.7% (12% / 2%) | Django 98%, asgiref 1% | 0.983 |
| `flask_marshmallow` | 0% | 0.6% | 0.7% | 26.2% | 16.7% | 44.4% | 11.3% (11% / 1%) | SQLAlchemy 67%, Werkzeug 14%, marshmallow 8% | 0.995 |
| `fastapi_sqlalchemy` | 23% | 0.3% | 0.9% | 19.6% | 29.4% | 37.6% | 12.2% (11% / 1%) | SQLAlchemy 46%, aiosqlite 13%, fastapi 7% | 0.994 |
| `strawberry_graphql` | 0% | 1.6% | 3.9% | 23.0% | 7.1% | 37.7% | 26.7% (15% / 12%) | graphql-core 75%, strawberry-graphql 16% | 0.998 |
| `falcon_api` | 0% | 15.8% | 0.6% | 21.6% | 28.7% | 16.7% | 16.5% (14% / 2%) | falcon 54% | 0.813 |
| `boto3_client` | 0% | 0.2% | 0.4% | 26.8% | 18.5% | 40.1% | 13.9% (14% / 0%) | botocore 64%, python-dateutil 29%, boto3 5% | 0.979 |
| `pydantic_payloads` | 0% | 3.0% | 1.0% | 34.7% | 7.7% | 37.5% | 16.2% (16% / 0%) | pydantic 79% | 0.989 |
| `jsonschema_validate` | 0% | 0.0% | 6.4% | 21.5% | 4.9% | 49.7% | 17.4% (17% / 0%) | jsonschema 71%, referencing 20%, attrs 2% | 1.001 |
| `yaml_config` | 0% | 0.0% | 0.0% | 22.6% | 1.4% | 68.9% | 7.0% (7% / 0%) | PyYAML 100% | 1.0 |
| `attrs_cattrs` | 0% | 4.4% | 21.3% | 17.1% | 19.1% | 19.4% | 18.8% (18% / 1%) | attrs 31%, cattrs 27% | 0.87 |
| `bs4_scrape` | 0% | 0.1% | 0.0% | 16.2% | 17.8% | 42.2% | 23.6% (8% / 15%) | beautifulsoup4 72%, soupsieve 28% | 0.97 |
| `uvicorn_fastapi` | 28% | 1.0% | 0.4% | 15.2% | 24.6% | 43.9% | 14.8% (12% / 3%) | anyio 29%, starlette 18%, uvicorn 17% | 0.966 |
| `waitress_flask` | 54% | 1.2% | 1.7% | 13.2% | 50.1% | 22.8% | 11.0% (10% / 1%) | waitress 45%, Werkzeug 13%, Flask 10% | 1.091 |
| `job_worker` | 25% | 4.2% | 0.1% | 8.8% | 63.7% | 6.8% | 16.4% (13% / 3%) | tenacity 12%, cachetools 6% | 0.668 |
| `cache_service` | 17% | 4.5% | 0.0% | 12.8% | 46.6% | 22.3% | 13.8% (13% / 0%) | redis 63%, cachetools 2% | 1.001 |

*kernel (sys)*: system / (user + system) time of the gated `run()` loop (`getrusage`). *Rustique
instr ratio*: `instructions:u` per `run()` after `rustique.enable()` / plain (perf stat, M; §6).

### 3.2 Family means (M)

| family | n | own | generated | language | stdlib | thirdparty | runtime |
|---|---:|---:|---:|---:|---:|---:|---:|
| frameworks | 6 | 3.1% | 1.0% | 24.6% | 19.2% | 35.6% | 16.4% |
| libs | 6 | 1.3% | 4.9% | 23.1% | 11.6% | 43.0% | 16.2% |
| services | 4 | 2.7% | 0.6% | 12.5% | 46.2% | 23.9% | 14.0% |
| all | 16 | 2.3% | 2.4% | 21.0% | 23.1% | 35.5% | 15.7% |

For comparison, wave 1's six framework workloads (Django, Flask, FastAPI, SQLAlchemy, requests,
aiohttp) had 26.5% reachable and a 1.36× cap (docs/atlas/workloads-web.md). The new
families confirm that picture and say where the rest goes:

- **own ≈ 0–4%** in framework apps and library-heavy code: the application's own bytecode
  (views, serializers, resolvers, validators) is a rounding error. The exception is
  `falcon_api` (15.8%), whose resources filter 1,000 in-memory devices in list
  comprehensions: once the framework is thin, the app's own loops show.
- **thirdparty 35–69%** (interpreter time in the libraries' own bytecode plus their native
  extensions): the largest layer everywhere except the stdlib-heavy services. Native
  extensions are a minority of it: pydantic-core 34% of `pydantic_payloads`, uvloop 12% of
  `uvicorn_fastapi`, rpds-py 7%, SQLAlchemy's Cython 3–5%, greenlet 2.5%.
- **language 13–35%**: the native protocol work third-party bytecode triggers (§3.3).
- **runtime 7–27%**: allocation/deallocation 7–18% everywhere; cyclic GC up to 15% (§3.4).
- **stdlib 1–64%**: small in library-heavy code (PyYAML 1.4%, jsonschema 4.9%, pydantic
  7.7%), 15–30% in framework apps, 46–64% in the services (asyncio, logging, json, sockets).
- **generated 0–21%**: only where a *library* generates code (attrs + cattrs 21%, attrs
  `__init__` in referencing 6.4% of `jsonschema_validate`); CPython-generated code
  (dataclass, namedtuple, enum) is ≤ 2% in all 16.

### 3.3 The language layer: what third-party bytecode makes CPython do (M)

Leaf components of language-layer samples (mean / max over the 16):

| item | mean | max (workload) |
|---|---:|---|
| `attr/type` | 7.22% | 14.2% (drf_api) |
| `dict` | 5.39% | 11.7% (falcon_api) |
| `str` | 2.00% | 7.1% (pydantic_payloads) |
| `calls` | 1.61% | 2.4% (flask_marshmallow) |
| `libc` | 1.51% | 7.4% (pydantic_payloads) |
| `list/tuple/set` | 1.44% | 2.9% (attrs_cattrs) |
| `abstract` | 0.51% | 0.9% (cache_service) |
| `generators` | 0.27% | 1.0% (jsonschema_validate) |
| `int` | 0.25% | 0.9% (cache_service) |
| `builtins-module` | 0.16% | 0.5% (django_admin) |
| `bytes` | 0.15% | 1.3% (cache_service) |
| `exceptions` | 0.15% | 0.4% (boto3_client) |
| `other` | 0.14% | 0.4% (cache_service) |
| `kernel` | 0.09% | 0.8% (fastapi_sqlalchemy) |
| `iter/range/slice` | 0.08% | 0.4% (strawberry_graphql) |
| `float` | 0.03% | 0.2% (job_worker) |
| `other-objects` | 0.03% | 0.1% (flask_marshmallow) |

Grouped by the C entry point reached from bytecode (M, per workload; mean / max over 16):
**attribute access** (`PyObject_GenericGetAttr` from unspecialized `LOAD_ATTR`,
`_PyObject_GetMethodStackRef`, `getattr`/`hasattr`/`setattr` builtins, `__getattr__` slot hooks,
`super()` lookups) **5.9% / 15.9%** (drf_api 15.9, flask_marshmallow 11.0, strawberry 9.8,
django_admin 9.6, yaml_config 8.2, boto3 8.2); **dict operations** 3.4% / 10.8% (falcon_api's
own loops); `isinstance` 0.8% / 1.8%; `sorted`/`list.sort` 0.2% / 1.2%. The attribute-access
time is spread thin: in `drf_api` and `django_admin` the largest single (C entry, Python
function) site is 0.5% (`LazyObject.__getattribute__`, `Field.__init__`, `Model.__init__`,
`Variable._resolve_lookup`, M: `language_sites` in the profiles). It is the interpreter's
fallback path for attribute sites the specializer does not cover (megamorphic receivers,
descriptors, `__getattr__`), not an entry point a kernel can own: evidence for w2-interp and
w2-machinery, not a target here. cProfile shows the call volumes behind it (M, one run):
botocore makes 2.14 M `dict.get`, 391 k `getattr` and 288 k `hasattr` calls per run;
Django (admin) 709 k `isinstance`, 352 k `hasattr`, 273 k `getattr`; bs4 1.08 M `isinstance`
and 186 k `typing.cast`.

### 3.4 Runtime: allocation and GC (M)

| item | mean | max (workload) |
|---|---:|---|
| `alloc/dealloc` | 12.57% | 18.0% (attrs_cattrs) |
| `gc` | 2.88% | 15.3% (bs4_scrape) |
| `threads/locks` | 0.20% | 0.7% (job_worker) |
| `import` | 0.07% | 0.3% (cache_service) |

Allocation/deallocation is the single largest CPython-provided cost under third-party code
(12.6% mean, 7–18% in every workload), with no concentration in any caller. Cyclic GC is
bimodal: 15.3% in `bs4_scrape` (tree building: linked `Tag`/`NavigableString` containers, I)
and 11.9% in `strawberry_graphql` (graphql-core's AST, visitor and result objects, I), 5.8% in `drf_api`, ≤ 3% elsewhere and 0 in `cache_service` (its
allocations are freed by refcounting before a young collection triggers). Both belong to
w2-runtime; the per-service GC counts are in §5.

### 3.5 Kernel time (M)

| item | mean | max (workload) |
|---|---:|---|
| `stdlib` | 4.58% | 29.7% (waitress_flask) |
| `thirdparty` | 0.55% | 7.4% (uvicorn_fastapi) |
| `runtime` | 0.11% | 1.0% (waitress_flask) |
| `language` | 0.09% | 0.8% (fastapi_sqlalchemy) |
| `generated` | 0.00% | 0.0% (strawberry_graphql) |

The services spend 17–54% of their CPU time in the kernel by `getrusage` (§3.1) and 10–31% of
the perf samples (cache_service 17.5%, waitress_flask 31%, uvicorn_fastapi 14%, job_worker
12.5%, fastapi_sqlalchemy 9.6%): socket `send`/`recv`, `select`/`epoll`, futex for thread
hand-offs (the two measurements come from different processes and count idle polling
differently). Not reachable by any
kernel on the Python side; it bounds what any service-side acceleration can achieve.

### 3.6 Stdlib modules under the programs (M: Python frames + C modules entered directly)

| item | mean | max (workload) |
|---|---:|---|
| `asyncio` | 3.96% | 39.5% (cache_service) |
| `json` | 3.26% | 19.2% (falcon_api) |
| `logging` | 2.44% | 26.9% (job_worker) |
| `C:socket` | 1.48% | 23.7% (waitress_flask) |
| `C:sqlite3` | 1.29% | 8.6% (drf_api) |
| `threading` | 0.87% | 6.2% (uvicorn_fastapi) |
| `html` | 0.87% | 12.9% (bs4_scrape) |
| `urllib.parse` | 0.55% | 3.1% (django_admin) |
| `C:re-engine` | 0.54% | 1.8% (boto3_client) |
| `C:functools` | 0.51% | 3.8% (fastapi_sqlalchemy) |
| `C:datetime` | 0.49% | 2.1% (pydantic_payloads) |
| `statistics` | 0.47% | 7.5% (job_worker) |
| `copy` | 0.45% | 2.9% (drf_api) |
| `C:hashlib` | 0.45% | 2.8% (falcon_api) |
| `C:elementtree` | 0.41% | 6.5% (boto3_client) |
| `selectors` | 0.35% | 3.1% (fastapi_sqlalchemy) |
| `C:decimal` | 0.34% | 2.2% (pydantic_payloads) |
| `C:select` | 0.33% | 5.3% (waitress_flask) |
| `C:pickle` | 0.27% | 4.3% (cache_service) |
| `abc` | 0.26% | 0.9% (jsonschema_validate) |
| `fractions` | 0.26% | 4.2% (job_worker) |
| `inspect` | 0.22% | 1.0% (drf_api) |
| `hmac` | 0.21% | 2.4% (job_worker) |
| `uuid` | 0.21% | 2.8% (falcon_api) |
| `contextlib` | 0.20% | 1.1% (uvicorn_fastapi) |

### 3.7 Generated code (M)

| item | mean | max (workload) |
|---|---:|---|
| `gen:attrs` | 1.04% | 10.3% (attrs_cattrs) |
| `gen:cattrs` | 0.65% | 10.4% (attrs_cattrs) |
| `gen:namedtuple` | 0.21% | 2.1% (strawberry_graphql) |
| `gen:enum` | 0.15% | 0.6% (attrs_cattrs) |
| `gen:dataclass` | 0.12% | 1.4% (strawberry_graphql) |
| `gen:jinja2` | 0.11% | 1.7% (waitress_flask) |
| `gen:exec` | 0.08% | 0.6% (falcon_api) |

## 4. Third-party distributions and the CPython entry points they spend time in

### 4.1 Distributions: share under their frames and the layers it falls into (M)

Share = all samples whose innermost own-or-third-party frame belongs to the distribution
(its bytecode plus everything CPython does on its behalf); *mean* is over all 16 workloads,
*max* names the workload. Columns split that time by layer.

| distribution | workloads | share mean / max (all layers under its frames) | own bytecode | language | stdlib | generated | runtime | native ext |
|---|---|---|---:|---:|---:|---:|---:|---:|
| Django | 2 | 10.8% / 98% (django_admin) | 39% | 27% | 17% | 0% | 16% | 0% |
| SQLAlchemy | 2 | 7.0% / 67% (flask_marshmallow) | 39% | 29% | 10% | 1% | 12% | 9% |
| PyYAML | 1 | 6.2% / 100% (yaml_config) | 69% | 23% | 1% | 0% | 7% | 0% |
| pydantic | 3 | 5.2% / 79% (pydantic_payloads) | 4% | 35% | 4% | 1% | 12% | 44% |
| graphql-core | 1 | 4.7% / 75% (strawberry_graphql) | 42% | 25% | 3% | 4% | 26% | 0% |
| beautifulsoup4 | 1 | 4.5% / 72% (bs4_scrape) | 32% | 16% | 22% | 0% | 31% | 0% |
| jsonschema | 1 | 4.5% / 71% (jsonschema_validate) | 48% | 23% | 6% | 2% | 18% | 2% |
| botocore | 1 | 4.0% / 64% (boto3_client) | 34% | 27% | 24% | 1% | 14% | 0% |
| redis | 1 | 4.0% / 63% (cache_service) | 34% | 14% | 37% | 0% | 15% | 0% |
| falcon | 1 | 3.4% / 54% (falcon_api) | 30% | 14% | 35% | 1% | 18% | 1% |
| waitress | 1 | 2.8% / 45% (waitress_flask) | 23% | 10% | 56% | 0% | 11% | 0% |
| anyio | 2 | 2.1% / 29% (uvicorn_fastapi) | 34% | 13% | 36% | 1% | 15% | 1% |
| attrs | 2 | 2.1% / 31% (attrs_cattrs) | 51% | 21% | 3% | 7% | 18% | 0% |
| python-dateutil | 1 | 1.8% / 29% (boto3_client) | 57% | 27% | 4% | 0% | 12% | 0% |
| soupsieve | 1 | 1.7% / 28% (bs4_scrape) | 70% | 18% | 7% | 0% | 5% | 0% |
| Werkzeug | 2 | 1.7% / 14% (flask_marshmallow) | 46% | 22% | 20% | 0% | 12% | 0% |
| cattrs | 1 | 1.7% / 27% (attrs_cattrs) | 14% | 9% | 6% | 59% | 12% | 0% |
| fastapi | 2 | 1.4% / 15% (uvicorn_fastapi) | 51% | 18% | 15% | 0% | 15% | 0% |
| starlette | 2 | 1.4% / 18% (uvicorn_fastapi) | 48% | 19% | 12% | 0% | 22% | 0% |
| referencing | 1 | 1.3% / 20% (jsonschema_validate) | 36% | 22% | 4% | 22% | 15% | 1% |
| djangorestframework | 1 | 1.2% / 19% (drf_api) | 36% | 40% | 10% | 0% | 13% | 0% |
| uvicorn | 1 | 1.1% / 17% (uvicorn_fastapi) | 16% | 9% | 34% | 0% | 12% | 30% |

### 4.2 Per distribution: its top CPython-provided entry points (M, share of the workload)

`py:` = the stdlib function the distribution called (the time inside it, inclusive);
`C:` = the C entry point its bytecode reached (inclusive of what that calls); `ext:` = native
extension. Rows with ≥ 4% of a workload under the distribution.

| distribution (workload) | under its frames | top entry points (layer: entry share) |
|---|---:|---|
| PyYAML (yaml_config) | 100% | runtime: `alloc/dealloc` 6.9%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 6.3%; language: `C:? [libc]` 2.4%; language: `C:object_recursive_isinstance [abstract]` 1.8%; language: `C:PyObject_GenericGetAttr [attr/type]` 1.7%; language: `C:do_richcompare [attr/type]` 1.4% |
| Django (django_admin) | 98% | runtime: `alloc/dealloc` 12.3%; language: `C:PyObject_GenericGetAttr [attr/type]` 4.6%; stdlib: `C:_pysqlite_query_execute [sqlite3]` 3.5%; runtime: `gc` 2.0%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 1.7%; language: `C:builtin_getattr [builtins-module]` 1.4% |
| pydantic (pydantic_payloads) | 79% | thirdparty: `ext:pydantic_core` 34.2%; runtime: `alloc/dealloc` 9.5%; language: `C:? [libc]` 6.4%; language: `C:PyDict_SetItem [dict]` 3.3%; language: `C:PyDict_GetItemRef [dict]` 2.4%; language: `C:PyUnicode_FromStringAndSize [str]` 2.2% |
| Django (drf_api) | 75% | runtime: `alloc/dealloc` 8.1%; stdlib: `C:_pysqlite_query_execute [sqlite3]` 8.0%; language: `C:PyObject_GenericGetAttr [attr/type]` 5.8%; runtime: `gc` 5.1%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 1.7%; language: `C:PyObject_GenericSetAttr [attr/type]` 1.1% |
| graphql-core (strawberry_graphql) | 75% | runtime: `alloc/dealloc` 10.7%; runtime: `gc` 8.7%; language: `C:builtin_getattr [builtins-module]` 4.9%; language: `C:PyObject_GenericGetAttr [attr/type]` 2.2%; generated: `gen:namedtuple:<lambda>` 1.5%; language: `C:object_recursive_isinstance [abstract]` 1.3% |
| beautifulsoup4 (bs4_scrape) | 72% | runtime: `gc` 15.3%; stdlib: `py:html/parser.py:HTMLParser.feed` 14.0%; runtime: `alloc/dealloc` 6.4%; language: `C:list_contains [list/tuple/set]` 1.7%; language: `C:_Py_slot_tp_getattr_hook [attr/type]` 1.2%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 1.1% |
| jsonschema (jsonschema_validate) | 71% | runtime: `alloc/dealloc` 12.6%; language: `C:PyDict_Contains [dict]` 2.1%; language: `C:dictiter_iternextitem [dict]` 1.8%; thirdparty: `ext:rpds-py` 1.7%; language: `C:builtin_getattr [builtins-module]` 1.6%; stdlib: `py:re/__init__.py:search` 1.5% |
| SQLAlchemy (flask_marshmallow) | 67% | stdlib: `C:_pysqlite_query_execute [sqlite3]` 6.4%; runtime: `alloc/dealloc` 6.1%; thirdparty: `ext:SQLAlchemy` 5.1%; language: `C:PyObject_GenericGetAttr [attr/type]` 3.4%; stdlib: `C:pysqlite_cursor_fetchall [sqlite3]` 1.8%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 1.6% |
| botocore (boto3_client) | 64% | runtime: `alloc/dealloc` 8.8%; stdlib: `C:_elementtree_XMLParser_feed [elementtree]` 6.1%; language: `C:PyObject_GenericGetAttr [attr/type]` 3.2%; language: `C:builtin_getattr [builtins-module]` 1.8%; language: `C:dict_get [dict]` 1.5%; stdlib: `C:_sre_SRE_Pattern_sub [re-engine]` 1.4% |
| redis (cache_service) | 63% | stdlib: `py:asyncio/streams.py:StreamWriter.writelines` 11.2%; runtime: `alloc/dealloc` 9.5%; stdlib: `py:asyncio/tasks.py:wait_for` 3.3%; stdlib: `py:asyncio/streams.py:StreamReader.readline` 2.5%; stdlib: `py:asyncio/timeouts.py:Timeout.__aenter__` 2.2%; stdlib: `py:asyncio/timeouts.py:Timeout.__aexit__` 1.0% |
| falcon (falcon_api) | 54% | stdlib: `py:json/__init__.py:dumps` 13.6%; runtime: `alloc/dealloc` 8.2%; runtime: `gc` 1.7%; stdlib: `py:uuid.py:UUID.__init__` 1.5%; stdlib: `py:json/__init__.py:loads` 1.4%; stdlib: `py:_strptime.py:_strptime_datetime_datetime` 1.2% |
| SQLAlchemy (fastapi_sqlalchemy) | 46% | runtime: `alloc/dealloc` 5.4%; thirdparty: `ext:SQLAlchemy` 3.0%; language: `C:PyObject_GenericGetAttr [attr/type]` 2.5%; thirdparty: `ext:greenlet` 2.5%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 1.2%; language: `C:_Py_slot_tp_getattr_hook [attr/type]` 1.0% |
| waitress (waitress_flask) | 45% | stdlib: `C:_socket_socket_send [socket]` 9.3%; stdlib: `C:select_select_impl [select]` 5.3%; runtime: `alloc/dealloc` 4.4%; stdlib: `py:threading.py:Condition.wait` 2.6%; stdlib: `py:logging/__init__.py:Logger.warning` 2.4%; stdlib: `C:os_write [os]` 1.7% |
| attrs (attrs_cattrs) | 31% | runtime: `alloc/dealloc` 5.2%; language: `C:builtin_getattr [builtins-module]` 3.1%; generated: `gen:attrs:__init__` 1.6%; stdlib: `C:_sre_SRE_Pattern_fullmatch [re-engine]` 0.8%; language: `C:do_richcompare [attr/type]` 0.5%; language: `C:_PyDict_LoadBuiltinsFromGlobals [dict]` 0.4% |
| anyio (uvicorn_fastapi) | 29% | stdlib: `py:queue.py:Queue.get` 4.6%; runtime: `alloc/dealloc` 3.1%; stdlib: `py:queue.py:Queue.put_nowait` 1.7%; runtime: `gc` 1.0%; stdlib: `py:reprlib.py:recursive_repr.<locals>.decorating_function.<locals>.wrapper` 0.7%; stdlib: `py:weakref.py:WeakKeyDictionary.get` 0.6% |
| python-dateutil (boto3_client) | 29% | runtime: `alloc/dealloc` 3.4%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 0.8%; language: `C:PyFloat_FromString [float]` 0.6%; language: `C:? [libc]` 0.5%; language: `C:_PyDict_Subscript [dict]` 0.4%; language: `C:builtin_getattr [builtins-module]` 0.4% |
| soupsieve (bs4_scrape) | 28% | runtime: `alloc/dealloc` 1.5%; stdlib: `C:bounded_lru_cache_wrapper [functools]` 1.5%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 1.1%; language: `C:object_recursive_isinstance [abstract]` 0.7%; language: `C:_PyFunction_Vectorcall [calls]` 0.5%; language: `C:slot_tp_iter [attr/type]` 0.3% |
| cattrs (attrs_cattrs) | 27% | generated: `gen:cattrs:structure_StockItem` 8.5%; runtime: `alloc/dealloc` 2.7%; generated: `gen:cattrs:unstructure_StockItem` 1.9%; generated: `gen:cattrs:structure_Receipt` 1.3%; stdlib: `C:datetime_datetime_isoformat [datetime]` 1.2%; generated: `gen:cattrs:structure_Transfer` 0.9% |
| referencing (jsonschema_validate) | 20% | generated: `gen:attrs:__init__` 4.5%; runtime: `alloc/dealloc` 3.0%; language: `C:slot_tp_init [attr/type]` 1.1%; language: `C:object_recursive_isinstance [abstract]` 0.5%; language: `C:_PyDict_FromItems [dict]` 0.5%; stdlib: `py:<frozen abc>:ABCMeta.__instancecheck__` 0.5% |
| djangorestframework (drf_api) | 19% | language: `C:PyObject_GenericGetAttr [attr/type]` 2.2%; runtime: `alloc/dealloc` 2.1%; language: `C:PyObject_GenericSetAttr [attr/type]` 0.9%; language: `C:builtin_getattr [builtins-module]` 0.7%; language: `C:_PyObject_GetMethodStackRef [attr/type]` 0.5%; stdlib: `py:json/__init__.py:dumps` 0.4% |
| starlette (uvicorn_fastapi) | 18% | runtime: `alloc/dealloc` 3.4%; runtime: `gc` 0.7%; stdlib: `py:json/__init__.py:loads` 0.3%; stdlib: `py:urllib/parse.py:parse_qsl` 0.2%; stdlib: `C:_sre_SRE_Pattern_prefixmatch [re-engine]` 0.2%; stdlib: `py:contextlib.py:_AsyncGeneratorContextManager.__aenter__` 0.2% |
| uvicorn (uvicorn_fastapi) | 17% | thirdparty: `ext:uvloop` 4.3%; stdlib: `py:logging/__init__.py:Logger.info` 3.7%; runtime: `alloc/dealloc` 1.9%; stdlib: `py:copy.py:copy` 1.2%; thirdparty: `ext:httptools` 0.8%; stdlib: `py:logging/__init__.py:Formatter.formatMessage` 0.2% |
| strawberry-graphql (strawberry_graphql) | 16% | runtime: `gc` 2.9%; runtime: `alloc/dealloc` 2.3%; language: `C:PyObject_GenericGetAttr [attr/type]` 0.7%; generated: `gen:namedtuple:<lambda>` 0.6%; stdlib: `py:asyncio/base_events.py:BaseEventLoop.call_soon` 0.4%; stdlib: `py:<frozen abc>:ABCMeta.__instancecheck__` 0.2% |
| fastapi (uvicorn_fastapi) | 15% | runtime: `alloc/dealloc` 1.7%; runtime: `gc` 0.9%; stdlib: `py:contextlib.py:AsyncExitStack.__aexit__` 0.4%; language: `C:object_recursive_isinstance [abstract]` 0.3%; language: `C:_PyFunction_Vectorcall [calls]` 0.3%; stdlib: `py:typing.py:get_origin` 0.3% |

### 4.3 The CPython entry points third-party and own code spend time in (M, all 16)

| layer | entry point | mean | max (workload) | main callers |
|---|---|---:|---|---|
| runtime | `alloc/dealloc` | 12.44% | 12.6% (jsonschema_validate) | own, Django, jsonschema |
| runtime | `gc` | 2.84% | 15.3% (bs4_scrape) | beautifulsoup4, graphql-core, Django |
| stdlib | `py:json/__init__.py:dumps` | 2.45% | 13.6% (falcon_api) | own, falcon, Flask |
| language | `C:PyObject_GenericGetAttr [attr/type]` | 2.13% | 5.8% (drf_api) | Django, SQLAlchemy, botocore |
| stdlib | `py:asyncio/base_events.py:BaseEventLoop.run_until_complete` | 1.65% | 12.4% (fastapi_sqlalchemy) | own |
| stdlib | `py:logging/__init__.py:LoggerAdapter.info` | 1.60% | 25.6% (job_worker) | own |
| language | `C:_PyObject_GetMethodStackRef [attr/type]` | 1.29% | 6.3% (yaml_config) | PyYAML, Django, SQLAlchemy |
| stdlib | `C:_pysqlite_query_execute [sqlite3]` | 1.12% | 8.0% (drf_api) | Django, SQLAlchemy |
| language | `C:builtin_getattr [builtins-module]` | 1.08% | 4.9% (strawberry_graphql) | graphql-core, attrs, Django |
| language | `C:? [libc]` | 0.95% | 6.4% (pydantic_payloads) | pydantic, PyYAML, own |
| language | `C:_PyFunction_Vectorcall [calls]` | 0.91% | 1.2% (django_admin) | Django, SQLAlchemy, graphql-core |
| stdlib | `py:html/parser.py:HTMLParser.feed` | 0.87% | 14.0% (bs4_scrape) | beautifulsoup4 |
| language | `C:object_recursive_isinstance [abstract]` | 0.80% | 1.8% (yaml_config) | pydantic, PyYAML, Django |
| stdlib | `py:json/__init__.py:loads` | 0.80% | 5.7% (attrs_cattrs) | own, falcon, starlette |
| language | `C:_PyDict_Subscript [dict]` | 0.78% | 4.0% (falcon_api) | own, SQLAlchemy, graphql-core |
| stdlib | `py:asyncio/streams.py:StreamWriter.writelines` | 0.70% | 11.2% (cache_service) | redis |
| stdlib | `py:statistics.py:pstdev` | 0.68% | 10.9% (job_worker) | own |
| stdlib | `C:sock_recv [socket]` | 0.67% | 9.0% (waitress_flask) | own, waitress |
| generated | `gen:attrs:__init__` | 0.65% | 4.5% (jsonschema_validate) | referencing, own, attrs |
| stdlib | `py:logging/__init__.py:Logger.info` | 0.62% | 6.2% (waitress_flask) | own, uvicorn |
| stdlib | `C:_socket_socket_send [socket]` | 0.58% | 9.3% (waitress_flask) | waitress |
| language | `C:slot_tp_init [attr/type]` | 0.58% | 1.1% (jsonschema_validate) | referencing, PyYAML, Django |
| generated | `gen:cattrs:structure_StockItem` | 0.53% | 8.5% (attrs_cattrs) | cattrs |
| language | `C:dict_get [dict]` | 0.52% | 1.5% (boto3_client) | own, botocore, SQLAlchemy |
| language | `C:PyObject_GenericSetAttr [attr/type]` | 0.51% | 1.5% (pydantic_payloads) | SQLAlchemy, Django, pydantic |
| language | `C:PyDict_GetItemRef [dict]` | 0.48% | 2.4% (pydantic_payloads) | pydantic, own, Django |
| language | `C:_Py_slot_tp_getattr_hook [attr/type]` | 0.47% | 2.1% (uvicorn_fastapi) | own, SQLAlchemy, beautifulsoup4 |
| stdlib | `py:asyncio/selector_events.py:_SelectorSocketTransport.write` | 0.45% | 7.2% (cache_service) | own |
| stdlib | `C:_elementtree_XMLParser_feed [elementtree]` | 0.38% | 6.1% (boto3_client) | botocore |
| stdlib | `py:queue.py:Queue.get` | 0.37% | 4.6% (uvicorn_fastapi) | anyio, own |
| language | `C:_PyDict_SetItem_Take2 [dict]` | 0.36% | 1.1% (falcon_api) | falcon, Django, SQLAlchemy |
| stdlib | `C:select_select_impl [select]` | 0.33% | 5.3% (waitress_flask) | waitress |

`C:? [libc]` is unresolved libc (memcpy/memmove/strlen variants that glibc does not export);
in `pydantic_payloads` it is pydantic-core copying strings (6.4%). `py:...run_until_complete`
and `py:...LoggerAdapter.info` collapse everything below an API the program entered
(the event loop, the logging call) into that API; §3.6 has the module-level split.

What the libraries use most, by kind (M):

| kind | entry points (largest users) | share |
|---|---|---|
| allocation/GC | every distribution | 15.5% mean (alloc 12.6, GC 2.9) |
| generic attribute access | Django, DRF, SQLAlchemy, botocore, graphql-core, PyYAML, attrs | 5.9% mean, 15.9% max |
| dict operations | botocore, pydantic, jsonschema, falcon apps | 3.4% mean, 10.8% max |
| json (`dumps`/`loads`, `iterencode`, `raw_decode`) | falcon, own handlers, Flask `jsonify`, DRF renderer, starlette | 3.3% mean, 19.2% max (falcon_api) |
| asyncio loop/transports/streams | redis-py, aiosqlite, SQLAlchemy async, httpx | 4.0% mean, 39.5% max (cache_service) |
| logging | uvicorn access log, waitress, workers | 2.4% mean, 26.9% max (job_worker) |
| sockets/select (C, mostly kernel) | waitress, asyncio | 1.8% mean, 29% max |
| sqlite3 (C module + engine) | Django ORM, SQLAlchemy | 1.3% mean, 8.6% max |
| html.parser | beautifulsoup4 | 0.9% mean, 12.9% max |
| xml.etree C parser | botocore | 0.4% mean, 6.5% max |
| copy (`deepcopy`/`copy`) | Django forms/fields, DRF fields, pydantic `model_copy`, botocore, uvicorn | 0.45% mean, 2.9% max |
| urllib.parse | Django (`reverse`, `urlencode`, admin querystrings), Werkzeug, botocore | 0.55% mean, 3.1% max |
| re engine (C) | botocore, Django, jsonschema `pattern` | 0.54% mean, 1.8% max |
| datetime/decimal C | pydantic, DRF, cattrs, botocore | 0.8% mean, 4.4% max |

## 5. Long-running services: steady state, memory, GC, free-threading

Each service ran 60 iterations of `run()` in one process (`services/_steady.py`), plain,
after `rustique.enable()`, and plain on 3.15.1t (M, dev box; wall medians of the second half,
loaded box ±20%).

| workload | build | steady wall s | steady CPU s | warm s | RSS after setup / it 1 / end (MB) | growth MB/100 it | peak RSS MB | GC collections per iteration (gen0/1/2) | digest constant |
|---|---|---:|---:|---:|---|---:|---:|---|---|
| `cache_service` | enabled | 1.024 | 1.024 | 1.018 | 52.5 / 58.4 / 64.5 | 3.54 | 64.5 | 0.0 / 0.0 / 0.0 | True |
| `cache_service` | plain | 1.011 | 1.011 | 1.088 | 47.7 / 53.5 / 59.8 | 0.39 | 59.8 | 0.0 / 0.0 / 0.0 | True |
| `cache_service` | t-plain | 1.093 | 1.093 | 1.120 | 55.3 / 60.4 / 61.3 | -0.05 | 61.3 | 0.0 / 0.0 / 0.0 | True |
| `job_worker` | enabled | 0.599 | 0.606 | 0.528 | 38.3 / 45.9 / 47.9 | 0.95 | 47.9 | 52.87 / 4.8 / 0.43 | True |
| `job_worker` | plain | 0.783 | 0.824 | 0.902 | 34.2 / 40.6 / 42.8 | 3.63 | 42.8 | 52.83 / 4.83 / 0.43 | True |
| `job_worker` | t-plain | 0.740 | 1.429 | 0.919 | 39.5 / 51.4 / 56.6 | 1.93 | 56.9 | 10.4 / 0.0 / 0.0 | True |
| `uvicorn_fastapi` | enabled | 1.915 | 1.602 | 1.559 | 70.8 / 75.1 / 77.1 | -0.25 | 77.1 | 39.3 / 3.57 / 0.33 | True |
| `uvicorn_fastapi` | plain | 1.283 | 1.315 | 0.962 | 66.2 / 70.7 / 72.7 | 1.06 | 72.7 | 73.3 / 6.67 / 0.6 | True |
| `uvicorn_fastapi` | t-plain | 1.975 | 2.145 | 2.077 | 76.5 / 93.7 / 98.8 | 2.89 | 98.8 | 1.2 / 0.0 / 0.0 | True |
| `waitress_flask` | enabled | 1.138 | 1.479 | 1.406 | 48.9 / 54.6 / 56.6 | 1.07 | 56.6 | 14.07 / 1.27 / 0.07 | True |
| `waitress_flask` | plain | 1.043 | 1.196 | 1.238 | 43.9 / 49.9 / 51.7 | 0.59 | 51.7 | 14.03 / 1.27 / 0.07 | True |
| `waitress_flask` | t-plain | 0.264 | 1.054 | 0.200 | 52.6 / 68.7 / 75.1 | -10.92 | 75.3 | 1.7 / 0.0 / 0.0 | True |

Findings (M unless tagged):

- **No leaks, little heap growth.** RSS grows 0–3.6 MB per 100 iterations (≤ 1 KB per
  request), digests stay constant over 60 iterations in all 12 runs. Peak RSS (`VmHWM`) is
  43–73 MB on 3.15.1.
- **Rustique costs a constant ~4.1–5.0 MB of RSS** (setup RSS enabled − plain: 4.8 cache,
  4.1 job, 4.6 uvicorn, 5.0 waitress) and does not change heap growth: evidence for
  w2-startup's install/extension-size item.
- **GC activity differs widely between services**: 0 collections per iteration in
  `cache_service` (refcounting frees everything), 14 young collections per iteration in
  `waitress_flask`, 53 in `job_worker`, 40–73 in `uvicorn_fastapi` (the count varies between
  processes with request interleaving). Old-generation collections stay ≤ 0.6 per
  iteration. GC is 0–3% of their samples (§3.4): a GC policy tier would barely move the
  services, but would move `bs4_scrape` (15%) and `strawberry_graphql` (12%).
- **Free-threaded build** (plain 3.15.1t vs 3.15.1): the threaded WSGI server scales —
  `waitress_flask` 0.26 s vs 1.04 s wall (3.9× faster) for about the same CPU time (1.05 vs
  1.20 s); `job_worker` runs in the same or less wall time (0.74 vs 0.78 s here; 0.73 vs 1.20 s in the
  paired timing of §1) but burns more CPU (1.43 vs 0.82 s, 1.73×: 4 workers contend on the queue, logging handler lock and caches);
  the single-threaded async services pay the t-build penalty: `uvicorn_fastapi` 1.5× slower
  (1.98 vs 1.28 s; uvloop + asyncio), `cache_service` 1.08×. Peak RSS is 2–26 MB higher on
  3.15.1t. Young collections drop to 0–10 per iteration (the free-threaded GC uses a
  different trigger). Hand-off to w2-threads.
- uvicorn's enabled-vs-plain steady wall in this table (1.92 vs 1.28 s) is noise from the
  loaded box: two interleaved re-runs (30 iterations each) gave enabled 1.58 / 1.07 s vs plain 1.67 / 1.37 s, and
  its instruction count is 0.966× plain (§6). Young-collection counts varied 42–105 per
  iteration between these processes.

## 6. Rustique today on these workloads

### 6.1 Indicative ratios (enabled / plain)

| workload | instructions (perf stat, M) | wall, `run.py compare -n 5 -p 3` (M, loaded box) |
|---|---:|---:|
| `drf_api` | 0.983 | 0.795 |
| `django_admin` | 0.983 | 1.029 |
| `flask_marshmallow` | 0.995 | 0.918 |
| `fastapi_sqlalchemy` | 0.994 | 1.020 |
| `strawberry_graphql` | 0.998 | 1.003 |
| `falcon_api` | 0.813 | 0.861 |
| `boto3_client` | 0.979 | 0.965 |
| `pydantic_payloads` | 0.989 | 1.009 |
| `jsonschema_validate` | 1.001 | 1.008 |
| `yaml_config` | 1.000 | 1.007 |
| `attrs_cattrs` | 0.870 | 0.854 |
| `bs4_scrape` | 0.970 | 0.988 |
| `uvicorn_fastapi` | 0.966 | 0.936 |
| `waitress_flask` | 1.091 (1.001 in a second run) | 0.990 |
| `job_worker` | 0.668 (0.649 in a second run) | 0.620 |
| `cache_service` | 1.001 | 1.018 |
| **geometric mean (16)** | **0.946** (waitress 1.001) | **0.932** (digests identical in all 96 processes) |

Instruction counts are the robust number here (single-threaded workloads repeat to
±0.3%; the loaded box moved cycles and wall by up to ±30% between runs with identical
instruction counts, e.g. `strawberry_graphql` 0.998× instructions but 1.30× cycles in one
pair). For the two threaded services instruction counts depend on thread timing
(`waitress_flask` measured 1.091× and 1.001× in two runs; `job_worker` 0.668× and 0.649×).
Wall ratios need the quiet bench box (`run.py compare frameworks libs services`).

Reading: Rustique already pays off where the program spends time in the stdlib it
accelerates — `job_worker` 0.65–0.67× instructions (logging `LogRecord`/`findCaller`/
`formatTime`, `statistics.pstdev`, `json.loads`), `falcon_api` 0.81× (`json.dumps`/`loads`,
`datetime.isoformat`/`strptime`), `attrs_cattrs` 0.87× (`json`,
`datetime.isoformat`, `list.sort`, `max`) — and is neutral (0.97–1.00×) on the 13 others,
where the time is in third-party bytecode, attribute access, allocation and already-C
modules. It is never measurably slower in instructions (max 1.001× outside the threaded
noise).

### 6.2 What fires and what falls back (M: `RUSTIQUE_STATS=1`, one steady-state run)

| workload | hits per run (top targets) | fallbacks per run (reason) |
|---|---|---|
| `drf_api` | copy.deepcopy 19,004, tuple.__contains__ 17,896, list.__contains__ 6,160, urlparse.quote 2,914, list.sort 2,214, datetime.isoformat 1,847 | copy.deepcopy 5,960 (deopt), tuple.__contains__ 5,320 (args), sorted 2,254 (args), list.__contains__ 640 (args), list.sort 300 (args) |
| `django_admin` | list.__contains__ 19,727, urlparse.quote_plus 8,836, urlparse.quote 7,460, tuple.__contains__ 7,314, urlparse._urlsplit 5,606, list.sort 3,362 | sorted 4,635 (args), tuple.__contains__ 4,056 (args), list.__contains__ 3,046 (deopt), tuple.__contains__ 3,012 (deopt), copy.deepcopy 2,136 (deopt) |
| `flask_marshmallow` | tuple.__contains__ 7,280, urlparse.quote 6,480, datetime.date.isoformat 3,927, list.sort 2,200, urlparse.unquote 1,260, datetime.isoformat 1,249 | tuple.__contains__ 15,480 (args), list.__contains__ 11,618 (args), sorted 4,455 (args), urlparse.unquote 4,100 (args), list.sort 2,255 (args) |
| `fastapi_sqlalchemy` | max 22,774, log.Logger.level 16,334, tuple.__contains__ 10,706, list.sort 3,471, urlparse.unquote 1,800, copy.deepcopy 530 | tuple.__contains__ 23,601 (args), list.__contains__ 8,402 (args), sorted 5,398 (args), list.sort 1,927 (args), tuple.__contains__ 450 (deopt) |
| `strawberry_graphql` | tuple.__contains__ 7,997, datetime.isoformat 5,734, list.__contains__ 4,829, max 2,706, float.repr 1,410 | tuple.__contains__ 933 (args), sum 330 (args) |
| `falcon_api` | tuple.__contains__ 40,221, json.dumps 8,023, list.__contains__ 6,021, list.count 3,750, datetime.isoformat 2,750, json.loads 1,250 | max 1,757 (args), list.sort 750 (args), sorted 500 (args) |
| `boto3_client` | tuple.__contains__ 618,000, log.Logger.level 39,840, list.__contains__ 30,220, copy.deepcopy 23,760, list.sort 3,520, urlparse.quote 3,460 | sum 20,520 (args), copy.deepcopy 12,580 (args), sorted 3,720 (args), datetime.isoformat 320 (args), list.sort 200 (args) |
| `pydantic_payloads` | copy.copy 28,800, copy.deepcopy 7,573 | sum 7,147 (args), list.sort 1 (args), sorted 1 (args) |
| `jsonschema_validate` | urlparse.unquote 46,222, list.sort 6,200, list.count 3,000, float.repr 100 | sorted 9,949 (args), list.sort 3,799 (args), max 500 (args) |
| `yaml_config` | list.sort 22,280, float.repr 320, datetime.isoformat 160, tuple.__contains__ 121, datetime.date.isoformat 80, max 60 | sorted 26,161 (args), list.sort 3,881 (args), sum 40 (args) |
| `attrs_cattrs` | tuple.__contains__ 64,417, list.sort 57,923, datetime.isoformat 29,831, datetime.date.isoformat 21,769, max 14,393, json.loads 2,000 | sorted 59,923 (args), tuple.__contains__ 28,092 (args), list.sort 4,000 (args), sum 2,000 (args), min 1 (deopt) |
| `bs4_scrape` | tuple.__contains__ 366,320, list.__contains__ 40,460, list.sort 5,554, min 2,304 | tuple.__contains__ 12,756 (args), sorted 8,478 (args), list.sort 2,924 (args) |
| `uvicorn_fastapi` | tuple.__contains__ 30,400, copy.copy 6,400, urlparse.quote 3,200, urlparse.parse_qsl 3,200, log.Logger.findCaller 3,200, log.LogRecord.__init__ 3,200 | log.Logger.level 3,200 (deopt), tuple.__contains__ 2,560 (args), list.sort 1 (args), sorted 1 (args) |
| `waitress_flask` | float.repr 11,400, tuple.__contains__ 4,950, log.Logger.findCaller 3,492, log.LogRecord.__init__ 3,492, log.Formatter.format 3,492, max 900 | list.sort 4,501 (args), sorted 4,501 (args), log.Logger.level 3,492 (deopt), sum 300 (args) |
| `job_worker` | tuple.__contains__ 43,400, json.dumps 13,464, log.Logger.findCaller 12,264, log.Formatter.formatTime 12,264, log.LogRecord.__init__ 12,172, json.loads 12,000 | log.Logger.level 12,264 (deopt), json.dumps 12,000 (args), sorted 2,402 (args), sum 1,200 (args), log.LogRecord.__init__ 92 (args) |
| `cache_service` | tuple.__contains__ 39,134, max 2,897, sum 1,920 | sum 25,662 (args), list.__contains__ 18,942 (args), tuple.__contains__ 9,856 (args), list.index 9,471 (args) |

Fallback patterns worth closing (counts per run; evidence for w2-floors):
`sorted(key=...)` falls back in 14 of 16 workloads (`args`: every call has `key=`; up to
60 k/run in `attrs_cattrs`, but sorting is ≤ 1.2% of any profile); `tuple.__contains__` with
class needles (`args`, up to 28 k/run); `copy.deepcopy` with `__deepcopy__` hooks (`deopt`,
Django/DRF form and serializer fields, 2–6 k/run); `json.dumps(default=...)` (`args`, every
`json.dumps` of `job_worker`, 12 k/run); and `logging.Logger.level` falls back with `deopt`
on **every** call in all three services that log (12 k/run in `job_worker`) — a coverage
gap in an existing target, not a new one.

## 7. Candidates

Format of the brief; scores `I × F × (1 − K) × (1 − U) × (0.5 + R)` with my estimates (I =
mean share × (1 − 1/s), in %; shares are means over these 16 workloads). Prototype: none
(this slice builds workloads and attribution; Phase 2 builds what is chosen).

| candidate | layer | entry points + mechanism | share mean / max (which) | s | exact subset & what falls back | floor vs current | risks | upstream overlap | reach | prototype | evidence |
|---|---|---|---|---:|---|---|---|---|---|---|---|
| **json `default=` / `cls=` in the json kernel** (wave-1 open item) | stdlib | `json.dumps` vectorcall (existing kernel); `default` is called from the kernel as the C encoder calls it | json 3.3% / 19.2% (falcon_api); the falling-back part ≈ 0.8% mean, 11.6% (job_worker: every `dumps` has `default=str`) | 3 | `default` a plain callable, `cls` None or `JSONEncoder` without overridden methods; other `cls` fall back | kernel 0.31–0.60× of `_json` (wave-1 proto) vs 1.0 now on these calls | `default` may raise/recurse (exceptions must match); ruling 15 is satisfied (the C encoder calls `default` with no Python frame in between) | none known | wave 1: 20% of `json.dumps` calls pass keyword arguments (M); `default=` is common in service code (I) | – | §6.2 fallbacks; job_worker profile |
| **asyncio selector-transport and stream fast paths** | stdlib | `_SelectorSocketTransport.write/writelines/_write_sendmsg` (buffer-empty or single-flush path), `StreamWriter.write/writelines`, `StreamReader.readuntil/readline/readexactly` (data already buffered); vectorcall on the Python methods | asyncio 4.0% / 39.5% (cache_service; 14.4% fastapi_sqlalchemy, 5.2% uvicorn, 4.2% strawberry; wave-1 asyncio_http 41%); ≈ 40% of it in these paths, kernel excluded (I) | 1.5 | exact `_SelectorSocketTransport`/`StreamReader`/`StreamWriter`, no SSL, not closing, buffer below the high-water mark (no `pause_writing` callback), separator already buffered; anything else (waiting, flow control, SSL, subclasses) falls back | the `send`/`recv` syscall (10–31% of the services' samples, unreachable); uvloop runs these paths in C | scheduling order (the fast paths schedule nothing; waiting paths fall back), protocol callbacks (never on the fast path: ruling 15), FT (loop-thread state) | none announced (C `_asyncio` covers Future/Task only) | asyncio programs not on uvloop: aiohttp (default loop), redis-py, httpx/anyio, SQLAlchemy async, Django async (R 0.35, I) | – | §3.6, cache_service/fastapi_sqlalchemy profiles |
| **`urllib.parse.urlencode` + `urljoin`** (extend the existing group) | stdlib | Python functions → vectorcall | 0.1% mean / 1.4% (django_admin: `urlencode` 0.9%, `urljoin` 0.45%; Django `reverse`/admin querystrings, Werkzeug) | 3 | `str`/`bytes` arguments, default `quote_via`; mappings with exact `dict`/`list` of pairs; else fall back | — | `quote_via` hooks, `doseq` semantics | none | high (I: requests, Django, Werkzeug, botocore use them) | – | entry tables §4.3 |
| **`html.escape`** | stdlib | Python function → vectorcall (single-pass Rust replace of `& < > " '`) | 0.06% mean / 0.9% (django_admin, via `django.utils.html.escape`) | 3 | exact `str` input; else fall back | 5 chained `str.replace` | none (pure function) | none | Django, many template helpers (I) | – | django_admin top functions |
| sorted / `list.sort` with `key=` (wave-1 open item) | language | `builtins.sorted`, `list.sort` slots (existing kernels) | 0.2% / 1.2% (attrs_cattrs); falls back in 14/16 workloads | 1.2 | key returns `str`/`int`/tuples thereof; else fall back | key calls dominate | user key function (called from C in the original too) | none | very high count, tiny share | – | §6.2 |
| `logging.Logger.level` `deopt` fallback (gap in an existing target) | stdlib | existing target | logging 2.4% / 26.9% (job_worker); the target falls back on every call in the 3 logging services | – | find why `deopt` fires (handlers/levels configured by `dictConfig`/`LoggerAdapter`?) | – | – | – | every service that logs | – | §6.2 (hand to w2-floors) |

Scores (my estimates): json `default=` I 0.53, F 0.75, K 0.25, U 0.1, R 0.30 → **0.21**;
asyncio fast paths I 0.53 (3.96 × 0.4 × (1 − 1/1.5)), F 0.5, K 0.4, U 0.1, R 0.35 → **0.12**;
`urlencode`/`urljoin` I 0.07, F 0.8, K 0.15, U 0, R 0.4 → **0.04**; `html.escape` I 0.04,
F 0.95, K 0.05, U 0, R 0.3 → **0.03**; sorted `key=` I 0.03, F 0.6, K 0.2, U 0, R 0.6 →
**0.02**. None of these is large: the big costs of these programs are in §8.

## 8. Settled / out of reach, because …

| item | share (M) | why it is settled or out of reach for entry-point kernels | who should look |
|---|---|---|---|
| **allocation/deallocation under third-party code** | 12.6% mean, 7–18% everywhere | not an entry point; cost spread over every call site of every library | w2-runtime (allocator, freelists) |
| **generic attribute access from third-party bytecode** (`PyObject_GenericGetAttr` from unspecialized `LOAD_ATTR`, `_PyObject_GetMethodStackRef`, `getattr`/`hasattr`/`setattr`, `__getattr__` hooks) | 5.9% mean, 15.9% (drf_api), 11.0% (flask_marshmallow), 9.6–9.8% (django_admin, strawberry) | the interpreter's own fallback path; spread over hundreds of sites (largest single site 0.5%); a Rust `tp_getattro` would not beat `PyObject_GenericGetAttr` | w2-interp (why these sites do not specialize), w2-machinery |
| **cyclic GC** | 2.9% mean, 15.3% (bs4), 11.9% (strawberry), 5.8% (drf) | behaviour/policy, not a kernel | w2-runtime (opt-in policy tier: these two workloads are its best case) |
| **third-party bytecode** (Django, SQLAlchemy, botocore, graphql-core, PyYAML, jsonschema, bs4/soupsieve, attrs, Werkzeug, waitress, anyio/starlette/fastapi) | thirdparty layer 35.5% mean, up to 69% (PyYAML) | third-party code is not a target (brief); only the interpreter can speed it up | w2-interp |
| **`copy.deepcopy`/`copy.copy` of objects with `__deepcopy__`/`__copy__`** (Django form and model fields, DRF serializer fields) | copy 2.9% (drf_api), 1.0% (django_admin) | the kernel falls back (`deopt`, 2–6 k calls/run) because the hooks are user code that the original calls with `copy.py` frames in between: ruling 15 | — (settled; the hook-free part already runs in the kernel: 19 k hits/run in drf_api) |
| **`html.parser`** (bs4's default parser; every `HTMLParser` subclass) | 12.9% of bs4_scrape (`parse_starttag` 7.2%, `parse_endtag` 6.6%, `goahead` 4.1% self) | `goahead`/`parse_starttag`/`parse_endtag` call overridable `handle_*` and helper methods from their own frames; real users always subclass: ruling 15 leaves no exact subset | needs a ruling only if the lead wants kernels that dispatch user callbacks; I recommend not |
| **already-C stdlib modules** (sqlite3 engine + glue, `_elementtree`/expat, `_pickle`, `_socket`/`select`, `_sre`, `_decimal`, `_datetime`, `_functools`, `_hashlib`) | C stdlib modules 6.9% mean (sqlite3 1.3%/8.6%, elementtree 0.4%/6.5%, pickle 0.3%/4.3%) | at their floor (wave 1: no headroom in expat, pickle, hashlib; sqlite is the engine) | — |
| **kernel time** (socket I/O, epoll, futex) | 10–31% of the services' samples, 17–54% system time | syscalls | — |
| **anyio worker-thread hand-off** (FastAPI runs `def` dependencies in a thread: `queue.Queue.get`, `threading.Condition.wait`) | ~10% of uvicorn_fastapi (queue 4.6%, threading 6.2%) | lock waits and futex syscalls; `queue`/`threading` semantics with FT | users: make dependencies `async def` (w2-dx) |
| **`typing.cast` and runtime `typing` subscripting** | 1.4% of bs4_scrape (186 k `cast` calls/run) | a vectorcall swap of a one-line identity function saves ~10–20 ns per call (I: 186 k × 15 ns ≈ 0.2% of bs4) | — (floor) |
| **`dateutil.parser.parse` inside botocore** | 28.5% of boto3_client (`_timelex.get_token` 7.1%: 646 k `str.isdigit` and 507 k `StringIO.read(1)` calls/run) | third-party code; botocore parses every response timestamp with dateutil | upstream note: `datetime.fromisoformat` (C, accepts botocore's ISO-8601 forms since 3.11) first would remove most of it; tell users (w2-dx) |
| **Falcon `TestClient`** | 39% of a first falcon profile | test-harness artefact (`wsgiref.validate` + `inspect.getfullargspec` per request), not production | — (workload fixed) |

## 9. Recommendations

1. **Do not expect entry-point kernels to move framework apps or library-heavy code.** The
   pure-Python stdlib that kernels could own is 12.3% of the framework apps, 7.2% of the
   library workloads and 35.6% of the services (mean, M; §3.6 split of the stdlib layer
   into Python frames vs C modules). If all of it became free: 1.14× (frameworks), 1.08×
   (libs) (I, Amdahl); with half of it reachable (ruling 15, callbacks, already-accelerated
   functions) at s = 3: ≈ 1.03–1.04× (I). The measured instruction ratios agree: 0.97–1.00×
   on 13 of 16 today.
2. **Where the framework time really is**: third-party bytecode (35.5%), allocation (12.6%),
   generic attribute access (5.9%, up to 16%), dict operations (3.4%), GC (2.9%, up to
   15%). Score the wave-2 interpreter and runtime work (w2-interp, w2-runtime) against these
   16 workloads: they are the programs where that work is the only lever.
3. **For services, the stdlib is the lever** (46% of their samples): close the json
   `default=` gap (0.21), fix the `Logger.level` `deopt` fallback (w2-floors), and consider
   asyncio transport/stream fast paths (0.12) only after uvloop-free asyncio services are
   confirmed common in the scoreboard.
4. **Keep ruling 15 as is** for `copy` hooks and `html.parser`: the shares (1–3%, 13% in one
   workload) do not justify kernels that would change tracebacks and profiler events.
5. **Add the 16 workloads to the wave-2 scoreboard** and measure their wall ratios on the
   quiet bench box; on the shared dev box only instruction counts are trustworthy.
6. **User-facing notes for w2-dx** (not Rustique work, but "will this help my program?"):
   FastAPI `def` dependencies cost a thread hop per request (~10%); botocore spends 28% of
   response handling in `dateutil`; free-threaded 3.15.1t makes threaded WSGI servers ~4×
   faster in wall time but async single-threaded services 1.1–1.5× slower.

## 10. Reproduce

```bash
R=~/rustique-research/w2/devbox/rbox; WS=w2-wl-frameworks
$R $WS build                                                  # Rustique into the 3.15.1 venv
$R $WS run -- uv pip install -r benches/workloads/frameworks/requirements.txt \
    -r benches/workloads/libs/requirements.txt -r benches/workloads/services/requirements.txt
# layer profile of one workload (plain perf + -X perf + cProfile + perf stat plain/enabled)
$R $WS run -- python benches/workloads/frameworks/_layers.py benches/workloads/libs/boto3_client.py \
    --out /work/$WS/prof/boto3_client.json --seconds 6 --stat-enable
# roll-up into the tables of §3-§5 (profiles + _steady outputs pulled locally)
python benches/workloads/frameworks/_rollup.py <prof_dir> --steady <steady_dir>
# steady state of a service
$R $WS run -- python benches/workloads/services/_steady.py services/job_worker --iters 60 [--enable]
# digests and wall ratios plain vs enabled
$R $WS run -- python benches/workloads/run.py compare frameworks libs services -n 5 -p 3
```

Raw data: `~/rustique-research/w2/wl-frameworks/` — `prof/*.json` (one per workload;
`.v2` = latest), `steady/*.json`, `rq_stats.jsonl` (Rustique counters), `tables.md`,
`tools/` (batch scripts).
