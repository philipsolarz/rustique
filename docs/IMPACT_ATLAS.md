STATUS: Part 1 (§1–§6): wave 1's atlas (2026-11-01) and its outcome. **Part 2 (§7–§14): wave 2's
atlas (2026-11-02)**, the Phase-1 result that chose wave 2's workstreams and targets; §15:
wave 2's measured outcome against it (2026-11-03). Evidence lives
in `docs/atlas/` (wave 1: `workloads-*.md`, `static-usage.md`, `headroom-*.md`, `mechanisms.md`;
wave 2: `w2-*.md`, one per Phase-1 slice, and `w2-phase0.md`) and the raw data in
`~/rustique-research/` (wave 2: `~/rustique-research/w2/<slice>/`). Every number is **measured**
(M) or **inferred** (I).

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

## 6. Wave 1 against this atlas (Phase 3, M)

Measured at `wave1` `11bad29` (`docs/atlas/phase3.md`; all 15 groups on, 34 workloads, plain
vs enabled, identical results):

| prediction (§1.5, I) | measured, 3.15.1 (3.15.1t) |
|---|---|
| mean workload ~10% less time | geometric mean 0.880 (0.843): 12% (16%) less |
| stdlib-heavy programs 1.3–2× | 1.2–2.3×: `report_render` 0.436, `tz_events` 0.591, `log_analytics` 0.594, `csv_etl` 0.625, `state_snapshot` 0.651, `fs_inventory` 0.743, `isort_stdlib` 0.771 (0.196), `asyncio_http` 0.791, `json_api` 0.835 |
| framework apps ~1.0–1.05× | 1.00–1.10×: Django 0.997, Flask 0.990, FastAPI 0.983, SQLAlchemy 0.997, black 0.990, pylint 1.002 |

Per candidate (score → workloads with only that group on, Phase 2, 3.15.1): copy (1.71) →
`state_snapshot` 0.675, `isort_stdlib` 0.763; json (1.07) → `asyncio_http` 0.789,
`csv_etl` 0.799; paths (0.98) → `fs_inventory` 0.742; datetime text (0.52) →
`tz_events` 0.593, `log_analytics` 0.617; logging (0.36) → `logging_service` 0.830;
utilities (0.22–0.31) → `report_render` 0.449. The ordering held, with one surprise:
datetime text, ranked 4th, gave the largest per-workload gains (its `ml_meth` mechanism has
free fallbacks and its kernels run 0.07–0.20 of CPython). Not reached: the time in
interpreting bytecode, allocation and GC (§3), as predicted.


# Part 2: wave 2 — from stdlib-heavy programs to all Python programs

## 7. The answer in eight lines (wave 2, Phase 1)

1. **The workload set now represents real software**: 65 programs in 8 families. Wave 1's 34
   (web, data, devtools) plus 31 new: framework apps (6), third-party-heavy code (6),
   long-running services (4), complex custom applications (10, stdlib-only, written like
   in-house code), multi-threaded programs (5). Every new workload is deterministic and
   self-checking; all 65 give identical digests plain and with Rustique on both builds (M).
2. **Where the time goes** (M, 65 workloads, 3.15.1, distortion-free sampling, §9): own code
   10.1%, generated code 2.1%, language operations 13.8%, stdlib 32.0%, third-party 20.5%,
   runtime 19.7%. The families differ completely: custom code is 42% its own bytecode,
   frameworks and libraries 36–49% third-party, data and web scripts ~50% stdlib, devtools
   32% runtime (GC up to 49%).
3. **The interpreter layer is not out of reach any more.** Wave 1's "46%" was 41.1% measured
   without distortion (`-X perf` disables inlined calls). Inside it, the specialized handler
   fast paths are a floor, but the *generic* paths the specializer gave up on are reachable by
   a new mechanism, **opcode-handler swap** (M2): in tail-calling builds each opcode handler is
   a function in a writable dispatch table, and Rustique can replace chosen entries with
   handlers generated from the same `bytecodes.c`, at zero cost for every other opcode (M).
   Measured whole runs: custom 0.952, frameworks 0.982, sqlglot 0.894 (instructions); CPython's
   full regression suite shows 0 differences with all 229 handlers swapped (M, §10).
4. **CPython-provided machinery** under custom and framework code (enum, namedtuple, dataclass-
   generated methods, `contextmanager`, `singledispatch`, `inspect`) is ≤ 1.3% each in
   framework apps but 2–40% of the custom programs that lean on it. Exact kernels: enum
   0.21–0.53 per call, namedtuple 0.22–0.65, frozen dataclass `__init__` 0.51–0.55; together
   0.962 on the custom family (M).
