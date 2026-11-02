# Workload atlas: `data` family (data processing & scripting)

Phase 1, round question 1: *where does real Python spend its time?* This file covers the
`data` family: 12 workloads in `benches/workloads/data/`, profiled on CPython 3.15.1
(python-build-standalone, the build we benchmark). Raw profiles:
`~/rustique-research/profiles/data/<workload>.json` (shared schema plus extra fields:
`categories`, `top_native_entry`, `plain_perf`, `cprofile_python`). Tool:
`benches/workloads/data/_profiler.py`. Every number is **measured** with the method in
§5 unless tagged *inferred*. Family means are unweighted means over the 11 stdlib
workloads; `pandas_etl` is reported on its own (§4.12) because frame-pointer unwinding
stops inside pandas/NumPy.

## Key findings

1. **73% of the time is behind an existing entry point; 27% is plainly unreachable.**
   Mean over 11 workloads: 38% inside C-accelerated module functions, 14% inside
   builtin type methods/slots, 21% in pure-Python stdlib code — versus 14% interpreter
   work in user code, 7% in third-party Python (PyYAML), 7% cyclic GC (§2.1).
2. **The biggest *new* targets are pure-Python stdlib functions and two "C but slow"
   entry points**, not the container builtins: `copy.deepcopy` (50% of `state_snapshot`),
   the path stack `pathlib`/`glob`/`fnmatch`/`posixpath` (83% of `fs_inventory`'s user
   time), `_strptime` (29% of `log_analytics`), `textwrap` (39% of `report_render`),
   `random` (30%), `ipaddress` (20%), the `json.dumps` glue (29% of `csv_etl` incl. the C
   encoder), `csv.DictReader` (20%), ElementTree's `ElementPath`/serializer (≈ 27% of
   `xml_feeds`), and the C method `datetime.strftime` (31% of `tz_events`, 13% of
   `csv_etl`, 9% of `log_analytics`), which imports `time.strftime` and builds a
   `struct_time` on every call.
3. **Inputs are small.** Hot string arguments are 2–30 characters (`strip` p50 9,
   `int(str)` p50 2 chars × 813k calls, `fnmatch` names p50 10, `strftime` formats 2–20
   chars, `strptime` inputs 16–26 chars with ≤ 3 distinct formats per workload). Wins
   must come from removing per-call Python/C overhead, not from bulk kernels — the
   opposite of the container round.
4. **zlib is the single largest native cost** (8.3% family mean, 91% of
   `backup_archive`, 71% in `longest_match` alone; this build links classic zlib).
   Upstream plans a Rust zlib for 3.16; `compress()` output bytes must stay identical,
   which rules out zlib-ng/zlib-rs style matchers (*inferred*).
5. **Cyclic GC costs 13–29% in graph-building jobs** (`billing_engine` 37% of samples,
   29% by `gc.disable()`; `state_snapshot` 17%/13%; `xml_feeds` 16%/14%). Not an
   entry-point target, but it caps what kernels can deliver there.
6. **`re` is everywhere underneath**: `_sre` entry points average 7.2% (up to 25%),
   called from user code and from `textwrap`, `fnmatch`, `_strptime`, `configparser`.
7. Surprises: `Counter[k] = v` goes through a slot wrapper (2.6× a plain dict subclass)
   because `Counter` defines `__delitem__`; `ZoneInfo(key)` with > 8 zones in rotation
   re-reads tz files (≈ 40% of an un-memoised `tz_events`); `json.dumps(..., sort_keys=True)`
   builds a new `JSONEncoder` + C encoder per call (§6).

## 1. Workloads

