# Atlas: headroom and tractability — objects, time, paths, logging and other utilities

Phase 1 research, 2026-11-01, CPython 3.15.1 (python-build-standalone, PGO+LTO+BOLT) unless
stated. Two questions per candidate: **how much room is there** for an exact Rust drop-in
(compared with the best existing alternatives), and **how tractable** is that drop-in?
Text and serialization (json, re, str/bytes, number formatting, csv, html, urllib.parse,
tomllib, xml) are covered in `headroom-text.md`. Entry-point mechanics are covered in
`mechanisms.md`, usage frequencies and upstream plans in `static-usage.md`, and real
workloads in `workloads-*.md`.

Ratios are *candidate time ÷ stdlib time* (lower is faster). Tags: **M** = measured (method
given), **I** = inferred (basis given), **P** = measured on a throwaway prototype.

## Key findings

1. **Room is large only where the stdlib runs interpreted Python.** Already-C paths have
   none: `datetime` arithmetic (33–57 ns), `zoneinfo`, `lru_cache` (42 ns), `partial`,
   `deque`/`defaultdict`/`OrderedDict`, `heapq`/`bisect`, `struct`, `hashlib` (OpenSSL is
   already the fastest backend here: HACL* `_sha2` is 1.30× slower), `bz2`/`lzma` (same libraries), `decimal`, `base64`
   (rewritten in C in 3.15), and every builtin-type operation in the brief **M**. The
   pure-Python paths, by contrast, run 5–60× slower than a native equivalent **M**:

   | path | stdlib | native equivalent | ratio |
   |---|---:|---|---:|
   | `datetime.strptime` | 3.7 µs | ciso8601 | 0.014–0.02 |
   | `copy.deepcopy` (records) | | copium | 0.11–0.18 |
   | `statistics.mean`/`stdev` | 414/706 µs per 1000 floats | fmean | 0.02 |
   | `difflib` ratio | 46 µs | cydifflib | 0.19 |
   | `dataclasses.asdict`/`replace` | | msgspec | 0.09–0.11 |
   | `queue.Queue` put+get | 927 ns | SimpleQueue | 0.085 |
   | `uuid4` | 866 ns | uuid_utils | 0.074 (not exact: own RNG) |
   | asyncio loop | | uvloop | 0.45–0.76 |
2. **Entry points.** Two of the best targets sit behind C entry points, where the existing
   `ml_meth` swap applies with free fallback: `datetime.strptime`/`time.strptime`, which call
   the Python `_strptime`, and `datetime.strftime`/`isoformat`. Everything else is a
   pure-Python function or method, reached by `PyFunction_SetVectorcall` on the existing
   object (`mechanisms.md` kind C). The mechanism was re-measured independently on a Rust
   prototype (**P**, callgrind):
   - it keeps identity and every captured reference;
   - fallback costs **+180–250 Ir** per direct call (the caller loses the inlined frame
     push), which is 1.03–1.11 on 2–8k-Ir functions and 1.12 on recursive `deepcopy`;
   - fallback is **~free (1.005)** for dunders reached through slots (`str(uuid)`,
     `hash(enum_member)`).
3. **Prototype gains.** Native `posixpath.join`/`splitext`/`basename`, untuned, run at
   0.28–0.46 of the stdlib's instructions **P**. A code-prologue shim is worse on both paths:
   native 0.63, fallback 1.11 **P**.
4. **Tractability is about hooks, not algorithms.** The algorithms are small. The work is in:
   - subclass extension points: `PurePath.with_segments`/`parser`, `LogRecord` factory,
     `Formatter.converter`, `Enum._missing_`, `Random` subclasses, `Queue._put`;
   - observable intermediate calls: `__deepcopy__`, `__reduce_ex__`, memo contents;
   - locale and `time.tzname` (`strptime`);
   - generator *types* (`os.walk`, `heapq.merge` and `unified_diff` return generators, and
     a native iterator would change `type()`);
   - version churn: error messages change between 3.15.1 and 3.16.0a1 (`_strptime`).

   copium (C) already passes **81/84** of CPython's `test_copy` as a drop-in. It fails only
   on 3.15's new `frozendict` and `copy.__all__`/`replace` **M**, so an exact `deepcopy` is
   tractable.
5. **Upstream erodes two targets on 3.16** (`static-usage.md` §4, verified there with `gh`):
   C `uuid` (`uuid4` 15×) and a C `strptime` for numeric formats are merged for 3.16.0
   (Oct 2027). The 3.16.0a1 binary predates both: `uuid.py` is identical to 3.15.1 **M**.
   `deepcopy` has no upstream overlap (C `deepcopy` rejected), and neither do pathlib,
   logging records, `contextmanager` or enum.
6. **Recommended order** (§4):
   1. `strptime`/`strftime`/`isoformat`;
   2. `copy.deepcopy`/`copy.copy`;
   3. the logging record path;
   4. pathlib `PurePath` core;
   5. `http.client.parse_headers` (email `feedparser`): 27 µs per response, and 39.7% of
      `requests_local` with email+http+urllib (`workloads-web.md`);
   6. `contextlib.contextmanager`;
   7. enum dunders.

   Then `uuid` (a 3.14/3.15-only window), `random` glue, `statistics`, `difflib`,
   `ipaddress`, `queue`. Not worth an exact drop-in: asyncio internals, traceback,
   pickle, compression (except one-shot decompress/crc32), importlib, argparse.
