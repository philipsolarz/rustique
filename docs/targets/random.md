STATUS: wave 1, group `random` (workstream utils). Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/random/` (kernels), `crates/rustique-core/src/random/`
(MT19937 and the index logic, `cargo test`), `crates/rustique-py/src/targets/utils_kit/`
(helpers shared by the utils groups). Tests: `tests/targets/test_random.py`,
`tests/targets/test_utils_mechanics.py`. Benchmarks: `benches/targets/random_cells.py`
(callgrind, `utils_cg.py`), `benches/targets/utils_pyperf.py random`.

# Targets `random.randint`, `randrange`, `choice`, `shuffle`, `sample`, `choices`

Vectorcall kernels (contract §3.7) on the six functions in `random.Random.__dict__`. The
module-level `random.randint` etc. are bound methods of `random._inst`, so they, user
`Random(seed)` instances, subclasses and every captured reference (`from random import
randint`, bound methods, `partial`) reach the kernels; identity, pickling, signature,
`__code__` and source are unchanged (tested with `_mechanics`).

| target | module (lazy install) | validated | free-threaded | checkable | JIT |
|---|---|---|---|---|---|
| `random.randint` | `random` | 3.14, 3.15 | yes | no (consumes state) | yes (§6.1) |
| `random.randrange` | `random` | 3.14, 3.15 | yes | no | yes |
| `random.choice` | `random` | 3.14, 3.15 | yes | no | yes |
| `random.shuffle` | `random` | 3.14, 3.15 | yes | no | yes |
| `random.sample` | `random` | 3.14, 3.15 | yes | no | yes |
| `random.choices` | `random` | 3.14, 3.15 | yes | no | yes |

`uniform` and `_randbelow_with_getrandbits` are not targets: `uniform` costs ~1,000 Ir, so
any fallback (Decimal, numpy scalars, a Python `random()`) would exceed 1.05, and
`_randbelow_*` is only called from the six methods (where the kernels inline it) and from
`secrets.randbelow`.

## 1. Exactness: the same stream, the same state

CPython's methods reach randomness only through `self._randbelow(n)`
(`_randbelow_with_getrandbits` unless a subclass changes it), `self.getrandbits(k)` and
`self.random()`. At entry, before anything observable, the kernel decides without running
Python code how `self` resolves these three attributes — exactly as `LOAD_ATTR` would: the
class MRO (`_PyType_Lookup`), then the instance `__dict__`:

| `self._randbelow` / `self.getrandbits` / `self.random` | what the kernel does |
|---|---|
| `_randbelow_with_getrandbits` + C `getrandbits` (exact `Random`, subclasses that override none of them); for `choices`, the C `random` | runs the algorithm natively on the object's `RandomObject` state (`rustique_core::random::Mt`): `genrand_uint32`, `getrandbits(k)` word order (least significant first, last word shifted), the rejection loop `while r >= n`, `random()` = `(a*2**26+b)/2**53`. Same words consumed in the same order, so `getstate()` afterwards is identical (tested on every call of the differential tests) |
| anything else: a Python `getrandbits` (`SystemRandom`, user generators), another `_randbelow` (a subclass overriding `random()` gets `_randbelow_without_getrandbits` from `Random.__init_subclass__`), a Python `random()` (`choices`), an instance attribute shadowing any of them | CPython's original runs (`stats()` reason `args`) |

**A kernel never calls user code** (lead's ruling on the wave-1 report, after ruling 15):
the overrides are known before anything observable, so the decision is made at entry and
the original runs with CPython's frames — an exception raised in a Python `getrandbits`,
`_randbelow` or `random()` carries the traceback of `randint` → `_randbelow_with_getrandbits`
→ `getrandbits` exactly as without Rustique (`test_user_code_errors_have_cpythons_frames`).
Only C code runs inside a kernel: the C `getrandbits`/`random` algorithm, `len()`/`[]` of
C-level sequences in `choice`, and exact-`int` arithmetic (`istart + istep * r` beyond
`i128`). The one Python function a kernel can call is CPython's own
`_randbelow_with_getrandbits(0)`, when a C-level sequence's length drops to 0 between
`choice`'s two `len()` calls (CPython then loops forever in it; so does the kernel).

`sample` chooses between the pool and the set method like CPython:
`n <= 21 + 4 ** ceil(log(3k, 4))` (k > 5). CPython computes the exponent in floating point;
`3k` is never a power of 4, and for `k < 2**40` the distance of `log4(3k)` to the next
integer is far larger than the error of the two `log` calls, so the integer computation
gives the same threshold (`cargo test`: every `k < 2**20`, sampled up to `2**40`).

`choices` with weights: the cumulative sums are computed with Python's int/float
semantics (`int + float` converts the int with correct rounding) and compared exactly;
every value must be representable exactly in a double (ints up to 2**53), otherwise the
call falls back. `bisect_right` compares `x < a[mid]` as `_bisect` does.

**Lookups without side effects.** The class side is classified once per (type,
`tp_version_tag`) — the tag changes whenever the class or a base is modified, e.g.
`mock.patch.object(random.Random, "_randbelow", ...)` — and cached (a seqlock). The
instance side reads the object's inline values (CPython 3.13+ layout, calibrated at install:
`utils_kit::InstLayout`): if the type's shared keys lack the three names and the object
uses only its inline values, no instance attribute can shadow them; otherwise the
instance's own value slot for the name is read, or `_PyObject_GetMethod` (the function
behind `LOAD_ATTR`) answers. A type whose attribute lookup is not the generic one
(`__getattribute__`/`__getattr__`) falls back.

**Free-threaded builds.** The `RandomObject` state is read and written under the object's
critical section (CPython's `getrandbits`/`random` are `@critical_section`), together with
the list's for `choice`/`shuffle`/`sample`/`choices` on lists; nothing that can run
Python code or trigger GC happens inside it (result lists are allocated first). Holding
the lock for a whole `shuffle` gives one of the interleavings CPython allows.

## 2. Exact subset and fallbacks

Accelerated (positional or keyword arguments):

- `randint(a, b)`, `randrange(start[, stop[, step]])`: `int` arguments (any subclass, read
  as `operator.index` reads them: `bool`, `IntEnum`), ranges up to 2**125;
- `choice(seq)`: `list`, `tuple`, `str`, `bytes`, `range`, and any sequence type implemented
  in C (its `len`/`[]` slots are not Python `__len__`/`__getitem__`: `deque`, `array`,
  `bytearray`, numpy arrays) — `len()` is called twice and `seq[i]` once, as in CPython;
- `shuffle(x)`: exact `list`;
- `sample(population, k)` (`counts` absent or `None`): `list`, `tuple`, `str`, `bytes`,
  `range`; `k` an int;
- `choices(population, weights=None, *, cum_weights=None, k=1)`: same populations,
  weights/cum_weights a `list`/`tuple` of exact ints/floats.

Falls back (CPython's original runs; `stats()` reason): a receiver whose `_randbelow` /
`getrandbits` (or `random` for `choices`) is not CPython's C one — Python overrides,
`SystemRandom`, instance attributes (`args`, §1); any other argument type
(`args`); every error CPython raises (empty sequence, empty range, `b < a`, zero step,
negative/oversized `k`, weights length mismatch, non-positive or non-finite total,
`counts=`, `TypeError`s) — decided before anything observable (`args`); a class or
instance with a non-generic `__getattribute__` or without `_randbelow` (`args`); a
replaced dependency (`deps`, see §3); a tool listening (`tools`).

## 3. Dependencies (pinned)

Module globals and builtins the originals read at call time on the accepted inputs,
pinned with `cx.pin`/`cx.pin_global` (LOAD_GLOBAL semantics: a module global shadowing
the builtin counts): `_index` (randint, randrange), `_ONE` (randrange), `len` (choice,
shuffle, sample, choices), `range`/`reversed` (shuffle), `_Sequence`, `_ceil`, `_log`,
`isinstance`, `list`, `set`, `range` (sample), `_floor`, `_repeat`, `_accumulate`,
`_bisect`, `_isfinite`, `list` (choices). Names read only on paths the kernels never take
(error messages, `counts=`) are not pinned. The class
attributes (`_randbelow`, `getrandbits`, `random`) are not pinned: they are resolved per
call through the type-version cache, so patching them sends the calls to the original.
Each function's `__code__`, `__defaults__` and `__kwdefaults__` are compared per call
(reassigning them keeps the patched vectorcall).

**Private state skipped:** the `isinstance(population, _Sequence)` ABC cache entries
(`Sequence._abc_cache`) that `sample`'s first call on a type would add.

## 4. Version coverage

3.14.9 and 3.15.1 (and their free-threaded builds): `random.py`'s six methods are
identical in both (fingerprints differ only by bytecode); refused on other minors, debug
builds, and when the self-test fails.

### Private CPython symbols and layouts (accepted by the lead)

| what | used for | obtained / checked at install | when missing or mismatched |
|---|---|---|---|
| `RandomObject` layout (`int index` at `sizeof(PyObject)`, `uint32_t state[624]` after it) | the native generator | self-test on one probe `_random.Random(seed)` (C calls only): 40 words generated natively from a copy of its raw state equal its C `getrandbits(1280)`, then one `random()` value and the index after it | the group is refused (self-test failure "random generator layout and stream") |
| `_PyObject_GetMethod` (exported, `Objects/object.c`) | the exact "does the instance shadow `_randbelow`/`getrandbits`/`random`" answer when the inline-values check cannot answer (materialized `__dict__`, no inline values) | `cx.symbol` | refused, category `symbol` ("symbol _PyObject_GetMethod not exported by this interpreter") |
| `_PyDictKeys_StringLookup` (exported, `Objects/dictobject.c`) | does a type's shared-keys table contain one of the three names | `dlsym`, during the inline-values calibration | not refused: the inline-values fast check stays off and every instance check goes through `_PyObject_GetMethod` (exact, slower) |
| inline-values layout (`Py_TPFLAGS_INLINE_VALUES`, `ht_cached_keys`, `dk_nentries` at offset 24 of the keys, the values' `valid` byte and slots after `tp_basicsize`) | the fast "no instance attribute can shadow" check (`utils_kit::InstLayout`) | calibrated on `os.environ` (an `os._Environ`): flags and shared keys present, its five attribute names distinct entries below `dk_nentries`, the `_data` slot holding the object the generic getattr returns | not refused: the fast check stays off (as above) |
| `slot_sq_length`, `slot_sq_item`, `slot_mp_length`, `slot_mp_subscript` (CPython's slot wrappers for Python `__len__`/`__getitem__`) | `choice` on C-level sequences: excludes types whose `len()`/`[]` run Python code | read from `os._Environ`'s type slots, after checking that class defines both methods as Python functions | refused ("os._Environ does not define __len__/__getitem__ in Python" / "... has no sequence/mapping slots") |
| `_PyType_Lookup` (`PyAPI_FUNC`, `Include/cpython/object.h`) | MRO lookups of the three names (borrowed, compared only) | linked (non-limited API) | exported by every 3.14/3.15 build |

## 5. Conformance

`python tests/conformance/run_target_suites.py --targets random --modules ...` (CPython's own test
modules from `$CPYTHON_SRC`, plain vs `enable(only=["random"])` before the tests import the
module, per test id; every target asserted `installed`), on 3.15.1, 3.15.1t, 3.14.9 and
3.14.9t: `test.test_random`: 114 pass, 2 skip — **identical outcome sets on all four builds, 0 differences**
(`benches/targets/utils_matrix.sh`, which also runs `tests/targets` and the whole
`pytest -q`). The runner normalizes object addresses in subTest ids (`0x…`), which differ
between processes (`test_urlparse` parametrizes with function reprs).

## 6. Measurements

### 6.1 Instructions per call (callgrind, region-instrumented)

`python benches/targets/utils_cg.py random_cells [--jit]` on the dev box, CPython 3.15.1
(python-build-standalone), `--profile dev-release` build; each cell in its own process,
Ir(2n) − Ir(n) over n calls, statements unrolled 10× (method of mechanisms.md §1). `orig`:
plain CPython; `rq`: `rustique.enable(only=["random"])`. **M**.

| cell | orig Ir | rq Ir | ratio | JIT orig | JIT rq | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| `randint(1, 6)` (module function) | 2169 | 769 | 0.355 | 1892 | 770 | 0.407 |
| `rng.randint(3, 7)` | 2291 | 777 | 0.339 | 2056 | 781 | 0.380 |
| `rng.randint(0, 2**40)` | 4691 | 1012 | 0.216 | 4315 | 1005 | 0.233 |
| `rng.randint(0, 2**64)` (128-bit path) | 4791 | 1635 | 0.341 | 4536 | 1639 | 0.361 |
| `rng.randint(1, 2**70)` | 4782 | 1768 | 0.370 | 4527 | 1761 | 0.389 |
| `rng.randint(a=1, b=6)` (keywords) | 2354 | 949 | 0.403 | 2102 | 952 | 0.453 |
| `rng.randint(True, 6)` | 2168 | 773 | 0.356 | 1915 | 776 | 0.405 |
| `rng.randrange(10)` | 2212 | 744 | 0.336 | 1946 | 748 | 0.384 |
| `rng.randrange(0, 100, 5)` | 3212 | 1049 | 0.327 | 2615 | 1053 | 0.403 |
| `rng.choice(list of 6)` | 1770 | 517 | 0.292 | 1629 | 520 | 0.319 |
| `rng.choice(tuple of 4)` | 2157 | 540 | 0.250 | 2035 | 547 | 0.269 |
| `rng.choice('abcdef')` | 1778 | 613 | 0.345 | 1633 | 616 | 0.377 |
| `rng.choice(range(10))` | 2887 | 1469 | 0.509 | 2761 | 1472 | 0.533 |
| `rng.choice(deque)` (C sequence: `len()` twice, `[]`) | 1906 | 750 | 0.394 | 1765 | 753 | 0.427 |
| `rng.choices(words, k=40)` | 33643 | 6893 | 0.205 | 32651 | 6906 | 0.212 |
| `rng.choices(T4, weights=[50,30,15,5])` | 8340 | 2575 | 0.309 | 7737 | 2579 | 0.333 |
| `rng.choices(L6, cum_weights=[1..6])` | 6101 | 3261 | 0.535 | 5574 | 3266 | 0.586 |
| `rng.choices(L6)` | 3894 | 872 | 0.224 | 3705 | 877 | 0.237 |
| `rng.sample(words, 5)` | 17249 | 2254 | 0.131 | 16338 | 2265 | 0.139 |
| `rng.sample(range(10**6), 20)` (set method) | 59154 | 18056 | 0.305 | 57311 | 18034 | 0.315 |
| `rng.shuffle(list of 100)` | 175741 | 8850 | 0.050 | 163572 | 8905 | 0.054 |
| `sub.randint(1, 6)` (subclass, no overrides) | 2153 | 768 | 0.357 | 1908 | 771 | 0.404 |
| `sub.choice(L6)` | 1796 | 518 | 0.288 | 1665 | 521 | 0.313 |
| `own_bits.choices(L6)` (Python `getrandbits`, C `random()`: native) | 3899 | 871 | 0.223 | 3709 | 876 | 0.236 |
| Python overrides of the randomness sources: the original runs, decided at entry (steady state: the target suspends after 64 consecutive fallbacks): | | | | | | |
| `own.randint(1, 6)` (Python `random()`, hence `_randbelow_without_getrandbits`) | 6330 | 6330 | 1.000 | 6025 | 6025 | 1.000 |
| `own.choice(L6)` (same) | 5974 | 5974 | 1.000 | 5775 | 5775 | 1.000 |
| `own.choices(L6)` (same) | 4616 | 4616 | 1.000 | 4380 | 4380 | 1.000 |
| `own.sample(L100, 2)` (same) | 17947 | 17947 | 1.000 | 17327 | 17327 | 1.000 |
| `own_bits.randint(1, 6)` (Python `getrandbits`, the SystemRandom shape) | 3047 | 3047 | 1.000 | 2418 | 2418 | 1.000 |
| `own_bits.choice(L6)` (same) | 2691 | 2691 | 1.000 | 2174 | 2174 | 1.000 |
| `own_bits.shuffle(L6)` (same) | 17023 | 17023 | 1.000 | 13838 | 13838 | 1.000 |
| `own_bits.sample(L6, 2)` (same) | 11420 | 11423 | 1.000 | 9878 | 9881 | 1.000 |
| `shadow.choice(L6)` (instance attribute shadows `getrandbits`) | 1642 | 1642 | 1.000 | 1385 | 1411 | 1.019 |
| `sysr.randint(1, 6)` (`SystemRandom`; see note) | 5213 | 4942 | 0.948 | 4398 | 4537 | 1.032 |
| `sysr.choice(L6)` (`SystemRandom`) | 4646 | 4698 | 1.011 | 4363 | 4190 | 0.960 |
| other fallbacks (steady state): | | | | | | |
| `rng.choices(L6, [Fraction(1)]*6, k=3)` | 152702 | 152698 | 1.000 | 143422 | 143417 | 1.000 |
| `rng.choice(UserList)` (Python `__len__`/`__getitem__`) | 3629 | 3629 | 1.000 | 3457 | 3457 | 1.000 |
| `rng.sample(L6, k=2, counts=[1]*6)` (calls `sample` again: accelerated) | 21338 | 12284 | 0.576 | 19835 | 11739 | 0.592 |

Every cell is ≤ 1.05, with and without the JIT. The cells with a Python override measure
1.000 in the steady state: the kernel declines at entry, and after 64 consecutive fallbacks
the registry suspends the target (its original vectorcall restored until the next
`enable()`). The `SystemRandom` cells vary by ±5% between runs in both columns: `os.urandom`
is not seeded, so the number of `getrandbits` rejections in `_randbelow` differs per process.
Before the wave-1 ruling, the kernels called those overrides from native code: 0.91–0.95
without the JIT but 1.10–1.21 under it (the JIT inlines CPython's own calls into a Python
`getrandbits`; a kernel's call enters the eval loop), so the group was `jit_safe: false`.
They now fall back, so the group is JIT-safe (default `jit_safe: true`).

### 6.2 Wall time, large inputs (pyperf)

`python benches/targets/utils_pyperf.py random --fast` (median of pyperf values, fresh
workers; the shared dev box was busy, so only the ≥ 1.5× claims are meaningful; the lead
re-measures on a quiet box). **M**.

| cell | plain | rustique | ratio |
|---|---:|---:|---:|
| `rng.shuffle(list of 1000)` | 108.7 µs | 7.7 µs | 0.071 |
| `rng.sample(range(10**6), 1000)` | 167.2 µs | 46.4 µs | 0.277 |
| `rng.sample(list of 1000, 500)` | 59.1 µs | 5.5 µs | 0.092 |
| `rng.choices(list of 1000, k=1000)` | 43.1 µs | 8.9 µs | 0.206 |
| `rng.choices(list of 1000, weights, k=1000)` | 261.3 µs | 72.6 µs | 0.278 |
| `[rng.randint(1, 6) for _ in range(1000)]` | 139.6 µs | 56.5 µs | 0.405 |

### Workloads

`python benches/workloads/run.py compare data/report_render data/log_analytics
data/billing_engine web/requests_local -n 5 -p 3 --only random,textwrap,urllib,statistics`
(3.15.1, busy shared box; medians of steady-state runs; digests identical in every pair). **M**.

| workload | plain | utils groups enabled | ratio | where the utils groups act |
|---|---:|---:|---:|---|
| `data/report_render` | 409.5 ms | 183.9 ms | **0.449** | `random` (choice/choices/sample/randint) + `textwrap` (fill/shorten/indent) |
| `data/log_analytics` | 770.4 ms | 751.5 ms | 0.975 | `urllib` (`urlsplit` misses, `parse_qs`); `ipaddress` (20%) is not a target yet |
| `web/requests_local` | 266.8 ms | 248.1 ms | 0.930 | `urllib` (`urlsplit`/`parse_qs` in the handler, `requests`' `quote`/`urlparse`) |
| `data/billing_engine` | 1024.2 ms | 1021.0 ms | 0.997 | `statistics` on `Decimal` totals falls back (by design); `stdev` of floats accelerated |

### Install cost (budget: ≤ 5% of the module's import; ≤ 0.1M Ir per group at `enable()`)

`python benches/targets/utils_install_cost.py random` (callgrind, fresh processes; the module
removed from `sys.modules` first, its dependencies imported), after the framework's
install-cost fixes (wave1 5a690e2, fingerprint format v2): lazy install on `import random`
+332k Ir on a 1.06M-Ir `import random` (+31%; its dependencies already imported). One target alone (`random.randint`) costs +159k, each further target ~+20–35k. `enable(only=["random"])` with the module not imported costs 0.56M Ir, the same for
every group (the registry's `enable()`, not the group). The group's own prepare is trimmed
(single-probe self-tests or pattern hashes, no `eval`, no probe classes — the inline-values
layout is calibrated on `os.environ`, the `slot_*` functions read from `os._Environ` —, no
class pins, pins only for names read on the accepted paths); the remaining fixed cost of the
first target of a module (~150k Ir) is in the framework's activation path.

### First 64 fallback calls (before the registry suspends a target that keeps falling back)

`python benches/targets/utils_first64.py random [--n N]` (Ir per call over the first N calls
in a fresh process, plain vs enabled; 3.15.1). **M**.

| cell | plain Ir/call | enabled Ir/call | ratio (N = 64) | extra Ir per call |
|---|---:|---:|---:|---:|
| `ob.randint(1, 6)` (Python `getrandbits`) | 3,712 | 4,383 | 1.181 | +670 |
| `ob.choice(L6)` (same) | 3,474 | 3,976 | 1.144 | +502 |
| `ob.shuffle(L6)` (same) | 17,120 | 17,629 | 1.030 | +509 |
| `orr.randint(1, 6)` (Python `random()`) | 6,979 | 7,650 | 1.096 | +671 |
| `orr.choices(L6)` (same) | 5,359 | 5,829 | 1.088 | +470 |
| `sysr.randint(1, 6)` (`SystemRandom`, ±5% noise) | 5,807 | 6,473 | 1.115 | +666 |
| `sysr.choice(L6)` (same) | 5,783 | 6,279 | 1.086 | +496 |
| `r.choice(UserList)` | 4,434 | 4,871 | 1.099 | +437 |
| `r.choices('abc', [Fraction]*3)` | 46,592 | 47,310 | 1.015 | +718 |

The extra cost is per call (N = 32 and N = 63 give the same +450–700 Ir). The suspension
itself, on the 64th call, costs about 1.5k Ir once. Each fallback pays for the kernel's
argument and class checks plus the original's eval-loop entry (a call site whose callee has
a replaced vectorcall is not specialized, so the original's frame is not pushed inline).
The steady state is 1.000 (§6.1). A program that interleaves accelerated calls with calls on
an overriding class (e.g. `random.randint` and `SystemRandom().randint`) resets the streak,
never suspends, and keeps paying this cost on the overriding calls.
