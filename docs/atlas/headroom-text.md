# Headroom & tractability: text and serialization

Phase 1, round questions 2–3, for `json`, `re`, `str`/`bytes` methods, number↔text,
`base64`/`binascii`, `csv`, `html`, `urllib.parse`, `http.client`/`email` headers, `tomllib`,
`xml.etree`, `string.Template`, `textwrap`, `shlex`, `tokenize`, `unicodedata`, plus `pickle`
and `struct` (added). Every number is **measured** (tool named) or **inferred** (basis
named). CPython 3.15.1 (python-build-standalone, PGO+LTO+BOLT), local WSL2 box shared with
other agents, 2026-11-01.

## 0. Method (how to reproduce)

All scripts and raw data: `~/rustique-research/h-text/` (`bench/`, `results/`, `proto/`).

| what | how |
|---|---|
| cells | `bench/cases.py` (+ `cases_extra.py` with `HT_EXTRA=1`): 146 cells × 1–9 variants, stdlib first. Inputs `bench/inputs.py`: *small* = what real code mostly does (10-field record, 40–86-char line, one URL, 32 bytes); *medium* = 1 KB / 20 records; *large* = ~1 MB text, 1000 records, 10k numbers. Non-ASCII inputs are fresh objects per cell (a cached UTF-8 form would turn `encode()` into a memcpy). |
| equivalence | `run.py check`: every variant evaluated once and compared with stdlib (`results/` lists which differ and why: orjson is compact/UTF-8/bytes, markupsafe escapes `'` as `&#39;`, …). Prototype variants marked *proto* produce stdlib-identical results for the cells shown. |
| instruction counts (primary for small cells) | `run.py cg`: callgrind with instrumentation only around the measured loop (client requests), runs of n and 2n iterations differenced, empty-loop cost (113 Ir) subtracted; per-function self cost bucketed into the brief's components (BOLT `.warm`/`.cold` fragments mapped back; additions: `json-accel`, `re-engine`, `xml-accel`=expat+_elementtree, `csv-accel`, `tokenizer`, `io`; glibc `malloc`/`free` counted as memory/gc). "Glue" = interpreter + calls + attr/type = Python-level code and call machinery. Files: `results/cg*.json` (with raw per-function data in `cg_stdlib*.json`). |
| wall time (large cells) | `run.py wall`: each variant timed with a ~40 ms loop, 9 repeats interleaved across variants; per-repeat ratio variant/stdlib → median [min, max]. The box was loaded (load 3–5): small-cell wall ratios are noisy (spreads up to ±50 %), so small cells are judged on Ir. Ir understates SIMD kernels (e.g. `str.count` 1 MB: Ir 0.39, wall 0.13), so large cells are judged on wall time. |
| prototypes | `proto/` (PyO3 0.30.1, Rust 1.99.1, `CARGO_BUILD_JOBS=2`, thin LTO): throwaway kernels for str methods, JSON encoding (exact `json.dumps` output for exact builtins), float repr (rustique-core `pyfmt`, ryu, zmij + CPython layout), html.escape, quote, NFC, Rust `regex` bindings (str and byte-level). Called as module functions, so they pay a PyO3 call (~150–400 Ir) that an `ml_meth` swap would not. |
| re corpus | `bench/re_classify.py` and `bench/re_diff.py` on the 12,190 literal patterns of `~/rustique-research/static/results/re_patterns.jsonl`. |
| tests | `python3.15 -m test --list-cases <mod>` against `~/src/cpython-3.15.1/Lib/test` (PBS ships without `test`). |

Reproduce: `cd ~/rustique-research/h-text/bench; ../venv/bin/python run.py {check|wall|cg} [-k cell,...] -o out.json`
(`HT_EXTRA=1` second pass, `HT_STDLIB_ONLY=1` stdlib-only callgrind); `report.py` tabulates.

## 1. Summary table

Ratios are alternative ÷ stdlib (lower = alternative faster). *Small* ratios are callgrind Ir;
*large* are wall medians. "Achievable" = a Rust drop-in that must return identical objects and
honour every hook, assuming it can take over the stated entry point; tagged.