7. **Tooling note for the other agents.** On this machine's perf 6.18, leaf sample IPs from
   the BOLTed `python3.15` are already virtual addresses. Adding (0x1800000 − 0x1569000), as
   the brief prescribes, mis-symbolizes them **M**. Validation on a `json.dumps` loop:
   - as-is gives `encoder_listencode_obj`, `_Py_dg_dtoa`, `long_to_decimal_string_internal`;
   - with the delta it gives `bytes_splitlines`, `_PyInterpreterConfig_AsDict`, `_mpd_qmul`.

## 1. Method

- **Wall time** (`bench/hb.py`):
  - each cell autoranges to ≈0.12 s per sample;
  - 5–7 repeats interleave stdlib and alternatives in one process;
  - ratio = alt ÷ stdlib per pair, reported as median [min–max];
  - shared WSL2 box: 8 CPUs, load ≈3 from other agents, ±1–45% per-cell spread.

  Only ratios ≤ 0.67 or ≥ 1.5 are claimed from wall time.
- **Instruction counts** (`bench/cg.py`, `cg_pass.py`, `cg_prologue.py`): per-call Ir =
  (Ir(N) − Ir(0)) / N, using whole-process callgrind in fresh processes with N = 10–20k.
  Used for every small effect (fallback costs, prototype kernels).
- **Where time goes**: `perf record -e cpu-clock -F 1999 -g` with `python -X perf` on a
  1–1.5 s hot loop per candidate (`bench/hot.py`). Samples are classified by innermost
  Python frame and by leaf native component (`tools/perfsym.py`; brief's component
  prefixes). cProfile is used where the call tree matters.
- **Alternatives**: venv `~/rustique-research/h-objects/venv`. Wheels or source builds:
  - copium 0.1.0, ciso8601 2.3.3, cydifflib 1.2.0, cdifflib and zlib-ng 1.0.0 (zlib-ng 2.2.5)
    were built from source on 3.15;
  - uuid_utils 1.2.0, whenever, pendulum, dateutil, rapidfuzz, quicktions, gmpy2, deflate
    (libdeflate), cramjam, orjson, ormsgpack, msgspec, attrs, cachetools, uvloop, numpy,
    wcmatch and netaddr came as wheels;
  - fastuuid and isal only on 3.14 (`venv314`);
  - picologging did not build: it uses removed private `_PyTime_*` APIs.
- **Tests**: `python3.15 -m test --list-cases <module>` with `Lib/test` from
  `~/src/cpython-3.15.1`.

## 2. The pure-Python entry point, measured (prototype)

`proto/fnswap` is a Rust cdylib on `pyo3-ffi` 0.30.1. It swaps a function object's vectorcall
(`PyFunction_SetVectorcall`) for a kernel with a guard, and falls back through
`_PyFunction_Vectorcall`. The differential test checked 3,645 `join`/`splitext`/`basename`
argument tuples: 0 mismatches, result identity kept (`join('a', x) is x`). Callgrind, Ir
per call **P**:

| call | stdlib | pass-through (= fallback cost) | native kernel |
|---|---:|---:|---:|
| `posixpath.join(a, 'b.py')` | 3,813 | 3,991 (1.047) | 1,746 (**0.458**) |
| `posixpath.join(a, 'b', 'c', 'd.py')` | 7,537 | 7,756 (1.029) | 2,104 (**0.279**) |
| `posixpath.splitext(p)` | 4,101 | 4,347 (1.060) | 1,427 (**0.348**) |
| `posixpath.basename(p)` | 2,271 | 2,511 (1.106) | 925 (**0.407**) |
| bytes `join` / non-ASCII `splitext` (guard miss) | 3,809 / 4,236 | — | 4,027 (1.057) / 4,585 (1.082) |
| `def f(x): return x` / with kwarg | 431 / 607 | 649 (1.51) / 876 (1.44) | — |
| `str(PurePosixPath / 'src' / 'mod.py')` (only `join` swapped) | 39,190 | 39,229 (1.001) | 35,249 (0.899) |
| `copy.deepcopy(3-key dict)` (recursive) | 25,625 | 28,813 (**1.124**) | — |
| `str(uuid)` (`__str__` via `slot_tp_str`) | 5,971 | 6,004 (**1.006**) | — |
| `d[enum_member]` (`__hash__` via slot) | 1,197 | 1,203 (**1.005**) | — |
| `logger.debug()` disabled (bound method, direct) | 1,313 | 1,562 (**1.190**) | — |
| `p.with_suffix('.txt')` | 12,998 | 13,249 (1.019) | — |
| code-prologue variant: `join` with `_try` + original body, hit / bytes miss | — | 4,235 (1.11) | 2,403 (0.63) |

Rules that follow (**P**, agreeing with `mechanisms.md` §1.2):

- **Direct calls pay about 200 Ir on fallback.**
  - Never-slower holds for targets whose call costs ≥ ~4.5k Ir (≈1.2 µs).
  - Smaller targets must take the native path on nearly every input: handle bytes and
    every PEP 393 kind natively, and leave only PathLike, subclasses and errors to the
    fallback.
- **Recursive targets pay the fallback on every level** (`deepcopy` 1.12). The kernel must
  cover objects too, through the reduce protocol, and never fall back wholesale on a
  common type.
- **Dunders reached through type slots fall back for free.** This covers `__str__`,
  `__hash__`, `__eq__`, `__lt__`, `__format__`, `__getitem__` and `__contains__` on
  pure-Python classes (UUID, Enum, PurePath, Fraction, IPv4Address). The exceptions are
  `__init__`, which `CALL_ALLOC_AND_ENTER_INIT` inlines, properties (`LOAD_ATTR_PROPERTY`)
  and `__enter__`/`__exit__`, which are called as bound methods.
