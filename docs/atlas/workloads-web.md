# Atlas: where web apps and services spend their time (family `web`)

Phase 1 research, 2026-11-01. Question: *where does real Python spend its time?* This part covers web
applications and services: 10 workloads in `benches/workloads/web/`, profiled on CPython 3.15.1
(python-build-standalone, PGO+LTO+BOLT) in steady state. Tags: **measured** (how) and **inferred**
(from what). Every share is a share of the user-mode CPU samples of `run()` unless stated otherwise.
Raw profiles: `~/rustique-research/profiles/web/<workload>.json`.

## Key findings

1. **Framework apps spend half their time where no entry point can reach.** In the six framework
   workloads (Django, Flask, FastAPI, SQLAlchemy, requests, aiohttp), 50.4% of samples are
   interpreter, call, attribute-lookup and allocation work inside third-party or app code, 10.7% go
   to thin builtins (`getattr`, `isinstance`, `hasattr`, `len`, dict/list subscripts), 10.7% to
   third-party native code and 1.7% to OS/runtime. Only **26.5% is reachable behind an existing entry
   point**: 11.9% in C kernels (json, str, re, datetime, sqlite3 glue) and 14.6% in stdlib
   pure-Python functions. Removing all of it would cap the gain at 1.36x. (shares measured with perf and the C-entry
   classification below; the 1.36x cap is inferred by Amdahl's law)
2. **Stdlib-centric services are the opposite.** In asyncio, logging, JSON-API and e-mail code,
   78% is reachable: 49.9% interpreted stdlib Python (email header parsing, logging records,
   asyncio streams, `copy`, `dataclasses`, `uuid`) and 28.1% stdlib C kernels (mostly the json
   accelerator). (measured)
3. **JSON is the biggest single stdlib target in this family.** The `_json` C encoder and
   scanner (behind `json.dumps`/`json.loads`) take 3.4% + 2.5% of the family mean, and 24% of
   `asyncio_http`, 17% of `json_api`, 6.5% of `aiohttp_app`, 3.7% of `requests_local`. Output
   sizes are moderate: 79% of `json.dumps` calls produce ≤ 256 characters (measured). Most of the
   bytes come from list pages of 1.5–4.5 kB (inferred from the size histograms). (perf C entries,
   `sys.monitoring` sizes)
4. **The e-mail package is pure interpretation and type creation.** 98% of `email_mime` is in
   `email.*` Python code. `HeaderRegistry.__getitem__` alone takes 11.3%: it builds a new class
   with `type()` for every header it parses (`email/headerregistry.py:604`). The rest is the
   `_header_value_parser` token machinery (`TokenList`, `Terminal.__new__`, `all_defects`
   generators). `http.client.parse_headers` goes through the same `email.feedparser`, which
   makes `email`+`http`+`urllib` 39.7% of `requests_local`. (measured)
5. **Logging is dominated by per-record Python and syscalls, not formatting kernels.**
   `stdlib:logging` owns 65.6% of `logging_service`. `Formatter.formatTime` takes 11.3%
   (`time.localtime` + `time.strftime` per record), `str % args` 6.6%, `LogRecord.__init__` 5.9%,
   `findCaller` 3.8%, and the per-record `flush()` 6.3% of user time plus most of the 11.6% kernel
   (sys) time. (measured)
6. **Most builtin calls are tiny and numerous.** The 10 workloads make 0.15–2.3 M builtin calls
   per run (cProfile). `isinstance` (1.34 M calls across the family per run), `len` (0.82 M),
   `list.append` (0.74 M), `getattr` (0.73 M) and `hasattr` (0.51 M) lead. Size-sensitive str
   methods see small receivers: 80% of `str.lower` receivers are ≤ 16 chars, 91% of `str.join`
   calls join ≤ 16 parts (p50 1–6), 97% of `str.replace` receivers are ≤ 64 chars. Per-call
   speedups on these are bounded by call overhead. (measured; cProfile, `sys.monitoring`)
7. **Third-party native code is a small share, except the database.** Leaf shares: sqlite engine
   33.7% of `sqlalchemy_orm` (3.9% of Django), pydantic-core 2.7% of FastAPI (5.0% counting what
   it calls back into CPython), aiohttp's C extensions 6.2% (multidict 3.3%, llhttp/writer 2.6%),
   SQLAlchemy's Cython helpers 1.0%, MarkupSafe 0.4% (68.6k `escape` calls per run). (measured)
8. **Surprises.** `copy.deepcopy` costs 6.0% of `json_api`, because `dataclasses.asdict`
   deep-copies every `Decimal`/`datetime`/`UUID`/`Enum` leaf. `uuid.UUID(str)` costs 6.3% in
   Python string surgery. `logger.exception` calls `compile()` (traceback caret anchors).
   `requests` scans `os.environ` for proxies on every request (`getproxies_environment` +
   `os.environ` decoding, about 3%). FastAPI runs a sync dependency through anyio's thread pool:
   anyio + threading + queue are 8.9% of samples, and kernel time is 6.8%. httpx's ASGI transport
   retries a failing `import sniffio` on every request when sniffio is not installed (7.4% in
   `importlib` before the workload pinned sniffio). (measured)

## Workloads

| workload | stands for | run() s | -X perf factor | kernel (sys) share | calls/run (cProfile) | builtin calls/run |
|---|---|---|---|---|---|---|
| `django_blog` | Django 6.1 content site via test client: middleware, URL resolving, ORM over sqlite, cached templates, ModelForm POST, JsonResponse (196 requests) | 0.59 | 1.06 | 0.0% | 2.64 M | 1.32 M |
| `flask_jinja` | Flask 3.1 storefront via test client: Werkzeug routing/forms/signed sessions, Jinja2 inheritance/macros/filters, MarkupSafe, jsonify (600 requests) | 0.54 | 1.08 | 0.0% | 2.86 M | 1.47 M |
| `fastapi_pydantic` | FastAPI/Starlette CRUD JSON API with dependency injection and nested pydantic v2 models, via `httpx.ASGITransport` on one loop (800 requests) | 0.39 | 1.12 | 6.8% | 1.62 M | 0.80 M |
| `sqlalchemy_orm` | SQLAlchemy 2.1 ORM over sqlite: joins, eager loading, aggregates, unit-of-work inserts/updates, rolled back per run (180 units) | 0.37 | 1.10 | 0.1% | 0.54 M | 0.15 M |
| `requests_local` | `requests.Session` API client against a stdlib `ThreadingHTTPServer` in the same process (540 requests over 127.0.0.1) | 0.36 | 1.06 | 17.9% | 1.45 M | 0.82 M |
| `asyncio_http` | pure-stdlib asyncio streams HTTP/1.1 JSON server + 16 keep-alive clients (5,120 requests) | 0.32 | 1.02 | 24.0% | 1.31 M | 0.80 M |
| `aiohttp_app` | aiohttp server with middleware and JSON handlers + pooled `ClientSession`, 16 workers (1,600 requests) | 0.33 | 1.09 | 10.7% | 0.75 M | 0.27 M |
| `logging_service` | stdlib `logging`: hierarchy, level filtering, adapter, filter, text/stream/JSON-lines handlers, exceptions (6,000 requests, ~21k records) | 0.34 | 1.13 | 11.6% | 1.73 M | 0.80 M |
| `json_api` | stdlib JSON REST core: `json.loads`, hand validation, dataclasses, `datetime`/`uuid`/`Decimal`, `asdict` + `json.dumps(default=)` (9,600 calls) | 0.48 | 1.08 | 0.0% | 3.56 M | 1.76 M |
| `email_mime` | transactional e-mail: templated `EmailMessage` + HTML alternative + attachments, `as_bytes`, `BytesParser(policy.default)` (240 messages) | 0.75 | 1.11 | 0.0% | 4.92 M | 2.30 M |

*run() s*: minimum of 15 steady-state runs in 3 fresh processes, alternating with the `-X perf`
runs (measured; wall time on the shared box varies about 18% per run). *-X perf factor*: the same
statistic with `-X perf` divided by plain. *kernel share*: `getrusage` sys / (user + sys) during
the perf window. All 10 workloads also run on **3.15.1t** (GIL disabled) with digests identical
to 3.15.1 (measured, one run each). `requirements.txt` installs from binary wheels on both builds;
frozenlist 1.8.0 has no cp315 wheel and installs as pure Python. Markdown rendering was left out to
stay within 10 workloads: it is regex-heavy pure Python, and `re` already shows up in Django,
Flask, e-mail and requests.

## Method (how to reproduce)

```
uv venv --python 3.15 ~/rustique-research/w-web/venv
uv pip install --python ~/rustique-research/w-web/venv/bin/python -r benches/workloads/web/requirements.txt
~/rustique-research/w-web/venv/bin/python benches/workloads/web/_profile.py all \
    benches/workloads/web/<name>.py --out ~/rustique-research/profiles/web/<name>.json
```

`_profile.py all` runs three fresh processes, each after `setup()` and one warm-up `run()`:

- **perf.** The workload runs under `python -X perf`. `perf record -e cpu-clock -F 1999 -g -p <pid>`
  is attached after the warm-up, and `run()` repeats until 3.2 s of user CPU have elapsed
  (6,320–8,088 samples). The digest is computed after perf stops.
  - Symbolization: this box's perf is built without libelf, so it resolves only `/tmp/perf-<pid>.map`
    (Python frames). `_profile.py` symbolizes everything else itself. The PBS binary is non-PIE
    and with this perf (6.18) its IPs come out as plain vaddrs, so no BOLT offset fix is needed.
    They are looked up in `nm -n -S`. Shared objects go through MMAP2 events and LOAD segments.
  - Validated on `json.dumps` of a 20k-element list: 98.8% of samples had the C entry
    `encoder_call`, and the leaves were `encoder_*`, `_Py_dg_dtoa` and `_PyUnicodeWriter_*`.
    In the real profiles 1–6 of ~3,500 distinct binary frames fell past their symbol's size.
  - libc has no symtab, so libc leaves are named by their caller (`libc<-X`).
- **Classification of each sample.**
  - *Owner*: the innermost Python frame's file (`stdlib:<module>`, `thirdparty:<distribution>`,
    `workload`; Jinja2-compiled templates count as `workload`; `<generated>` for other `<string>` code).
  - *Component*: the leaf native symbol, mapped by the prefix table in `_profile.py`. Additions to
    the brief's table: `perf-trampoline`, `sqlite3-module` vs `sqlite-engine`, `gen/coro`,
    `exceptions`, `arg-parsing`, `abstract` (generic `PyObject_*`/`PyNumber_*` dispatch), `os`,
    `socket`, `select`, `locks`, `time`, `io`, `ext:<dist>` for third-party extension modules.
  - *C entry*: the first native function below the innermost interpreter frame, skipping
    vectorcall/arg-parsing/generic-protocol dispatch. This is the builtin method, type slot or C
    function the bytecode called into, and its share covers self plus native children.