| candidate | entry-point kind | stdlib → best alt, small / large | where stdlib time goes (small; large) | exact-subset coverage (guard) | CPython tests | main traps | risk | upstream overlap | achievable drop-in speedup small / large |
|---|---|---|---|---|---|---|---|---|---|
| **float → str** (`repr`/`str`/`__format__('')`, JSON floats) | slot `PyFloat_Type.tp_repr` (static type) + `float.__format__` `ml_meth` | n/a (no drop-in alt); zmij/ryu+layout proto 0.27 (10k list, wall & Ir) | `_Py_dg_dtoa`+`lshift`+`PyOS_double_to_string` = 77 % of `repr(x)` (3,259 Ir); 87 % of a 10k list | all finite floats (inf/nan → original); zmij & ryu == CPython on 3.3 M values | test_float 54, test_format 19, test_strtod 9, json, fstring 95 | layout (1e16 / 1e-4 cut-offs, `.0`), ties-to-even, subclasses copied the old slot, `float.__repr__` wrapper keeps the original | **low** | none known | **3–3.5× / 3.5–4×** (*inferred* from Ir split; list *measured* 0.27) |
| **json.dumps** | pure-Python `json.dumps` → `JSONEncoder.encode/iterencode` → C `_json.make_encoder` (heap type, `tp_call`) | 0.14 (orjson→str) / 0.20 (orjson→str); exact Rust proto 0.31 / 0.60 | glue 30 % (c-encoder-only 0.70), float 19 %, json-accel 17 %, str 15 %; large: float 32 %, json-accel 30 % | exact dict/list/tuple/str/int/float/bool/None, default or common kwargs; hooks callable at the same points | test_json 237 (C and Py) | `default`/`cls`, subclass rules (`PyMapping_Items` on dict subclasses), circular markers, RecursionError, exception notes, `int_max_str_digits`, FT critical sections, monkeypatched `c_make_encoder` | medium | **json is a Rust-for-CPython 3.17 candidate** | 3–7× / 3–5× with `json.dumps` taken over; 2.2–2.5× / 3× with only the C encoder (*inferred*); floats-only 5–15× |
| **json.loads** | pure-Python `json.loads` → `JSONDecoder.decode` (2 regex matches) → C scanner (heap type) | 0.30 (msgspec, orjson) / 0.41 (orjson) | json-accel 21 %, dict 21 %, memory 15 %, glue 16–28 % (scanner-only 0.72); large: object creation ≥ 50 % | any `str`/`bytes` input without hooks; hooks called in CPython's order | test_json 237 | `JSONDecodeError` msg/pos/lineno/colno, key memo identity, `object_hook`/`object_pairs_hook`/`parse_*`/`array_hook` (new in 3.15), big ints, `strict` | medium | json 3.17 candidate | 2.5–3× / 2–2.5× (*inferred* from msgspec/orjson) |
| **re** module functions | pure-Python `re.match/search/sub/…` → Python `_compile` cache → C `Pattern` methods | regex module is slower (1.4–11×); n/a | `re._compile` cache hit alone = 42 % (2,140 of 5,037 Ir) | every call (cache is internal; keys `(type, pattern, flags)`) | test_re 168 + 414 `re_tests` rows | `re.purge`, `RegexFlag` keys, `DEBUG` flag, bytes/str keys | low | none | **1.7–1.8× / –** (*inferred*: compiled call = 0.55) |
| **re** engine (search/findall/sub/split) | static `PyMethodDef pattern_methods` in `_sre` (`ml_meth` swap) | Rust `regex` proto 0.65 (search, span only) / **0.01–0.08** sparse, 0.31–0.67 dense | small: engine 43–57 %, Match/state alloc 28–43 %; large: engine 47–100 % | 95 % of real patterns regular; **87 % (7,058/8,078) ran exact, 0 mismatches in 2.82 M pairs** (ASCII subject + final-`\n` guard) | test_re 168 | Unicode 17 vs crate tables (`\w` differs on 8,214 cps, `\d` on 10, `\s` on `\x1c-\x1f`), `$` before final `\n`, `\<` meaning, `re.I` (`İ`), empty-match iteration, UTF-8 cache side effect, Rust compile 34× slower | medium-high | none | 1.0× small (keep sre) / **4–27× at 1 KB, 13–90× at 1 MB sparse**, 1.2–2× dense (*measured* proto; dense *inferred*) |
| **str** methods (`replace`, `find`, `count`, `lower`/`upper`, `splitlines`, `split(sep)`) | static `PyMethodDef unicode_methods` (+ `sq_contains`) | n/a; proto 1.0–1.9 (PyO3 call) / **0.07–0.13** (1 MB), 0.3–0.65 (1 KB) | small: call + result allocation; algorithm ≤ 50 % of 300–900 Ir; large: 84–100 % algorithm | ASCII (1 flag read) and UCS1 with CPython-derived 256-entry tables; length guard | test_str 139, string_tests mixins, test_userstring 71 | Unicode 17 case tables, special casing (ß, final sigma, İ), identity of unchanged results, `\x1c-\x1f`/`\x85`/`\xa0` whitespace, UTF-8 cache | low-medium | none | 1.0× small / **1.5–3× at 1 KB, 7–15× at 1 MB** for replace/find/count/case (*measured* proto), `split(sep)`/`splitlines` 1.7–2.2×; Latin-1 `lower` 20–50× (*inferred*: 54 vs ~1–2 Ir/char) |
| str `split()`, `join`, `encode`/`decode` ASCII, formatting | same | proto 1.0–1.1 (split), 1.0 (encode=memcpy), decode 0.43 | allocation of result objects; `memcpy` | — | test_codecs 288, test_fstring 95 | — | low | none | ≤ 1.1× except UTF-8 decode ~2× large (*measured* proto) |
| **bytes/bytearray** | static `PyMethodDef bytes_methods`/`bytearray_methods` | stringzilla 0.22–0.37 (find/count 1 MB) | same `stringlib` code as str (identical Ir) | all (no Unicode); bytearray needs FT locking | test_bytes 159 defs (PBS lacks `_testlimitedcapi`, so it skips) | bytearray mutation under FT, buffer exports | low | none | same as str ASCII; `fromhex` ~1.3× (*measured* `unhexlify` 0.67) |
| **int ↔ str** | `tp_repr`/`tp_str` slot, `int()` type call | gmpy2 0.23 / 0.17 (≈4,000 digits) | small: `long_to_decimal` 44 %, alloc 23 %; big: quadratic conversion | all; huge ints limited by `int_max_str_digits` | test_long 47, test_int 52 | limit + message, `_pylong` thresholds | low | none | ≤ 1.2× small, 2× for 10k lists (proto itoa 0.51 Ir) / 3–5× big (*measured* gmpy2) |
| **base64** | pure-Python `base64.b64*` → static `binascii` functions | pybase64 0.89 (enc) 0.51 (dec) / **0.07** enc, 0.63 dec | Python wrapper 35 % (enc), **72 % (dec)** of the call; large: codec 100 % | all; 3.15 parameters fall back | test_base64 67, test_binascii 237 | new 3.15 params (`padded`, `wrapcol`, `ignorechars`, `canonical`), Deprecation/FutureWarnings | low | 3.15 already 2–3× faster; base64 was the pre-PEP's demo module (*inferred*) | 1.5–3× small (needs Python entry) / 10–15× enc, ~1.6× dec |
| **csv** | C `_csv` reader `tp_iternext`, `writerow` | none drop-in | csv-accel 46 %, str creation 27 %, memory 13 % | dialect default | test_csv 140 | dialects, quoting, `field_size_limit`, line numbers | medium | none | 1.3–2× (*inferred*) |
| **html.escape** / unescape | pure Python (5 × `str.replace`; `re.sub` + Python callback) | markupsafe 0.69 (different output) / 0.48; proto **0.23–0.30** / 0.42 | escape: 5 scans + 5 allocations, glue 27–43 %; unescape: re + Python callback per entity | exact str input (fall back on subclasses/duck types) | test_html 2 (!), test_htmlparser 68 | identity when unchanged, subclass copies, duck-typed `.replace` | low | none | escape 3–4× / 2.4× (*measured* proto); unescape 5–10× (*inferred*) |
| **urllib.parse** (`quote`, `unquote`, `urlencode`, `parse_qsl`, `urljoin`, uncached `urlsplit`) | pure Python (`urlsplit` is `lru_cache`d) | proto `quote` **0.12–0.16**; yarl slower (1.9×) | glue 52–67 % (interpreter 53–57 %); 1–76 k Ir per call | str input, default encoding/errors | test_urlparse 81, test_urllib 106 | `lru_cache` observability, `missing_as_none` (3.15), bytes inputs, warnings | medium | none | 5–8× quote (*measured* proto); 3–10× others (*inferred*) |
| **http.client / email headers** | pure Python (`http.client.parse_headers` → `email.feedparser`) | none | 256 k Ir (23 µs) for 11 headers; glue 55 %, spread over ~30 functions | clean ASCII `Name: value` lines, no continuations | test_httplib 151, test_email 1,817 | `Message` internal state (`_headers`, `defects`, `policy`), folding, surrogateescape, limits | high | none | 5–15× (*inferred*) |
| **tomllib** | pure Python | rtoml **0.12** / 0.11; tomli (mypyc-compiled same code) 0.38 / 0.39 | interpreter 81–86 % | TOML 1.1 documents (3.15) | test_tomllib 19 + 64 data files | TOML 1.1 vs rtoml 1.0, `TOMLDecodeError` positions, `parse_float` | medium | none | 5–10× (*inferred*: rtoml), 2.6× from compiling alone (*measured* tomli) |
| **xml.etree** parse | C `_elementtree` + expat | lxml 0.73 / 0.83 | expat + tree builder 39–60 %, dict/alloc | — | test_xml_etree 238 + _c 256 | — | — | xml 3.17 candidate | ≤ 1.3× (*measured* lxml bound) |
| **xml.etree** `tostring` | pure Python `_serialize_xml` | lxml **0.10** / 0.09 | glue 67 % | default method/namespaces | same | namespace map, `short_empty_elements`, html/text/c14n methods, Element subclasses | medium | xml 3.17 candidate | 5–9× (*inferred* from lxml) |
| `string.Template` | pure Python (`re.sub` + callback) | `str.format` ref 0.32 | re 28 %, glue 28 % | default delimiter/idpattern | test_string 55 | subclass `delimiter`/`pattern` overrides | low | none | 2–3× (*inferred*) |
| `textwrap.wrap` / `dedent` | pure Python (regex chunking) | none | 1.09 M Ir (71 µs) / 1.0 M Ir; re 48 %, glue 27 % | default options | test_textwrap 68 | many options, width semantics | medium | none | 5–20× (*inferred*) |
| `shlex.split` | pure Python char-by-char lexer | none | 313 k Ir (21 µs), glue 78 % | posix=True defaults | test_shlex 45 | `ValueError` messages, `punctuation_chars` | low | none | 10–30× (*inferred*) |
| `tokenize` | Python generator over C `_tokenize.TokenizerIter` | none | 10 M Ir/500 lines: glue 51 %, C tokenizer 15 % | — | test_tokenize 132 | `TokenInfo` exactness, encodings | medium | parser is a 3.17 candidate | ≤ 2× (*inferred*) |
| `unicodedata.normalize` | static C function | unicodedata2 1.3–6.5×, unicode-normalization crate 1.3–2.8× **slower** | quick-check returns early; table walk | — | test_unicodedata 73 | Unicode version | — | none | **none** (*measured*: alternatives slower) |
| `pickle` (added) | C `_pickle` | msgspec msgpack (other format) 0.27 / 1.0 | Pickler setup per call dominates small | — | test_pickle 1,081 | protocol/memo exactness | medium | none | 1.2–2× small, ~1× large (*inferred*) |