| workload | stands for | run() wall | user / sys | samples | owners (innermost Python frame) |
|---|---|---|---|---|---|
| `csv_etl` | nightly CSV export → `DictReader`, `fromisoformat`, `strftime` → JSON lines + summary CSV (40k rows) | 0.63 s | 0.63 / 0.00 | 8154 | workload 50%, json 29%, csv 20% |
| `log_analytics` | access-log + syslog analysis: `re` named groups, `strptime`, `urllib.parse`, `ipaddress`, `Counter` (60k lines) | 1.36 s | 1.35 / 0.01 | 8189 | workload 44%, _strptime 25%, ipaddress 20%, urllib.parse 7% |
| `tz_events` | scheduling job: `fromisoformat`/`strptime`, `zoneinfo`, `timedelta`, `strftime` (24k events) | 0.74 s | 0.73 / 0.00 | 9172 | workload 90%, _strptime 8% |
| `text_corpus` | text normalisation + word stats: `unicodedata`, `casefold`, `re.findall`, `translate`, `Counter` (4.8 MB) | 0.80 s | 0.80 / 0.00 | 9400 | workload 86%, collections 12% |
| `fs_inventory` | file-tree scan: `os.walk`+`fnmatch`, `scandir`, `pathlib.rglob`, `glob` (13k files) | 0.40 s | 0.24 / 0.16 | 5302 | pathlib 25%, glob 25%, workload 17%, fnmatch 14%, posixpath 11% |
| `backup_archive` | backup/packaging: `hashlib`, `zipfile`, `tarfile` gz, `gzip`, `crc32` (6 MB) | 0.68 s | 0.68 / 0.00 | 8937 | gzip 60%, zipfile 34% |
| `config_merge` | deploy config load: `configparser`, `tomllib`, PyYAML, `json`, deep merge (960 files) | 0.74 s | 0.74 / 0.00 | 9207 | PyYAML 80%, configparser 8%, tomllib 4% |
| `xml_feeds` | product feed + sitemap: `ElementTree` parse/find/build/`tostring`, `iterparse`, `minidom` (15 MB) | 0.58 s | 0.57 / 0.01 | 7810 | xml.etree 68%, workload 32% |
| `state_snapshot` | object-graph persistence: `deepcopy`, `pickle`, `asdict`, dataclass `__eq__` (368k-node graph) | 0.78 s | 0.75 / 0.02 | 8299 | copy 49%, workload 40%, dataclasses 8% |
| `billing_engine` | invoicing: `Decimal` + `quantize`, `Fraction`, `enum`, slotted dataclasses, `statistics` (40k customers) | 1.08 s | 1.04 / 0.04 | 9396 | workload 80%, fractions 10%, statistics 6% |
| `report_render` | fixtures + Markdown report: `random`, `uuid`, `secrets`, f-strings, `format`, `%`, `textwrap` | 0.53 s | 0.53 / 0.01 | 8761 | textwrap 39%, random 30%, workload 20%, uuid 10% |
| `pandas_etl` | pandas ETL: `read_csv`, `to_datetime`, `.str`, `merge`, `groupby`, `pivot_table`, `apply` (240k rows) | 0.62 s | 0.58 / 0.04 | 8499 | (unwinding stops in pandas: 82% no owner) |

All 12 are deterministic (digest stable across runs, `PYTHONHASHSEED` 1/2, and identical
on 3.15.1t with the GIL staying off); pins install on 3.15.1 and 3.15.1t
(`requirements.txt`). Wall times are on a shared box (±~20%).

## 2. Family roll-up (11 workloads; `pandas_etl` in §4.12)

### 2.1 Where the time could be accelerated

| workload | C entry: builtin type/fn | C entry: C module | interp: pure-Py stdlib | interp: 3rd-party Py | interp: user code | cyclic GC |
|---|---|---|---|---|---|---|
| `csv_etl` | 17.8% | 51.3% | 14.4% | – | 16.5% | – |
| `log_analytics` | 16.8% | 37.1% | 35.1% | – | 10.4% | – |
| `tz_events` | 15.3% | 55.5% | 6.1% | – | 22.8% | 0.3% |
| `text_corpus` | 25.3% | 43.4% | 1.1% | – | 29.7% | 0.5% |
| `fs_inventory` | 13.5% | 21.4% | 55.8% | – | 9.1% | 0.2% |
| `backup_archive` | 1.0% | 95.8% | 3.0% | – | 0.3% | – |
| `config_merge` | 10.1% | 5.5% | 8.6% | 71.0% | 3.1% | 1.7% |
| `xml_feeds` | 12.0% | 34.0% | 23.0% | – | 15.0% | 16.0% |
| `state_snapshot` | 13.1% | 29.2% | 35.4% | – | 4.8% | 17.4% |
| `billing_engine` | 16.1% | 10.3% | 9.9% | – | 27.0% | 36.7% |
| `report_render` | 17.6% | 31.0% | 39.8% | – | 10.4% | 1.2% |
| **mean (11)** | **14.4%** | **37.7%** | **21.1%** | **6.5%** | **13.6%** | **6.7%** |

Definitions (§5): a sample is in a *C entry* when, below the innermost interpreter frame,
it is inside an accelerable C function (builtin method/slot or C-module function,
inclusive of its callees); otherwise it is *interpreter* work attributed to the owner of
the innermost Python frame. **Plainly unreachable** (user-code interpretation +
third-party Python + GC): **27%** mean, worst `billing_engine` 64%, `config_merge` 76%.
The *C entry* columns are reachable only where Rust beats CPython's C (zlib, `strftime`,
`re`, `pickle`… — §3), so "73% reachable" is an upper bound. Without `-X perf` the
split is the same (C entries 15.1% + 39.0%, GC 6.8%).

### 2.2 Top 15 (Python owner × native component) cells