- **Reachability buckets**, decided from the C entry and the owner:
  - `kernel`: the entry is a builtin method, slot or C-accelerator function that does real work
    (str, bytes, re, json, datetime, io, list.sort, sorted, ...).
  - `thin`: the entry's cost is call overhead or generic protocol (`getattr`, `isinstance`,
    `hasattr`, `len`, container subscripts, ...).
  - `stdlib-py`: interpreter and object-model work inside a stdlib Python function, reachable only
    by replacing that function behind its entry point.
  - `ext-native`: third-party or bundled native libraries.
  - `os-runtime`: sockets, epoll, locks, `time`, libc entries.
  - `interp`: everything else, i.e. the interpreter, calls, attribute lookup, allocation/GC and
    exceptions in third-party and app code.
- **cProfile** around one `run()`: tottime/cumtime/calls per function, builtin entries.
- **Profile JSON**: the shared schema, plus these extensions: `top_native_entry`, `entry_matrix`,
  `reachability`, `reachability_by_owner_kind`, `stdlib_functions`, `cpu_user_s`/`cpu_sys_s`/
  `kernel_share_rusage`, `run_s_min15`/`xperf_factor_min15`, `cprofile_top`/
  `cprofile_builtin_by_calls`/`cprofile_owner_tottime`, and `arg_sizes_all` (every C callable and
  stdlib function). The perf data is kept in `~/rustique-research/w-web/perfdata/` and can be
  re-classified with `_profile.py reanalyze`.