- **A tuned kernel would be cheaper still.** In the prototype, `vc_join` spends 308 Ir in
  the kernel itself: a Rust `Vec` grown by `realloc`, plus `PyUnicode_DATA` and an ASCII
  scan made out of line. The rest is the generic `CALL` and allocation. A single
  `PyUnicode_New` with calibrated inline access (as `takeover.rs` already does) should take
  `join` to ≈0.2–0.25 **I**.
- **Generators cannot be swapped exactly.** `os.walk`, `heapq.merge`, `difflib.unified_diff`,
  `Path.glob`/`rglob`/`iterdir` return generator objects, and `type()`, `gi_frame`,
  `send`/`throw` and `inspect.isgenerator` would change. Accelerate the helpers they call,
  never the generator.
- **Per-version fingerprint.** Native code reimplements one CPython version's Python source,
  and 3.16.0a1 already changed 19 of 27 inspected modules (diff of `Lib/`, **M**). The
  fingerprint should be `co_code` plus `co_consts` of each swapped function, checked at
  install, with refusal on mismatch. Without the per-function fingerprint, refusal would be
  version-wide.
- **Errors re-run Python.** For pure functions (path functions, `strptime` parsing,
  `UUID(str)`, `ip_address`), any error makes the kernel return "miss". CPython's code then
  raises the exact exception, message and traceback. This removes error-message churn,
  e.g. 3.16 rewrote every `_strptime` message.

## 3. Summary table

"Achievable" = estimated Rustique ÷ stdlib for an *exact* drop-in. It is given at small
size (one typical call) / large size (bulk), with tags.