| # | Python owner | native component | mean share | workloads where ≥ 5% |
|---|---|---|---|---|
| 1 | workload | memory/gc | 10.7% | csv_etl, log_analytics, tz_events, text_corpus, xml_feeds, state_snapshot, billing_engine |
| 2 | workload | interpreter | 7.7% | 8 of 11 |
| 3 | stdlib:gzip | zlib | 5.4% | backup_archive |
| 4 | thirdparty:PyYAML | interpreter | 4.6% | config_merge |
| 5 | workload | str | 3.4% | tz_events, text_corpus |
| 6 | workload | dict | 3.3% | csv_etl, log_analytics, tz_events, text_corpus |
| 7 | stdlib:zipfile | zlib | 2.9% | backup_archive |
| 8 | workload | re-engine | 2.6% | log_analytics, text_corpus |
| 9 | workload | libc | 2.5% | csv_etl, tz_events, text_corpus |
| 10 | workload | attr/type | 2.3% | tz_events |
| 11 | workload | calls | 2.2% | tz_events |
| 12 | stdlib:textwrap | re-engine | 1.8% | report_render |
| 13 | stdlib:random | interpreter | 1.8% | report_render |
| 14 | stdlib:xml.etree | memory/gc | 1.6% | xml_feeds |
| 15 | stdlib:copy | memory/gc | 1.5% | state_snapshot |

Components (leaf symbol), mean over 11, with / without `-X perf`: interpreter 26.7/27.1%,
memory/gc 18.4/18.8%, zlib 8.3/8.3%, dict 6.0/6.2%, str 5.7/5.9%, re-engine 5.2/5.5%,
attr/type 5.2/5.4%, libc 4.6/4.9%, calls 4.2/2.6%, list/tuple/set 2.3/2.5%, pickle 1.3%,
expat 1.2%, decimal 1.2%, datetime 1.2%, int 1.0%, everything else < 1% each. Only
`calls` moves with `-X perf` (the perf trampoline disables inlined Python calls).
"workload × libc" is mostly `PyOS_snprintf`/`sprintf` under `datetime.isoformat`/
`strftime` and `memcpy`/`malloc` under string building.

### 2.3 C entry points with the largest inclusive time

| entry point (C function) | Python-level entry | component | mean (11) | where ≥ 2% |
|---|---|---|---|---|
| `zlib_Compress_compress` + `zlib_compress` | `zlib.compressobj().compress`, `zlib.compress` (via gzip/zipfile/tarfile) | zlib | 7.8% | backup_archive 85.0% |
| `datetime_date_strftime` | `date/datetime.strftime` | datetime | 4.8% | tz_events 31.5%, csv_etl 12.8%, log_analytics 9.0% |
| other `datetime` methods | `astimezone`, `isoformat`, `timedelta()`, `utcoffset`, `ZoneInfo` | datetime | 3.0% | tz_events 21.1%, csv_etl 8.4%, log_analytics 2.7% |
| `_sre_SRE_Pattern_{split,findall,search,prefixmatch,sub}`, `Match.groupdict` | `re` pattern methods | re-engine | 7.2% | report_render 24.7%, text_corpus 23.1%, log_analytics 21.0%, fs_inventory 6.4% |
| `_pickle_dumps` + `_pickle_loads` | `pickle.dumps/loads` | pickle | 2.5% | state_snapshot 27.8% |
| `_elementtree_XMLParser_feed` | `ET.fromstring`, `iterparse` | elementtree + expat | 2.3% | xml_feeds 25.3% |
| `encoder_call` | `json.dumps` (C encoder) | json-accel | 1.9% | csv_etl 18.0%, config_merge 2.2% |
| `unicode_*` / `_PyUnicode_*` entries | `str.join/split/translate/…` | str | 4.0% | text_corpus 21.7%, report_render 7.6%, log_analytics 4.1%, xml_feeds 4.0% |
| `_PyDict_Subscript`, `PyDict_*` entries | `d[k]` on dicts and subclasses, `dict()` | dict | 3.6% | csv_etl 9.8%, state_snapshot 9.4% |
| `Reader_iternext` | `next(csv.reader)` | csv-accel | 0.9% | csv_etl 9.7% |
| `_collections__count_elements` | `Counter.update` | collections | 0.9% | text_corpus 10.4% |
| `nfc_nfkc` / `nfd_nfkd` | `unicodedata.normalize` | unicodedata | 0.7% | text_corpus 7.9% |
| `dec_new`, `quantize`, `PyNumber_*`→mpd | `Decimal(str)`, arithmetic | decimal | 1.2% | billing_engine 12.7% |
| `ScandirIterator_iternext`, `DirEntry.stat`, `os_scandir` | `os.scandir` | os | 0.7% | fs_inventory 8.1% (+ kernel time) |
| `gc_collect_main` | (none: allocation-triggered) | memory/gc | 6.7% | billing_engine 36.7%, state_snapshot 17.4%, xml_feeds 16.0% |

### 2.4 Pure-Python library code (innermost frame, incl. native children)

