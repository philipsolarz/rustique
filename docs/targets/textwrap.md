STATUS: wave 1, group `textwrap` (workstream utils). Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/textwrap/` (kernels, options, identity),
`crates/rustique-core/src/textwrap/` (the algorithms on code points, `cargo test`).
Tests: `tests/targets/test_textwrap.py`, `tests/targets/test_utils_mechanics.py`.
Benchmarks: `benches/targets/textwrap_cells.py` (callgrind), `utils_pyperf.py textwrap`.

# Targets `textwrap.wrap`, `fill`, `shorten`, `dedent`, `indent`, `TextWrapper.wrap`, `TextWrapper.fill`

Vectorcall kernels on the module functions and on the two methods in
`TextWrapper.__dict__`; captured references (`from textwrap import dedent`, bound
`wrapper.fill`) reach them; identity, signature, source and pickling are unchanged.
All seven are side-effect free and `checkable` (check mode compares every fast-path result
with the original: values, types, identity with the arguments).

| target | validated | free-threaded | checkable |
|---|---|---|---|
| `textwrap.wrap`, `fill`, `shorten` | 3.14, 3.15 | yes | yes |
| `textwrap.dedent`, `indent` | 3.14, 3.15 | yes | yes |
| `textwrap.TextWrapper.wrap`, `.fill` | 3.14, 3.15 | yes | yes |

`textwrap.py` is identical in 3.14.9 and 3.15.1 (`dedent` is the 3.14 rewrite in both: still
pure Python, measured first — §5).

## 1. Exactness

The kernels run CPython's algorithm on the text's code points (`rustique_core::textwrap`):

- **Munging**: `str.expandtabs(tabsize)` (column reset on `\n`/`\r`, `tabsize <= 0` drops
  tabs) and `translate(unicode_whitespace_trans)` (the six ASCII whitespace characters to
  spaces).
- **Chunking** replicates `re.split` with `wordsep_re` / `wordsep_simple_re`. `wordsep_re`
  matches at every position (whitespace: first alternative; anything else: the third, at
  the end of the non-whitespace run at the latest), so the chunks are the consecutive
  matches, each found as `sre` would: alternatives in order, the lazy `\S+?` tried with 1,
  2, ... characters, lookbehinds reaching before the match start, `-{2,}` greedy with the
  backtracking outcome made explicit. Character classes use CPython's own Unicode
  database (`_PyUnicode_IsAlpha/IsDecimalDigit/IsDigit/IsNumeric` for `\w` and `\d`,
  `_PyUnicode_IsWhitespace` for `str.strip/isspace/split`), ASCII inline. A self-test at
  install compares the chunking of probe strings (hyphens, em-dashes, digits, Arabic-Indic
  digits, superscripts, non-breaking and em spaces) with the compiled patterns' `split`.
- **`_fix_sentence_endings`**, **`_wrap_chunks`** and **`_handle_long_word`** are ported
  line for line, including `max_lines`/`placeholder` (the placeholder placement loop, the
  previous-line `rstrip()` rule), `drop_whitespace` (Unicode `strip()`), `break_on_hyphens`
  (`is True` selects the pattern, truthiness the hyphen rule of `_handle_long_word`).
- **Identity**: CPython returns its argument in a few cases (`dedent` of one line with no
  margin; `indent` of a single all-whitespace line; `wrap`/`fill`/`shorten` with
  `replace_whitespace=False` when the text is one chunk: `PyUnicode_Substring` of the whole
  string, `''.join([x])` and `'' + x` return `x`). The kernels return the argument in
  exactly these cases (tested: `result is text` compared with CPython).
- **Endless loops**: CPython's `_wrap_chunks` never terminates when an indent is at least
  as wide as `width` and the first line can take nothing (e.g. text starting with a space).
  The kernel detects an iteration that consumes nothing and lets CPython's original run (and
  loop, interruptibly) — a kernel loop would not be interruptible.

## 2. Exact subset and fallbacks

- text: exact `str`; module functions: `width` (and `tabsize`, `max_lines`) exact `int`,
  indents and `placeholder` exact `str`, the flags `bool`/`int`/`None` (truth tested
  without user code); keyword arguments are `TextWrapper`'s. `shorten` adds `max_lines=1`
  (a `max_lines` keyword falls back: CPython raises).
- `TextWrapper.wrap/fill`: the instance's class resolves every method and class attribute
  the algorithm uses (`wrap`, `_munge_whitespace`, `_split`, `_split_chunks`,
  `_fix_sentence_endings`, `_handle_long_word`, `_wrap_chunks`, `wordsep_re`,
  `wordsep_simple_re`, `sentence_end_re`, `unicode_whitespace_trans`) to CPython's own
  (subclasses that override none qualify; cached per type version), no class attribute is
  named like an option, the instance does not shadow a method (inline-values check, else
  `_PyObject_GetMethod`), and its options are of the types above.
- `dedent(text)`, `indent(text, prefix)` (predicate absent or `None`).

Falls back (`args`): str subclasses, other types, options of other types, a `predicate`,
a subclass overriding any method; every error (`width <= 0`, placeholder too large,
non-str text); the endless-loop case. `deps`: a replaced dependency (§3).

## 3. Dependencies

Pinned: `textwrap.TextWrapper` (module global), the class attributes above plus `fill`,
`__init__`, `__new__` (MRO pins: patching any of them on the class suspends the targets);
for `dedent` the builtins it reads (`min`, `max`, `enumerate`). Checked per call:
`TextWrapper.__init__.__defaults__`/`__kwdefaults__` identity (the module functions use
its defaults) and the contents of `unicode_whitespace_trans` (a mutable dict: six lookups).

**Private state skipped:** none (no caches involved; the `TextWrapper` instance the module
functions would create is not created).

## 4. Version coverage

3.14.9 and 3.15.1 and their free-threaded builds; refused elsewhere, on fingerprint
mismatch, and when the chunking self-test fails.

## 5. Measurements

### 5.1 Instructions per call (callgrind)

`python benches/targets/utils_cg.py textwrap_cells [--jit]`, CPython 3.15.1, dev box
(method: random.md §5.1). **M**.

| cell | orig Ir | rq Ir | ratio | JIT orig | JIT rq | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| `fill(desc ~330 chars, 88, initial/subsequent indent)` (report_render) | 500111 | 77011 | 0.154 | 468994 | 77013 | 0.164 |
| `shorten(title, 30, placeholder='...')` (report_render) | 107876 | 17867 | 0.166 | 97302 | 17869 | 0.184 |
| `indent(30 lines, '  ')` (report_render) | 34054 | 12476 | 0.366 | 26973 | 12478 | 0.463 |
| `wrap(540 chars)` | 1089578 | 162015 | 0.149 | 989530 | 162017 | 0.164 |
| `fill('hello world')` | 31932 | 7519 | 0.235 | 27857 | 7521 | 0.270 |
| `wrap('word')` | 18719 | 5529 | 0.295 | 16240 | 5536 | 0.341 |
| `dedent(20 lines)` | 52906 | 24686 | 0.467 | 46594 | 24688 | 0.530 |
| `dedent('x')` | 6468 | 1017 | 0.157 | 6324 | 1019 | 0.161 |
| `indent('x', '> ')` | 2545 | 2104 | 0.826 | 2302 | 2111 | 0.917 |
| `TextWrapper(width=60).wrap(540 chars)` | 1086416 | 165243 | 0.152 | 987801 | 165245 | 0.167 |
| `TextWrapper.fill(desc)` | 509272 | 80556 | 0.158 | 466870 | 80558 | 0.173 |
| fallbacks (steady state): | | | | | | |
| `wrap(str subclass)` | 102797 | 103336 | 1.005 | 89079 | 89079 | 1.000 |
| subclass overriding `_split` | 214942 | 215278 | 1.002 | 170251 | 169999 | 0.999 |
| `indent(text, prefix, predicate)` | 41735 | 41735 | 1.000 | 35526 | 35545 | 1.001 |
| `dedent(str subclass)` | 7077 | 7077 | 1.000 | 7154 | 7140 | 0.998 |

The cheapest fallback (`dedent` of a str subclass, ~7k Ir) is above the ~7k Ir JIT
threshold; every cell is ≤ 1.005 with and without the JIT, so the group is `jit_safe`.

Per-call cost of the class checks that replace pins (TextWrapper methods, `__init__`
defaults, the translation table): one `tp_version_tag` compare for the class, two pointer
compares for the defaults, six dict lookups for the table (~150 Ir, included above).
