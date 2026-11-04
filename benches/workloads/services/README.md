# Workloads: `services` family (long-running services, wave 2)

Services measured in steady state: the server, its worker threads, connection pools and
caches start once in `setup()` and keep running across `run()` calls, so the heap, caches
and GC generations age as in a process that has been up for hours. Each `run()` is a fixed
amount of work (a request or job batch) and returns once all of it has completed. Wave 1
already has the pure-stdlib asyncio server (`web/asyncio_http`) and aiohttp
(`web/aiohttp_app`); these add the production servers and the worker/cache tiers. Module
format and rules: `~/rustique-research/w2/PHASE1_BRIEF.md`. Install with
`uv pip install -r requirements.txt` into CPython 3.15.1 or 3.15.1t (all four run on both
with identical digests). Profiles, RSS/GC data and analysis:
[`docs/atlas/w2-wl-frameworks.md`](../../../docs/atlas/w2-wl-frameworks.md).

- `uvicorn_fastapi` — `uvicorn[standard]` (uvloop + httptools, access log on) serving a FastAPI app; 16 thin keep-alive clients over 127.0.0.1 in the same loop (3,200 requests).
- `waitress_flask` — waitress (pure-Python threaded WSGI server, 4 worker threads) serving Flask + Jinja2 pages and JSON; 8 client threads on keep-alive sockets (2,400 requests).
- `job_worker` — 4 persistent worker threads consuming signed JSON jobs: registry dispatch, tenacity retries, cachetools LRU/TTL caches, JSON log lines (6,000 jobs).
- `cache_service` — an asyncio RESP cache server (cachetools LRU + logical-clock TTL) and 16 redis-py asyncio clients storing pickled objects (9,600 operations, ~24,800 commands).

`_steady.py` runs one workload for many iterations in one process and records time per
iteration, RSS growth, peak RSS (`VmHWM`) and GC collections per iteration:
`python benches/workloads/services/_steady.py services/job_worker --iters 60 [--enable]`.