5. **Runtime**: deallocation 7.4%, steady GC 6.1% (max 49%), allocation 3.8% (M). An adaptive
   GC policy that defers full collections while they find nothing cuts GC work to 0.886
   (3.15.1) / 0.772 (3.15.1t), billing to 0.71, with peak RSS ≤ 1.03 and no longer pauses (M,
   dev box); it reports two API deviations, so it ships opt-in. `mimalloc` on the GIL build is
   slower (1.051, M); exit-time GC (40% of a whole pylint run) cannot be cut exactly.
6. **Free-threading**: CPU +17% on 3.15.1t (M). One CPython bug, `set.__reduce__` contending
   with its own critical section, is 70% of `isort` and 27% of `state_snapshot` there; an
   exact `ml_meth` kernel removes it (isort 0.26 with the copy group off, M). Threaded programs
   scale 2.2–6.4× at 8 threads on 3.15.1t and Rustique keeps 0.73–0.82 of their time (M);
   four kernel scaling cliffs and two framework thread-safety bugs were found.
7. **Startup, install, versions**: wave 1 misattributed its startup overage (the `.pth`
   loader string's compile is 32–44%, `dlopen` 22%); three fixes reach 1.026 / 1.027
   instructions (M). The opt-in native types are 60% of the extension and earn nothing on the
   default path. Older patch releases silently lose up to half the targets (3.15.0: 43/84) —
   fingerprints hash docstrings and compiler-version details (M).
8. **The developer journey**: 14 real projects' own suites (~53k outcomes) show 0 behaviour
   differences (M). But a shipped kernel breaks "never slower" (`list.sort` on ordered or
   string input, up to 1.84× instructions / 3.2× wall, M), the CI `diff` gave false alarms on
   6 of 14 suites, reports lose forked processes and never show suspensions, sdist installs on
   unvalidated platforms enable every target, and a prototype estimator predicts a program's
   gain within ±0.02 on 22 of 34 workloads.

## 8. Method (wave 2)

- **Ten Phase-1 slices in parallel** (one agent each, `docs/atlas/w2-<slice>.md`): workloads
  (`wl-frameworks`, `wl-custom`), free-threading (`threads`), layer attribution (`attribution`),
  the interpreter (`interp`), generated code and machinery (`machinery`), allocation and GC
  (`runtime`), startup/import/native types (`startup`), the 84 existing targets (`floors`), the
  developer journey (`dx`). **Phase 0** (`w2-phase0.md`) re-measured wave 1 on a quiet AWS
  c7a.8xlarge: workloads 0.877 / 0.843 (wave 1: 0.880 / 0.843), every workload within 2%,
  conformance 0 differences on 3.15.1/1t/2, 3.14.9/10, every result identical (M).
- **Attribution without distortion** (`benches/atlas/layers.py --modes bpf`): eBPF sampling on
  hardware cycles, the current Python frame read from `tstate` through `_Py_DebugOffsets`, the
  opcode handler from the frame-pointer chain (3.15's tail-calling interpreter compiles each
  opcode to its own `_TAIL_CALL_<OP>` function). Owner exact for 98–100% of samples, validated
  on four known cases (M). Wave 1's `-X perf` method installed a PEP 523 trampoline that
  disables inlined Python calls (1.06–1.4× slower processes, interpreter share +5 points).
- **Six layers** (shared vocabulary of all slices): own, generated, language, stdlib,
  thirdparty, runtime (definitions in `w2-attribution.md` §2.2).
- **Score**, as in §2: `I × F × (1 − K) × (1 − U) × (0.5 + R)`. For handler-swap candidates F
  folds in build reach (tail-calling builds only); for opt-in tiers the score is what users of
  the tier get.
- Dev-box wall times were noisy (load 14–70): every claim below a 1.3× effect rests on
  callgrind instruction counts or CPU time per call; quiet-box wall time comes in Phase 3.

## 9. Where the time goes (M, 3.15.1 steady state, % of cycles, `w2-attribution.md` §3)

| family | n | own | generated | language | stdlib | third-party | runtime | other |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| custom | 10 | 41.8 | 6.8 | 17.5 | 18.4 | 0.0 | 14.0 | 1.6 |
| frameworks | 6 | 1.7 | 1.0 | 20.2 | 21.4 | 35.7 | 16.7 | 3.3 |
| libs | 6 | 1.3 | 4.9 | 16.4 | 11.1 | 49.2 | 14.9 | 2.2 |
| services | 4 | 3.1 | 0.2 | 10.9 | 37.0 | 25.0 | 21.9 | 2.0 |
| threads | 5 | 16.8 | 0.5 | 12.4 | 50.4 | 0.0 | 18.4 | 1.6 |
| web | 10 | 3.9 | 0.6 | 11.6 | 47.8 | 19.4 | 14.3 | 2.4 |
| data | 12 | 6.9 | 0.8 | 9.6 | 50.5 | 10.9 | 20.4 | 1.0 |
| devtools | 12 | 0.1 | 1.0 | 13.5 | 18.0 | 33.3 | 32.2 | 1.8 |
| **all** | 65 | **10.1** | **2.1** | **13.8** | **32.0** | **20.5** | **19.7** | 1.9 |

3.15.1t has the same shape (all: 9.9 / 2.0 / 14.8 / 32.6 / 19.3 / 19.3); its costs differ (§7.6).

**Inside the interpreter** (M, `w2-attribution.md` §4, `w2-interp.md` §2): 90.1% of cycles
run under an opcode handler, 37.7% inside handlers themselves (custom 52%, frameworks 43%,
data 22%). Specialized forms carry 56% (inline plus callees), generic or non-specializing
forms 32%. The generic paths that end in C, where the specializer gave up:

| generic path | all 65 | frameworks | custom | why it stays generic (django_blog, M) |
|---|---:|---:|---:|---|
| `LOAD_ATTR` → `PyObject_GenericGetAttr` / `_PyObject_GetMethodStackRef` (+ `STORE_ATTR`, `getattr`/`hasattr`) | 5.5 | 11.6 | 4.3 | polymorphic sites 31%, methods on polymorphic or dict-materialized instances 25%, `cached_property` 10.6%, class attributes 5.7%, `__getattribute__` 3.9% |
| C→Python call machinery (`type_call` with kwargs, `f(*a, **k)`, `__call__`, `partial`) | ~2.5 | 4.1 | | +155 to +1,331 Ir per call over the inlined form |
| `CALL_ISINSTANCE` | 1.6 (sqlglot 11.4) | 0.9 | 0.7 | negative checks look up `x.__class__` (155 of 347 Ir); metaclasses build a bound `__instancecheck__` |
| `object.__setattr__` (frozen dataclasses) | 0.4 | | 2.2 (order_domain 6.5) | `wrapper_descriptor` has no vectorcall |

The `CALL_*_NON_PY` handlers (10.2% + 5.4%) are dominated by the C callee's own work (zlib,
`_json`, `_sre`, `pydantic_core`), not dispatch.