| candidate | entry point | stdlib → best alt (small / large) | where stdlib time goes | exact-subset coverage | tests (cases) | traps | risk | upstream overlap | achievable |
|---|---|---|---|---|---|---|---|---|---|
| `datetime.strptime`, `time.strptime` | C classmethod / C function calling Python `_strptime` → `ml_meth` swap, free fallback | 3.7 µs → ciso8601 0.014–0.020, `fromisoformat` 0.031–0.035, whenever 0.030 / n.a. **M** | interpreter 45%, locale lookups 12%, dicts 10%, regex only 5% **M** | ASCII input, directives `%Y m d H M S f z y b B a A p j %` plus literals and whitespace, `LC_TIME` = cached C/POSIX, exact `datetime` class: ~90% **I** | test_strptime 60, test_datetime 3,668, test_time 65 | regex alternation and backtracking order, Unicode `\d`, IGNORECASE, `time.tzname`, 3.15 `%d`-without-year error, `%e` warning, messages differ in 3.16 (fallback) | low–med | 3.16: C fast path for numeric formats (merged) | **0.03–0.06** / n.a. **I** |
| `datetime.strftime` / `isoformat` / `time.strftime` | C methods → `ml_meth` | strftime 1.23 µs → `isoformat` 0.34; `isoformat` 506 ns → whenever 0.154 **M** | `wrap_strftime` imports `time.strftime` and rewrites the format on every call; libc `strftime` **M** (source) | numeric directives with any locale; `%a %b %p` only in C locale: ~90% **I** | test_datetime, test_time | locale, `%Z`/`%z` through `tzinfo` calls, `%G` year normalization, out-of-range years | low | 3.15 caches lookups (small) | strftime **0.2–0.35**, isoformat **0.15–0.25** **I** |
| `copy.deepcopy` | pure-Py function (recursive) → vectorcall | record 2.18 µs → copium 0.128; 10k records 23.5 ms → copium 0.177; objects → pickle round trip 0.29 (copium 0.78–0.90) **M** | interpreter 57%, calls 8%, memo/result dicts 12%, allocation 7% **M** | trees of exact builtins (incl. `frozendict`) with the dispatch tables untouched: ~60–70%; ~90% with a native reduce protocol for plain objects/dataclasses **I** | test_copy 84, test_copyreg 6 | `__deepcopy__(memo)` sees the memo; `__reduce_ex__(4)`, `copyreg.dispatch_table`, `_deepcopy_dispatch` patching; keep-alive list `memo[id(memo)]`; recursion depth/RecursionError; fallback 1.12 per level | low–med | none (C deepcopy rejected upstream) | **0.12–0.2** / **0.15–0.2**; objects 0.3–0.5 **I** (copium **M**) |
| `copy.copy` | pure-Py → vectorcall | plain object 867 ns → copium 0.57; `list`/`dict` already call `.copy()` (174/339 ns) **M** | `__reduce_ex__(4)` + Python `_reconstruct` **I** (source) | exact classes without `__copy__`: ~90% **I** | test_copy | `__copy__`, reduce protocol, `__setstate__` | low | none | objects **0.3–0.5**, list/dict ~0.8 **I** |
| logging record path (`LogRecord.__init__`, `Formatter.format`/`formatTime`, `findCaller`, `isEnabledFor`) | pure-Py methods → vectorcall | enabled call 6.2–7.8 µs, disabled 143 ns; no exact alternative (picologging unbuildable here) **M** | `formatTime` 16%, `LogRecord.__init__` 15%, `_log` 6%, `findCaller` + `_is_internal_frame` 10%, `PercentStyle._format` 5%, handler lock/emit/flush ~10% **M** | default record factory, exact `Formatter`/`LogRecord` types, `converter is time.localtime`, `_srcfile` default: ~70–80% of records **I** | test_logging 287 | `setLogRecordFactory`, `setLoggerClass`, formatter subclasses, `Formatter.converter = time.gmtime`, `record.__dict__` key order (JSON formatters), `logThreads`/`logProcesses`/`logMultiprocessing`/`taskName`, `stacklevel`, handler lock order | med | 3.16: getLogger fast path only | per enabled record **0.45–0.6** piecewise, 0.2–0.3 full chain; disabled 0.5 **I** |
| pathlib `PurePath`/`Path` core | pure-Py methods and properties → vectorcall | `str(Path(s))` 2.54 µs, `p.parent` 863 ns, `with_suffix` 1.4 µs, `==` 2.7 µs, hash 3.5 µs, `relative_to` 12.3 µs (relpath 0.14); no alternative **M** | interpreter 43%, `_parse_path`/`__init__`/`drive`/`_tail`/`__str__` layers with `try/except AttributeError` lazy slots, string ops 10% **M** | exact `PurePosixPath`/`PosixPath`/`Path`, str segments: ~95% **I** | test_pathlib 1,398 | `with_segments` override (public), `parser`, subclass `__str__`/`__fspath__`, cached-slot contents (`_tail_cached` list), Windows flavour | med | none relevant | construct+str **0.1–0.2**, ops 0.15–0.4 **I** |
| `os.path`/`posixpath` (`join`, `splitext`, `basename`, `dirname`, `split`, `relpath`, `abspath`) | pure-Py functions → vectorcall | `join` 360 ns, `splitext` 358, `relpath` 1.95 µs; `normpath` already C **M** | interpreter + calls; `os.fspath`/`_get_sep` global lookups **M** | exact `str`/`bytes`, no `os.fspath`/`_get_sep` patches: ~98% **I** | test_posixpath 96, test_genericpath 26 | result identity (`join(a, '/abs') is` the argument), fallback 1.05–1.11 on small functions, module-global patching (dict watcher) | low | none | **0.28–0.46** **P** (tuned ≈0.2–0.3 **I**) |
| `http.client.parse_headers` / `email.feedparser` (added) | pure-Py function → vectorcall | 14 headers 27.5 µs; `email.message_from_string` 13 µs (compat32), 161 µs (`policy.default`); no exact alternative **M** | `FeedParser._parse_headers`/`_parsegen` 28%, `Compat32.header_source_parse` 12.5%, `Message.get` 8%, `BufferedSubFile` 10% **M** | ASCII/latin-1 headers, no continuation lines or defects, `HTTPMessage` class exact: ~95% of HTTP responses **I** | test_httplib 151, test_email 1,817 | builds an `HTTPMessage` with `_headers` list, policy, defects; the `email` header classes are mutated at runtime; huge surface if extended beyond `parse_headers` | med | none | **0.05–0.15** **I** |
| `contextlib.contextmanager` | pure-Py (`helper`, `_GeneratorContextManager.__init__`/`__enter__`/`__exit__`) → vectorcall | `with cm():` 815 ns → class-based CM 0.19 **M** | `__exit__` 18%, `__init__` 15%, `helper` 12%, `__enter__` 10%, generator body 5% **M** | every exact `_GeneratorContextManager` (no subclass) **I** | test_contextlib 100 | `__exit__` exception protocol (StopIteration and RuntimeError, `exc.__traceback__` restore), `gen.throw` semantics, recreate for `ContextDecorator` | med | 3.16a1 small edits | **0.45–0.55** (floor: 2 generator resumes) **I** |
| enum (`EnumType.__call__`, `__hash__`, `__str__`/`__format__`, `.value`/`.name` property) | pure-Py metaclass `__call__`, dunders via slots (free fallback), properties | `Color(2)` 150 ns (dict lookup 0.19), `d[member]` 148, `str` 119, `.value` 82, `Flag \|` 354 **M** | `Enum.__str__` 10%, `__call__` 10%, `__new__` 6%, `property.__get__` 6%, `__hash__` 5% **M** | members of `Enum`/`IntEnum`/`StrEnum`/`Flag` without overridden dunders: ~90% **I** | test_enum 1,087 | `_missing_`, `_value2member_map_` (Flag pseudo-members are *created and cached*: a side effect), `_generate_next_value_`, mixins `__format__` | med | 3.16a1 edits (37 lines) | lookup **0.3–0.4**, hash/str **0.3–0.4**, property 0.4–0.5 **I** |
| `uuid` (`uuid4`, `UUID.__str__`, `UUID(str)`, `hex`) | pure-Py function / methods (str via slot: free fallback) | `uuid4` 866 ns → uuid_utils 0.074 (own RNG), `os.urandom(16)` alone 0.47; `str(UUID)` 399 → 0.20; `UUID(str)` 647 → 0.18 (fastuuid 0.13, 3.14) **M** | `uuid4` 27%, `__str__` 19%, `_from_int` 17%; syscall ~400 ns **M** | 100% with exact `UUID` and the `os.urandom` identity guard **I** | test_uuid 120 | random source (see §5), `is_safe`, immutability (`object.__setattr__`), `__slots__` | low | **3.16: C UUID/uuid4/uuid7 (merged)**, so 3.14/3.15 only | `uuid4` **0.5–0.6** with os.urandom (0.1–0.15 with vDSO getrandom: decision); str **0.15–0.25**; parse **0.15–0.2** **I** |
| `random` glue (`randint`, `randrange`, `choice`, `shuffle`, `sample`, `choices`) | pure-Py methods of `Random` (bound to `_inst`) → vectorcall | `randint` 199 ns, `choice` 145, shuffle 1000 115 µs (numpy 0.15, different stream), `sample` 2.8 µs **M** | 4 Python frames per `randint` around C `getrandbits` **I** (source) | exact `Random` (not a subclass, `random`/`getrandbits` not overridden): ~95% **I** | test_random 116 | must consume the MT stream exactly as `_randbelow_with_getrandbits` does; `__init_subclass__` picks `_randbelow` | low | none | `randint`/`choice` **0.25–0.4**, shuffle/sample **0.1–0.2** **I** |
| `statistics` (`mean`, `stdev`, `variance`, `median`) | pure-Py functions → vectorcall | `mean`(1000 floats) 414 µs → `fmean` 0.019; `stdev` 706 µs → numpy 0.063; `mean`(10) 10.2 µs **M** | `_exact_ratio`/`float.as_integer_ratio` 58%, `_sum` 35%, `Fraction` 17% **M** | float/int sequences: ~90% **I** | test_statistics 400 | exact rational semantics (result = correctly rounded `Fraction`), mixed types, `_convert`, NaN/inf | low | 3.16a1 edits (43 lines) | **0.02–0.05** **I** |
| `difflib` (`SequenceMatcher.ratio`/`get_opcodes`, `unified_diff`, `get_close_matches`) | pure-Py class → vectorcall | ratio(43 chars) 45.6 µs → cydifflib 0.19, cdifflib 0.48; `unified_diff` 2000 lines 2.86 ms → cydifflib 0.63 (rapidfuzz 0.008 is a different metric) **M** | `find_longest_match` loops (interpreter) **I** | str/list-of-str inputs, default `isjunk`/`autojunk` **I** | test_difflib 61 | `autojunk` popularity heuristic, `b2j` exposure, generator outputs (`unified_diff`) | low–med | 3.16a1 edits | ratio **0.05–0.15**, diff 0.3–0.6 **I** |
| `ipaddress` (`ip_address`, `ip_network`, `str`, containment) | pure-Py → vectorcall; dunders via slots | `ip_address` 1.79 µs (netaddr 1.13), IPv6 4.3 µs, `ip_network` 2.26 µs, `str` 690 ns **M** | `_ip_int_from_string` 28%, `_parse_octet` 25%, `__init__`s 21% **M** | str/int inputs, exact classes **I** | test_ipaddress 212 | error messages (re-run Python), leading-zero rules, scoped IPv6, `functools.cached_property` slots | low | 3.16a1 edits (66 lines) | **0.05–0.1** **I** |
| `queue.Queue` put/get | pure-Py methods over `threading.Condition` (pure Py) | put+get 927 ns → SimpleQueue 0.085; 2-thread ping-pong 64 ms → SimpleQueue 0.81 **M** | `put`/`get` 36%, `Condition.notify`/`__exit__`/`_is_owned` 27% **M** | exact `Queue` and `Condition` objects: ~90% **I** | test_queue 165 | public `mutex`/`not_empty`/`queue` attributes (`with q.mutex: q.queue.clear()`), `_put`/`_get` overrides (`LifoQueue`, `PriorityQueue`), `task_done`/`join` | med | none | uncontended **0.2–0.3**, contended 0.85–0.95 **I** |
| dataclasses `asdict`/`astuple`/`replace` | pure-Py → vectorcall | `asdict` nested 1.94 µs → msgspec 0.105; `replace` 911 ns → msgspec 0.094 **M** | recursive Python (`_asdict_inner`) **I** | dataclass trees of builtins **I** | test_dataclasses 285 | `dict_factory`, `deepcopy` of leaves, `__post_init__` and `InitVar` in `replace` | low | none | **0.1–0.2** **I** |
| dataclass/namedtuple *creation* (import-time) | pure-Py decorator / factory | `@dataclass` (3 fields) 133 µs (`attrs` 1.73); `namedtuple` 28 µs **M** | 71% in `add_fns_to_class`, i.e. `exec`/`compile` of generated source **M** | all **I** | test_dataclasses, test_collections | code objects from cached compiles would be shared between classes (identity observable) | low | none | **0.3–0.5** with a compile cache (not Rust work) **I** |
| `typing` runtime (`Protocol` `isinstance`, `get_type_hints`) and `inspect.signature` | pure-Py metaclass method / functions | Protocol miss 1.64 µs (hit 209 ns); `get_type_hints` 4.3–6.4 µs; `signature` 6.4 µs **M** | `inspect.getattr_static` per protocol attribute (miss); annotation evaluation **M** (cProfile) | non-generic protocols **I** | test_typing 742, test_inspect 382 | `__subclasshook__`, ABC caches, PEP 649/749 annotationlib formats | med | 3.16a1 edits | Protocol miss **0.05–0.1**; hints/signature 0.2–0.4 **I** |
| `functools.singledispatch` / `singledispatchmethod` | pure-Py closure | 204 ns / 749 ns **M** | `dispatch()` + `WeakKeyDictionary` cache **I** | registry without ABCs **I** | test_functools 338 | ABC-based dispatch, cache invalidation via `_abc_registry`, `register` at runtime | low | none | **0.3–0.5** **I** |
| `fnmatch.filter`/`fnmatch` | pure-Py → vectorcall | `filter`(1000) 189 µs (listcomp endswith 0.19); `fnmatch` 605 ns **M** | per-name `normcase` call + regex match **I** | `*`, `?`, `[...]` patterns: ~95% **I** | test_fnmatch 24 | must equal the translated regex (`(?s:...)\z`), `lru_cache` side effect | low | none | **0.15–0.3** **I** |
| `os.walk`, `glob`, `Path.rglob`, `shutil.copytree`/`rmtree` | generators / pure-Py | `os.walk` 220 dirs 3.35 ms; `glob('**')` 7.4 ms (wcmatch 1.0); `copytree`+`rmtree` 110 files 15.6 ms **M** | walk: interpreter 31%, libc/syscalls 20%, allocation 16% **M** | inner helpers only (generator trap) **I** | test_os 552, test_glob 22, test_shutil 227 | generator type, in-place `dirnames` pruning, `onerror`, symlinks | med | none | walk/glob **0.5–0.7** via helpers; shutil **0.85–0.95** (syscall-bound) **I** |
| asyncio loop internals | pure-Py (`BaseEventLoop._run_once`, `Handle._run`, `selectors`) | `sleep(0)` 2.77 µs → uvloop 0.48; `call_soon` 2.0 µs → 0.45; TCP echo 43 µs → 0.76 **M** | `_run_once` 24%, `Handle._run` 15%, `EpollSelector.select` 12%, `call_soon` 12% **M** | default loop, no debug **I** | test_asyncio 2,927 | `_ready`/`_scheduled` observable, debug mode, slow-callback warnings, exception handler, policies | high | 3.14 native task list | **0.6–0.8** **I** (uvloop bound) |
| `traceback.format_exception` (added) | pure-Py | 12 frames 677–755 µs; `logger.exception` 90 µs **M** | `compile()` for caret anchors 17%, `_wlen`/width 15%, `format_frame_summary` **M** (cProfile) | all; anchors still need a parser **I** | test_traceback 441 | colorization, caret rules, `linecache`, exception groups, notes | med | 3.16a1: 93 lines changed | **0.3–0.6** **I** |
| `pickle` dumps/loads | C (`_pickle`) | dumps record 796 ns → orjson/msgspec 0.28 (other formats); loads 858 ns → 0.45–0.62; loads 10k records → 0.78–1.0 **M** | memo hash table, per-object dispatch **I** | byte-identical output required **I** | test_pickle 1,081 | memo/BINPUT numbering, framing, `persistent_id`, `reducer_override`, `__reduce_ex__` | high | none | **0.6–0.9** **I** |
| zlib one-shot `decompress`, `crc32`/`adler32` | C → `ml_meth` | 1 MB decompress 1.28 ms → libdeflate 0.29, zlib-ng 0.45; crc32 1 MB → libdeflate 0.13; 1 KB decompress → 0.87 **M** | inflate loop (zlib 1.3.2) **I** | one-shot calls; streaming `decompressobj` excluded (`unconsumed_tail` depends on the inflater) **I** | test_zlib 88 | error messages from zlib's `msg` (re-run zlib on error), `wbits` variants, `bufsize` | low | 3.16: `--with-zlib=zlib-rs` option, R4C Rust zlib | **0.3–0.45** (≥ 64 KB) / ~0.9–1.0 (1 KB); crc32 **0.13** **I** |
| zlib/gzip `compress`, `bz2`, `lzma` | C | `zlib.compress` 1 MB: zlib-ng 0.38–0.76, libdeflate 0.56–1.54, **no alternative byte-identical at levels 1/6/9**; bz2 and lzma via cramjam are byte-identical but 0.81–1.33 (same libraries) **M** | library | none exact | — | compressed bytes differ across implementations | — | R4C | **1.0** (no exact headroom) **M** |
| importlib (startup) | frozen pure-Py `_bootstrap(_external)` | `python -I -c pass` 10.9 ms; importing 60 stdlib modules 88 ms **M** | unmarshal + code objects 18%, module bodies (class creation, enum, namedtuple, `re.compile`) ~35–40%, GC ~10%, finder/loader Python ~15% **M** (perf, 10 processes) | default `FileFinder`/`SourceFileLoader` with no custom hooks **I** | test_importlib 350 | `sys.meta_path`, `path_hooks`, `path_importer_cache`, `pycache_prefix`, verbose messages, module locks | high | PEP 810 lazy imports (3.15) | import-heavy startup **0.88–0.95** **I** |
| argparse, shlex, pprint, textwrap, `string.Template` (added) | pure-Py | build parser (12 args) 261 µs, `parse_args` 48 µs, `format_help` 316 µs; `shlex.split` 11.2 µs; `pprint.pformat` 505 µs (`repr` 0.038); `textwrap.wrap` 1.5 KB 194 µs **M** | interpreter **I** | most **I** | 1,963 / 45 / 77 / 68 | formatter classes, `HelpFormatter` subclasses, color (3.14+) | med | 3.16a1 argparse edits | `shlex` 0.05–0.1, others 0.3–0.5; once per process **I** |
| fractions, decimal, secrets, hashlib/hmac, struct/array, base64, heapq/bisect/itertools/operator, `collections` C types, `Counter` build, `lru_cache`/`partial`/`reduce`, datetime arithmetic, zoneinfo | C, or thin Python over C | `Fraction` ops 409–479 ns (quicktions 0.43–0.49, gmpy2 0.21–0.27); `Fraction('3.1415')` 2.1 µs (quicktions 0.098); `hmac.new().hexdigest()` 2.2 µs (`hmac.digest` 0.78); the others 33–130 ns or bulk-C **M** | already native | — | — | `Fraction` operator fallbacks | — | base64 3.15 C rewrite | Fraction **0.3–0.5** (rare use); `hmac.new` 0.7; rest **1.0** **I** |
| builtins: `int`/`float` arithmetic, `isinstance`, `getattr`/`hasattr`, `hash` of str/tuple, object creation, `dict` ops | interpreter-specialized | `isinstance(d, dict)` ~30 ns, `hasattr` 47 ns, `P(1,2,3,4)` 152 ns, `dict.copy`(100) 280 ns **M** | inline in the specialized instructions | — | — | — | — | — | **1.0**: unreachable or no work to remove **I** (`bytecodes.c`) |

