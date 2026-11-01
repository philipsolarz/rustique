STATUS: Phase 1 result, 2026-11-01. Decides wave 1. Evidence lives in `docs/atlas/`
(`workloads-{web,data,devtools}.md`, `static-usage.md`, `headroom-{text,objects}.md`,
`mechanisms.md`) and the raw profiles in `~/rustique-research/` (roll-up script:
`benches/workloads/rollup.py`). Every number is **measured** (M) or **inferred** (I).

# Impact atlas: where real Python spends its time, and what Rust can take

## 1. The answer in five lines

1. **Two thirds of real-program time is out of reach** of any entry-point acceleration:
   interpreting the program's own bytecode, calls and attribute lookups (46% mean),
   allocation and cyclic GC (16%), dict work from bytecode (5.5%), and builtins the
   specializer inlines (`isinstance`, `len`, `list.append`) (M, 35 workloads).
2. **The reachable third is concentrated in pure-Python stdlib modules**, not in builtin
   methods: `copy`, `json` glue, `email`/`http`, `posixpath`/`pathlib`, `logging`,
   `_strptime`, `textwrap`, `random`, `urllib.parse`, `ipaddress`, `statistics`. Each is
   2–5% of the mean and **20–98% of the programs that use it** (M).
3. **Small calls dominate.** Median `str` method input 9–16 chars, `int(str)` 2 chars, 79%
   of `json.dumps` outputs ≤ 256 chars (M). Bulk `str`/`bytes`/regex kernels win 7–90× at
   1 MB and ~1.0× at real sizes (M): last round's #1 next step (str kernels) is demoted.
4. **The winning mechanism exists and is measured** (M, `mechanisms.md`): `ml_meth`/slot swaps
   for C entry points (+2 Ir on fallback), and `PyFunction_SetVectorcall` on the existing
   function object for pure-Python functions (keeps identity, captured references,
   pickling, signatures, source, tracebacks; +180–260 Ir on fallback), activated lazily by a
   `sys.modules` watcher (`enable()` ≈ 10 µs).
5. **Expected payoff of wave 1** (I, from the shares and achievable ratios below): ~10% less
   time on the mean workload, 1.3–2× on the stdlib-heavy programs (snapshots, report
   generation, log analytics, JSON services, file inventories), ~1.0–1.05× on framework
   apps whose time is in their own bytecode.

## 2. Method

- **Workload set** (`benches/workloads/`): 35 deterministic, self-checking programs in three
  families — web/services (Django, Flask+Jinja2, FastAPI+pydantic, SQLAlchemy, requests,
  asyncio, aiohttp, logging service, JSON API, e-mail), data/scripting (csv→json ETL, log
  analytics, tz events, text corpus, file inventory, backup archive, config merge, XML
  feeds, state snapshot, billing, report render, pandas ETL) and developer tools (pyflakes,
  pycodestyle, pylint, black, isort ×2, pytest on networkx, Sphinx, pip resolver, coverage,
  sqlglot, rich, import startup). Each profiled on CPython 3.15.1 with perf (`-X perf`,
  cpu-clock, ≥ 5k samples, own symbolizer validated on `json.dumps`) and cProfile, plus
  argument-size histograms for the top hotspots.
- **Attribution**: every sample → (innermost Python frame's owner, leaf native component).
  A candidate's share = samples under its stdlib modules (including the native code they
  call) + its native components under other owners (C entry points called directly).
- **Static usage**: the top 1000 PyPI packages (94k files) AST-scanned; reach =
  download-weighted share of packages whose library code uses the feature (M).
- **Headroom**: stdlib vs best existing alternatives and Rust prototypes, small and large
  inputs, callgrind for small cells, paired wall-time repeats for large ones (M/I as tagged).
- **Score** = `I × F × (1 − K) × (1 − U) × (0.5 + R)`:
  - `I` impact = mean share × (1 − 1/s), the percentage of mean workload time saved, with
    `s` the conservative achievable speedup of an exact drop-in on the targeted part;
  - `F` feasibility (0–1) = exact-subset coverage × mechanism fit (C entry with free
    fallback 1.0; vectorcall on calls ≥ 5k Ir 0.9; vectorcall on cheap calls 0.7) ×
    tractability (test suite, API size);
  - `K` risk (0–0.6): hooks, subclasses, state, semantic traps;
  - `U` upstream overlap (0–0.7): Rust-for-CPython (R4C) or CPython itself doing it, by when;
  - `R` reach (download-weighted share of packages), so that a target used everywhere is
    not undervalued by a 35-program sample.

## 3. Where the time goes (mean of 35 workloads, M)