**Third-party code** (M, `w2-attribution.md` §5): in the 35 programs that use it, it is the
innermost owner of 71% of cycles, and half of that is CPython's work on its behalf: language
operations 7.4 points (attribute lookup 2.5, dict 1.6), stdlib Python 5.8, C stdlib 2.9,
runtime 7.2 (allocation 4.2, GC 3.0). That is what Rustique can reach for third-party code.

**What wave 1 left** (M, `w2-attribution.md` §6): with all 15 groups on, 4.8% of time remains in
wave-1 modules (12.1% before): logging handlers (53% of `logging_service`), `glob`/`os.scandir`
(54% of `fs_inventory`), dataclasses in isort (16%), json with `default=` (10% of `json_api`),
zoneinfo arithmetic (17% of `tz_events`).

**Whole processes** (M): interpreter startup 1.3%, workload imports 0.8%, setup 14.1%,
finalization 6.0% — but exit-time GC is 40.6% of a whole `pylint` run and 16–21% of pytest,
Sphinx, SQLAlchemy and FastAPI runs. Real programs' import time (12 programs, M,
`w2-startup.md` §3): class machinery 22%, unmarshal 16%, importlib 13%, compiling generated
source 8.4%, `re.compile` 7.5%.

## 10. What can be reached, and how (per layer)