| owner | mean (11) | max | of which interpreter / C entries / GC |
|---|---|---|---|
| thirdparty:PyYAML (pure-Python loader) | 7.3% | 79.8% config_merge | 6.5 / 0.6 / 0.2% |
| xml.etree (+ xml.dom) | 6.2% | 68.3% xml_feeds | 2.1 / 3.2 / 0.9% |
| gzip + zipfile + tarfile (wrappers around zlib) | 8.7% | 95.8% backup_archive | 0.2 / 8.5 / 0% |
| path stack: pathlib, glob, fnmatch, posixpath, genericpath, os, _collections_abc | 7.5% | 83.0% fs_inventory | 4.9 / 2.5 / 0% |
| copy + copyreg | 4.6% | 50.4% state_snapshot | 2.6 / 1.1 / 0.9% |
| textwrap | 3.6% | 39.3% report_render | 1.0 / 2.6 (re.split) / 0% |
| _strptime + locale (`_getlang` on every call) | 3.5% | 28.6% log_analytics | 2.1 / 1.4 / 0% |
| json (Python glue + C encoder) | 3.0% | 29.4% csv_etl | 0.9 / 2.1 / 0% |
| random | 2.7% | 29.6% report_render | 2.1 / 0.6 / 0% |
| ipaddress | 1.8% | 20.2% log_analytics | 1.2 / 0.7 / 0% |
| csv (`DictReader.__next__`) | 1.8% | 20.1% csv_etl | 0.5 / 1.4 / 0% |
| fractions + statistics | 1.4% | 15.8% billing_engine | 0.6 / 0.5 / 0.3% |
| configparser + tomllib | 1.1% | 12.4% config_merge | 0.7 / 0.4 / 0% |
| collections (`Counter.update`) | 1.1% | 11.6% text_corpus | 0.1 / 1.0 / 0% |
| enum + dataclasses (+ generated `__eq__`/`__init__`) | 1.1% | 8.5% state_snapshot | 0.9 / 0.1 / 0% |
| uuid | 0.9% | 9.8% report_render | 0.5 / 0.3 / 0.1% |
| urllib.parse | 0.6% | 6.6% log_analytics | 0.5 / 0.1 / 0% |

Hottest single functions (perf, innermost frame incl. children → cProfile self time and
calls per run): `gzip.GzipFile._write_raw` 37.7% of backup_archive; `textwrap.TextWrapper._split`
27.1% (7k calls, a 483-char verbose regex); `copy.deepcopy` 26.1% (733k calls, self
26.6%); `json.encoder.JSONEncoder.iterencode` 23.0% (32.7k calls); `_strptime._strptime`
20.6% (45k calls, self 14.9%); `csv.DictReader.__next__` 19.2% (40k); `ElementTree.XMLPullParser.feed`
18.5%; `random.Random.choices` 14.4% (40k, self 14.4%); `copy._deepcopy_dict` 12.3%;
`glob._iterdir` 9.4% (50k); `ElementPath.iterfind` 8.6% (74k); `_serialize_xml` 8.6%
(54k); `fnmatch.fnmatchcase` 7.2% (69k); `uuid.UUID.__init__` 5.5%;
`pathlib.PurePath._str_normcase` 5.3%; `ipaddress.IPv4Network.__init__` 3.7%,
`_parse_octet` (180k calls).

### 2.5 Builtins by calls (cProfile, one run each; counts matter as much as time)

| builtin | calls (11 runs) | mean tottime share | where ≥ 2% |
|---|---|---|---|
| `unicodedata.combining` | 2,761,834 | 1.0% | text_corpus 10.6% (1-char args) |
| `isinstance` | 2,408,961 | 1.5% | xml_feeds 4.1%, config_merge 3.7% |
| `len` | 2,349,491 | 1.1% | report_render 3.0%, text_corpus 2.4% |
| `list.append` | 1,351,166 | 0.9% | report_render 3.0% |
| `Random.random` / `math.floor` / `getrandbits` | 1,013,437 / 953,437 / 399,365 | 0.4/0.3/0.2% | report_render 4.2/3.8/2.3% |
| `int(str)` (constructor; not in cProfile) | 813,036 in log_analytics alone | – | p50 2 chars |
| `dict.get` | 631,744 | 0.4% | state_snapshot 2.2% |
| `id` | 613,451 | 0.3% | state_snapshot 3.7% (deepcopy memo) |
| `str.split` | 323,335 | 0.7% | text_corpus 3.1% |
| `Pattern.match` | 307,713 | 0.8% | log_analytics 3.9%, fs_inventory 3.5% |
| `Decimal.quantize` | 273,434 | 0.3% | billing_engine 3.0% |
| `Element.findtext` | 250,542 | 0.4% | xml_feeds 4.6% |
| `datetime.astimezone` / `strftime` | 229,875 / 200,282 | 0.8 / 3.3% | tz_events 7.2 / 23.1% |

Calls per run: 0.1M (backup_archive) to 7.3M (text_corpus); 33–92% of them are builtin
calls. cProfile slows runs 0.9–3.6× (most for call-heavy text/yaml/report code).

### 2.6 Argument sizes (what a kernel would see)

