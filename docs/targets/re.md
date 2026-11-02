STATUS: wave 2, workstream `re` (2026-11-02). Contract: docs/ACCELERATION.md.

# Group `re`

Two kinds of targets, all vectorcall sites on CPython's own function objects (contract kind C):

1. **`re._compiler.compile`** — CPython's regular-expression compiler, pure Python (~20 µs per
   pattern; 7.5% of real programs' import time, up to 42% of `import yaml`,
   `docs/atlas/w2-startup.md` §3). The kernel parses and compiles the pattern in Rust and calls
   CPython's own `_sre.compile` with **exactly the arguments `re._compiler.compile` would pass**:
   the pattern object itself, the final flags, the code list, the group count, `groupindex`
   (same insertion order) and `indexgroup` (holding the very key objects of `groupindex`). The
   `Pattern` that comes back is built by CPython, from identical inputs.
2. **The module functions** `re.match` (`re.prefixmatch` on 3.15: the same function object),
   `re.fullmatch`, `re.search`, `re.sub`, `re.subn`, `re.split`, `re.findall`, `re.finditer` and
   `re.compile`: `_compile`'s cache hit, done in Rust on **CPython's own `re._cache2`** with
   CPython's key, then the same `Pattern` method the original calls.

Code: `crates/rustique-core/src/re/` (the port: `parser.rs` = `re._parser`, `compiler.rs` =
`re._compiler`, `template.rs` = `parse_template`'s checks; `cargo test` on 5,242 recorded cases),
`crates/rustique-py/src/targets/re/` (kernels, validation, pins). Tests:
`tests/targets/test_re.py`. Benchmarks: `benches/targets/re_cells.py`, `re_imports.py`,
`re_install.py`, `re_coverage.py`; fingerprints recorded with `benches/targets/re_record.py`;
conformance with `benches/targets/re_conformance.sh`.

## How the compiler stays exact

The port is a statement-by-statement translation of `re._parser.parse` and
`re._compiler._code`, written against their 3.14.9 and 3.15.1 sources (identical except an unused
variable in `_parse_sub` and `bytes(charmap)` → `charmap.take_bytes()`, same results). It
reproduces CPython's object model where results depend on it:

- `SubPattern`s live in an arena, so "the same object" stays expressible: the common-prefix
  extraction of `_parse_sub` compares items with tuple equality, where `SubPattern`s compare by
  identity (items holding one never compare equal across alternatives).
- `getwidth()` is cached per sub-pattern like CPython's `self.width`, in Python's integer
  arithmetic (128-bit, capped at `MAXWIDTH = 2**64`), with the `hi = MAXWIDTH` *replacement* for
  unbounded repeats.
- `_optimize_charset` replays CPython's `IndexError` control flow: a store past the 256-entry
  charmap grows it to 65,536 and *retries the item* (LITERAL keeps its already lowered `av`), a
  store past the BMP sends the item to the tail (RANGE becomes RANGE_UNI_IGNORE under Unicode
  IGNORECASE) with the stores made before the error kept; runs, CHARSET bitmaps (`_mk_bitmap`
  bit order) and BIGCHARSET chunk tables (`_bytes_to_codes`: native-endian words) as CPython.
- Case folding comes from the running interpreter: `_PyUnicode_ToLowercase` /
  `_PyUnicode_ToUppercase` (what `_sre.unicode_tolower` / `_sre.unicode_iscased` call) and
  `re._casefix._EXTRA_CASES` read at install (content hash recorded per minor version). The
  result therefore follows the running CPython's Unicode database, whatever its version.
- `MAXREPEAT`, `MAXGROUPS`, `CODESIZE` and `MAGIC` are read from `_sre` and checked; every
  opcode, position, category and flag value is checked against `re._constants` at install.

**Fallback before anything observable.** The Rust compile has no side effects, so the kernel
falls back (calls the original with the unchanged arguments) whenever:

- CPython would raise (`re.error`, `OverflowError`, `ValueError` from `fix_flags`, ...): CPython's
  compiler then raises its own exception, with its message, position, line/column and frames;
- CPython would warn (`FutureWarning`: possible nested set, set operations `--`, `&&`, `~~`,
  `||`, `x--`): CPython warns, with its stack level;
- the pattern uses `\N{...}` (CPython imports `unicodedata` there), the `LOCALE` flag (argument
  or inline `(?L)`), `DEBUG` (CPython prints), a flag bit outside I/M/S/U/X/A, a non-ASCII group
  name (`str.isidentifier` follows the Unicode database), or nests groups deeper than 32 levels
  (CPython's recursion — and its recursion limit — apply);
- the pattern is not an exact `str`/`bytes` (subclasses can override `__getitem__`; a parsed
  `SubPattern` from `re.Scanner`), or flags are not an exact `int` in `[0, 2**31)`;
- `_sre.compile` rejects the arguments (never observed; nothing observable happened: the
  original runs and decides).

A fallback costs the caller its inlined frame push (+~250 Ir), under 2% of the cheapest compile,
so the compiler target never debits the net-benefit score (§5.6): its fallbacks are always worth
keeping it installed (a run of declined patterns, e.g. `test_re`'s error tests, must not suspend
it for the rest of the process).

Check mode (`RUSTIQUE_CHECK`): the original compiles too and the two `Pattern`s must be equal
(`==` compares flags, code and pattern) with the same `groups`, `groupindex` items in order and
the same `pattern` object.

## The module functions

`re.match(p, s, flags)` is `_compile(p, flags).match(s)`. The kernel reproduces the hit path of
`_compile`: `flags.value` for a `RegexFlag` (`_value_` by generic attribute lookup, as the enum
property does), the key `(type(p), p, flags)`, `_cache2[key]`; then calls the `Pattern` method
descriptor (the type is immutable) with the original's arguments. Using CPython's own cache keeps
`re.purge()`, the FIFO/LRU eviction and the identity of returned patterns exactly CPython's: **no
private state is skipped**.

Exact subset (anything else runs the original):

- the original's signature as bound by CPython: `flags` positional or keyword (or the function's
  `__defaults__`/`__kwdefaults__`, read at call time), `count`/`maxsplit` by keyword only
  (positional ones take CPython's `DeprecationWarning` path);
- `pattern` an exact `str` or `bytes` found in `_cache2` (a miss compiles: the original runs and
  the compile kernel takes over), or a compiled `Pattern` with falsy flags (returned as is);
- `flags` an exact `int`, `re._zero_sentinel`, or a `RegexFlag` member;
- `string`: any `str` for a str pattern (subclasses included: `_sre` reads the buffer), an exact
  `bytes` or `bytearray` for a bytes pattern — so no type error can be raised without the
  original's frame, and no user `__buffer__` runs;
- `count`/`maxsplit`: `_zero_sentinel` or an exact `int` that fits `Py_ssize_t`;
- `sub`/`subn` replacement: an exact `str`/`bytes` of the pattern's type that is either a literal
  (no backslash: `_sre` uses it as is) or a template that CPython's `parse_template` accepts for
  this pattern (checked by the port in `template.rs`: escapes, `\g<name>` against
  `Pattern.groupindex`, group numbers against `Pattern.groups`). `_sre` then calls CPython's own
  `re._compile_template` exactly as under the original (its `lru_cache` sees the same calls).
  Callable replacements (user code, ruling 15) fall back.

Dependencies: the `re` globals `_compile`, `_cache2`, `RegexFlag`, `_zero_sentinel`, `Pattern`,
`_compile_template`, `_parser`, `_sre` (key pins); builtins `isinstance`, `type`; the `re._parser`
and `_sre` namespaces (snapshot pins); version tags of `RegexFlag`, `EnumType` and
`_ZeroSentinel` (a modified class falls back). Fingerprints: every module function, `_compile`,
`_compile_template.__wrapped__`, `parse_template` and the `Tokenizer` methods it uses.

Net benefit: a hit saves ~1,100 Ir, a fallback costs ~250–340 Ir, so `hit_credit = 3`,
`fallback_debit = 1` (installed while at least a quarter of the calls hit).

## Validation and pins of the compiler

Fingerprints (per minor version, `fps.rs`): `_compiler.compile`, `isstring`, `_code`,
`_compile_info`, `_compile`, `_compile_charset`, `_optimize_charset`, `_mk_bitmap`,
`_bytes_to_codes`, `_simple`, `_generate_overlap_table`, `_get_iscased`, `_get_literal_prefix`,
`_get_charset_prefix`, `_combine_flags`; `_parser.parse`, `fix_flags`, `_parse_sub`, `_parse`,
`_parse_flags`, `_escape`, `_class_escape`, `_uniq`; every method (and property getter) of
`Tokenizer`, `State`, `SubPattern` the success path calls. Pins: snapshot pins of the
`re._compiler`, `re._parser` and `_sre` namespaces (rebinding `_sre.compile`, `_parser.parse`,
... falls back until restored), the builtins the two modules use, and the version tags of
`Tokenizer`, `State`, `SubPattern` and `_NamedIntConstant` (checked per call: one load each).

## Private state, deviations

- None skipped (CPython's caches and `_compile_template`'s `lru_cache` are used as they are).
- Known edge, documented: if a program replaces the private `re._compile_template` *before the
  first template `sub` of the process* and restores it afterwards, `_sre` keeps calling the
  replacement (it caches the function on first use) and a template `re.sub` reaches it without
  `re.sub`'s frame on the stack. Out of contract (private function).
- Like every vectorcall kernel, an accelerated call does not push the Python frames of the
  original: a program within ~100 frames of the recursion limit can compile a pattern that
  CPython's (recursive) compiler would have refused with `RecursionError`. Nesting is capped so
  the difference stays bounded.

## Version coverage

3.14 and 3.15 (GIL and free-threaded builds; same fingerprints on both). Other minors refuse
(`version`); a changed function refuses (`fingerprint`); `python -O` changes the compiler's code
objects (asserts) and refuses.

## Measured results

TODO