| layer | pool | least invasive mechanism | verdict (evidence) |
|---|---|---|---|
| own / third-party bytecode | specialized handler self time (≈ 37%) | none beats CPython's BOLT-laid-out handler on its own fast path | **floor** (our generated copies 0.96–1.00, `w2-interp.md` §3) |
| | generic paths (≈ 10%) | **M2 opcode-handler swap** on tail-calling builds (python-build-standalone / uv: Linux x86_64 measured, aarch64/macOS expected) | **pursue as opt-in tier**: isinstance 0.30–0.60, attribute cache 0.57–0.80, `object.__setattr__` 0.40 per op; whole runs custom 0.952, frameworks 0.982; full regrtest 0 differences (swap-all), two override bugs caught by it and fixed |
| | the same on GCC/computed-goto builds (Debian, Fedora, Docker, pyenv, Homebrew, conda) | M1: a generated copy of the loop as a 3.15 PEP 523 hook that keeps specialization | **research only**: 0.98–0.99 per call but 1.02–1.04 on whole programs (no BOLT/LTO inlining) |
| | | CPython's own JIT as a policy | **no**: slower on 5 of 6 workloads (cycles 1.03–1.35, `w2-interp.md` §3.4) |
| | | vectorcall "Rust JIT" of user functions | **no**: caller penalty, no exact deopt story, tiny share |
| generated | dataclass / namedtuple / attrs-style generated functions | vectorcall on the *generated* function objects, recognised by re-running CPython's generator and comparing code (lazy, ≤ 1.2% of import) | **pursue**: namedtuple 0.22–0.65, frozen dataclass (keyword calls) 0.51; positional `__init__` calls bypass vectorcall (`CALL_ALLOC_AND_ENTER_INIT`), reached only by M2 |
| | enum machinery | slot swaps on stdlib heap types (`enum.property.tp_descr_get`, `EnumType.tp_call`) + vectorcall on functions reached only from C slots (fallback +2–3 Ir) | **pursue**: 0.21–0.53 per call, 39% PyPI reach |
| language | str/dict/list/formatting/getattr builtins | already C; Rust re-implementations of C gain ≤ 1.1× at real sizes (wave 1) | **settled**; cache-based fast paths only (M2, or `getattr`/`hasattr` `ml_meth` with the attribute cache) |
| stdlib | remaining wave-1 gaps, new Python modules | wave 1's entry-point kernels (`ml_meth`, slots, vectorcall) | **pursue**: gaps first (json `default=`, logging, paths, copy of UUID/PurePath), then `contextlib`/`singledispatch` bookkeeping, `re`, `inspect` |
| third-party | their bytecode | not a target by contract (fingerprints would churn) | reached through the language/stdlib/runtime they call (half their time) |
| runtime: GC | full collections that find nothing (1.6% mean, 1.9% on t; billing 34%) | opt-in policy on public `gc` APIs, steps aside when the program tunes the GC | **pursue as opt-in tier** (`w2-runtime.md`) |
| runtime: exit | exit-time collections (pylint 40%) | none exact: every process has finalizable objects and weakref callbacks at exit; "fast exit" loses finalization of cyclic garbage | **settled: not shipped** (changes semantics, not just timing) |
| runtime: allocation | allocator choice, freelists | `PYTHONMALLOC=mimalloc` | **settled no** (1.051, worst 1.30, RSS +7.5%); Rustique's kernels already allocate 10–64% less |
| runtime: startup | Rustique's own `RUSTIQUE=1` cost | lean init, top-level extension import, no native types | **pursue** (1.026 / 1.027) |
| runtime: import | `re.compile` at import (7.5%), generated-source compile (8.4%) | a Rust `re` parser/compiler emitting exactly `_sre.compile`'s arguments; a persistent code cache | **`re`: pursue**; code cache: **no** (disk writes are side effects) |
| runtime: free-threading | `set.__reduce__` self-contention; 3.15t `isinstance(x, ABC)` serialization | `ml_meth` kernels | **pursue** (`set.__reduce__` now; ABC cache later) |

## 11. Candidates scored (wave 2)

`s` and F/K/U/R are the owning slice's estimates, normalized here; shares are means over the 65
workloads (max). "Mandatory" items are never-slower or safety fixes and are not ranked by score.

