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

| target | module (lazy install) | validated | free-threaded | checkable |
|---|---|---|---|---|
| `random.randint` | `random` | 3.14, 3.15 | yes | no (consumes state) |
| `random.randrange` | `random` | 3.14, 3.15 | yes | no |
| `random.choice` | `random` | 3.14, 3.15 | yes | no |
| `random.shuffle` | `random` | 3.14, 3.15 | yes | no |
| `random.sample` | `random` | 3.14, 3.15 | yes | no |
| `random.choices` | `random` | 3.14, 3.15 | yes | no |

`uniform` and `_randbelow_with_getrandbits` are not targets: `uniform` costs ~1,000 Ir, so
any fallback (Decimal, numpy scalars, a Python `random()`) would exceed 1.05, and
`_randbelow_*` is only called from the six methods (where the kernels inline it) and from
`secrets.randbelow`.

## 1. Exactness: the same stream, the same state

CPython's methods reach randomness only through `self._randbelow(n)`
(`_randbelow_with_getrandbits` unless a subclass changes it), `self.getrandbits(k)` and
`self.random()`. For each call the kernel decides, without running Python code, how
`self` resolves these three attributes — exactly as `LOAD_ATTR` would: the class MRO
(`_PyType_Lookup`), then the instance `__dict__`:

| `self._randbelow` / `self.getrandbits` / `self.random` | what the kernel does |
|---|---|
| `_randbelow_with_getrandbits` + C `getrandbits` (exact `Random`, subclasses that override none of them) | runs the algorithm natively on the object's `RandomObject` state (`rustique_core::random::Mt`): `genrand_uint32`, `getrandbits(k)` word order (least significant first, last word shifted), the rejection loop `while r >= n`, `random()` = `(a*2**26+b)/2**53`. Same words consumed in the same order, so `getstate()` afterwards is identical (tested on every call of the differential tests) |
| `_randbelow_with_getrandbits` + a Python `getrandbits` (`SystemRandom`, user generators) | the loop is inlined and `self.getrandbits(k)` is called each time exactly as the bytecode would (looked up again before every call; `r >= n` through the rich comparison) — `randint`/`randrange`/`choice`/`shuffle`/`sample` |
| another `_randbelow` (a subclass overriding `random()` gets `_randbelow_without_getrandbits`), or an instance attribute shadowing either | `self._randbelow(n)` is called (`randint`/`randrange`/`choice`); `shuffle`/`sample` take `randbelow = self._randbelow` once, as CPython does, and call it |
| a Python `random()` (`choices`) | `random = self.random` once, then `floor(random() * n)` / `bisect(cum_weights, random() * total, 0, hi)` with `_floor`/`_bisect` called whenever `random()` does not return an exact float |

Results built from these draws use the generic number and sequence protocols on the
objects the user code returns (`a + r`, `istart + istep * r`, `seq[r]`, `x[i], x[j] =
x[j], x[i]`, `j in selected`), in CPython's order.

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

