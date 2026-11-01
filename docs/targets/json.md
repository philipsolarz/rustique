STATUS: wave 1, workstream `json`. 2026-11-01.

# Target group `json`

Two targets, both vectorcall sites on the pure-Python functions of the `json` package,
installed when `json` finishes importing (or at `enable()` if it already has):

| target | site | kernel |
|---|---|---|
| `json.dumps` | `json.dumps` (`PyFunction_SetVectorcall`) | Rust encoder: what `_json.c`'s `encoder_call` writes, with the `dumps` → `JSONEncoder.encode` → `iterencode` → `c_make_encoder(...)(o, 0)` → `''.join` glue removed |
| `json.loads` | `json.loads` | Rust decoder: what `JSONDecoder.decode` + `_json.c`'s `scan_once_unicode` return, glue (two `WHITESPACE.match` calls, `raw_decode`) removed |

Floats are formatted by the group `float` core (`rustique_core::float_repr`, exact; see
`float.md`), so `json.dumps` of floats gains even when the `float` group is excluded.

Entry point choice (measured, `benches/targets/json_cg.py`, 3.15.1): taking over the C
encoder only (`_json.make_encoder.tp_call`) leaves CPython's Python glue, 30% of a
small `dumps` (the C part alone is 0.70 of the call, headroom-text §3.2), so it cannot
beat ~0.70; the vectorcall on `json.dumps` reaches 0.13–0.29 on small documents, and its
fallback cells stay ≤ 1.05 (below). `JSONEncoder.encode` called directly, `json.dump`
(chunks to `fp.write` are observable) and calls with `default=`/`cls=` stay on CPython.

## Exact subset

### `json.dumps(obj, **kw)`

- Arguments: one positional; keywords among `skipkeys`, `ensure_ascii`,
  `check_circular`, `allow_nan`, `sort_keys` with exact `bool`/`int`/`None` values (their
  truthiness runs no code), `indent` (`None`, an exact `int` in −4096..4096 — `' ' * n`,
  so ≤ 0 is an empty indent with newlines — or an exact `str`), `separators` (`None`, or
  an exact 2-tuple of exact `str`; a list falls back: on free-threaded builds another thread
  could drop its items mid-call), `cls=None`, `default=None`. Under
  `ensure_ascii`, separators and indent must be ASCII. Anything else (`default=`, `cls=`,
  unknown keywords, other types) → the original (`fallback.args`).