- **Argument sizes** via `sys.monitoring` around one `run()`:
  - `CALL` events give, for every call from Python into a C callable, `len(self)` and `len()` of the
    first explicit argument. The first argument is resolved from the call site's bytecode when it
    is a plain load (fast/global/const, `self.attr`); resolution rates are in the JSON.
  - `PY_START`/`PY_RETURN` give the size of the main argument and of the str/bytes result of
    every stdlib Python function.
  - Calls made from C, and `%`/f-string formatting, are not seen.

Caveats (all measured):

- **`-X perf` cost.** It installs a PEP 523 frame hook, which disables Python-call inlining and the
  specialization of Python calls, properties, generators and `CALL_FUNCTION_EX`
  (`Python/specialize.c`). Runs are 1.02–1.13x slower. The extra time lands in `interpreter`/`calls`
  (`_PyFunction_Vectorcall` leaves 2–6%) and in `perf-trampoline` (1.6% mean), so treat
  `interpreter + calls + perf-trampoline` as one bucket that is overstated by up to that factor.
- **Kernel time is invisible to perf here** (`perf_event_paranoid=2`, user-only samples). The
  `getrusage` sys share is reported separately: 0–24%, socket and logging workloads only.
- **The harness is inside `run()`.** Per-response `hashlib.sha256` digests and test clients are part
  of each run. Hashing is ≤ 1% (openssl leaf). `httpx` is the FastAPI client and `requests` the
  `http.server` client. Both ends of each exchange are real library code.

## Per-workload profiles

Owners and C entries are shares of all samples (measured, perf). C entries are named by their
Python-level entry point.

