# Workloads: `threads` family (multi-threaded programs)

Module format: the Phase-1 brief (`REQUIRES`, `setup()`, `run(state)`, `digest(result)`).
Stdlib only (`requirements.txt` is empty), so every workload runs on 3.15.1 and 3.15.1t.
Each `run()` uses `WL_THREADS` threads (environment variable read in `setup()`, default 8)
and does a **fixed amount of work** whatever the thread count, so time falls as threads are
added when the build can run them in parallel (3.15.1t with the GIL disabled). Results are
collected per request/item index or aggregated commutatively and sorted: the digest is the
same for every thread count, every schedule and both builds.

| workload | stands for | run() 3.15.1, 1 thread |
|---|---|---|
| `request_pool` | threaded web worker (gunicorn `gthread`, `ThreadPoolExecutor` job API): `urllib.parse`, `json.loads/dumps`, shared compiled regexes, `fromisoformat`/`strptime`, dataclasses + `asdict`, a shared read-only catalog, `deepcopy` of a response template (12,000 requests) | 0.43 s |
| `pipeline_queue` | log shipper / ETL stage pipeline: producer -> bounded `queue.Queue` -> N parser threads (regex, `strptime`, `urlsplit`/`parse_qsl`, `ipaddress`) -> aggregator thread (`Counter`) (36,000 lines) | 0.58 s |
| `http_server` | thread-per-connection `ThreadingHTTPServer` JSON service with N keep-alive `http.client` clients over 127.0.0.1; shared dict store under a `Lock` (3,600 requests) | 0.50 s |
| `compute_pool` | CPU-bound pure Python on a thread pool: tokenizing, TF-IDF cosine against shared profiles, edit-distance fuzzy matching, Dijkstra over a shared graph (300 documents) | 1.2 s |
| `shared_cache` | shared service state: read-through `dict` cache + `Lock`, `lru_cache`, `threading.local`, metrics under a lock, `logging` to one shared handler, `Decimal` pricing, `json.dumps` (36,000 requests) | 0.50 s |

Scaling runs (1/2/4/8 threads, plain vs `rustique.enable()`, per build):
`python benches/ft/scaling.py run threads -t 1,2,4,8 -p 3 -o out.json` on a CPU set of at
least 8 cores, then `python benches/ft/scaling.py table out-3.15.1.json out-3.15.1t.json`.
Results and analysis: `docs/atlas/w2-threads.md`.