| bucket | mean | max | reachable? |
|---|---:|---:|---|
| interpreter + calls + attribute lookup (all owners) | 46.2% | 75% (config_merge) | no: the program's own bytecode |
| allocation + cyclic GC | 16.2% | 48% (billing) | no (GC is 30–41% of pylint/pytest) |
| dict (from bytecode and C) | 5.5% | 13% | mostly no (specialized, M last round) |
| `str` methods/slots (all owners) | 4.5% | 16% (text_corpus) | yes, but inputs are tiny (≤ 1.1×) |
| e-mail + http header parsing | 4.0% | 98% (email_mime) | yes (pure Python) — large, risky surface |
| `copy` (`deepcopy`/`copy`) | 3.7% | 50% (state_snapshot); 34% isort | yes (pure Python, recursive) |
| `json` (glue + `_json`) | 3.6% | 36% (asyncio_http); 29% csv_etl | yes (Python + C) |
| zlib/gzip/zipfile/tarfile | 3.0% | 96% (backup_archive) | only decompress/crc32 can be exact |
| paths (`posixpath`, `pathlib`, `fnmatch`, `glob`, `os`) | 3.0% | 80% (fs_inventory) | yes (pure Python, cheap calls) |
| `re` engine (all owners) | 3.0% | 20% (report_render) | yes, little gain at real subject sizes |
| `tokenize`/`ast` | 2.2% | 33% (pyflakes) | `ast.parse` is CPython's C parser (R4C 3.17 candidate) |
| `xml.etree` | 2.0% | 71% (xml_feeds) | serializer yes (pure Python); parser no headroom |
| `logging` | 2.0% | 66% (logging_service) | yes (pure Python, hook-rich) |
| `asyncio` loop/streams | 1.9% | 41% (asyncio_http) | yes, high risk, ≤ 1.7× |
| datetime text (`_strptime`, `strftime`, iso) | 1.4% | 26% (log_analytics) | yes (C entry points) |
| import system | 1.3% | 37% (startup) | high risk; PEP 810 lazy imports upstream |
| `textwrap` 1.2, `dataclasses` 1.0, `random` 0.9, `urllib.parse` 0.9, `statistics`/`fractions` 0.8, `ipaddress` 0.6, `csv` 0.6, `uuid` 0.5, `re` module glue 0.5, `pickle` 0.4, `contextlib` 0.4, `enum` 0.3 | | 4–39% each | yes (pure Python, mostly) |
| float → text (dtoa) | 0.1% | 3% (csv_etl, all float) | yes (one slot), small share in this set |

Family view (M): framework web apps are 26.5% reachable (cap 1.36×), stdlib-heavy services
78%, data scripts ~73% (upper bound), developer tools ~29%.

## 4. Candidates, scored

`s` = conservative achievable speedup on the targeted part (I unless marked; sources in the
atlas docs). Score columns as defined in §2. Sorted by score.