| workload | top owners | top non-interpreter cells (owner × leaf component) | top C entries (self + children) | top stdlib functions (self + children) |
|---|---|---|---|---|
| `django_blog` | django 85.8%, asgiref 5.0%, stdlib:copy 1.7%, stdlib:inspect 1.2% | django×attr/type 8.0%, django×memory/gc 6.5%, django×calls 5.7%, django×dict 4.6%, django×sqlite-engine 3.9% | `Cursor.execute` 5.9%, generic getattr slot 4.1%, `getattr()` 2.7%, class call 2.7%, GC 2.2%, `hasattr()` 1.3% | `copy.copy` 0.8%, `html.escape` 0.8%, `inspect.Signature._bind` 0.4% |
| `flask_jinja` | werkzeug 27.3%, jinja2 20.7%, workload (app + templates) 17.2%, flask 13.0%, markupsafe 5.5%, stdlib:urllib 4.6% | werkzeug×calls 2.6%, jinja2×memory/gc 2.5%, werkzeug×dict 2.1%, workload×str 2.1% | `getattr()` 4.3%, `dict[k]` 2.1%, class call 2.0%, `sorted()` 1.9%, `hasattr()` 1.6%, `Pattern.split` 1.4%, `str.join` 1.1% | `urllib.parse.quote`/`quote_from_bytes` 1.1%, `json` encode 0.7%, `hmac` 0.4% |
| `fastapi_pydantic` | httpx 24.5%, fastapi 19.6%, stdlib:asyncio 8.7%, starlette 7.1%, anyio 6.7%, pydantic 6.4%, stdlib:urllib 4.0% | pydantic×ext:pydantic-core 2.7%, fastapi×calls 2.2%, httpx×memory/gc 2.1% | pydantic-core 5.0%, class call 3.1%, GC 2.0%, `Pattern.match` 1.3%, `socket.recv` 1.0% | asyncio loop (`_run_once`, `Handle._run`) 3.5%, `copy.deepcopy` 1.6%, `selectors` 1.5%, `typing.get_origin` 0.8% |
| `sqlalchemy_orm` | sqlalchemy 95.0%, workload 3.7% | sqlalchemy×sqlite-engine 33.7%, ×memory/gc 7.9%, ×attr/type 7.7%, ×libc 7.0% | `Cursor.execute` 33.5%, `Cursor.fetchall` 7.7%, GC 5.0%, generic getattr 3.4%, SQLAlchemy Cython cache key 2.0% | (stdlib < 0.5%) |
| `requests_local` | requests 18.9%, urllib3 17.5%, stdlib:http 14.9%, stdlib:email 13.8%, stdlib:urllib 11.0%, stdlib:json 4.8%, stdlib:os 3.4% | requests×calls 2.1%, requests×dict 1.6%, email×memory/gc 1.3% | class call 3.3%, json scanner 2.0%, json encoder 1.7%, `Pattern.match` 1.7%, `socket.recv_into` 0.8% | `urllib.parse._urlsplit` 2.6% + `urlparse` 1.4%, `http.server.parse_request` 1.6%, `putheader` 1.5%, proxy env scan ~3%; `email` (header parsing under `http.client.parse_headers`, `Message.get`) 13.8% in total |
| `asyncio_http` | stdlib:asyncio 40.0%, stdlib:json 36.3%, workload 22.1% | json×json-accel 6.2%, json×dict 4.2%, json×memory/gc 4.2%, json×str 3.5%, asyncio×memory/gc 3.0% | json encoder 14.2%, json scanner 10.1%, `Context.run` 1.7%, `Pattern.match` 1.2% | `StreamReader.readuntil` 9.8%, `readline` 3.1%, transport `write` 4.1%, `Handle._run` 4.0%, `_run_once` 2.8% |
| `aiohttp_app` | aiohttp 59.3%, workload 14.1%, stdlib:asyncio 12.0%, stdlib:json 9.2%, yarl 3.9% | aiohttp×memory/gc 4.8%, ×attr/type 4.7%, ×calls 4.2%, ×ext:multidict 2.8%, ×ext:aiohttp 2.6% | llhttp `feed_data` 4.3%, json scanner 3.4%, json encoder 3.1%, `x in list` 2.2% | json 6.7%, asyncio loop 2–3%, transport `write` 0.8% |
| `logging_service` | stdlib:logging 65.6%, stdlib:json 12.7%, workload 12.2%, stdlib:traceback 3.2%, stdlib:posixpath 2.5% | logging×memory/gc 6.1%, logging×libc 5.5%, logging×str 4.3% | `str % args` 6.6%, json encoder 4.8%, `TextIOWrapper.flush` 4.0%, `time.strftime` 4.0%, `time.localtime` 3.8% | `Formatter.formatTime` 11.3%, `StreamHandler.flush` 6.3%, `LogRecord.__init__` 5.9%, `getMessage` 3.9%, `findCaller` 3.8% |
| `json_api` | workload 42.7%, stdlib:json 24.5%, stdlib:dataclasses 10.9%, stdlib:copy 9.1%, stdlib:uuid 8.1%, stdlib:enum 3.4% | json×dict 4.3%, workload×dict 4.2%, json×json-accel 3.9%, workload×list/tuple/set 3.9% | json encoder 8.8%, json scanner 8.6%, `list.sort` 6.0%, `datetime.isoformat` 4.6%, `getattr()` 2.3% | `dataclasses._asdict_inner` 6.8%, `uuid.UUID.__init__` 6.3%, `copy.deepcopy` 6.0%, `enum.property.__get__` 2.6% |
| `email_mime` | stdlib:email 98.1% | email×attr/type 14.2%, email×memory/gc 10.1%, email×re-engine 5.3%, email×calls 5.3% | class creation/call 13.4%, `str.join` 2.4%, `Pattern.split` 2.2%, `Pattern.search` 2.1%, GC 2.2% | `HeaderRegistry.__getitem__` 11.3%, `TokenList.all_defects` 7.1%, `MimeParameters.params` 3.6%, `Terminal.__new__` 2.7%, `_get_ptext_to_endchars` 2.7% |

## Family roll-up