| hotspot | argument | p50 | p90 | max | note |
|---|---|---|---|---|---|
| `datetime.strftime` | format | 2–14 chars | 20 | 20 | constant: `'%Y-%m'`, `'%Y-%m-%d %H:00'`, `'%A'`, `'%a %d %b %Y %H:%M %Z'` |
| `_strptime._strptime` | data / format | 16–26 / 14–20 chars | 26 / 20 | 26 / 20 | 1–3 distinct formats per workload (`'%d/%b/%Y:%H:%M:%S %z'`, `'%Y-%m-%d %H:%M'`, `'%m/%d/%Y %I:%M %p'`) |
| `int(str)` | string | 2 chars | 6 | 6 | 813k calls in log_analytics (strptime fields, IP octets) |
| `str.strip` / `split` / `isascii` | self | 9 / 12–45 / 3 chars | 27 / 15–90 / 6 | 30 / 1.5k / 9 | ASCII in 42–100% |
| `Pattern.match` (log line) / `search` (UA) | subject | 136 / 71 chars | 237 / 114 | 318 / 135 | patterns 39–246 chars |
| `fnmatch` → `Pattern.match` | name / pattern | 10 / 5 chars | 13 / 10 | 16 / 10 | 12 patterns, 69k calls |
| `Pattern.findall` / `split` (documents) | subject | 873 chars | 1218 | 1522 | non-ASCII |
| `textwrap` `wordsep_re.split` | pattern / chunks | 483 chars / 45 chunks | 137 | 159 | |
| `json.dumps` (per record) | object | 28 nodes | 30 | 103 | 32.7k calls, `sort_keys`, `ensure_ascii=False` |
| `copy.deepcopy` | top-level graph / memo | 368k nodes / 93k | 169k | 188k | 733k recursive calls per top-level call |
| `pickle.dumps` / `loads` | object / bytes | 368k nodes / 6.1 MB | | | 1–2 calls per run |
| `posixpath.normcase` / `join` | path | 9 / 27 chars | 12 / 55 | 16 / 70 | 142k / 25k calls |
| `random.choices` / `_randbelow` | population / n | 4–35 / 6 bits | 35 | 35 | |
| `Fraction.__new__` | num / den | 1 / 5 bits | 5 / 5 | 117 / 107 | |
| `hashlib.sha256` / `zlib.crc32` | data | 24.7 KB / 12.9 KB | 40 KB | 1.9 MB | |
| `ElementTree` find paths | path | 7–12 chars (`'g:price'`) | 25 | 25 | namespaced → pure-Python `ElementPath` |

## 3. Accelerable behind an existing entry point vs. interpreter work

Ranked by family mean (11 workloads) and by how plausibly Rust beats what runs today.
"Overlap" = upstream "Rust for CPython" candidates (zlib 3.16; io, json, xml for 3.17).