| # | candidate | entry points | share mean / max | `s` | I | F | K | U | R | score | evidence |
|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | **copy** `deepcopy`/`copy` (+ `dataclasses.asdict/astuple/replace`) | Python fn → vectorcall, recursion inside the kernel | 3.7% (+1.0%) / 50% | 5 | 3.8 | 0.75 | 0.2 | 0 | 0.25 | **1.71** | copium 0.11–0.18 (M) passes 81/84 `test_copy`; C deepcopy rejected upstream |
| 2 | **json** `dumps`/`loads` (+ exact float→text core) | Python fn → vectorcall; `_json` heap-type slots; float `tp_repr` | 3.6% / 36% | 3 | 2.4 | 0.85 | 0.25 | 0.15 | 0.32 | **1.07** | exact Rust proto 0.31 small / 0.60 large (M); orjson 0.14–0.20 (not exact); 80% of `dumps` calls have no kwargs (M) |
| 3 | **paths** `posixpath`, `pathlib` core, `fnmatch` (+ helpers of `os.walk`/`glob`) | Python fns/methods → vectorcall | 3.0% / 80% | 3 | 2.0 | 0.65 | 0.25 | 0 | 0.49 | **0.98** | `posixpath.join` kernel 0.29 with all guards (M); pathlib ops 2–12 µs (M) |
| 4 | **datetime text** `strptime`, `strftime`, `isoformat`, `fromisoformat`, `time.strftime/strptime` | C methods → `ml_meth` (free fallback) | 1.4% / 26% | 5 | 1.1 | 0.9 | 0.2 | 0.2 | 0.30 | **0.52** | `strptime` 3.7 µs, interpreted (M); ciso8601 0.014–0.02 (M); 3.16 adds a C path for numeric formats only |
| 5 | e-mail / `http.client.parse_headers` | Python → vectorcall | 4.0% / 98% | 5 | 3.2 | 0.3 | 0.5 | 0 | 0.30 | 0.38 | 27.5 µs per response (M); `Message` state, policies, header classes created per header |
| 6 | **logging** record path | Python methods → vectorcall | 2.0% / 66% | 2 | 1.0 | 0.65 | 0.35 | 0.05 | 0.40 | 0.36 | `formatTime` 11–16%, `LogRecord.__init__` 6–15% (M); hooks: factories, converters, subclasses |
| 7 | `textwrap` | Python → vectorcall | 1.2% / 39% | 8 | 1.05 | 0.6 | 0.3 | 0 | 0.20 | 0.31 | 1.09 M Ir per `wrap` of 1.5 KB (M) |
| 8 | `random` glue (`randint`, `choice`, `shuffle`, `sample`) | Python methods → vectorcall | 0.9% / 30% | 3 | 0.6 | 0.8 | 0.15 | 0 | 0.20 | 0.29 | 4 Python frames around `getrandbits` (I); stream must match exactly |
| 9 | `urllib.parse` (`quote`, `unquote`, `urlencode`, `parse_qsl`, uncached `urlsplit`) | Python → vectorcall | 0.9% / 11% | 5 | 0.72 | 0.65 | 0.25 | 0 | 0.29 | 0.28 | `quote` proto 0.12–0.16 (M) |
| 10 | `statistics` (`mean`, `stdev`, `median`) | Python → vectorcall | 0.8% / 27% | 20 | 0.76 | 0.7 | 0.2 | 0 | 0.10 | 0.26 | `mean` of 1000 floats 414 µs, `fmean` 0.019 (M); exact rational semantics |
| 11 | `ipaddress` parse/str | Python → vectorcall, slots | 0.6% / 20% | 10 | 0.54 | 0.8 | 0.15 | 0 | 0.11 | 0.22 | `ip_address` 1.79 µs (M) |
| 12 | float → text everywhere | float `tp_repr` slot, `__format__` | 0.1–0.5% / 3% | 3.5 | 0.25 | 0.95 | 0.1 | 0 | 0.80 | 0.28 | zmij/ryu identical on 3.3 M values, 0.27 of stdlib (M) — ships inside the json workstream |
| 13 | `re` module functions (Rust `_compile` cache) | Python → vectorcall | 0.5% / 5% | 1.8 | 0.22 | 0.9 | 0.1 | 0 | 0.66 | 0.21 | `_compile` cache hit = 42% of a small `re.match` (M) |
| 14 | `str`/`bytes` bulk kernels | `ml_meth` + length guard | 4.5% / 16% | 1.05 at real sizes | 0.2 | 0.9 | 0.2 | 0 | 0.86 | 0.20 | 1.0× ≤ 100 chars; 1.5–3× at 1 KB, 7–15× at 1 MB (M) |
| 15 | `re` engine (DFA start-finder + sre Match) | `_sre` `ml_meth` | 3.0% / 20% | 1.2 at real sizes | 0.5 | 0.5 | 0.4 | 0 | 0.66 | 0.17 | 87% of 8,078 PyPI patterns exact, 0 mismatches in 2.82 M pairs; 4–90× at ≥ 1 KB (M) |
| 16 | `tokenize` glue | Python generator helpers | ~0.8% / 7% | 2 | 0.4 | 0.6 | 0.3 | 0.3 | 0.10 | 0.10 | `TokenInfo` construction 6.6% of pycodestyle (M) |
| 17 | `xml.etree` serializer | Python → vectorcall | ~0.8% / 27% | 6 | 0.67 | 0.7 | 0.3 | 0.2 | 0.05 | 0.14 | lxml `tostring` 0.10 (M) |
| 18 | zlib decompress / crc32 | `ml_meth` | ~0.5% / 15% | 2.5 | 0.3 | 0.8 | 0.1 | 0.6 | 0.13 | 0.05 | libdeflate 0.29–0.45 at 1 MB (M); compress output must stay byte-identical: no faster deflate is (M) |
| 19 | `uuid` | Python → vectorcall, slots | 0.5% / 10% | 3 | 0.33 | 0.9 | 0.1 | 0.7 | 0.19 | 0.06 | C `uuid` merged for 3.16 |
| 20 | `asyncio` loop internals | Python → vectorcall | 1.9% / 41% | 1.4 | 0.5 | 0.3 | 0.6 | 0.1 | 0.17 | 0.04 | uvloop bound 0.45–0.76 (M); scheduling order is observable |
| – | not candidates | | | | | | | | | | GC (behaviour change), inlined builtins (`isinstance`, `len`, `append`: the specializer never reaches a swap, M), `int`/`float` arithmetic, dict ops, `unicodedata.normalize` (alternatives slower, M), `pickle`/`struct`/`hashlib` (already at the floor, M), `ast.parse` (CPython's parser; R4C candidate), import system (risk) |

## 5. Decisions

### 5.1 Highest-impact targets

copy (1.71), json (1.07), paths (0.98), datetime text (0.52); then e-mail/HTTP headers
(0.38) and logging (0.36), whose shares are large where they occur but whose hook surfaces
are wide.

### 5.2 Best places to start

- **The acceleration framework** (`accel`): every target above needs the same machinery —
  sites, all-or-nothing install, version gate and fingerprints, dependency pins through
  dict/type watchers, the tools guard, lazy activation on import, counters and status. It
  also fixes a measured defect of the shipped kernels: 3.15's JIT copies `ml_meth` into
  traces, so traces built while enabled keep calling Rust after `disable()` (M).
- **datetime text** is the low-risk proof of the C-entry pattern: `datetime.strptime` is a
  C classmethod that calls interpreted `_strptime`, so an `ml_meth` swap replaces ~3.7 µs
  of Python with a closed-grammar Rust parser and falls back for +2 Ir.
- **copy** is the proof of the pure-Python pattern at its hardest point (recursion with
  user hooks), with an existing exact C reference (copium) bounding the gain.
- **Developer experience** unblocks adoption and is the vehicle for proving "nothing changed
  except speed" in users' CI.

### 5.3 Wave 1 (Phase 2 workstreams)

| workstream | scope | why now |
|---|---|---|
| `accel-core` | framework per `ACCELERATION.md`; retrofit of the existing list/tuple/builtins kernels (JIT cache clearing, version gate, subinterpreter refusal); startup cost of `enable()` | foundation for all targets; fixes a shipped defect |
| `dx` | zero-code switch, fine-grained control and instant off, status/report, CI check mode, wheels for every platform/build, docs | required; makes the rest usable |
| `json` | `json.dumps`/`loads` (Python entry + `_json` types), exact float→text core (also behind `float.__repr__`) | #2 score; most common serializer; float core is shared |
| `copy` | `copy.deepcopy`/`copy.copy`, `dataclasses.asdict/astuple/replace` | #1 score |
| `paths` | `posixpath`, `pathlib` pure-path core, `fnmatch`, helpers under `os.walk`/`glob` | #3 score; ubiquitous |
| `datetime` | `strptime`/`strftime`/`isoformat`/`fromisoformat`, `time.strftime/strptime` | #4 score; proves the C-entry pattern |
| `logging` | `LogRecord` creation, `Formatter.format`/`formatTime`, `findCaller`, `PercentStyle` | 66% of logging-heavy services, 40% reach; hook surface handled by type/dict pins |
| `utils` | `random` glue, `textwrap`, `urllib.parse`, `statistics`, `ipaddress` (in that order) | together 1.36 — more than logging; each small once `accel` exists |

### 5.4 Next waves (ranked)

1. `http.client.parse_headers`/`email.feedparser` (HTTP subset), then e-mail header classes.
2. `re`: module-function fast path, then the DFA start-finder for long subjects.
3. `str`/`bytes` bulk kernels behind a length guard (log/text processing on large inputs).
4. `xml.etree` serializer, `tokenize` helpers, `csv` writer quoting, `contextlib`, `enum`.
5. zlib decompress/crc32 (only if R4C's Rust zlib slips), `uuid` (3.14/3.15 only).
6. Out of scope by contract: GC tuning (`gc.freeze`-style behaviour changes), inlined
   builtins, the import system.

### 5.5 Rulings on questions raised in Phase 1

- **Private state** (`_strptime._regex_cache`, internal encoder instances) may differ when no
  public API exposes it; each target lists what it skips (`ACCELERATION.md` §2.4).
- **Generators**: never replace a generator-returning function; accelerate its helpers.
- **Randomness**: same source, same amounts (`ACCELERATION.md` §2.5); `uuid4`'s choice of
  `getrandom` vs `os.urandom` is deferred with `uuid`.
- **`_PyFunction_SetVersion`** (exported, private) is used to restore specialization on
  `disable()`, version-gated; without it a disabled function stays correct but
  despecialized.
- **Subinterpreters**: the extension refuses to load in them; process-global kernels are
  interpreter-agnostic.
- **Startup is a measured cell** under the never-slower rule: `RUSTIQUE` unset must cost
  nothing measurable, and `enable()` must fit the ≤ 1.05 bar on `python -c pass`
  (last round: 1.17×) — owned by `accel-core` and `dx`.
- **Two-version windows**: a target that CPython 3.16 will cover (numeric `strptime`,
  `uuid`) is still worth it for 3.14/3.15 only if its share is large; datetime qualifies
  because named and `%z` formats stay interpreted in 3.16.