## 4. Ranking and reasoning

The score is the product of three terms. *Reach* is how much real time flows through the
target: `static-usage.md` share of packages, `workloads-*.md` shares. *Gain* is the
achievable ratio. *Tractability* covers exact subset, hooks, tests and upstream erosion.

| rank | target | why |
|---:|---|---|
| 1 | **`datetime.strptime`/`time.strptime`** (+ `strftime`, `isoformat`) | Largest gain per call (20–50×), behind C entry points (`ml_meth` swap: no inlining loss, free fallback), a closed directive grammar, and error paths that simply re-run `_strptime`. Datetime parsing appears in 14% and formatting in 22% of packages (`static-usage.md`). 3.16 covers numeric formats only, so names, `%z` and `%b` logs stay in reach. |
| 2 | **`copy.deepcopy`/`copy.copy`** | In 22–25% of packages (deepcopy captured at 34% of sites: vectorcall in place is required). 5–9× on data trees, measured by an existing C drop-in that already passes 81/84 of `test_copy`. No upstream plan. The kernel must also do the reduce protocol natively (objects 0.3–0.5), because recursive fallback costs 1.12. |
| 3 | **logging record path** | In 40% of packages; owns 65.6% of `logging_service` (`workloads-web.md`). Piecewise replacement gives ~2× per emitted record (`formatTime`, `LogRecord.__init__`, `findCaller`, `Formatter.format`), each guarded by exact-type and default-hook checks. The hook surface (factories, converters, subclasses) is the main cost of exactness. |
| 4 | **pathlib core** | In 33% of packages, and per-operation costs are µs-scale for pure string work (`relative_to` 12 µs). 5–10× is plausible, and 1,398 tests exist. The `with_segments`/`parser` hooks must be guarded per type (type watcher). |
| 5 | **`http.client.parse_headers` (+ `email.feedparser` for HTTP headers)** | Measured 27.5 µs per response. email+http+urllib take 39.7% of `requests_local`. A narrow exact subset (plain ASCII headers) builds the same `HTTPMessage`. Extending into `email.policy.default` (161 µs) is a much larger project. |
| 6 | **`contextlib.contextmanager`** | In 38% of packages (`static-usage.md`); ~2× per `with`; small code. Exception semantics in `__exit__` need care, and `test_contextlib` covers them. |
| 7 | **enum dunders and value lookup** | In 41% of packages. Slot-reached dunders fall back for free, for a 2.5–3× per operation gain. Flag pseudo-member caching is the trap. |
| 8 | **`posixpath` functions** | Ubiquitous, and the prototype already shows 0.28–0.46. The functions are tiny, though, so the kernels must cover bytes and every str kind to keep fallback cells within 1.05. |
| 9 | `uuid` | Simple and exact, but C `uuid` lands in 3.16: worth it only for the 3.14/3.15 window. |
| 10 | `random` glue, `statistics`, `difflib`, `ipaddress`, `queue.Queue`, dataclasses `asdict`/`replace`, `typing.Protocol` `isinstance` | Each has a clean exact subset and large per-call gains, but narrower reach (inferred). Do them when the shared pure-Python machinery exists, since each is then small. |
| — | not worth an exact drop-in now | asyncio internals (≤ 1.7×, uvloop bound 0.45–0.76, 2.9k tests, uvloop exists), traceback (error path), pickle (already C, ≤ 1.7×), compression (only one-shot decompress/crc32 exact; R4C zlib), importlib (≤ 10% of startup, highest risk, PEP 810), argparse/shlex/pprint/textwrap (once per process), everything already in C, builtins. |