| # | entry point (existing) | today | mean / max | why it may pay | exactness risk / notes |
|---|---|---|---|---|---|
| 1 | `copy.deepcopy` | pure Python | 4.6% / 50% | 733k Python calls per graph; hint 0.22× (*earlier round*) | `__deepcopy__`/`__reduce_ex__`/`copyreg` protocol order, memo identity, `_keep_alive`; fall back on any custom hook |
| 2 | `datetime.strftime` (+ `isoformat`) | C, but imports `time.strftime` and calls `timetuple()` per call | 4.8% / 31.5% (+ isoformat 0.9%) | short constant formats; fixed-field formatting | locale (`%a %b %A %B %p %c %x`) → only C/POSIX `LC_TIME`; `%Z`/`%z` via tzinfo calls; subclass `timetuple` overrides |
| 3 | `datetime.strptime` → `_strptime._strptime` | pure Python (+`re`, `locale.getlocale` per call) | 3.5% / 28.6% | 26-char inputs, 1–3 formats; hint 0.01× | locale-dependent names, error messages, `%z`/`%Z` rules, `DeprecationWarning` for day-without-year; C locale only |
| 4 | path stack: `posixpath.join/normcase/relpath/splitext`, `fnmatch.fnmatch(case)`, `glob._iterdir`, `pathlib.PurePath` parsing/str, `os.walk` | pure Python | 7.5% / 83% (of user CPU; fs_inventory is 41% kernel) | tiny strings, huge call counts (142k `normcase`, 69k `fnmatch`) | bytes paths, `os.fspath` protocol, `PathLike` subclasses; `fnmatch` must keep `re` cache semantics |
| 5 | `re` pattern methods (`_sre`) | C | 7.2% / 25% | used by user code and stdlib (`textwrap`, `fnmatch`, `_strptime`) | a bit-exact `sre` port is a large project; regex-crate semantics differ (*inferred*): defer, but specialise trivial patterns (literal/prefix, `fnmatch`-translated) |
| 6 | `json.dumps`/`JSONEncoder` glue + `encoder_call` | Python glue + C | 3.0% / 29.4% | per-call encoder construction for non-default kwargs; hint 0.23× | `default=`, `sort_keys` ordering, float repr, subclass hooks; upstream json candidate for 3.17 |
| 7 | `textwrap.TextWrapper` (`_split`, `_wrap_chunks`) | Python + 483-char regex | 3.6% / 39% | whole `fill/wrap/shorten` in one call | all `TextWrapper` options, `break_on_hyphens`, Unicode whitespace |
| 8 | `random.Random` methods (`choice/choices/sample/_randbelow/gauss`) | pure Python over C `random()` | 2.7% / 29.6% | ~1M `random()` + `floor()` calls per run | must consume the Mersenne Twister stream identically; subclass overrides of `random()`/`getrandbits()` |
| 9 | ElementTree pure-Python layer: `ElementPath` (`find*` with namespaces), `_serialize_xml`, `XMLPullParser` | pure Python over C parser | ≈ 2.1% interp / ≈ 27% of xml_feeds | every `find*` with a `namespaces=` map runs the Python `ElementPath` (57k calls/run) | upstream xml candidate for 3.17 |
| 10 | `csv.DictReader.__next__` | pure Python over C reader | 1.8% / 20.1% | `dict(zip(fieldnames, row))` per row | `restkey`/`restval`, subclassing |
| 11 | `ipaddress` (`ip_network`, `IPv4Address`, `__str__`) | pure Python | 1.8% / 20.2% | 180k `_parse_octet`, 45k networks per run | exception messages, `strict` semantics |
| 12 | `pickle.dumps/loads` | C | 2.5% / 27.8% | 368k-node graphs | already C; gain unclear (*inferred*) |
| 13 | zlib deflate/inflate, crc32 | C (classic zlib) | 8.3% / 91% | `longest_match` 70% of backup_archive | compressed bytes must be identical to this zlib → only a bit-exact port of its matcher; inflate/crc32 are free; upstream overlap (3.16) |
| 14 | `uuid.UUID` init/str, `fractions.Fraction`, `statistics`, `urllib.parse.urlsplit/parse_qs`, `configparser`, `tomllib`, `enum.Flag.__contains__` | pure Python | 0.4–1.4% each / up to 16% | | each small; batch later |
| – | `Counter.__setitem__` (slot wrapper due to Python `__delitem__`) | type slot | 0.7% / 2.7% | 52 → ~20 ns per store | per-type slot; niche |
| – | user-code interpretation, third-party Python (PyYAML), cyclic GC | interpreter | 27% / 76% | not reachable through stdlib entry points | PyYAML users can opt into `CSafeLoader`; GC is allocation-driven |