### Reachability (share of samples; measured)

| workload | kernel | thin | stdlib-py | ext-native | os-runtime | interp |
|---|---|---|---|---|---|---|
| `django_blog` | 9.9% | 14.2% | 5.5% | 5.8% | 0.4% | 64.3% |
| `flask_jinja` | 17.4% | 15.0% | 9.0% | 0.4% | 0.4% | 57.8% |
| `fastapi_pydantic` | 7.0% | 10.2% | 21.4% | 5.0% | 3.3% | 53.0% |
| `sqlalchemy_orm` | 3.5% | 9.3% | 0.5% | 43.8% | 0.1% | 42.7% |
| `requests_local` | 18.8% | 8.0% | 39.5% | 0.0% | 3.5% | 30.3% |
| `aiohttp_app` | 14.5% | 7.6% | 11.7% | 9.0% | 2.6% | 54.6% |
| **framework mean (6)** | **11.9%** | **10.7%** | **14.6%** | **10.7%** | **1.7%** | **50.4%** |
| `asyncio_http` | 35.3% | 7.5% | 42.0% | 0.0% | 3.3% | 11.9% |
| `logging_service` | 23.0% | 7.7% | 55.4% | 0.0% | 4.9% | 8.9% |
| `json_api` | 35.2% | 11.9% | 29.7% | 0.0% | 0.5% | 22.7% |
| `email_mime` | 18.8% | 7.2% | 72.6% | 0.0% | 0.5% | 0.9% |
| **stdlib-centric mean (4)** | **28.1%** | **8.6%** | **49.9%** | **0.0%** | **2.3%** | **11.1%** |
| **family mean (10)** | **18.3%** | **9.9%** | **28.7%** | **6.4%** | **2.0%** | **34.7%** |

*Plainly unreachable* (interp + thin + ext-native + os-runtime): **73.6%** of framework workloads
and 22.0% of stdlib-centric ones. Kernel time (sys, 0–24%) comes on top and is not in these
shares. The `kernel` bucket is code that is already C. It breaks down as json accelerator 5.9%,
str 2.9%, re 1.8%, datetime/time 1.1%, list/sort 0.9%, bytes 0.7%, io 0.6% (family means;
sqlite3 glue ≈ 1% inferred from `sqlalchemy_orm`).

### Top 15 cells (owner × leaf component), mean share over the 10 workloads

| # | owner | component | mean | max (workload) |
|---|---|---|---|---|
| 1 | stdlib:email | interpreter | 4.47% | 37.8% (email_mime) |
| 2 | workload | interpreter | 4.37% | 14.2% (json_api) |
| 3 | thirdparty:django | interpreter | 3.61% | 36.1% (django_blog) |
| 4 | thirdparty:sqlalchemy | sqlite-engine | 3.37% | 33.7% (sqlalchemy_orm) |
| 5 | stdlib:asyncio | interpreter | 3.15% | 22.1% (asyncio_http) |
| 6 | stdlib:logging | interpreter | 2.89% | 28.9% (logging_service) |
| 7 | thirdparty:aiohttp | interpreter | 2.65% | 26.5% (aiohttp_app) |
| 8 | thirdparty:sqlalchemy | interpreter | 2.05% | 20.5% (sqlalchemy_orm) |
| 9 | stdlib:json | interpreter | 1.65% | 6.6% (asyncio_http) |
| 10 | stdlib:json | json-accel | 1.56% | 6.2% (asyncio_http) |
| 11 | stdlib:email | attr/type | 1.48% | 14.2% (email_mime) |
| 12 | thirdparty:werkzeug | interpreter | 1.39% | 13.9% (flask_jinja) |
| 13 | workload | dict | 1.38% | 4.2% (json_api) |
| 14 | thirdparty:httpx | interpreter | 1.22% | 12.2% (fastapi_pydantic) |
| 15 | stdlib:urllib | interpreter | 1.16% | 6.7% (requests_local) |

With third-party owners collapsed into one, third-party × interpreter is 16.7% of the family mean,
× sqlite-engine 3.8%, × memory/gc 3.5%, × attr/type 3.1%, × calls 2.6%, × dict 2.5%. Mean leaf
components: interpreter 39.5%, memory/gc 8.9%, attr/type 6.6%, dict 6.4%, calls 5.6%, libc 4.6%,
str 4.4%, list/tuple/set 4.1%, sqlite-engine 3.8%, abstract 1.9%, json-accel 1.6%, re-engine 1.2%.

### Stdlib Python functions with the largest self + native-children time (mean share)