| # | candidate | layer | mechanism | share mean / max | s | I | F | K | U | R | score | evidence |
|---:|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `set/frozenset.__reduce__` (3.15.1t only) | runtime:ft | `ml_meth` | t: 1.5 / 70 (isort) | 50 | 1.5 | 1.0 | 0.1 | 0.3 | 0.3 | **0.76** | w2-threads §1–2 |
| 2 | adaptive GC tier (opt-in) | runtime:gc | policy on public `gc` APIs + `ml_meth` swaps of `gc` setters | 1.6 / 34 (billing) taken; t 1.9 | — | 0.5 (t 1.0) | 0.8 | 0.3 | 0 | 1.0 | **0.42** (t 0.84) | w2-runtime §3–4 |
| 3 | interp: megamorphic attribute cache | language | M2: `LOAD_ATTR` family | 3.7 / 13.8 (drf) | 1.6 | 1.39 | 0.45 | 0.4 | 0.35 | 0.9 | **0.34** | w2-interp §4.2 |
| 4 | interp: `isinstance` | language | M2: `CALL_ISINSTANCE` | 1.6 / 11.4 (sqlglot) | 2 | 0.8 | 0.6 | 0.3 | 0.5 | 0.9 | **0.24** | w2-interp §4.1 |
| 5 | interp: C→Python frame pushes | language | M2: `CALL_KW_NON_PY`, `CALL_EX_*`, `CALL_NON_PY_GENERAL` | 2.5 / 4.1 (frameworks) | 1.8 | 1.1 | 0.35 | 0.4 | 0.35 | 0.8 | **0.20** | w2-interp §4.5 |
| 6 | `getattr`/`hasattr` with the attribute cache (all builds) | language | `ml_meth` | 1.2 / 16 (mini_interp) | 1.4 | 0.34 | 0.7 | 0.2 | 0.2 | 0.8 | **0.20** | w2-interp §9 |
| 7 | enum group | generated | heap-type slots + slot-reached vectorcall | 0.6 / 5.0 (rules_engine) | 3 | 0.4 | 0.8 | 0.25 | 0.1 | 0.39 | **0.19** | w2-machinery §4 |
| 8 | json: lazy `default=`, `make_encoder` route, UUID/PurePath in copy/asdict | stdlib | existing json/copy sites | 0.6 / 18 (logging_service) | 2 | 0.3 | 0.9 | 0.2 | 0 | 0.32 | **0.18** | w2-floors §2 |
| 9 | logging remainder: handler/format path, `findCaller` cache, FT format-cache lock | stdlib | existing logging sites | 1.2 / 53 (logging_service) | 1.5 | 0.4 | 0.6 | 0.35 | 0 | 0.40 | **0.14** | w2-floors §1, w2-threads §4 |
| 10 | `xml.etree` serializer | stdlib | vectorcall | 0.8 / 50 (xml_feeds) | 6 | 0.67 | 0.7 | 0.3 | 0.2 | 0.05 | **0.14** | wave-1 atlas #17, w2-attribution §3 |
| 11 | namedtuple generated methods | generated | vectorcall on generated functions | 0.4 / 4.1 (pylint `TokenInfo._make`) | 2 | 0.2 | 0.8 | 0.2 | 0 | 0.35 | **0.11** | w2-machinery §3 |
| 12 | paths remainder: `glob`/`scandir` helpers, `relpath` | stdlib | vectorcall | 0.8 / 54 (fs_inventory) | 1.5 | 0.27 | 0.6 | 0.3 | 0 | 0.49 | **0.11** | w2-attribution §6 |
| 13 | email/http header parsing | stdlib | vectorcall | 1.2 / 59 (email_mime) | 3 | 0.8 | 0.3 | 0.5 | 0 | 0.30 | **0.10** | wave-1 atlas #5 |
| 14 | dataclass generated code (frozen `__init__`, `__eq__`) | generated | vectorcall (+ M2 `object.__setattr__` for positional calls) | 1.0 / 16 (order_domain) | 1.5 | 0.3 | 0.5 | 0.3 | 0.2 | 0.29 | **0.07** | w2-machinery §3, w2-interp §4.3 |
| 15 | `inspect.signature` / `getfullargspec` | stdlib | vectorcall | 0.6 / 31 (di_plugins) | 3 | 0.4 | 0.4 | 0.4 | 0 | 0.19 | **0.07** | w2-wl-custom §5 |
| 16 | `contextmanager` construction, `singledispatch` dispatch cache | stdlib | vectorcall (ruling 15 leaves only the bookkeeping) | 1.2 / 40 (event_ledger) | 1.1–1.3 | 0.1 | 0.7 | 0.3 | 0 | 0.4 | **0.05** | w2-machinery §5 |
| 17 | weakref containers (`WeakValueDictionary.get`) | stdlib | vectorcall | 0.2 / 6.4 (orm_mapper) | 2 | 0.1 | 0.7 | 0.3 | 0 | 0.23 | **0.04** | w2-wl-custom §6 |
| 18 | `re` module functions + Rust `re` compiler | stdlib + runtime:import | vectorcall on `re.match`…, `re._compiler.compile` | steady 0.5; **startup**: 7.5% of import time (42% of `import yaml`) | 1.4 / 5 | 0.14 steady; whole-process of short CLIs ≈ 1.2 (I) | 0.6 | 0.3 | 0 | 0.49 | **0.06** steady / **0.50** whole-process (I) | w2-startup §3, w2-machinery §6 |
| — | never-slower and safety fixes | — | — | — | — | — | — | — | — | — | **mandatory** | §13 |

## 12. Floors: what is settled, and why

- **The 84 wave-1 targets** (`w2-floors.md` §1): 48 settled (at or near the floor of result
  allocation and C-call overhead, or too small a share), 36 open; the open remainder is Rust
  `Vec` traffic in textwrap/urllib/`relpath`, format strings re-parsed per call in
  `strptime`/`strftime`, and `findCaller`'s line-number decoding (79% of its time).
- **Settled, with the reason** (M unless tagged):
  - specialized handler self time: no entry point; our copies reach 0.96–1.00 at best;
  - the classic PEP 523 hook (4.5–6.8× per call) and M1 as a default (1.02–1.04 whole);
  - CPython's JIT as a policy (slower on 5 of 6), a vectorcall "Rust JIT" (I);
  - language operations that are already C (str methods, dict/list ops, formatting): ≤ 1.1×
    at real input sizes (wave 1);
  - allocator switching (`mimalloc` 1.051 on the GIL build), static GC thresholds and
    freeze-based policies (memory up to 15×, hidden objects), fast exit (finalization lost);
  - `key=` sorts, `sum`/`min`/`max` over generators, user `__deepcopy__` restarts (ruling 15),
    `html.parser` handlers (user callbacks), exceptions and generators (≤ 0.8%);
  - class needles in `in` (0.33–0.54 per call, but 0.26% of pyflakes) and tuple needles
    (black, ~0.8%): cheap to keep, not a priority;
  - a persistent code cache for generated source (writes to disk are side effects).