Cross-cutting: ranks 2–10 all need the same **pure-Python takeover layer**:

- vectorcall swap;
- dict watchers on module globals and type watchers on classes for subclass/monkeypatch
  guards;
- a tools guard (`mechanisms.md` §8);
- a per-function `co_code` fingerprint;
- "errors re-run Python";
- a `sys.modules` watcher so modules imported after `enable()` (and in subinterpreters)
  get swapped too.

Build it once, then each target is a kernel plus a guard list.

## 5. Candidate notes (details beyond the table)

**strptime.**
- *Grammar.* `_strptime` builds one regex per format:
  - `%m` = `1[0-2]|0[1-9]|[1-9]`, `%d` = `3[01]|[12]\d|0[1-9]|[1-9]| [1-9]`;
  - `%H` = `2[0-3]|[0-1]\d|\d`, `%f` = `[0-9]{1,6}`;
  - whitespace becomes `\s+`, flags are IGNORECASE.

  A kernel must reproduce leftmost-alternative backtracking: `"%m%d"` on `"111"` matches as
  11/1. It must also send non-ASCII digits (`\d` is Unicode) to the fallback.
- *Hidden state.* Every call checks `locale.getlocale(LC_TIME)` (12% of the time) and
  `time.tzname` and `time.daylight`, and then fills the private `_regex_cache`. A kernel
  caches `setlocale(LC_TIME, NULL)` and the tzname tuple identity. Deviation to approve:
  private caches are not populated.
