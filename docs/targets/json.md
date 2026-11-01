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
  an exact 2-tuple / 2-list of exact `str`), `cls=None`, `default=None`. Under
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

RESULTS_PLACEHOLDER

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

BENCH_PLACEHOLDER

## Versions

Validated: 3.14 and 3.15, GIL and free-threaded. Fingerprints differ between the two
minors (bytecode), identical between GIL and free-threaded builds of one minor.