- **Upstream items** (documented, not filed this round): `set.__reduce__`'s nested critical
  section (3.14t–3.16a1t), `isinstance`'s `__class__` lookup on negative checks, the
  `utf-8-sig` decode that every `.pth` file costs at startup (0.61–0.66M instructions), a
  vectorcall for `wrapper_descriptor` (frozen dataclasses), and remote-frame sampling crashing
  3.15.1t.

## 13. Where the developer journey breaks (`w2-dx.md`, `w2-floors.md` §5, `w2-threads.md`, `w2-startup.md`)

| step | break (M unless tagged) | severity | fix |
|---|---|---|---|
| never slower | `list.sort`/`sorted` up to 1.84× instructions / 3.2× wall on ordered and short-string input (22 of 90 cells > 1.05) | **high** | run detection + hand-offs (prototype: 90/90 ≤ 1.05) |
| never slower | unconfirmed: sqlglot's `deepcopy` calls replayed on 3.15.1t 4.4× slower enabled (0.57 on GIL) | **check** | measure first in Phase 2 |
| threads | `enable()` raises `PanicException` when another thread mutates a pinned module dict (3.15.1t, 1 in 834); two latent lock inversions; suspension decided only on the main thread | **high** | accel hardening before any class-attribute pin |
| install | sdist on unvalidated platforms (s390x, ppc64le, armv7, FreeBSD) enables every target; maturin silently downloads a 1.4 GB toolchain | **high** (I) | platform allowlist in the process gate, `--only-binary` guidance |
| versions | older patch releases lose 12–41 targets silently (3.15.0: 43/84); 3.16 tests fail instead of skipping on version refusals | medium | fingerprint v3, recorded sets per patch, refusal `unvalidated-patch`, suite skips on refusal |
| startup | `RUSTIQUE=1` 1.045 / 1.063 instructions, 1.08–1.11 wall | medium | engine fixes (1.026 / 1.027 measured) |
| trust | `python -m rustique diff` reported DIFFERENT on 6 of 14 suites, all false | medium | fixed on `w2/p1-dx` (volatile-output rule) |
| visibility | reports lose fork-pool and worker processes, double-count forked children, never show suspensions or call sites | medium | per-process counters, zeroed after fork; suspensions in the report |
| runner | `python -m rustique app.py` added traceback frames, a relative `__file__`, no child acceleration | fixed | `w2/p1-dx` |
| platforms | Windows 30 targets, macOS/musl 74 | medium | patches to 48 / 77 + a per-platform CI job |
| "will it help me?" | no answer before trying | — | `python -m rustique estimate` (median error 0.007 on 34 workloads; disabled on 3.15.1t) |

## 14. Decisions for wave 2

### 14.1 The highest-impact targets