| function | mean | of which interpreted | where (≥ 0.5%) |
|---|---|---|---|
| `json.encoder.JSONEncoder.iterencode` (→ C encoder) | 4.21% | 0.73% | asyncio_http 16.4%, json_api 10.5%, logging 7.9%, aiohttp 4.1%, requests 1.8% |
| `json.decoder.JSONDecoder.raw_decode` (→ C scanner) | 2.58% | 0.11% | asyncio_http 10.8%, json_api 8.9%, aiohttp 3.7%, requests 2.0% |
| `logging.Formatter.formatTime` | 1.13% | 0.25% | logging 11.3% |
| `email.headerregistry.HeaderRegistry.__getitem__` | 1.13% | 1.13% | email 11.3% |
| `asyncio.streams.StreamReader.readuntil` | 0.98% | 0.83% | asyncio_http 9.8% |
| `copy.deepcopy` | 0.77% | 0.58% | json_api 6.0%, fastapi 1.6% |
| `asyncio.events.Handle._run` | 0.73% | 0.35% | asyncio_http 4.0%, aiohttp 1.7%, fastapi 1.6% |
| `dataclasses._asdict_inner` | 0.68% | 0.52% | json_api 6.8% |
| `json.dumps` (wrapper) | 0.67% | 0.67% | logging 2.8%, asyncio_http 2.0%, json_api 1.6% |
| `uuid.UUID.__init__` | 0.63% | 0.34% | json_api 6.3% |
| `logging.StreamHandler.flush` | 0.63% | 0.20% | logging 6.3% |
| `asyncio.base_events.BaseEventLoop._run_once` | 0.59% | 0.53% | asyncio_http 2.8%, fastapi 1.9%, aiohttp 1.3% |
| `logging.LogRecord.__init__` | 0.59% | 0.46% | logging 5.9% |
| `asyncio.selector_events._SelectorSocketTransport.write` | 0.56% | 0.34% | asyncio_http 4.1%, aiohttp 1.6% |
| `json.decoder.JSONDecoder.decode` | 0.50% | 0.23% | asyncio_http 2.3%, json_api 2.0% |
| `logging.LogRecord.getMessage` | 0.39% | 0.12% | logging 3.9% |
| `email._header_value_parser.TokenList.all_defects` (+ genexpr) | 0.71% | 0.54% | email 7.1% |
| `logging.Logger.findCaller` | 0.38% | 0.38% | logging 3.8% |
| `email._header_value_parser.MimeParameters.params` | 0.36% | 0.28% | email 3.6% |
| `email.feedparser.FeedParser._parsegen` | 0.34% | 0.19% | requests 1.5%, email 1.9% |
| `urllib.parse._urlsplit` | 0.33% | 0.22% | requests 2.6% |