Builtin type methods (`str.*`, `dict` slots, `list.append`, `isinstance`, `len`) total
14% but run on 2–30-character inputs at 20–60 ns per call: CPython's C is already near
the floor there (*inferred* from the arg sizes and the container round's experience);
only fused paths (e.g. `int(str)` for 1–6 digit ASCII, `str.strip/split` fast paths) could
move them, and only by single-digit percents of a workload.

## 4. Per-workload profiles

Shares are of all samples in the steady-state `run()` loop (perf, `-X perf`). "C entry"
= inclusive time under that C function when called from bytecode.

### 4.1 `csv_etl` (0.63 s)
- C entries: `encoder_call` 18.0% (from json), `datetime_date_strftime` 12.8% (`'%Y-%m'`),
  `Reader_iternext` 9.7%, `datetime_datetime_isoformat` 6.4%, `dict(zip())` in
  `DictReader` 4.2%, dict subscripts 3.9%.
- Python: `JSONEncoder.iterencode` 23.0%, `DictReader.__next__` 19.2%, `json.dumps` 4.0%.
- Cells: workload × interpreter 10.4%, × memory/gc 7.8%, × libc 7.1% (sprintf in
  isoformat/strftime), × dict 6.5%. 1.1M calls/run, 77% builtin.

### 4.2 `log_analytics` (1.36 s)
- `_strptime` 25.4% (+ `locale.getlocale` 3.2% called from it on every parse), `ipaddress`
  20.2%, `urllib.parse` 6.6%.
- C entries: `Pattern.search` 9.6% (UA classification), `strftime` 9.0%, `Pattern.match`
  6.0%, `groupdict` 3.9%.
- Cells: workload × re-engine 12.8%, _strptime × interpreter 11.6%, ipaddress ×
  interpreter 9.0%. 4.4M calls/run; `int(str)` 813k calls on 1–6-char strings.

### 4.3 `tz_events` (0.74 s)
- `datetime_date_strftime` 31.5% (122k calls; `'%A'` and `'%a %d %b %Y %H:%M %Z'`),
  `astimezone` 7.3%, `list.sort` of datetime tuples 4.7%, `timedelta()` 4.6%,
  `isoformat` 3.7%, `_strptime` 8.2%.
- Cells: workload × memory/gc 16.8%, × interpreter 12.0%, × calls 10.4%, × datetime 9.5%.
- Without memoised `ZoneInfo` lookups the same job spent ≈ 40% re-reading tz files (§6).

### 4.4 `text_corpus` (0.80 s)
- C entries: `findall` 15.8%, `Counter.update` → `_count_elements` 10.4%,
  `str.translate` 7.0%, `re.split` 6.4%, `str.split` 5.9%, `normalize` 5.1%+2.0%,
  `str.join` 4.4%.
- User code 29.7%: the `strip_accents` generator (`c for c in s if not combining(c)`,
  2.76M one-character calls) alone is 17.5% — a classic unreachable idiom.

### 4.5 `fs_inventory` (0.40 s; user 0.24 s + **sys 0.16 s**)
- 83% of user-space samples are in the pure-Python path stack: pathlib 25.2%, glob 24.7%,
  fnmatch 14.0%, posixpath 11.0%, genericpath 3.8%. Interpreter-leaf samples 44.9%.
- C entries: `Pattern.match` from fnmatch 6.4%, `scandir` iteration 4.8%, `lru_cache`
  wrapper 2.2%. Kernel time (stat/getdents) is outside every user-space accelerator.

### 4.6 `backup_archive` (0.68 s)
- zlib 91.2% (`longest_match` 71%, `deflate_slow` 11%), hashlib 3.0% (sha256 1.8%,
  md5 1.2%), everything Python < 4%. Level 6/9 deflate of text dominates any archive job.

### 4.7 `config_merge` (0.74 s)
- PyYAML pure-Python loader/dumper 79.8% (61% interpreter-leaf: `Scanner`, `Reader`,
  `Parser`), configparser 7.9%, tomllib 4.4%, json 2.5%. `-X perf` slowed it 1.42× (most
  call-heavy workload). Accelerating stdlib entry points cannot reach PyYAML (*inferred*).

### 4.8 `xml_feeds` (0.58 s)
- `XMLParser.feed` 25.3% (expat 13.1% + tree building), GC 16.0% (Element objects),
  `ElementPath` find/iterfind/findtext 18.1% (lookups with `namespaces=` always take the
  Python path), `_serialize_xml` 8.6%, `iterparse` events 3.7%.

### 4.9 `state_snapshot` (0.78 s)
- `copy` 48.5% (`deepcopy` 26.1%, `_deepcopy_dict` 12.3%, `_reconstruct` 6.0%),
  `pickle.dumps` 21.5% + `loads` 6.2%, GC 17.4%, generated dataclass `__eq__` 6.0%,
  `asdict` 1.4%.

### 4.10 `billing_engine` (1.08 s)
- GC 36.7% (40k invoices × lists of slotted `Line`s), user code 27.0% (`rate()` tier
  loop), Decimal 12.7% (`Decimal(str)` 4.9%, `quantize` 1.8%, arithmetic via
  `PyNumber_*`), fractions 10.0%, statistics 5.8% (`median` sort of 40k Decimals),
  `enum.Flag.__contains__` 1.4%.

### 4.11 `report_render` (0.53 s)
- `textwrap` 39.3% (`wordsep_re.split` 24.7%, `_wrap_chunks` 8.6%), `random` 29.6%
  (`choices` 14.4%, `sample` 6.5%, `_randbelow` 4.3%), `uuid` 9.8% (`UUID.__init__`
  5.5%), workload 20.2% (f-strings, `format`, `%`, dict lookups). 5.7M calls/run, 86%
  builtin.

### 4.12 `pandas_etl` (0.62 s)
- Native split (plain perf): pandas C/Cython 44.5%, NumPy 13.9%, libc 3.0%; CPython
  itself 38.7%: str 9.0% (`.str` accessor runs Python `str.strip/split/rstrip` 192–245k
  times and `re.search` 192k times), attr/type 7.7% (`PyType_IsSubtype` from Cython),
  memory/gc 7.5%, interpreter 6.7%. Pandas-heavy code leaves ≈ 10% for
  builtins/stdlib-level acceleration (*inferred* from this split).

## 5. Method and reproduction

| step | what | notes |
|---|---|---|
| interpreter | CPython 3.15.1 PBS (PGO+LTO+BOLT, tail-calling interpreter), venv `~/rustique-research/w-data/venv` | shared 8-CPU WSL2 box with other agents running |
| steady state | each measurement in a fresh process: `setup()`, one warm-up `run()`, then measure | |
| timing | `run()` looped ≥ 4 s (≥ 5 runs): median wall, user/sys CPU per run (`getrusage`) | spread in each JSON's `timing` |
| perf (owners) | `perf record -e cpu-clock -F 1999 -g` on `python -X perf`, gated by a perf control FIFO to the `run()` loop; 5.3k–9.4k samples | owner = innermost Python frame (perf-map trampoline); component = leaf symbol |
| perf (plain) | same without `-X perf` | `-X perf` / plain wall: 0.92–1.42×, median 1.04× |
| cProfile | one `run()` | constructors (`int(s)`, `Decimal(s)`) are invisible to it |
| arg sizes | one `run()` with `sys.monitoring` CALL (callable + arg0) and PY_START (all args of pure-Python library functions from frame locals), plus call-site probes inserted into the workload's own AST (all args of every call it makes) | top 10 hotspots by cProfile tottime + busiest constructors; C methods called *from library code* show only `self` |
| self-check | every instrumented run's digest equals the plain one | 12 × 4 passed |

Symbolization: this box's `perf` has no libelf and resolves nothing, so `_profiler.py`
reads `perf script -F tid,period,ip,sym,dso,dsoff`, maps the DSO file offset to a vaddr
via the DSO's `PT_LOAD` headers and looks it up in `nm -n -S`. That handles the BOLT
layout too (`perf report` shows those IPs as file offsets; `perf script`'s `ip` is
already the vaddr). Validated on `json.dumps` of 200k small dicts: 99.98% under
`encoder_call`, leaves `_Py_dg_dtoa` 14%, `encoder_listencode_obj` 14%,
`long_to_decimal_string_internal` 5%, `_PyUnicodeWriter_WriteStr` 5%. libc internals are
named by their caller (`[libc.so.6]<-PyOS_snprintf`).

Components: the brief's prefix rules first (incl. `*_dealloc`/`*_traverse` → memory/gc),
then a name → file index of the CPython 3.15.1 sources (19k functions; e.g.
`Modules/_datetimemodule.c` → datetime), then library prefixes (zlib, OpenSSL/HACL →
hashlib, expat), then the DSO (numpy, pandas(C/Cython)). Added components:
`perf-trampoline`, `abstract`, `runtime`, `import`, `generators`, `exceptions`,
`iter/range/slice`, `builtins-module`, one per C module (datetime, decimal, zlib, hashlib,
expat, elementtree, csv-accel, io, os, unicodedata, collections, random, functools…);
argument parsing/building (`vgetargs*`, `_PyArg_*`, `do_mkvalue`) → `calls`; string
hashing → `str`. *C entry point* = outermost native frame below the innermost
interpreter frame, skipping generic call wrappers (`type_call`, `cfunction_*`,
`PyObject_Call*`, `builtin_next`…); entries that are interpreter-side work
(`_Py_Dealloc`, `PyObject_GenericGetAttr`, `slot_tp_*`, call dispatch) count as
interpreter. *Cyclic GC* = a collection frame on the stack.

Limits: user-space sampling only (`perf_event_paranoid=2`): kernel time is the `sys`
column. Frame-pointer unwinding stops in pandas/NumPy and in md5's assembly (owner lost).
cProfile adds 0.9–3.6×; the probe run 2–53×.

Reproduce (≈ 1 min per workload):

```sh
cd benches/workloads/data
uv venv --python 3.15 ~/rustique-research/w-data/venv
uv pip install --python ~/rustique-research/w-data/venv/bin/python -r requirements.txt
PY=~/rustique-research/w-data/venv/bin/python
for w in csv_etl log_analytics tz_events text_corpus fs_inventory backup_archive \
         config_merge xml_feeds state_snapshot billing_engine report_render pandas_etl; do
  $PY _profiler.py profile $w.py --python $PY --out ~/rustique-research/profiles/data/$w.json --keep
done
$PY _profiler.py rollup ~/rustique-research/profiles/data/*.json --out rollup.json
# after changing the classifier: _profiler.py reparse <json> --workload <w.py> --tmpdir <kept dir>
```

## 6. Side findings

| finding | evidence |
|---|---|
| `Counter[k] = v` costs 52 ns vs 20 ns for a plain dict subclass: `Counter.__delitem__` is Python, so the shared `mp_ass_subscript` slot becomes `slot_mp_ass_subscript` → `vectorcall_method` → `wrapperdescr_call` for every store | timeit, min of 5; perf chain in log_analytics; cause *inferred* from `Lib/collections/__init__.py` |
| `ZoneInfo(key)` keeps only 8 zones strongly; with 10 in rotation each run re-read tz files 4,775× (zoneinfo + pathlib + importlib.resources ≈ 40% of samples). `tz_events` now memoises with `lru_cache`, as careful scripts do | first-round profile of `tz_events` (12k events) |
| `json.dumps` with any non-default kwarg constructs `JSONEncoder` and a C encoder per call (`encoder_new` 1.2% of csv_etl) | perf + `Lib/json/__init__.py` |
| `datetime.strftime` imports `time.strftime`, calls `self.timetuple()` (a `struct_time`) and goes through libc `strftime` + `PyOS_snprintf` per call | perf chains + `Modules/_datetimemodule.c:wrap_strftime` |
| GC share confirmed by `gc.disable()`: billing 28.9%, state_snapshot 13.3%, xml_feeds 13.9%, config_merge 3.0% | median of 5 alternating runs |
| On 3.15, bound `METH_METHOD` methods (e.g. `re.Pattern.match`) are a new `builtin_method` type (subclass of `builtin_function_or_method`): `type(f) is BuiltinFunctionType` is False — relevant to any tooling or patching that type-checks callables | `type(re.compile('a').match).__mro__` |