1. **The interpreter's generic paths through an opt-in `interp` tier** (M2): attribute cache,
   `isinstance`, `object.__setattr__`, C→Python frame pushes — the only lever on framework apps
   and custom code beyond machinery kernels (#3–5).
2. **Runtime policy through an opt-in `gc` tier** (#2): GC-heavy programs (billing, pylint,
   pyflakes, black, xml) and the free-threaded build, where every collection walks the heap.
3. **Free-threading**: `set.__reduce__` (#1) and the kernel scaling cliffs.
4. **The data-model machinery custom code leans on**: enum, namedtuple, dataclass-generated
   code, `contextmanager`/`singledispatch` bookkeeping, `inspect` (#7, #11, #14–16).
5. **Closing wave 1's gaps** where real workloads still spend time: json, logging, paths,
   copy (#8, #9, #12).

### 14.2 The best places to start

- **Never-slower and safety first**: the sort fix, the free-threading hardening of the
  framework, the `deepcopy` check on 3.15.1t, the platform allowlist.
- **Foundations that unblock everything else**: retire the opt-in native types (the
  extension shrinks from 6.8 to 2.3 MB, test time −68%), validate each target on its first
  call (startup and install budgets at once), fingerprint v3 with recorded patch releases,
  cost-weighted credits.
- **Low-risk wins that prove a new layer**: `isinstance` and `object.__setattr__` as the first
  M2 overrides (small, exact, measurable); enum as the first heap-type-slot group.

### 14.3 Phase-2 workstreams (one worktree and agent each; branch `ws2/<name>`, base `wave2`)

| workstream | scope | owns |
|---|---|---|
| `engine` | retire the opt-in native types cleanly; lean extension init; top-level `_rustique` (wheel repack in CI, in-package fallback); every startup cell ≤ 1.05 | `crates/rustique-py/src/lib.rs` and the native-tier modules, `crates/rustique-api`, `examples/`, `python/rustique/__init__.py` API, `pyproject.toml`, `.pth` |
| `accel` | free-threading hardening; validate on first call; fingerprint v3 + recorded patch sets + `unvalidated-patch`; platform allowlist; credits v2 (cost-weighted, deopt split, suspensions visible); fork-safe per-process counters; site support for stdlib heap-type slots; ACCELERATION amendments | `crates/rustique-py/src/accel/` |
| `core` | sort fix; `set/frozenset.__reduce__`; json lazy `default=` + `make_encoder` route; UUID/PurePath native copying; needles; shared-list scaling; the `deepcopy` t check | `targets/{containers,builtins,json,copy,dataclasses,float}`, new `targets/set` |
| `text` | logging remainder (+ FT lock), datetime format plans, allocation-free textwrap/urllib/`relpath`, `glob`/`scandir` helpers, `random` variates | `targets/{logging,datetime,textwrap,urllib,posixpath,pathlib,fnmatch,random,statistics}` |
| `datamodel` | enum group; namedtuple and dataclass generated code; `contextmanager`/`singledispatch` bookkeeping; `inspect.signature` (if a prototype meets the bar); adoption of classes created before `enable()` | new `targets/{enum,generated,functools,contextlib,inspect}` |
| `re` | a Rust `re` parser/compiler producing exactly `_sre.compile`'s arguments; `re` module-function fast path | new `targets/re`, `rustique-core/src/re` |
| `interp` | the M2 `interp` tier (opt-in): per-patch generated handlers, build verification and ABI self-test, overrides (isinstance, attribute cache, `object.__setattr__`, keyword instantiation), FT-safe caches, full-regrtest gate | new `crates/rustique-interp` (C + Rust), `targets/interp` |
| `gc` | the adaptive GC tier (opt-in), in Rust, stepping aside on user tuning, its eligibility tests | new `targets/gc` |
| `dx` | ship the `diff`/runner fixes; reports (per-process, suspensions); `estimate`; wheel repack with `engine`; platform patches + per-platform CI; refusal UX and suite skips; opt-in tier switches; docs | `python/rustique/` CLI/report/diff, `.github/`, `docs/USAGE.md` |

### 14.4 Measurable targets for wave 2 (Phase 3, quiet c7a.8xlarge; default tier unless stated)

Baselines: wave 1's 34 workloads 0.877 / 0.843 (Phase 0, M); the 31 new workloads with wave-1
code ≈ 0.98 (frameworks), 0.95 (libs), 0.85 (services), 0.98 (custom), 0.82 (threads on t at
8 threads) (dev-box instruction counts, I).

| # | target | metric |
|---|---|---|
| T1 | never slower | no workload > 1.02; no measured cell > 1.05 (small inputs, fallbacks, sort, install, startup, 8-thread CPU per call) |
| T2 | wave 1's 34 workloads | ≤ 0.85 (3.15.1), ≤ 0.80 (3.15.1t) |
| T3 | all 65 workloads | ≤ 0.88 (3.15.1), ≤ 0.84 (3.15.1t) |
| T4 | per family (3.15.1) | custom ≤ 0.95, frameworks ≤ 0.98, libs ≤ 0.95, services ≤ 0.85; threads on 3.15.1t at 8 threads ≤ 0.78 |
| T5 | `interp` tier (opt-in, PBS 3.15.1) | frameworks ≤ 0.96, custom ≤ 0.90; CPython's full regression suite 0 differences (GIL and t) |
| T6 | `gc` tier (opt-in) | workloads with GC > 10%: geometric mean ≤ 0.87; peak RSS ≤ 1.05×; max pause ≤ default; others ≤ 1.02 |
| T7 | free-threading | isort / state_snapshot on 3.15.1t with the copy group off ≤ 0.35 / 0.75 |
| T8 | startup | `RUSTIQUE=1` vs unset ≤ 1.05 instructions and wall on 3.14.9 and 3.15.1 (except the documented upstream `.pth` floor in bare 3.14 venvs); extension ≤ 2.5 MB |
| T9 | install | every lazy install ≤ max(5% of the module's import, 0.1M instructions) |
| T10 | reach | ≥ 80 targets on every recorded 3.15.x and ≥ 75 on every 3.14.x patch release; Windows ≥ 48, macOS/musl ≥ 77; unvalidated platforms and patches refuse visibly |
| T11 | correctness | CPython's test modules of every group: 0 differences on 4 builds; 14 real-project suites: 0; Hypothesis differential suites for every new target |
| T12 | pyperformance | ≤ 0.93 (3.15.1), ≤ 0.94 (3.15.1t) |
| T13 | "will it help me?" | `estimate` within ±0.03 of the measured ratio on ≥ 80% of the 65 workloads |

### 14.5 Not in wave 2 (ranked next)

1. email/http header parsing (#13), `xml.etree` serializer (#10).
2. `inspect.signature` and weakref containers if `datamodel` cannot meet the bar this wave.
3. The `interp` tier on GCC builds (M1 needs BOLT-class layout to reach 1.00), and as the
   default on verified builds once its gates have held for a release.
4. The `isinstance(x, ABC)` positive cache for 3.15t threads; `functools.cache` hits under
   threads.
5. The upstream items of §12.

## 15. Wave 2's outcome against this atlas (Phase 3, `docs/atlas/w2-phase3.md`)

Measured on the quiet c7a.8xlarge, default tier unless stated; the full target-by-target verdict
is DECISIONS §11.

| what the atlas said (§7, §11, §14) | what wave 2 measured | |
|---|---|---|
| stdlib is the largest reachable layer (32%); closing wave 1's gaps in json, logging, paths, copy (#8, #9, #12) | wave 1's 34 programs 0.877 → **0.858** (3.15.1), 0.843 → **0.826** (3.15.1t); `logging_service` 0.813 → 0.686, `json_api` 0.835 → 0.717, `report_render` 0.438 → 0.357 | as predicted; T2 (0.85 / 0.80) missed by 0.008 / 0.026 |
| `set.__reduce__` on 3.15.1t (#1, score 0.76) | `isort_stdlib` 0.201 with everything on, 0.260 with the copy group off (CPython: 4.0× slower than its GIL build) | as predicted (T7 met) |
| machinery under custom code (#7, #11, #14, #16): 2–40% of the custom programs that lean on it | custom family **0.936** by default (`rules_engine` 0.845, `report_engine` 0.843); framework family **0.940** (falcon 0.795) | better than the scored estimates on frameworks; custom about as predicted |
| the `interp` tier (#3–5): whole runs custom 0.952, frameworks 0.982 in instructions (§7.3) | on top of the default tier: frameworks **0.933**, custom **0.920** against plain (0.986 / 0.987 of the default tier) | as predicted in size; T5's custom ≤ 0.90 missed; stays opt-in (its own gate, whole runs ≤ 1.00, not met) |
| the `gc` tier (#2): 0.42 (GIL) / 0.84 (t) score, "GC-heavy programs ≤ 0.87" (T6) | GC-heavy set **0.982 / 0.952**, RSS ≤ 1.04, pause bound in objects held; pause milliseconds 1.30 once | **over-estimated**: most full collections in these programs find real garbage, and only near-zero-yield ones may be deferred exactly |
| `re` (#18): small steady share, large on startup of short CLIs (I) | `regex_compile` 0.044, `import yaml` 0.61, `import_startup` whole process 1.022 (wave 1) → 0.990 | as predicted |
| third-party-heavy code: 36–49% third-party, little reachable by default | libs **0.961** (attrs/cattrs 0.847 through `copy`/`dataclasses`; pydantic, jsonschema, PyYAML 0.99–1.00) | as predicted; T4's 0.95 missed |
| threaded programs: kernel cliffs and the FT penalty | threads family at 8 threads 0.685 / **0.792**; every kernel ≤ 1.05× CPU per call at 8 threads | T4's 0.78 on t missed by 0.012 |
| pyperformance (T12 ≤ 0.93 / 0.94) | **0.911 / 0.936** | met |
| the whole 65 (T3 ≤ 0.88 / 0.84) | **0.870 / 0.872** | met on 3.15.1; on 3.15.1t the new families gain less (the 31 new: 0.925) |

What the atlas missed:

- **A new never-slower case in a wave-1 kernel**: the wave-2 sort strategy is slower than
  timsort on long nearly ordered runs (a 1M-element set of numbers, 1.23–1.28; memory traffic,
  not instructions). The 0–100 scoreboard and the 10k cells did not show it; only the 1M
  wall-time board did. Fixed before the merge (0.714–0.746 on the final code). Large-input
  cells of every kernel whose algorithm changes now belong in the workstream's own gate.
- **Layout effects at the 5–8% level** (pyperformance `coroutines` on 3.15.1t) that no
  group causes alone: the never-slower bar on whole benchmarks needs a layout control
  (enabled ÷ imported) before a cell is attributed.
- **The estimator** under-counts targets that validate at their first call (69% within ±0.03
  on 3.15.1, 80% on 3.15.1t; T13).

Next waves, ranked: DECISIONS §17.