## 2. Cross-cutting findings

1. **Small calls are dominated by things a C-entry swap cannot remove.** For pure-Python
   entry points the Python glue is 28–87 % of a realistic call (json.dumps 30 %, json.loads
   28 %, `re.match(P, s)` 45 %, `base64.b64decode` 72 % — each = 1 − Ir(C part called
   directly)/Ir(public call); urllib 52–67 %, tomllib 84 %, `xml tostring` 67 %, shlex 78 % —
   interpreter+calls+attr components; *measured* Ir). Rustique's current mechanism (swap
   `ml_meth`/slots of C code) reaches only the C part. The large small-call wins need the
   pure-Python function itself to be taken over (owned by the plug-in-mechanisms slice); the
   guard (kwargs default, module globals unmonkeypatched, exact input types) costs ~20–50 Ir
   against ≥ 2,000 Ir of glue, so the fallback stays ≤ 1.02 (*inferred*).
2. **C-implemented small operations are at the floor.** `str.find`/`count`/`lower`/`encode`
   on ≤ 100 chars cost 330–900 Ir of which call dispatch is 20–60 % and result allocation
   most of the rest (*measured*); a kernel can save at most the scan (≤ 1.1×, *inferred*).
   Bulk gains start around 1 KB (`lower` 3.2×, `replace` 2.9×, `count` 2.3×, wall proto) and
   reach 7–15× at 1 MB. A length guard (~5 Ir) keeps small calls at 1.00.