- Objects: `None`/`True`/`False`; `str` and its subclasses (the C encoder reads their
  buffer, never `__str__`); `int` and subclasses (`PyLong_Type.tp_repr`: an `IntEnum`
  encodes its value; ≤ 64 bits formatted in Rust, larger by CPython's own conversion so
  `sys.set_int_max_str_digits` applies); `float` and subclasses (`float_repr`, `NaN`/
  `Infinity`/`-Infinity` with `allow_nan`); exact `list`/`tuple`; exact `dict` (keys
  converted exactly like `encoder_encode_key_value`: `str`, `float`, `True`/`False`/`None`,
  `int`, else skipped with `skipkeys`). `sort_keys` needs all-`str` keys (code-point order =
  CPython's tuple sort, keys being unique).
- Restart in the original (`fallback.deopt`), before anything observable: any other object
  (needs `default()`; dict/list subclasses go through `PyMapping_Items`/`PySequence_Fast`,
  which may run user code), `frozendict`, a circular reference, a non-finite float with
  `allow_nan=False`, an int beyond the digit limit, mixed key types with `sort_keys`,
  `Py_EnterRecursiveCall` failing (C recursion limit). CPython then produces the result or
  raises its own error (message, notes).
- Output: the same `str` object CPython builds — canonical kind (ASCII / Latin-1 / UCS2 /
  UCS4, `sys.getsizeof` equal), one-character results are the cached singletons
  (`json.dumps(5) is json.dumps(5)`), fresh strings otherwise.

### `json.loads(s, **kw)`

- Arguments: one positional; keywords only among `cls`, `object_hook`, `parse_float`,
  `parse_int`, `parse_constant`, `object_pairs_hook` (and `array_hook` on 3.15), all
  `None` (the default decoder path). Hooks, `strict=`, other keywords → the original.
- Input: exact `str` not starting with U+FEFF; exact `bytes` that `json.detect_encoding`
  reads as plain UTF-8 (no BOM, no UTF-16/32 NUL pattern): ASCII bytes are parsed in place,
  others decoded with `PyUnicode_DecodeUTF8(..., "surrogatepass")` first (what `loads`
  does). `bytearray`, `str` subclasses, BOMs, UTF-16/32 → the original.
- Result built with CPython's constructors in CPython's order: `PyDict_New` +
  `PyDict_SetItem` per member (duplicates: last value, first key), `PyList_New(0)` +
  `PyList_Append` (same over-allocation: `sys.getsizeof` equal), keys memoized per call
  (repeated keys are one object, like the scanner's `memo`; values are not), escape-free
  strings as `PyUnicode_Substring` (`_PyUnicode_FromASCII` / `PyUnicode_FromKindAndData`:
  canonical kind, `""` and one Latin-1 character are singletons), escaped strings as the
  writer's canonical string (surrogate pairs joined exactly under CPython's `end + 6 < len`
  condition, lone surrogates kept), ints ≤ 18 digits via `PyLong_FromLongLong` (small-int
  cache), longer via `PyLong_FromString` (digit limit → the original raises), floats
  correctly rounded (Rust's `f64::from_str`, like `float(s)`; checked on 20,000 random
  literals and the Hypothesis number grammar).
- Restart in the original (`fallback.deopt`): every malformed document (so
  `JSONDecodeError` — `msg`, `pos`, `lineno`, `colno` — is CPython's), control characters
  in strings (strict mode), the `NaN`/`Infinity`/`-Infinity` literals (the scanner's
  `parse_constant`), allocation failures, the C recursion limit.

## Guards and pins (per call: ~15 Ir + the tools check)

Per call: argument shape; one `DepFlag` load; identity of the patched function, its
`__code__` and `__kwdefaults__` dict; the class of `json._default_encoder` /
`_default_decoder` (`__class__` assignment notifies no watcher); `__code__` and
`__defaults__` of `JSONEncoder.encode`/`iterencode` and `JSONDecoder.decode`/`raw_decode`;
tools listening (`accel::tools_quiet`: `sys.settrace`/`setprofile`/monitoring → the
original, so profilers and debuggers see CPython's frames).

Pinned at install (dict watcher; the registry suspends the site while a pin is broken and
re-installs it when restored):
- `json.dumps`: globals `_default_encoder`, `JSONEncoder`; in `json.encoder`:
  `c_make_encoder` (must be `_json.make_encoder`), `encode_basestring_ascii`,
  `encode_basestring` (must be `_json`'s, fingerprinted), `INFINITY`, and the builtins
  `isinstance`, `str`, `list`, `tuple`, `float` as `LOAD_GLOBAL` resolves them; snapshot
  pins of `JSONEncoder.__dict__`, `json._default_encoder.__dict__` (materialized) and
  `json.dumps.__kwdefaults__`.
- `json.loads`: globals `_default_decoder`, `detect_encoding`, `codecs` (+ its five BOM
  constants), builtins `isinstance`, `str`, `bytes`, `bytearray`, `len` (also `len` in
  `json.decoder`); snapshot pins of `JSONDecoder.__dict__`, `json._default_decoder.__dict__`
  and `json.loads.__kwdefaults__`; validated once: the default decoder's `scan_once` is
  the C scanner with `strict=True`, no hooks, `parse_float is float`, `parse_int is int`,
  and `decode`'s `_w` default is `[ \t\n\r]*` (`VERBOSE|MULTILINE|DOTALL`).
- Fingerprints (canonical code) recorded for 3.14 and 3.15 of `json.dumps`,
  `JSONEncoder.encode`/`iterencode`/`__init__`, `json.loads`, `detect_encoding`,
  `JSONDecoder.decode`/`raw_decode`/`__init__`; C fingerprints of
  `_json.encode_basestring(_ascii)`.
- Install-time self-tests run without Python code (module targets prepare inside the
  `sys.modules` watcher): a 20-element probe document encoded in three option sets against
  texts recorded from CPython, and a probe document decoded and checked (types, values,
  float bits, small-int cache, key memo identity).
- Fallbacks count streaks: 64 consecutive fallbacks (a program that always passes
  `default=`, or runs under a profiler) suspend the site until the next `enable()`.
- Both targets are `checkable` (check mode re-runs the original and compares).

Private state skipped: the encoder's `markers` dict and C encoder instance (replaced by a
stack of container pointers), the scanner's per-call `memo` dict (replaced by a Rust table
with the same result), `JSONEncoder.iterencode`'s closure. None is reachable through a
public API.

Known differences (none in results): a `RecursionError` raised at the C stack limit by the
original after a fallback happens a few levels earlier or later than without Rustique
(the fallback runs one C frame deeper; CPython's own depth varies with the caller's C
stack the same way), so the number of `when serializing ... item` notes can differ.
Allocation counts (`gc.get_count()`) differ, as for every kernel.

## Conformance (measured, `tests/targets/conformance.py --groups float,json`)

Same outcome for every test id with and without the targets (groups `float` + `json`
enabled before the test modules are imported) on 3.15.1, 3.15.1t, 3.14.9 and 3.14.9t:

| module | 3.15.1 / 3.15.1t | 3.14.9 / 3.14.9t | kernel hits (3.15.1) |
|---|---|---|---|
| `test_json` (C and Python variants) | 232 pass, 1 skip | 220 pass, 1 skip | dumps 14, loads 6, float 51 |
| `test_json@real` (C variant run on the real `json` module) | 232 / 1 | 220 / 1 | dumps 129, loads 29, float 28 |
| `test_float`, `test_format`, `test_strtod`, `test_fstring` | identical (see `float.md`) | identical | |

`test_json` imports fresh copies of `json` (`import_fresh_module`), which the targets do
not patch (they are installed on the process's `json` module, the one programs use); the
`@real` variant makes its C tests (`CTest`, `self.json = cjson`) use the real module, so
the kernels run under CPython's own json test suite. Hit counts are modest because many
tests exercise hooks, `cls=`, invalid documents (fallbacks) and the adaptive suspension
stops the sites after 64 consecutive fallbacks (re-armed by the next `enable()`).
`tests/targets/conformance.py` also asserts both targets `installed`.

## Tests

`tests/targets/test_json_dumps.py`, `tests/targets/test_json_loads.py`: examples × option
sets; Hypothesis differential tests (JSON-like trees with surrogates, huge/small floats,
big ints, mixed keys, unsupported objects, every keyword combination; `loads` on
round-trip, mutated and random documents, str and bytes) comparing results (type, value,
float bits, sizes, singletons, key identity pattern) or exceptions (type, message, notes,
`JSONDecodeError` fields); check mode; captured references (`from json import dumps`
before `enable()`, `functools.partial`); introspection and pickling; `disable()`; tools;
`mock.patch` of every pinned dependency (falls back, re-arms); `__code__`/`__defaults__`/
`__kwdefaults__`/`__class__` changes; free-threaded stress (concurrent calls and
mutation, enable/disable under load).

## Benchmarks

### Small inputs: callgrind (Ir per call, region-instrumented; `benches/targets/json_cg.py`)

3.15.1, plain → `enable(only=["json", "float"])`; JIT column: `PYTHON_JIT=1`, warmed
30,000 iterations. Inputs: a 10-field record (`{"id": 12345, "name": "Ada Lovelace",
..., "score": 98.5, ...}`), an 8-element mixed list, an 8-float record, a nested order
document, a unicode record.

| cell | plain | enabled | ratio | JIT ratio |
|---|---:|---:|---:|---:|
| `dumps(record)` | 31,030 | 6,547 | **0.211** | 0.222 |
| `dumps(small_list)` | 18,106 | 2,374 | 0.131 | 0.144 |
| `dumps(floats)` | 37,801 | 6,228 | 0.165 | 0.172 |
| `dumps(nested)` | 58,536 | 17,026 | 0.291 | 0.299 |
| `dumps(unicode_rec)` | 18,481 | 3,256 | 0.176 | 0.192 |
| `dumps(unicode_rec, ensure_ascii=False)` | 23,905 | 4,660 | 0.195 | 0.209 |
| `dumps('hello')` | 3,304 | 874 | 0.264 | 0.325 |
| `dumps(7)` | 11,286 | 619 | 0.055 | 0.064 |
| `dumps(record, indent=2)` | 40,293 | 7,877 | 0.196 | 0.205 |
| `dumps(record, sort_keys=True)` | 45,893 | 12,305 | 0.268 | 0.282 |
| `dumps(record, separators=(',', ':'))` | 36,836 | 6,695 | 0.182 | 0.191 |
| `dumps([Color.RED])` (IntEnum) | 13,131 | 1,328 | 0.101 | 0.115 |
| `loads(record_text)` | 27,894 | 12,979 | **0.465** | 0.477 |
| `loads(record_bytes)` | 32,562 | 13,059 | 0.401 | 0.420 |
| `loads(small_list_text)` | 13,163 | 2,876 | 0.218 | 0.229 |
| `loads(floats_text)` | 24,493 | 10,623 | 0.434 | 0.445 |
| `loads(nested_text)` | 57,271 | 32,236 | 0.563 | 0.571 |
| `loads(unicode_text)` | 16,352 | 6,344 | 0.388 | 0.404 |

`loads` keeps CPython's object-building cost (dict inserts with key hashing, list appends,
string and number objects; ~60% of the enabled cost): the gain is the scanner and the
Python glue.

Fallback cells, steady state (after the warm-up the site is suspended by its fallback
streak where every call falls back; the float kernel still speeds up the C encoder):

| cell | plain | enabled | ratio | JIT ratio |
|---|---:|---:|---:|---:|
| `dumps(record, default=str)` | 36,645 | 31,125 | 0.849 | 0.844 |
| `dumps(with_decimal, default=default)` | 39,816 | 34,277 | 0.861 | 0.855 |
| `dumps(OrderedDict)` | 36,587 | 30,973 | 0.847 | 0.843 |
| `dumps(record, cls=json.JSONEncoder)` | 36,561 | 31,041 | 0.849 | 0.844 |
| `JSONEncoder().encode(record)` (not patched) | 28,890 | 23,370 | 0.809 | 0.803 |
| `loads(text, object_hook=dict)` | 40,959 | 41,014 | 1.001 | 1.003 |
| `dumps(with_decimal)` → TypeError | 40,457 | 34,714 | 0.858 | 0.863 |
| `dumps(circular)` → ValueError | 19,920 | 19,750 | 0.991 | 0.995 |
| `loads('{"id": 1, "name": "x",}')` → JSONDecodeError | 26,293 | 26,403 | 1.004 | 1.005 |

First 64 fallbacks (`--first64`: 20 and 40 calls after a hit that resets the streak,
i.e. before suspension; the cost a program pays when fallbacks are interleaved with hits):

| cell | plain | enabled | ratio |
|---|---:|---:|---:|
| `dumps(record, default=str)` / `OrderedDict` / `cls=` | 36,6xx | 31,4xx | 0.86 |
| `loads(text, object_hook=dict)` | 40,895 | 41,478 | 1.014 |
| `dumps(with_decimal)` → TypeError (restart after a partial encode) | 40,405 | 41,287 | 1.022 |
| `dumps(circular)` → ValueError (restart) | 19,959 | 20,778 | 1.041 |
| `dumps(circular, skipkeys=[])` (declined at once: the mechanism's floor) | 20,125 | 20,258 | 1.007 |
| `loads('{"id": 1, "name": "x",}')` → JSONDecodeError (restart after a partial parse) | 26,293 | 28,720 | **1.092** |

The one cell above 1.05 is an invalid document during the first 64 consecutive invalid
documents: the kernel parses up to the error (building and discarding objects), then the
original re-parses and raises. Steady state (the site suspended) is 1.004; a program that
mixes valid and invalid documents pays ~2.4k Ir per invalid one and saves ~15k per valid
one. Float cells: see `float.md`.

### Large inputs: pyperf wall time (`benches/targets/json_pyperf.py`, 3.15.1, shared box)

Median of 4 processes × 5 values each, plain → enabled (`json`, `float`):

| case | plain | enabled | ratio |
|---|---:|---:|---:|
| `dumps` 1000 records (190 KB) | 1.204 ms | 0.400 ms | 0.33 (3.0×) |
| `dumps` 1000 records, `indent=2` | 1.224 ms | 0.491 ms | 0.40 |
| `dumps` 1000 records, `sort_keys=True` | 1.708 ms | 0.708 ms | 0.41 |
| `dumps` 10k floats | 3.772 ms | 0.475 ms | 0.13 (7.9×) |
| `dumps` 10k ints | 0.400 ms | 0.202 ms | 0.51 |
| `dumps` 2000 non-ASCII strings | 1.036 ms | 0.651 ms | 0.63 |
| `dumps` 2000 non-ASCII strings, `ensure_ascii=False` | 0.957 ms | 0.583 ms | 0.61 |
| `loads` 1000 records | 1.299 ms | 0.541 ms | 0.42 (2.4×) |
| `loads` 1000 records (bytes) | 1.316 ms | 0.558 ms | 0.42 |
| `loads` 10k floats | 2.024 ms | 0.568 ms | 0.28 (3.6×) |
| `loads` 2000 non-ASCII strings | 1.246 ms | 0.527 ms | 0.42 |
| `list(map(repr, floats))`, 10k (group `float`) | 3.783 ms | 0.686 ms | 0.18 (5.5×) |

### Workloads (`benches/workloads/run.py compare ... -n 5 -p 3 --only json,float`, 3.15.1)

Median steady-state `run()` time, 3 interleaved process pairs; digests identical in every
pair (the programs' outputs are byte-for-byte CPython's):

| workload | plain | enabled | ratio | what hits |
|---|---:|---:|---:|---|
| `web/asyncio_http` | 231.0 ms | 182.2 ms | **0.789** | `dumps(separators=)`, `loads` of request/response bodies |
| `data/csv_etl` | 397.0 ms | 317.4 ms | **0.799** | `dumps(ensure_ascii=False, sort_keys=True)` of float-heavy records |
| `web/aiohttp_app` | 274.2 ms | 248.5 ms | 0.906 | aiohttp's `json.dumps`/`loads` |
| `web/json_api` | 405.1 ms | 371.0 ms | 0.916 | `loads` of every body; `dumps(default=...)` falls back (suspended), the float kernel speeds the C encoder |
| `web/logging_service` | 283.7 ms | 282.2 ms | 0.995 | `dumps(default=str)`: falls back (suspended) |
| geometric mean | | | 0.878 | |

Measured on the shared box (wall time; pairs interleaved). `default=` callers (json_api,
logging, Flask/FastAPI-style providers) need a kernel behind `_json.make_encoder.tp_call`
that calls `default` at the C encoder's points: the next step for this group.

## Install cost (`benches/targets/json_install_cost.py`, 3.15.1, callgrind region)

Budget: ≤ 0.1M Ir per group at `enable()`, lazy install ≤ 5% of the module's import.

| cell | Ir |
|---|---:|
| `rustique.enable(only=["float"])` whole call | 549,047 |
| — of which `float.repr` `prepare` (layout, slot and wrapper checks, fingerprint, self-test) | **45,388** |
| — of which the registry for the group (`registry::enable`) | 56,927 |
| — the rest: the Python `enable()` wrapper and the `status()` it returns | ~490,000 |
| `rustique.enable(only=["json"])` before `import json` (targets pending) | 528,428 |
| `import json` plain | 4,378,080 |
| `import json` with the targets pending (installs both) | 5,429,667 (**+1,051,587, +24%**) |
| — `Prepare::fingerprint_code` (8 code fingerprints, accel) | 541,073 |
| — `Prepare::vectorcall` (2 sites; accel's `function_vectorcall` evaluates Python each time) | 141,253 |
| — the targets' own `prepare` code (pins, layout checks, self-tests ~70k) | ~290,000 |

`float` is within budget. `json` is not: most of its lazy install is the framework's
fingerprinting and site declaration (being made cheaper by accel-core); the group's own
part was trimmed (snapshot pins of `json.encoder`/`json.decoder` instead of per-name pins,
no `JSONDecoder.__init__` fingerprint, smaller self-tests) and is ~6.6% of `import json`.

## Versions

Validated: 3.14 and 3.15, GIL and free-threaded. Fingerprints differ between the two
minors (bytecode), identical between GIL and free-threaded builds of one minor.