Falls back (CPython's original runs; `stats()` reason): any other argument type
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
call through the type-version cache, so patching them changes the path, not the result.
Each function's `__code__`, `__defaults__` and `__kwdefaults__` are compared per call
(reassigning them keeps the patched vectorcall).

**Private state skipped:** the `isinstance(population, _Sequence)` ABC cache entries
(`Sequence._abc_cache`) that `sample`'s first call on a type would add.

## 4. Version coverage

3.14.9 and 3.15.1 (and their free-threaded builds): `random.py`'s six methods are
identical in both (fingerprints differ only by bytecode); refused on other minors, debug
builds, and when the self-test fails (the `RandomObject` layout checked against
`getstate()`, 1,400 natively generated words and three `random()` values against CPython's
on probe generators, C calls only).

## 5. Measurements

### 5.1 Instructions per call (callgrind, region-instrumented)

`python benches/targets/utils_cg.py random_cells [--jit]` on the dev box, CPython 3.15.1
(python-build-standalone), `--profile dev-release` build; each cell in its own process,
Ir(2n) − Ir(n) over n calls, statements unrolled 10× (method of mechanisms.md §1). `orig`:
plain CPython; `rq`: `rustique.enable(only=["random"])`. **M**.

| cell | orig Ir | rq Ir | ratio | JIT orig | JIT rq | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| `randint(1, 6)` (module function) | 2195 | 799 | 0.364 | 1894 | 802 | 0.423 |
| `rng.randint(3, 7)` | 2291 | 809 | 0.353 | 2056 | 813 | 0.395 |
| `rng.randint(0, 2**40)` | 4691 | 1044 | 0.222 | 4315 | 1037 | 0.240 |
| `rng.randint(0, 2**64)` (128-bit path) | 4791 | 1680 | 0.351 | 4536 | 1684 | 0.371 |
| `rng.randint(1, 2**70)` | 4782 | 1814 | 0.379 | 4527 | 1807 | 0.399 |
| `rng.randint(a=1, b=6)` (keywords) | 2354 | 981 | 0.417 | 2102 | 984 | 0.468 |
| `rng.randint(True, 6)` | 2168 | 805 | 0.371 | 1915 | 808 | 0.422 |
| `rng.randrange(10)` | 2212 | 776 | 0.351 | 1946 | 780 | 0.401 |
| `rng.randrange(0, 100, 5)` | 3212 | 1081 | 0.337 | 2615 | 1085 | 0.415 |
| `rng.choice(list of 6)` | 1770 | 546 | 0.308 | 1629 | 549 | 0.337 |
| `rng.choice(tuple of 4)` | 2157 | 569 | 0.264 | 2035 | 576 | 0.283 |
| `rng.choice('abcdef')` | 1778 | 642 | 0.361 | 1633 | 645 | 0.395 |
| `rng.choice(range(10))` | 2887 | 1498 | 0.519 | 2761 | 1501 | 0.544 |
| `rng.choice(deque)` (C sequence, generic protocol) | 1906 | 787 | 0.413 | 1765 | 790 | 0.448 |
| `rng.choices(words, k=40)` | 33643 | 7025 | 0.209 | 32651 | 7038 | 0.216 |
| `rng.choices(T4, weights=[50,30,15,5])` | 8340 | 2595 | 0.311 | 7737 | 2599 | 0.336 |
| `rng.choices(L6, cum_weights=[1..6])` | 6101 | 3359 | 0.551 | 5574 | 3364 | 0.604 |
| `rng.choices(L6)` | 3894 | 957 | 0.246 | 3705 | 962 | 0.260 |
| `rng.sample(words, 5)` | 17249 | 2300 | 0.133 | 16337 | 2313 | 0.142 |
| `rng.sample(range(10**6), 20)` (set method) | 59154 | 18087 | 0.306 | 57311 | 18068 | 0.315 |
| `rng.shuffle(list of 100)` | 175741 | 8876 | 0.051 | 163572 | 8931 | 0.055 |
| `sub.randint(1, 6)` (subclass, no overrides) | 2153 | 800 | 0.372 | 1908 | 803 | 0.421 |
| `sub.choice(L6)` | 1796 | 547 | 0.304 | 1665 | 550 | 0.330 |
| generic paths: | | | | | | |
| `own.randint(1, 6)` (Python `random()`: `_randbelow_without_getrandbits` called) | 6330 | 6328 | 1.000 | 6025 | 6222 | 1.033 |
| `own.choice(L6)` (same) | 5974 | 6109 | 1.023 | 5775 | 6003 | 1.040 |
| `own.choices(L6)` (Python `random()` called) | 4616 | 2546 | 0.551 | 4380 | 2608 | 0.596 |
| `own.sample(L100, 2)` | 17947 | 13110 | 0.730 | 17327 | 12957 | 0.748 |
| `own_bits.randint(1, 6)` (Python `getrandbits`, the SystemRandom shape) | 3047 | 2764 | 0.907 | 2418 | 2838 | **1.174** |
| `own_bits.choice(L6)` | 2691 | 2545 | 0.946 | 2174 | 2619 | **1.205** |
| `own_bits.shuffle(L6)` | 17023 | 15503 | 0.911 | 13838 | 15622 | **1.129** |
| `own_bits.sample(L6, 2)` | 11420 | 6272 | 0.549 | 9878 | 6389 | 0.647 |
| `own_bits.choices(L6)` (C `random()`: native) | 3899 | 956 | 0.245 | 3709 | 961 | 0.259 |
| `shadow.choice(L6)` (instance attribute shadows `getrandbits`) | 1642 | 1521 | 0.927 | 1385 | 1525 | **1.102** |
| fallbacks (steady state: the target suspends after 64 consecutive fallbacks): | | | | | | |
| `rng.choices(L6, [Fraction(1)]*6, k=3)` | 152319 | 152329 | 1.000 | 143032 | 143041 | 1.000 |
| `rng.choice(UserList)` (Python `__len__`/`__getitem__`) | 3629 | 3629 | 1.000 | 3457 | 3457 | 1.000 |
| `rng.sample(L6, k=2, counts=[1]*6)` (calls `sample` again: accelerated) | 21338 | 12336 | 0.578 | 19835 | 11791 | 0.594 |

Every cell is ≤ 1.05 without the JIT. **Under the JIT**, CPython inlines its own calls into
a Python `getrandbits` (SystemRandom, user generators) while a kernel's call into it enters
the eval loop (C → Python): those cells are 1.10–1.21. The targets are therefore declared
`jit_safe: false` (refused with reason `jit` when the JIT is enabled); the native paths would
be 0.22–0.47 under the JIT.

### 5.2 Wall time, large inputs (pyperf)

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

`python benches/targets/utils_first64.py` (Ir per call over 64 calls, fresh process):
`r.choice(UserList)`: 4,434 → 4,870 Ir/call (1.098, +436); `r.choices('abc', [Fraction]*3)`: 46,589 → 47,378 (1.017). After 64 consecutive fallbacks the target is suspended and the cells above
measure 1.000 (steady state).