- *Entry points.* Hook `datetime.strptime` and `time.strptime` (C `PyMethodDef`s). The C
  code imports `_strptime._strptime_datetime_datetime` on every call, so guard that
  attribute's identity with a dict watcher.

**deepcopy semantics to keep:**
- atomic types returned as is;
- `memo[id(x)]` written *before* recursing into containers (cycles);
- the keep-alive list at `memo[id(memo)]`;
- `__deepcopy__(memo)` receives the real memo dict, so with Python hooks present the kernel
  must keep a real dict memo (cost acceptable);
- `_reconstruct` order: `callable(*args)`, then deep state, then `__setstate__` or
  `__dict__.update`, then listitems, then dictitems.

Fast path when `memo is None` and every node is an exact builtin: memo-free copying with a
native visited set. This is unobservable, because the memo is discarded.

**uuid4 randomness: what "exact" can mean (decision needed).**
- *Floor today.* uuid4 must draw from `os.urandom` (a `getrandom` syscall, ≈400 ns here),
  so an exact kernel saves only the Python glue: 0.5–0.6 **I**.
- *Option.* Same distribution, same kernel CSPRNG, fork- and VM-snapshot-safe, through the
  Linux ≥ 6.11 vDSO `getrandom`: 0.1–0.15 **I**.