3. **Float → text is a hidden hot spot.** `repr(1.23)` is 3,259 Ir, 77 % in David Gay's
   bignum `_Py_dg_dtoa`; floats are 19 % of `json.dumps` of a typical record, 32 % of 1000
   records, 86 % of a float array, and 37 % of `f'{k}={v} t={t}ms'`. zmij and ryu (with
   CPython's layout) produced **identical** strings on 3.0 M random doubles + all 2,098 powers of
   two (vs rustique-core `pyfmt`) and 300 k values compared with `repr` (*measured*,
   `proto check_float_algos`), at 0.27 of the stdlib's Ir and wall for a 10k list even with a
   crude string re-layout. Note: rustique-core's own `pyfmt` is exact but only 2× faster
   (241 ns/op in pure Rust): it needs a ryu/zmij core before it is a speed asset.
4. **Two exactness side effects every text kernel must avoid**: `PyUnicode_AsUTF8` caches a
   UTF-8 copy inside non-ASCII str objects (`sys.getsizeof` 457 → 958 bytes, *measured*), so
   kernels must read PEP 393 buffers directly; and unchanged results are returned by identity
   for exact str (`s.replace` with no hit, `html.escape` of clean text) but copied for subclasses.
5. **The regex design that is exact by construction:** let a DFA (Rust `regex-automata`) find
   the leftmost match *start*, then let sre produce the `Match` with one anchored attempt
   (`pattern.match(s, start)`). For regular patterns the set of positions where a match exists
   is a property of the language, so the start is identical; groups, `lastindex`, the Match type
   come from sre itself. Validated on the PyPI corpus: 7,058 of 8,078 distinct patterns
   translated, **0 start and 0 Match mismatches over 2,823,200 (pattern, subject) pairs**
   (ASCII subjects; `$` + subject ending in `\n` guarded, 2,080 pairs) (*measured*,
   `re_diff.py`). The quick translator needed three fixes found by this test (`\s` under
   `re.ASCII`, final-newline `$`, `\<` is a word boundary in Rust); a production translator
   should start from sre's parse tree.
6. **Already fast, little headroom:** `unicodedata.normalize` (quick-check; crate alternatives
   1.3–6.5× slower), `xml.etree` parsing (lxml only 1.2×), UTF-8 encode/decode of non-ASCII
   (my naive kernels were 1.4–4.6× slower than CPython's; only simdutf-class code would win),
   `str.split()` and `join` (allocation-bound), `pickle.loads` large.

## 3. Candidate cards

### 3.1 float → str (`repr`, `str`, `format(x, '')`, JSON, `%r`)
- **Measured**: `repr(123.456789)` 3,259 Ir (wall 255 ns): `_Py_dg_dtoa` 2,066 + `lshift` 271 +
  `PyOS_double_to_string` 169 + `_PyUnicode_FromASCII`/alloc. 10k floats: stdlib 48.9 M Ir /
  3.24 ms; `pyfmt` 0.51 [0.46, 0.56]; ryu+layout 0.29 [0.25, 0.30]; zmij+layout 0.27
  [0.24, 0.31] (wall; Ir 0.51/0.30/0.27). In pure Rust: pyfmt 241 ns, ryu 112 ns, zmij 98 ns per
  value including my string re-layout (box loaded; *measured*); raw ryu/zmij are ~20–30 ns
  (*inferred*, published figures), so a tuned path is ~5× the algorithm.
- **Entry**: `PyFloat_Type.tp_repr` (static type, `tp_str` inherits it) — same kind of swap as
  `sq_contains` today. Callers that benefit: `repr`, `str`, `%s`/`%r`, logging, `print`, `_json`
  (`PyFloat_Type.tp_repr(obj)` directly). `f'{x}'` goes `PyObject_Format` → `float.__format__` →
  static `float_repr` call: needs the `__format__` `ml_meth` swapped too (empty spec only).
  Not reached: `float.__repr__` (wrapper captured the old pointer — still exact), heap subclasses
  created before `enable()` (copied the slot — still exact).
- **Exactness**: shortest round-trip digits with ties-to-even == dtoa mode 0 (verified above);
  layout rules are 20 lines (rustique-core already has them). `'.2f'`/`'g'` use other dtoa
  modes: leave to CPython. Guard: `isfinite` (1 instruction).
- **Achievable**: 3–3.5× per `repr` (*inferred*: ~2,500 of 3,259 Ir replaced by ~150–300);
  3.5–4× for float lists (*measured* 0.27 with a crude layout); `json.dumps` of float arrays
  5–15× together with the JSON card.

### 3.2 json.dumps / json.loads
- **Measured** (small = 10-field record, 2.04 µs dumps / 2.08 µs loads wall):

  | variant | dumps small Ir | dumps 1000 rec wall | loads small Ir | loads 1000 rec wall |
  |---|---:|---:|---:|---:|
  | stdlib | 31,897 | 1.12 ms | 27,935 | 1.88 ms |
  | stdlib C part only (pre-built encoder / scanner) | 0.70 | 0.99 | 0.72 | – |
  | exact Rust proto (exact builtins, default args) | 0.31 | 0.60 [0.31, 0.88] | – | – |
  | orjson → `.decode()` (str) | 0.14 | 0.20 [0.18, 0.32] | 0.31 | 0.41 [0.31, 0.63] |
  | msgspec | 0.17 (bytes) | 0.23 (bytes) | 0.30 | 0.43 |
  | ujson / yyjson | 0.39 / 0.35 | 0.69 / 0.65 | 0.61 / 0.45 | 0.81 / 0.55 |

  Other kwargs cost more glue: `indent=2` 41.2 k Ir, `sort_keys` 46.1 k, `separators` 37.7 k,
  `default=` 37.6 k (orjson 0.11–0.13 of each). 10k floats: 3.9 ms, orjson 0.056, float 86 %.
- **Where time goes**: dumps small — glue 30 % (`dumps` → `encode` → `iterencode` builds a
  closure, a markers dict and a new C encoder per call → `''.join` of a 1-tuple), `_Py_dg_dtoa`
  15 %, escaping (`ascii_escape_size`, `write_escaped_ascii`) 11 %, unicode writer 16 % (growth/
  realloc 7 %).
  loads small — dict building 21 %, scanner 21 %, alloc 15 %, two `WHITESPACE.match` calls + frames.
- **Entry points**: `json.dumps`/`loads` are pure Python; the C parts are heap types created per
  interpreter (`tp_call` of `c_make_encoder`/`c_make_scanner`). Swapping only the C types keeps
  30 % glue → ~2.2–2.5× small (*inferred*). Taking over `json.dumps` (guard: no `cls`, hooks
  callable, `json._default_encoder`/`json.encoder.c_make_encoder` unchanged, exact containers)
  → 3–7× small (*inferred* between proto 0.31 and orjson 0.14).
- **Exact subset**: exact `dict`/`list`/`tuple`/`str`/`int`/`float`/`bool`/`None` with default
  or common kwargs (`indent`, `sort_keys`, `separators`, `ensure_ascii`, `allow_nan`); `default=`
  can be called by Rust at the same point as the C encoder (depth-first order), so `cls`/`default`
  users stay on the fast path for the builtin parts. Fallback for subclasses (C semantics:
  str/int/float subclasses use the base repr; dict subclasses go through `PyMapping_Items`,
  i.e. user `items()`). Coverage of real calls: *inferred* high (web payloads are exact
  builtins); to be confirmed by the profiling slice.
- **Traps**: circular check (`ValueError: Circular reference detected`), recursion depth and
  message (`while encoding a JSON object`), exception notes (`when serializing dict item 'k'`),
  key coercion (`True` → `"true"`, float keys via repr), `sort_keys` TypeError on mixed keys,
  `int_max_str_digits` ValueError, surrogate escaping, free-threaded critical sections on dicts
  and lists, tests that force the Python encoder by monkeypatching `c_make_encoder`. loads:
  exact `JSONDecodeError` messages/positions, key memo (repeated keys are one object), hook order,
  `array_hook` (new in 3.15).
- **Tests**: test_json 237 cases (each run against both C and Python implementations).
- **Upstream**: json is named a Rust-for-CPython candidate for 3.17 (Oct 2028) — value until then.

### 3.3 re
- **Measured** module-level vs compiled (Ir): `re.match(P, s)` 5,037 = `re._compile` hit 2,140
  (42 %) + Python frames + `p.match(s)` 2,768. Compiled small calls: `search` on an 86-char line
  2,718 Ir (engine 54 %, Match + state allocation 33 %), `fullmatch` identifier 2,346, `split`
  8,568, `sub` 32,168 (12 hits). The `regex` module is slower everywhere except `re.I`.
- **Measured** large (wall, 1 MB log text) — sre vs Rust `regex` proto:

  | case | sre | Rust | ratio [min,max] |
  |---|---:|---:|---|
  | search, literal-prefix miss `FATAL: (\w+)` | 258 µs | | 0.076 [0.05, 0.12] |
  | search, alternation miss `FATAL\|CRITICAL\|PANIC` | 3.14 ms | | 0.020 [0.01, 0.04] |
  | search, `re.I` miss | 5.45 ms | | 0.011 [0.01, 0.02] |
  | same at 1 KB (literal / alternation) | 342 ns / 3.56 µs | | 0.24 / 0.038 |
  | `sub(r'\s+', ' ')` (~200k hits) | 20.9 ms | | 0.31 [0.16, 0.39] |
  | `findall(r'\d+')` (~300k hits) | 16.2 ms | | 0.67 [0.37, 1.07] |
  | `findall` with 2 groups | 1.96 ms | | 1.40 (slower) |
  | `compile` (cache miss) | 36 µs | | **26× slower** (Ir 34×) |

- **Corpus** (*measured*, 12,190 literal patterns from popular packages): 95 % regular (no
  backref/lookaround/atomic/possessive/conditional); 41 % use Unicode classes or `re.I`; 34 % of
  regular ones use `$`. Static call sites: `compile` 5,722, `match` 2,509, `sub` 1,713, `search`
  1,047, `findall` 349, `split` 262 — module-level calls are common.
- **Design**: (a) module functions in Rust/C with the same cache semantics → 1.7–1.8× on every
  small module-level call, exact; (b) tiered DFA prefilter: compile a Rust DFA lazily only after a
  pattern has scanned enough text (compile is 26× sre's), use it for `search`/`finditer`/`sub`/
  `split`/`findall` on subjects ≥ ~256 chars, hand each start to sre for the Match (§2.5);
  subjects < threshold stay on sre (guard: one length compare).
- **Traps (all measured here)**: `\w` Python≠Rust on 8,214 code points (Python excludes combining
  marks, includes `²³¹¼½¾`), `\d` on 10 (crate on Unicode 16, CPython 17), `\s` on
  `\x1c-\x1f` even in ASCII and not under `re.ASCII`; `$` matches before a final `\n`;
  `re.sub('x*', '-', 'abxd')` is `-a-b--d-` in Python, `-a-b-d-` in Rust (empty-match rules);
  `re.I`: `'i'` matches `'İ'` in Python only; `\<` is a literal in Python, a word boundary in
  Rust; verbose mode keeps whitespace in classes in Python only; `re.L` → always sre. Solution:
  byte-level DFA on UCS1 subjects with classes generated from CPython's own predicates (exact for
  any Unicode version), ASCII/UCS1 guard is O(1); UCS2/4 subjects fall back.
- **Safety**: compiled Rust regexes are `Sync` (free-threaded builds fine); keep them in a side
  table keyed by the pattern object (patterns support weakrefs) per interpreter, bounded in
  memory (lazy-DFA caches) and never holding a lock while sre or user callbacks run.
- **Tests**: test_re 168 + 414 table rows (`re_tests.py`); differential fuzzing is natural here.
- **Upstream**: none known for `_sre`.

### 3.4 str / bytes methods
- **Measured** small (Ir): `find` 637, `count` 780, `'x' in s` 458, `startswith` 334, `strip`
  497, `lower` 891, `split()` 3,526 (53 % str, 24 % alloc, 12 % list), `replace` 1,484, `encode`
  451 (ASCII), `'%s=%d'` 1,725, `'{}={}'.format` 2,147, f-string with a float 4,199 (37 % float).
- **Measured** 1 KB (wall proto ratio): `lower` 0.32, `replace` 0.34, `count` 0.44, `splitlines`
  0.53, `find` miss 0.65, `split()` 1.17. 1 MB: `replace` 0.11, `find` miss 0.12 (stringzilla
  0.19), `count` 0.13 (stringzilla 0.29), `count('\n')` 0.07, `lower`/`upper` 0.10, `split('\n')`
  0.59, `splitlines` 0.46, `split()` 1.12, `encode` ASCII 1.0, `decode` ASCII 0.43.
- **Latin-1 text**: `lower()` costs 54 Ir/char vs 6 for ASCII (`do_lower` + `lower_ucs4` per char,
  4.4 ms per MB, *measured*); a 256-entry table derived from CPython's own `str.lower` at
  `enable()` is exact for `lower` (Latin-1 lowercases stay in Latin-1); `upper`/`casefold` need
  escapes for `ÿ`, `µ`, `ß` (→ fallback). *Inferred* 20–50×.
- **Entry**: static `unicode_methods`/`bytes_methods` `ml_meth` (process-wide, shared by
  subinterpreters), `sq_contains`, `nb_remainder`. Not reachable: `BUILD_STRING`/f-strings,
  `BINARY_OP_ADD_UNICODE`, `PyUnicode_Join` called from C.
- **Traps**: identity (`replace`/`strip` return self for exact str when unchanged), special
  casing (`'Σ'.lower()` context, `'İ'.lower()` = 2 chars, `ß`), Unicode 17 tables, whitespace set
  (`\x1c-\x1f`, `\x85`, `\xa0`), empty needles and negative slice args, UTF-8 cache side effect,
  bytearray under free-threading.
- **Decision note**: DECISIONS.md §4 ranks str methods first; this data says the win exists only
  above ~1 KB, so its value depends on how much real time is spent on long strings (profiling slice).

### 3.5 int ↔ str
`str(1234567)` 797 Ir (44 % `long_to_decimal_string_internal`, 23 % alloc), `int('1234567')` 812.
10k ints: stdlib 8.7 M Ir, proto itoa 0.51 (Ir) / 0.64 (wall). ≈4,000 digits (below the 4,300
limit): `str` 147 µs, gmpy2 0.17; `int(s)` 69 µs, gmpy2 0.28 (*measured*). Entry: `tp_repr`/
`tp_str` slot; `PyUnicodeWriter_WriteRepr` (json fast path) calls `_PyLong_FormatWriter` directly.
Traps: `sys.set_int_max_str_digits` (ValueError text), `_pylong` thresholds. Achievable ≤ 1.2×
small, 3–5× big (rare).

### 3.6 base64 / binascii / hex
Small (32 bytes): `b64encode` 2,215 Ir, binascii direct 0.65 → glue 35 %; `b64decode` 4,200 Ir,
direct 0.28 → glue 72 % (`_bytes_from_decode_data`, `validate`/`altchars` handling, kwargs);
pybase64 0.89/0.51. 1 MB: encode 589 µs, pybase64 0.074 [0.06, 0.11]; decode 658 µs, pybase64
0.63. 1 KB encode: pybase64 0.41 wall. `bytes.hex` 1 MB = `hexlify`; `fromhex` 21 M Ir,
`unhexlify` 0.67. Entry: Python wrappers over static binascii functions. 3.15 already sped the
codec up 2–3× and grew new parameters (`padded`, `wrapcol`, `ignorechars`, `canonical`) and new
warnings; base64 was the demo module of the Rust-for-CPython pre-PEP (*inferred* overlap).
Achievable: 1.5–3× small (Python entry), 10–15× large encode, ~1.6× decode.

### 3.7 csv
Reader, 1000 rows (550 µs): `parse_process_char`/`parse_add_char` 46 %, field str creation 27 %,
alloc 13 %; `DictReader` adds 25 % glue; writer: str 50 %, csv 44 %. No drop-in alternative.
*Inferred* 1.3–2× (SIMD field scanning; object creation stays). test_csv 140. Medium risk
(dialects, quoting modes, `field_size_limit`, `line_num`).

### 3.8 html.escape / html.unescape
`escape` small 6,657 Ir (five `str.replace` calls, glue 27 %), clean text 3,930 (43 % glue);
proto 0.30 / 0.23 (Ir), 1 KB 0.38, 100 KB 0.39 Ir / 0.42 [0.35, 0.59] wall; markupsafe 0.48–0.99
with a different output (`&#39;`, `&#34;`). `unescape` small 35 k Ir, 100 KB 8.8 ms (`re.sub`
with a Python callback per entity). Used per variable by Django's autoescape (`django.utils.html.escape`
calls `html.escape`, *inferred* from Django source knowledge). Traps: identity for unchanged
exact str, copy for subclasses, duck-typed `.replace` (non-str → fallback). test_html has only 2
tests: needs differential fuzzing. Achievable escape 3–4× small / 2.4× large (*measured* proto);
unescape 5–10× (*inferred*).

### 3.9 urllib.parse
Per call (Ir): `quote` 15.8 k (`''.join(map(quoter, bytes))` per byte), clean `quote` 5.5 k,
`unquote` 25.9 k, `urlsplit` 1.0 k when the `lru_cache` hits / 25.3 k when it misses, `urlparse`
27.6 k, `urlencode` (5 keys) 76 k, `parse_qsl` 70 k, `urljoin` 42 k; interpreter 53–57 %.
proto `quote` 0.12 (0.16 clean). yarl is slower (1.9×) and not a drop-in. Exact subset: str
input, default `encoding`/`errors`/`safe`. Traps: cache observability (`cache_info`), 3.15
`missing_as_none`/`keep_empty`, bytes inputs, deprecation warnings. Tests: test_urlparse 81 +
test_urllib 106. Achievable 5–8× `quote` (*measured*), 3–10× the rest (*inferred*).

### 3.10 http.client / email header parsing
`http.client.parse_headers` on 11 typical response headers: 256 k Ir, 23 µs wall; `email`
`HeaderParser` alone 199 k Ir. Time is spread over `feedparser._parsegen`/`_parse_headers`,
`header_source_parse`, `BufferedSubFile.readline`, 4 × `get_content_type` (cProfile, *measured*).
Every requests/urllib3 response pays it. A drop-in for clean input (ASCII `Name: value`, no
continuation, < 100 headers) builds the same `HTTPMessage` (`_headers` list of raw tuples,
empty `defects`, compat32 policy) — exactness hinges on reproducing `email.message.Message`
internals. *Inferred* 5–15×; high risk; test_httplib 151 + test_email 1,817.

### 3.11 tomllib
pyproject-sized doc 76 µs; 500 `[[package]]` tables 10.4 ms; interpreter 81–86 %. rtoml 0.11–0.12,
tomli (same code, mypyc-compiled) 0.38–0.39 (*measured*). Traps: 3.15 parses TOML 1.1 (rtoml is
1.0), `TOMLDecodeError` positions, `parse_float`, datetime/tz objects. Rarely hot (tools load a
few files). Achievable 5–10× (*inferred*).

### 3.12 xml.etree
`fromstring` is C (expat + `_elementtree`): lxml only 0.73–0.83. `tostring` is pure-Python
serialization: small 17.6 µs / 20 items 140 µs / 5,000 items 36 ms, lxml 0.09–0.10 (wall), glue 67 %. `findall`
lxml 0.50–0.63. Traps for `tostring`: global namespace map, `short_empty_elements`,
`method="html"/"text"/"c14n"`, `xml_declaration`, escaping of `\n`/`\r` in attributes, Python
`Element` subclasses. Achievable `tostring` 5–9× (*inferred*), parse ≤ 1.3×. Upstream: xml is a
3.17 Rust-for-CPython candidate.

### 3.13 Small pure-Python utilities
| function | cost (*measured*) | where | achievable (*inferred*) |
|---|---|---|---|
| `string.Template.substitute` (3 fields) | 19.5 k Ir, 1.46 µs | `re.sub` + Python callback | 2–3× (`str.format` equivalent 0.32) |
| `textwrap.wrap(540 chars, 70)` | 1.09 M Ir, 71 µs | regex chunking 48 %, glue 27 % | 5–20× |
| `textwrap.dedent(500-line source)` | 0.99 M Ir | str 38 %, glue 45 % | 3–5× |
| `shlex.split(80-char command)` | 313 k Ir, 21 µs | char-by-char lexer, glue 78 % | 10–30× |
| `shlex.quote` | 4.1 k Ir | regex + glue | 2–3× |
| `tokenize` (500 lines) | 10 M Ir, 1.0 ms | glue 51 %, C tokenizer 15 % | ≤ 2× |
Rarely on hot paths (argparse help, build tools, linters); low priority despite the large factors.

### 3.14 unicodedata.normalize, pickle, struct
`normalize('NFC', ascii_line)` 437 Ir (quick check, returns the same object); decomposed
Latin text 8.5 k Ir / 1.3 ms per 200 KB; unicodedata2 and the `unicode-normalization` crate are
1.3–6.5× **slower** — no headroom. `pickle.dumps`/`loads` small 13 k / 14 k Ir (Pickler setup
dominates), msgspec msgpack 0.27 / 0.49 for a different format, `loads` large 1.0 — modest.
`struct.unpack` 1.0 k Ir — at the floor.

## 4. Ranking and reasoning

Score = gain where real programs spend time × share of calls in the exact subset × (1 − risk),
discounted for upstream overlap. Ranked:

1. **float → text via a zmij/ryu core behind `PyFloat_Type.tp_repr` (+ `float.__format__('')`).**
   3–4× on every float repr, 5–15× on float-heavy JSON, exactness shown on 3.3 M values, one
   slot of a static type (today's mechanism), tiny API, no upstream overlap. Cheapest real win;
   also makes rustique-core's `pyfmt` fast.
2. **json.dumps / json.loads.** The most common serializer in services; 3–7× small and 3–5× large
   if `json.dumps`/`loads` themselves are taken over (2–2.5× with only the C types). Medium risk,
   big CPython test suite (237) to hold it. Overlap with upstream 3.17 is years away.
3. **re: Rust module functions + tiered DFA prefilter with sre-built Matches.** 1.7–1.8× on
   every small module-level call (exact, low risk); 4–90× on searches over ≥ 1 KB that miss or
   hit sparsely (log scanning, validation of long text). 87 % of real patterns eligible, 0
   mismatches in 2.8 M pairs. Highest ceiling, highest engineering cost (translator from sre
   parse tree, CPython-derived classes, compile tiering).
4. **str/bytes bulk kernels** (`replace`, `find`/`count`/`in`, `lower`/`upper` ASCII + Latin-1
   tables, `splitlines`, `split(sep)`) behind `ml_meth` with a length guard: 1.5–3× at 1 KB,
   7–15× at 1 MB, 1.00 below the threshold. Low risk; value depends on long-string share.
5. **urllib.parse and html.escape** (pure-Python, per-call 3–8×, simple semantics, frequent in
   web code); **xml.etree `tostring`** (5–9×) next.
6. **http.client/email header parsing**: large per-response gain (5–15×) but the `Message`
   state makes exactness costly; worth a spike only if the web workload profiles show it.
7. **base64** (glue-only gains small, 10–15× large encode; upstream already moving), **tomllib**,
   **textwrap/shlex/Template/tokenize** (big factors, rarely hot), **csv**, **big-int text**.
8. **Not worth it**: `unicodedata.normalize`, `xml` parsing, UTF-8 codecs (except a simdutf-class
   ASCII/UTF-8 decode, ~2×), `str.split()`/`join`/small `str` methods, `pickle`, `struct`.

**Decision needed**: items 2, 3 (module functions), 5 and 6 get most of their small-call gain
only if Rustique can take over *pure-Python* stdlib functions (json.dumps, re.match, base64,
urllib, html) — the C-entry swap reaches 30–60 % of it. That mechanism question (identity-
preserving `__code__` swap vs module-attribute replacement vs C-entry only) decides the ranking
between "str kernels" and "json/re/urllib".