By stdlib module (mean owner share): email 11.3%, json 9.1%, logging 6.6%, asyncio 6.1%,
urllib 2.0%, http 1.6%, copy 1.3%, dataclasses 1.1%, uuid 0.8%, `_collections_abc` 0.4%,
contextlib 0.4%, enum 0.4%, os 0.4%, selectors 0.4%, inspect 0.3%, traceback 0.3%, re 0.3%
(the `re` module's Python wrapper only; the engine is under the C entries).

### Builtin methods, slots and C functions (C entry, self + native children; mean share)

| entry point | C symbol | mean | where (≥ 0.5%) | cProfile calls/run (family) |
|---|---|---|---|---|
| `sqlite3.Cursor.execute` | `_pysqlite_query_execute` | 3.94% | sqlalchemy 33.5%, django 5.9% | 1.3k |
| class instantiation `C(...)` | `type_call` | 3.64% | email 13.4%, all others 1.4–3.4% | — |
| `json.dumps` C encoder | `encoder_call` | 3.37% | asyncio_http 14.2%, json_api 8.8%, logging 4.8% | 33k |
| generic attribute lookup | `PyObject_GenericGetAttr` | 2.57% | django 4.1%, aiohttp 3.9%, email 3.7% | — |
| `json.loads` C scanner | `scanner_call` | 2.46% | asyncio_http 10.1%, json_api 8.6% | 20k |
| cyclic GC | `gc_collect_main` | 1.28% | sqlalchemy 5.0%, django 2.2%, email 2.2% | — |
| `getattr()` | `builtin_getattr` | 1.00% | flask 4.3%, django 2.7%, json_api 2.3% | 733k |
| `dict[k]` (unspecialized) | `_PyDict_Subscript` | 0.90% | flask 2.1%, json_api 1.9% | — |
| `re.Pattern.match` | `_sre_SRE_Pattern_prefixmatch` | 0.79% | requests 1.7%, email 1.8%, fastapi 1.3% | 119k |
| `isinstance()` | `object_recursive_isinstance` | 0.78% | django 1.3%, json_api 1.2% | 1,339k |
| `sqlite3.Cursor.fetchall` | `pysqlite_cursor_fetchall` | 0.77% | sqlalchemy 7.7% | — |
| `str % args` | `PyUnicode_Format` | 0.79% | logging 6.6%, django 0.6% | (opcode) |
| `list.sort` | `list_sort` | 0.60% | json_api 6.0% (200 items, key=lambda) | 2k |
| `datetime.isoformat` | `datetime_datetime_isoformat` | 0.52% | json_api 4.6% | 26k |
| `hasattr()` | `builtin_hasattr` | 0.52% | flask 1.6%, django 1.3% | 509k |
| pydantic-core | `_pydantic_core` | 0.50% | fastapi 5.0% | 6k |
| `str.join` | `unicode_join` | 0.41% | email 2.4%, flask 1.1% | 319k |
| `TextIOWrapper.flush` | `_io_TextIOWrapper_flush` | 0.40% | logging 4.0% | 28k |
| `time.strftime` / `time.localtime` | `time_strftime` / `time_localtime` | 0.40% / 0.38% | logging 4.0% / 3.8% | 29k each |
| `re.Pattern.split` / `.search` | `_sre_SRE_Pattern_split` / `_search` | 0.38% / 0.29% | email 2.2% / 2.1%, flask 1.4% | 35k / 24k |
| `bytes.decode` | `bytes_decode` | 0.32% | asyncio 1.0%, requests 0.9% | 182k |
| `x in list` | `list_contains` | 0.24% | aiohttp 2.2% (2-element tag lists) | — |
| `str.lower`/`upper` | `ascii_upper_or_lower` | 0.24% | requests 0.8% | 449k |

Highest call counts per run, summed over the family (cProfile; measured): `isinstance` 1.34 M,
`len` 0.82 M, `list.append` 0.74 M, `getattr` 0.73 M, `hasattr` 0.51 M, `str.lower` 0.45 M,
`dict.get` 0.42 M, `str.join` 0.32 M, `str.encode` 0.32 M, `str.replace` 0.30 M, `T.__new__`
0.26 M (e.g. `str.__new__` from `markupsafe.Markup`, email `Terminal`), `bytes.decode` 0.18 M,
`issubclass` 0.16 M, `str.startswith` 0.16 M, `str.strip` 0.15 M, `sum` 0.13 M, `re.Pattern.match`
0.12 M. By tottime share the top C entries are `sqlite3.Cursor.execute` 2.7%, `socket.send` 1.9%,
`isinstance` 1.6%, `getattr` 1.0%, `len` 0.8%, `str.join` 0.8%, `TextIOWrapper.flush` 0.7%,
`hasattr` 0.7%.

### Argument sizes (calls from Python; measured with `sys.monitoring`, one run per workload)

| hotspot | calls/run (family) | workloads | ≤ 16 | ≤ 64 | ≤ 256 | ≤ 1024 | p50 per workload | max |
|---|---|---|---|---|---|---|---|---|
| `str.lower` (receiver chars) | 458,730 | 9 | 80% | 89% | 100% | 100% | 4–31 | 277 |
| `str.encode` (receiver chars) | 318,760 | 9 | 67% | 85% | 97% | 99% | 7–173 | 37,704 |
| `bytes.decode` (receiver bytes) | 190,742 | 8 | 72% | 95% | 98% | 99% | 4–173 | 18,637 |
| `str.join` (number of parts) | 319,505 | 9 | 91% | 100% | 100% | 100% | 1–6 | 211 |
| `str.join` (total chars joined) | 319,505 | 9 | 37% | 68% | 93% | 97% | 8–213 | 16,334 |
| `str.replace` (receiver) | 301,843 | 8 | 26% | 97% | 98% | 100% | 5–36 | 1,567 |
| `str.strip` (receiver) | 160,859 | 9 | 54% | 100% | 100% | 100% | 4–38 | 867 |
| `str.startswith` (receiver) | 162,715 | 10 | 55% | 82% | 97% | 98% | 4–173 | 4,488 |
| `str.split` (receiver) | 68,547 | 8 | 52% | 98% | 99% | 99% | 7–54 | 1,296 |
| `re.Pattern.match` (subject) | 46,431 | 5 | 57% | 96% | 100% | 100% | 11–159 | 243 |
| `re.Pattern.search` (subject) | 23,648 | 5 | 62% | 93% | 95% | 98% | 3–16 | 18,131 |
| `re.Pattern.split` (subject) | 35,020 | 3 | 45% | 87% | 98% | 100% | 0–27 | 16,122 |
| `re.Pattern.findall` (subject) | 50,946 | 2 | 91% | 100% | 100% | 100% | 6–26 | 36 |
| `json.dumps` (output chars) | 33,241 | 7 | 2% | 15% | 79% | 90% | 2–4,395 | 4,488 |
| `json.loads` (input chars/bytes) | 19,548 | 6 | 14% | 31% | 67% | 78% | 68–175 | 4,488 |
| `markupsafe` escape (input) | 68,588 | 1 | 83% | 91% | 100% | 100% | 7 | 277 |
| `html.escape` (input) | 16,764 | 1 | 57% | 92% | 95% | 100% | 12 | 1,296 |
| `urllib.parse.quote` (input) | 21,356 | 3 | 81% | 100% | 100% | 100% | 4–33 | 52 |
| `urllib.parse.unquote` (input) | 11,308 | 4 | 94% | 100% | 100% | 100% | 1–6 | 23 |
| `urllib.parse._urlsplit` (URL) | 7,460 | 3 | 6% | 100% | 100% | 100% | 21–36 | 57 |
| `uuid.UUID(hex)` | 49,680 | 1 | 0% | 100% | 100% | 100% | 36 | 36 |
| `datetime.fromisoformat` | 51,938 | 2 | 0% | 100% | 100% | 100% | 19–25 | 25 |
| `sqlite3.Cursor.execute` (SQL chars) | 609 | 1 | 0% | 13% | 80% | 100% | 111 | 343 |

Not measured: subjects of `re.Pattern.sub` (second argument), `%`/f-string formatting (opcodes),
and calls made from C. Per-workload top-10 lists are in each profile's `arg_sizes`, and every C
callable and stdlib function is in `arg_sizes_all`.

### Third-party native code (not our targets, but part of the time; measured)

| workload | leaf share | C-entry share | detail |
|---|---|---|---|
| `sqlalchemy_orm` | sqlite engine 33.7% + SQLAlchemy Cython 1.0% | 43.8% | `Cursor.execute` 33.5% (the SQL engine; libc under it counted here) |
| `django_blog` | sqlite engine 3.9% | 5.8% | |
| `fastapi_pydantic` | pydantic-core 2.7% | 5.0% | `SchemaValidator.validate_python` 5.2k calls, `SchemaSerializer.to_json` 0.7k calls per run |
| `aiohttp_app` | multidict 3.3%, aiohttp 2.6%, propcache 0.2%, yarl 0.1% | 9.0% | llhttp `feed_data` 4.3%, `_serialize_headers` 1.6% |
| `flask_jinja` | MarkupSafe 0.4% | 0.4% | 68.6k `_escape_inner` calls; `Markup.__new__` + `escape` in Python cost 5.5% |

## What could be accelerated behind existing entry points

Ranked by family-wide share and ease of exactness. All gain figures are *inferred* from the shares
above via Amdahl's law; none are measured yet.

1. **`json` encoder and scanner (`_json.c_make_encoder`, `scanstring`/`scan_once`).** These are
   already C. They take 5.9% of the family mean and up to 24% of a JSON-heavy service. The C
   entry points are stable and exactness is testable byte for byte.
   - A Rust kernel that is 2x faster would save ≈ 3% family-wide and 12% on `asyncio_http`.
   - Most calls produce 100–4,500 bytes, so per-call overhead does not dominate.
   - Upstream lists json as a 3.17 Rust candidate.
2. **Interpreted stdlib hot spots with narrow, testable contracts** (stdlib-py bucket):
   - `uuid.UUID.__init__` from a str: 6.3% of `json_api`, a fixed 36-char input.
   - `copy.deepcopy` of atoms and reduce-able leaves: 6.0% of `json_api`, 1.6% of FastAPI. Hint:
     copium reaches 0.095x.
   - `dataclasses.asdict`: 6.8%.
   - `urllib.parse._urlsplit`/`quote`/`unquote`: 1–4% of client and Flask code, inputs ≤ 64 chars.
   - `http.client.parse_headers` → `email.feedparser`/`email.message`: the `email` owner is
     13.8% of `requests_local`, reached on both the client and the `http.server` side, at the
     `http.client` entry point.
   - `logging.LogRecord.__init__`/`Formatter.formatTime`/`findCaller`: 21% of `logging_service`,
     though `formatTime` is mostly `time.localtime` + `strftime` libc work.

   Each of these is a single Python function whose `__code__`, or the C function it wraps, could
   be swapped. Each must reproduce subclass hooks and exception messages.
3. **`email` header parsing and folding.** This is the largest stdlib owner (11.3% of the family
   mean, 98% of `email_mime`, part of `requests_local`). It is all interpreted Python and is
   reachable only by reimplementing `headerregistry`/`_header_value_parser` behind
   `HeaderRegistry.__call__` and `policy.header_fetch_parse`. That is a large surface, with defects
   lists, folding and token objects that are observable. The 11.3% from `type()` per header is a
   design cost: caching the class would be the cheap win, but it changes class identity (`type(h)`),
   so it is not exact.
4. **asyncio streams and event loop** (`StreamReader.readuntil`/`readline`, `_run_once`,
   `Handle._run`, transport `write`): 25% of `asyncio_http`, 3–5% of FastAPI and aiohttp. This is
   pure Python and reachable, but it is protocol- and ordering-sensitive. The `_asyncio` C
   accelerator already covers futures and tasks.
5. **Small str/re/bytes kernels are not worth it here.** `str.lower`/`join`/`replace`/`strip`/
   `encode`, `bytes.decode` and `Pattern.match`/`search` together are about 3–4% of the family
   mean. Their inputs are tiny (p50 ≤ 40 chars; 80–91% ≤ 16 for `lower`/`join` parts), so a
   per-call Rust kernel has almost no work to win back.
6. **Plainly unreachable: interpreter, call overhead, attribute lookup, allocation and GC in
   framework code** (50% of framework workloads, plus 11% thin builtins). This is what decides
   Django, Flask, FastAPI and aiohttp performance. No entry-point swap touches it.

Not targets, but worth knowing:
- Database time in the sqlite engine (Rustique could only shave the `_sqlite3` row-building glue).
- pydantic-core, MarkupSafe and aiohttp's C extensions.
- GC: 1.3% mean, 5.0% in SQLAlchemy.
- Kernel time: up to 24% for socket and logging workloads (per-record `flush`, per-request
  syscalls).