- *What differs.* Byte streams can never be compared. Only the syscall pattern would
  differ.
- *Guard either way.* `uuid.os.urandom is` the original, so `mock.patch('os.urandom')`
  keeps working.
- *Rejected.* A userspace buffered RNG (`uuid_utils` style) is not equivalent: fork safety
  and VM cloning.

**logging, piecewise.**
- *Pieces:*
  - `LogRecord.__init__`: 2.15 µs → ≈0.3 µs. It writes the same attributes in the same
    `__dict__` order.
  - `Formatter.formatTime`: 1.5 µs → ≈0.25 µs, only when `converter is time.localtime` and
    `datefmt is None`.
  - `findCaller`: frame walk with `_is_internal_frame` semantics; `_srcfile` respected.
  - `Formatter.format` / `PercentStyle._format`: exact `Formatter` and `PercentStyle` only.
- *Leave alone:* handler locking and `emit`/`flush` (I/O-bound, user-overridden).

**pathlib.** Lazy slots raise and catch `AttributeError` on first access (`drive`, `_tail`,
`_str`, `_hash`). A native `_parse_path`/`__str__`/`__truediv__`/`parent`/`with_suffix`
fills the *same* slots with equal values, and skips the exceptions. The guard is
`type(self).with_segments is PurePath.with_segments`, `parser is posixpath` and no override
of the swapped methods: a type watcher plus `tp_version_tag`.

**queue.** Users touch `q.mutex`, `q.queue`, `q.not_empty`, so a native put/get must operate
on those same objects:
- acquire the C lock;
- append to the deque;
- notify by popping `Condition._waiters` and releasing waiter locks, as `notify` does.

Exact while `type(q) is Queue` and the conditions are exact `threading.Condition`.

**Compression identity.** zlib-ng 2.2.5, libdeflate and cramjam (miniz) all produce
different bytes from the linked zlib 1.3.2 at levels 1, 6 and 9 on 1 MB of JSON **M**. Only
decompression output and checksums are implementation-independent. CPython itself does not
guarantee compressed bytes across builds (Windows uses zlib-ng since 3.14), but Rustique's
bar is identity with the running build.

**Free-threaded spot check** (`bench/hot.py`, wall time per iteration, 3.15.1 vs 3.15.1t):

| path | 3.15.1 | 3.15.1t |
|---|---:|---:|
| `deepcopy`(100 records) | 231 µs | 246 µs |
| `strptime` | 3.53 µs | 3.81 µs |
| logging enabled call | 6.24 µs | 7.35 µs |
| pathlib chain | 7.02 µs | 8.06 µs |
| `str(uuid4())` | 1.66 µs | 1.72 µs |
| `@contextmanager` | 786 ns | 940 ns |
| `posixpath` trio | 989 ns | 1082 ns |

Headroom is the same or slightly larger on 3.15t **M**. Swapping vectorcalls stops the world
(`PyFunction_SetVectorcall`), so install at enable or import time.

## 6. Reproduce

Everything is under `~/rustique-research/h-objects/` (outside the repo):

```bash
cd ~/rustique-research/h-objects
# venv with alternatives (copium, ciso8601, cydifflib, cdifflib, zlib-ng built from source)
uv venv --python 3.15 venv && uv pip install --python venv/bin/python copium uuid_utils ciso8601 \
  whenever pendulum python-dateutil cydifflib cdifflib rapidfuzz quicktions gmpy2 zlib-ng deflate \
  cramjam orjson ormsgpack msgspec attrs cachetools uvloop numpy wcmatch netaddr
# wall-time comparisons (results/*.txt, *.jsonl)
for b in copy dt uuid obj path log misc zip aio; do venv/bin/python bench/b_$b.py; done
# perf: where time goes (results/perf/*.txt); symbolizer validated on json.dumps
perf record -e cpu-clock -F 1999 -g -o x.perf -- python3.15 -X perf bench/hot.py deepcopy
python3.15 tools/perfsym.py x.perf 20
# prototype + callgrind (results/cg_*.txt)
cd proto/fnswap && CARGO_BUILD_JOBS=2 PYO3_PYTHON=$(readlink -f ~/.local/bin/python3.15) cargo build --release \
  && cp target/release/libfnswap.so ../lib/fnswap.cpython-315-x86_64-linux-gnu.so && cd ../..
python3.15 bench/cg.py plain,pass,native 20000; python3.15 bench/cg_pass.py deepcopy_cfg,uuid_str,enum_hash
python3.15 bench/cg_prologue.py
# copium against CPython's test_copy (needs a test/ symlink to ~/src/cpython-3.15.1/Lib/test)
venv/bin/python bench/copium_conf.py $PWD/testpath copium
# test counts
PYTHONPATH=$PWD/testpath python3.15 -m test --list-cases test_pathlib | wc -l
```
