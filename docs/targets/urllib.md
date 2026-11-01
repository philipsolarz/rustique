STATUS: wave 1, group `urllib` (workstream utils). Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/urllib/`, `crates/rustique-core/src/urlparse/`
(`cargo test`). Tests: `tests/targets/test_urllib.py`, `tests/targets/test_utils_mechanics.py`.
Benchmarks: `benches/targets/urllib_cells.py` (callgrind), `utils_pyperf.py urllib`.

# Targets `urllib.parse.quote`, `quote_plus`, `unquote`, `unquote_plus`, `parse_qsl`, `parse_qs`, `_urlsplit`

Vectorcall kernels on the module's function objects: the 63–90% of call sites that hold
`from urllib.parse import quote` reach them (tested). `urlsplit` is wrapped in
`functools.lru_cache`, whose `cache_info()` is public: the C wrapper stays CPython's and
the kernel sits behind it, on `_urlsplit` — the uncached work under `urlsplit` (cache
misses), `urlparse` (never cached) and `urljoin`. All seven are side-effect free and
`checkable`.

| target | validated | free-threaded | checkable |
|---|---|---|---|
| all seven | 3.14, 3.15 | yes | yes |

The seven functions (and the helpers they call) are identical in 3.14.9 and 3.15.1 (the
3.15 `missing_as_none` changes are in `urlsplit`/`urlparse`/`urlunsplit`, which stay
CPython's).

## 1. Exactness

- **`quote(string, safe='/', encoding=None, errors=None)`**: `str` → UTF-8 (`strict`; text
  with a lone surrogate falls back), or exact `bytes`; `safe` `str` (non-ASCII ignored, as
  `safe.encode('ascii', 'ignore')`) or `bytes`; `_ALWAYS_SAFE` bytes and `safe` bytes kept,
  others `%XX` (upper-case hex). `quote('')` returns its argument. `quote_plus`: if the input
  has a space, `safe + ' '` then spaces to `+`; else `quote` itself.
- **`unquote(string, encoding='utf-8', errors='replace')`**: no `%` → the argument itself;
  otherwise CPython's split into ASCII runs and non-ASCII runs (`_asciire`), each ASCII run
  percent-decoded with `_unquote_impl`'s rules (`%` + two hex digits of either case, else a
  literal `%`) and decoded **by CPython's own UTF-8 decoder** (`PyUnicode_DecodeUTF8` with
  `replace`, `strict`, `ignore` or `surrogateescape`), non-ASCII runs kept. A strict decode
  error falls back (nothing observable happened). `unquote_plus` replaces `+` first.
- **`parse_qsl`/`parse_qs`**: `qs.split(separator)`, `partition('=')`, blank handling,
  `max_num_fields`, then `unquote_plus` on each name and value; names/values are slices of
  `qs` made with `PyUnicode_Substring` (CPython's slicing), so a single field returns `qs`
  itself where CPython does. `parse_qs` builds the dict of lists in CPython's order.
- **`_urlsplit(url, scheme=None, allow_fragments=True)`**: `lstrip` of C0 controls and
  space, removal of tab/CR/LF (`_UNSAFE_URL_BYTES_TO_REMOVE`, a mutable list checked per
  call), the scheme rule (`url[0]` an ASCII letter, all of `url[:i]` in `scheme_chars`,
  lower-cased by calling `str.lower`), `_splitnetloc`, `#`/`?` splits; components are
  slices of the cleaned url (the argument itself when nothing was stripped), `None` where
  CPython returns `None`. Netlocs with brackets (`ValueError` or `_check_bracketed_netloc`
  via `ipaddress`) and non-ASCII netlocs (`_checknetloc`'s NFKC check) fall back.

## 2. Exact subset and fallbacks

Accelerated: exact `str` arguments (and `bytes` for `quote`/`quote_plus`); default codecs
(UTF-8; `encoding` given as `'utf-8'`/`'utf8'` accepted); `parse_qsl`/`parse_qs` with a
`str` separator, `strict_parsing` false, `keep_blank_values`/`allow_fragments`
`bool`/`int`/`None`, `max_num_fields` `None` or int; inputs below 200,000 bytes for `quote`
(CPython switches to a chunked join with `math.isqrt`).

Falls back (`args`): str/bytes subclasses, `bytearray`, other codecs or error handlers,
`strict_parsing=True`, bytes query strings, every error (bad separator, too many fields,
encode errors), bracketed or non-ASCII netlocs. `deps`: a replaced dependency (§3).

## 3. Dependencies

Pinned (identity, LOAD_GLOBAL semantics): `quote_from_bytes`, `_ALWAYS_SAFE_BYTES`,
`_byte_quoter_factory` (quote, quote_plus), `quote` (quote_plus), `unquote`,
`_unquote_impl`, `_generate_unquoted_parts`, `_asciire` (unquote*, parse_qs*),
`unquote_plus` (parse_qsl), `parse_qsl` (parse_qs), `_WHATWG_C0_CONTROL_OR_SPACE`,
`_UNSAFE_URL_BYTES_TO_REMOVE` (+ contents per call), `scheme_chars`, `_splitnetloc`,
`_check_bracketed_netloc`, `_checknetloc` (_urlsplit), and the builtins they read.
Constants' contents are checked at install.

**Private state skipped:** `_byte_quoter_factory`'s `lru_cache` entries and the `_Quoter`
dicts `quote` would create; the lazily built `_hextobyte` table (`unquote` would build it on
its first call with a `%`).

## 4. Version coverage

3.14.9 and 3.15.1 and their free-threaded builds; refused elsewhere and on fingerprint or
constant mismatch.
