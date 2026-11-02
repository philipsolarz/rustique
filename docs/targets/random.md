STATUS: wave 1, group `random` (workstream utils); wave 2 (workstream `text`, 2026-11-02):
the continuous distributions (§7) and heap-free `choices`/`sample` (§7.6). Contract:
`docs/ACCELERATION.md`. Code: `crates/rustique-py/src/targets/random/` (kernels;
`variates.rs` for §7), `crates/rustique-core/src/random/` (MT19937, the index logic, the
distributions' arithmetic, `cargo test`), `crates/rustique-py/src/targets/utils_kit/`
(helpers shared by the utils groups). Tests: `tests/targets/test_random.py`,
`tests/targets/test_random_variates.py`, `tests/targets/test_utils_mechanics.py`.
Benchmarks: `benches/targets/random_cells.py` (callgrind, `utils_cg.py`),
`benches/targets/random_ft.py` (8 threads, 3.15.1t), `benches/targets/utils_pyperf.py random`.

# Targets `random.randint`, `randrange`, `choice`, `shuffle`, `sample`, `choices`, `normalvariate`, `lognormvariate`, `expovariate`, `paretovariate`, `gauss`

Vectorcall kernels (contract §3.7) on the eleven functions in `random.Random.__dict__`. The
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
| `random.normalvariate` (wave 2) | `random` | 3.14, 3.15 | yes | no | yes |
| `random.lognormvariate` (wave 2) | `random` | 3.14, 3.15 | yes | no | yes |
| `random.expovariate` (wave 2) | `random` | 3.14, 3.15 | yes | no | yes |
| `random.paretovariate` (wave 2) | `random` | 3.14, 3.15 | yes | no | yes |
| `random.gauss` (wave 2) | `random` | 3.14, 3.15 | yes | no | yes |

§1–§6 describe the six wave-1 targets; §7 the five distributions.

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
`mock.patch.object(random.Random, "_randbelow", ...)` — and cached (`utils_kit::TypeCache`:
8 direct-mapped seqlock entries, so traffic alternating between a few receiver types does
not re-classify on every call). The
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

The module globals and builtins the six originals read at call time on the accepted
inputs, pinned once for the whole group (`GroupSpec`, flag `RANDOM_DEPS`):
`_index`, `_floor`, `_ceil`, `_log`, `_isfinite`, `_repeat`, `_accumulate`, `_bisect`,
`_Sequence` (each checked at install to be the object `random` imports it as: `operator.index`,
`math.floor`, ..., `bisect.bisect`, `_collections_abc.Sequence`), `_ONE` (checked = 1), and
what `LOAD_GLOBAL` resolves `len`, `range`, `reversed`, `isinstance`, `list`, `set` to (a
module global shadowing the builtin counts). Replacing any of them suspends all six targets
until the original object is back (CPython's own call meanwhile). Names read only on paths
the kernels never take (error messages, `counts=`) are not pinned. The class attributes
(`_randbelow`, `getrandbits`, `random`) are not pinned: they are resolved per call through
the type-version cache, so patching them sends the calls to the original. Each function's
`__code__`, `__defaults__` and `__kwdefaults__` are compared per call (reassigning them
keeps the patched vectorcall).

**Private state skipped:** the `isinstance(population, _Sequence)` ABC cache entries
(`Sequence._abc_cache`) that `sample`'s first call on a type would add.

## 4. Version coverage

3.14.9 and 3.15.1 (and their free-threaded builds): `random.py`'s six methods are
identical in both (fingerprints differ only by bytecode); refused on other minors, debug
builds, and after the generator self-test failed (it runs at the first native use).

### Private CPython symbols and layouts (accepted by the lead)

| what | used for | obtained / checked at install | when missing or mismatched |
|---|---|---|---|
| `RandomObject` layout (`int index` at `sizeof(PyObject)`, `uint32_t state[624]` after it) | the native generator | self-test at the **first native use** (not at install: ~45k Ir most processes never need), on one private probe `_random.Random(seed)` (C calls only): 40 words generated natively from a copy of its raw state equal its C `getrandbits(1280)`, then one `random()` value and the index after it | the six targets are degraded (`status()` "degraded", every call falls back) and the next `enable()` refuses the group (category `selftest`) |
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
(wave 2, all eleven targets installed: the same, 0 differences on the four builds, merged base
`ws2/text` c9fb335)
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
| `randint(1, 6)` (module function) | 2186 | 775 | 0.355 | 1887 | 777 | 0.412 |
| `rng.randint(3, 7)` | 2291 | 786 | 0.343 | 2056 | 790 | 0.384 |
| `rng.randint(0, 2**40)` | 4691 | 1021 | 0.218 | 4315 | 1014 | 0.235 |
| `rng.randint(0, 2**64)` (128-bit path) | 4791 | 1644 | 0.343 | 4536 | 1648 | 0.363 |
| `rng.randint(1, 2**70)` | 4782 | 1777 | 0.372 | 4527 | 1770 | 0.391 |
| `rng.randint(a=1, b=6)` (keywords) | 2354 | 958 | 0.407 | 2102 | 961 | 0.457 |
| `rng.randint(True, 6)` | 2168 | 782 | 0.361 | 1915 | 785 | 0.410 |
| `rng.randrange(10)` | 2212 | 753 | 0.340 | 1946 | 757 | 0.389 |
| `rng.randrange(0, 100, 5)` | 3212 | 1058 | 0.330 | 2615 | 1062 | 0.406 |
| `rng.choice(list of 6)` | 1770 | 526 | 0.297 | 1629 | 529 | 0.325 |
| `rng.choice(tuple of 4)` | 2157 | 549 | 0.254 | 2035 | 556 | 0.273 |
| `rng.choice('abcdef')` | 1778 | 622 | 0.350 | 1633 | 625 | 0.383 |
| `rng.choice(range(10))` | 2887 | 1478 | 0.512 | 2761 | 1481 | 0.536 |
| `rng.choice(deque)` (C sequence: `len()` twice, `[]`) | 1906 | 759 | 0.398 | 1765 | 762 | 0.432 |
| `rng.choices(words, k=40)` | 33643 | 6902 | 0.205 | 32651 | 6915 | 0.212 |
| `rng.choices(T4, weights=[50,30,15,5])` | 8340 | 2584 | 0.310 | 7737 | 2588 | 0.335 |
| `rng.choices(L6, cum_weights=[1..6])` | 6101 | 3270 | 0.536 | 5574 | 3275 | 0.588 |
| `rng.choices(L6)` | 3894 | 881 | 0.226 | 3705 | 886 | 0.239 |
| `rng.sample(words, 5)` | 17249 | 2263 | 0.131 | 16337 | 2294 | 0.140 |
| `rng.sample(range(10**6), 20)` (set method) | 59154 | 18065 | 0.305 | 57311 | 18043 | 0.315 |
| `rng.shuffle(list of 100)` | 175741 | 8859 | 0.050 | 163572 | 8914 | 0.054 |
| `sub.randint(1, 6)` (subclass, no overrides) | 2153 | 777 | 0.361 | 1908 | 780 | 0.409 |
| `sub.choice(L6)` | 1796 | 527 | 0.293 | 1665 | 530 | 0.318 |
| `own_bits.choices(L6)` (Python `getrandbits`, C `random()`: native) | 3899 | 880 | 0.226 | 3709 | 885 | 0.239 |
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
| `sysr.randint(1, 6)` (`SystemRandom`; see note) | 5169 | 5136 | 0.994 | 4565 | 4407 | 0.965 |
| `sysr.choice(L6)` (`SystemRandom`) | 4742 | 4807 | 1.014 | 4425 | 4254 | 0.961 |
| other fallbacks (steady state): | | | | | | |
| `rng.choices(L6, [Fraction(1)]*6, k=3)` | 152702 | 152698 | 1.000 | 143422 | 143417 | 1.000 |
| `rng.choice(UserList)` (Python `__len__`/`__getitem__`) | 3629 | 3629 | 1.000 | 3457 | 3457 | 1.000 |
| `rng.sample(L6, k=2, counts=[1]*6)` (calls `sample` again: accelerated) | 21338 | 12293 | 0.576 | 19835 | 11748 | 0.592 |
| mixed traffic on one target (stays installed under the net-benefit suspension: ≥ 20% hits; one cell = the whole statement; after wave1 b162e61): | | | | | | |
| `rng.randint(1, 6); own_bits.randint(1, 6)` | 5608 | 4430 | 0.790 | 4407 | 3797 | 0.862 |
| `rng.choice(L6); sysr.choice(L6)` (±5%: `SystemRandom`) | 7037 | 5581 | 0.793 | 5910 | 5226 | 0.884 |
| 4 × `rng.randint(1, 6)` + `own_bits.randint(1, 6)` | 12222 | 6739 | 0.551 | 10161 | 6106 | 0.601 |

Every cell is ≤ 1.05, with and without the JIT. The cells with a Python override measure
1.000 in the steady state: the kernel declines at entry, and after 64 consecutive fallbacks
the registry suspends the target (its original vectorcall restored until the next
`enable()`). The `SystemRandom` cells vary by ±5% between runs in both columns: `os.urandom`
is not seeded, so the number of `getrandbits` rejections in `_randbelow` differs per process.
Before the wave-1 ruling, the kernels called those overrides from native code: 0.91–0.95
without the JIT but 1.10–1.21 under it (the JIT inlines CPython's own calls into a Python
`getrandbits`; a kernel's call enters the eval loop), so the group was `jit_safe: false`.
They now fall back, so the group is JIT-safe (default `jit_safe: true`).

Mixed traffic: when accelerated calls and calls on an overriding class alternate on one
target, the target stays installed under the framework's net-benefit suspension (a hit
credits 4, a fallback debits 1: installed while at least 20% of the calls hit). Each
overriding call then pays the unsuspended fallback cost: about +325 Ir (the pair
`rng.randint` + `own_bits.randint`: 4,430 Ir = 786 native + 3,317 original + 327). The
type cache used to hold one entry, so alternating receiver types re-classified on every
call (0.973, and 1.094 under the JIT). It now has 8 entries, giving the rows above.

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

### Install cost (budget: lazy install ≤ max(5% of the module's import, 0.1M Ir))

| | cold | warm | budget (max(5%, 0.1M Ir)) |
|---|---:|---:|---:|
| `import random` plain | 1.06M Ir | 1.08M Ir | |
| lazy install overhead | +195k (+18.4%) | **+142k (+13.1%)** | 100k: **over by 42k** |

Before this pass: +332k cold, +226k warm. Where the warm 142k goes (callgrind, inclusive):
the framework's per-target work dominates. The six code fingerprints cost ~44k (~7k each:
`_PyCode_GetCode` copies the deoptimized bytecode, then hashing), activating the group's
22 dict pins ~23k, the six vectorcall sites ~8k. The group's own prepare costs ~35k: the 15
interned keyword names the kernels compare by pointer (~10k), the identity validation of
the pinned globals, the class checks. One target alone costs ~80k, each further target
5–17k (`sample` 17k, `choices` 13k: fingerprint size). The generator self-test (~45k: seeding
a probe and a full twist) moved to the first native use (§4), so a process that imports
`random` without calling these methods never pays it. **Accepted by the lead as a
documented wave-1 overage** (+42k over 0.1M Ir): the levers are framework work for wave 2
(cheaper fingerprints and pin activation, deferred validation), not fewer targets.

Method: `python benches/targets/utils_install_cost.py` (callgrind, fresh processes; the
module removed from `sys.modules` first, its dependencies imported; every setup ends with
`gc.collect()` so neither side is charged a collection the other's setup absorbed). *Cold*:
the first group of the process. *Warm*: another group with an imported vectorcall target is
enabled first, so the framework's one-time costs are not charged to this group
(REGISTRY_ANNOUNCE_2 item 16). `enable(only=[group])` with the module not imported costs
0.50M Ir, the same for every group (the registry's first `enable()`, not the group).

The group's install (wave1 b162e61, accel-core's second install-cost pass) is one
`GroupSpec` (`SHARED`): the statics, symbols, validations and **one set of pins for the
whole group** are declared once, and each target adds only its function's fingerprint and
its vectorcall site. Install-time keys are plain `str` objects (`utils_kit::key`): a
`pyo3::intern!` costs ~1.3k Ir on first use, interning ~650 Ir, a plain `str` ~200 Ir, and
dict lookups and the pin watcher compare keys by value.

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
an overriding class (e.g. `random.randint` and `SystemRandom().randint`) stays installed
while at least 20% of the calls hit (net-benefit suspension). Its overriding calls then pay
about +325 Ir each, once the class classifications are cached (the mixed rows of §6.1).
Below 20% hits the target is suspended (after about 640 calls from the ceiling).

## 7. Wave 2: the continuous distributions, and `choices`/`sample` without heap traffic

`normalvariate`, `lognormvariate`, `expovariate`, `paretovariate` and `gauss` are 3.6% of
`custom/fulfilment_sim` (`w2-wl-custom.md` §4: a SimPy-style simulation calling them on a
`random.Random(seed)` instance) and `gauss` 2.5% of `data/report_render`. Code:
`targets/random/variates.rs`; the arithmetic in `rustique_core::random::variates` and
`rustique_core::random::libm`.

### 7.1 Exactness

- **The same stream, consumed in the same amounts** (contract §2.5): each kernel draws
  exactly the `random()` values the method draws, from the object's own state (as §1):
  `expovariate`/`paretovariate` one; `normalvariate`/`lognormvariate` two per round of the
  Kinderman–Monahan loop until acceptance; `gauss` two when `gauss_next` is `None`, none
  otherwise. `getstate()` (which includes `gauss_next`) is identical afterwards (every
  differential test compares it).
- **The same floats, bit for bit**: the same IEEE operations in the same order (Rust
  never contracts `a * b + c` into an FMA), and the same C library functions: `random.py`
  binds `_log`, `_exp`, `_sqrt`, `_cos`, `_sin` from `math`, and `u ** e` is `float_pow`,
  which calls libm `pow` after its special cases; Rust's `f64::ln/exp/cos/sin/powf` lower to
  the same `log/exp/cos/sin/pow` calls (`sqrt` is the IEEE instruction in both). On glibc,
  python-build-standalone links the `@GLIBC_2.2.5` compatibility symbols and the extension
  the `@GLIBC_2.29` ones: the compatibility wrappers differ only in `errno`/SVID handling
  of error inputs, which these arguments never reach (logs of `(0, 1]`, finite
  exponents, …), around the same core. `sin`'s argument passes through `black_box`, so a
  code generator cannot fuse the `cos`/`sin` pair into one `sincos` call. **Checked at the
  first use** (like the generator self-test, §4): 56 probe arguments × the six functions
  against `math.log/exp/sqrt/cos/sin` and `float ** float`; a mismatch degrades the five
  targets (every call falls back) and the next `enable()` refuses them (`selftest`).
  `cargo test` checks the arithmetic against bit patterns produced by CPython 3.15.1 from a
  fixed state (`variates_match_cpython`); Hypothesis compares thousands of calls per run.
  Module constants are read from the pinned globals (`NV_MAGICCONST`, `TWOPI`), not
  recomputed.
- **No error after the stream moved.** Where CPython raises *after* drawing, the kernel
  decides before anything observable and runs the original, which draws and raises:
  `expovariate(0)` (`ZeroDivisionError`), `paretovariate(0)`; `paretovariate` with
  `|alpha| < 1/16` or a non-finite `alpha` (`pow` can overflow, or underflow to a subnormal
  with `ERANGE`, which `float_pow` reports as `OverflowError`; with `|alpha| >= 1/16` the
  result stays within `2**±848`); `lognormvariate` unless `mu + 12.5 |sigma| <= 709` (an
  accepted normal deviate has `|z| <= 2 sqrt(53 ln 2) < 12.13`, so `exp` cannot overflow;
  NaN/inf parameters fail the test). `normalvariate` and `gauss` never raise on accepted
  parameters.
- **Parameters**: an exact `float`, or an exact `int` within ±2**63 (converted as float
  arithmetic converts it, `PyLong_AsDouble`, correctly rounded; `i64 as f64` rounds the same
  way). Everything else falls back: `bool`, `int`/`float` subclasses (`numpy.float64`
  overrides the reflected operators: CPython's result is a `numpy.float64`), `Decimal`,
  `Fraction`, ints beyond 2**63, wrong arity or keywords. Defaults come from the function's
  validated `__defaults__`.
- **Attribute resolution** without running Python code (as §1): `self.random` must be the C
  `random()` of `_random.Random` (class and instance); `lognormvariate`'s
  `self.normalvariate` must be CPython's `Random.normalvariate` (class MRO and instance),
  with its fingerprinted code and no local tool on that code (a breakpoint in
  `normalvariate` sends `lognormvariate` to the original, so the tool sees the frame);
  `gauss` reads and writes `gauss_next` with `PyObject_GenericGetAttr`/
  `PyObject_GenericSetAttr` (what `LOAD_ATTR`/`STORE_ATTR` run there) only when the class
  has the generic `__setattr__`, no `gauss_next` descriptor (slots and properties fall
  back), and the instance keeps its attributes in valid inline values (then they are the
  type's shared keys, all `str`: the lookup cannot run a foreign key's `__eq__`); the value
  read must be `None` or an exact `float`, checked before the first store. Order as
  CPython: read, store `None`, draw, store the second deviate. The inline-values check has
  its own calibration (on `os.environ`, at install), so it also works on 3.14, where
  `_PyDictKeys_StringLookup` is not exported and `utils_kit::INST` is off.
- **Free-threaded builds**: only the draws run under the object's critical section (as
  CPython's `random()`); the arithmetic runs outside it. `normalvariate` takes the section
  once per round (CPython once per `random()` call), so another thread can draw between
  rounds in both. `gauss` is "not thread-safe without a lock" in CPython too: its
  `gauss_next` accesses are the same separate attribute operations.
- **Dependencies**: the five have their own `GroupSpec` (`random.variates`) and flag:
  `_log`, `_exp`, `_sqrt`, `_cos`, `_sin` (checked to be `math`'s), `TWOPI` (`math.tau`) and
  `NV_MAGICCONST` (an exact float) are pinned; replacing one suspends these five, not the
  six wave-1 targets. Each function's `__code__`/`__defaults__` is compared per call.
- **Private state skipped**: none beyond §3's.

### 7.2 Net benefit (contract §5.6)

A fallback in mixed traffic (accelerated calls interleaved with calls the kernel declines)
costs the caller 330–440 Ir (the original's frame is pushed through the generic path);
a hit saves ~800 Ir (`expovariate`, `paretovariate`: break-even at 31–35% hits), ~1,300 Ir
(`gauss`: 24%), ~3,100–3,400 Ir (`normalvariate`, `lognormvariate`: 10%). So `expovariate`
and `paretovariate` credit 2 per hit (installed while at least a third of the calls hit),
`gauss` 3 (a quarter), the other two the default 4 (a fifth). The mixed rows of §7.3 show
the result at and below those thresholds.

### 7.3 Instructions per call (callgrind, 3.15.1, `utils_cg.py random_cells`, **M**)

| cell | CPython Ir | Rustique Ir | ratio |
|---|---:|---:|---:|
| `rng.expovariate(2.0)` | 1,457 | 641 | **0.440** |
| `rng.expovariate()` (default) | 1,519 | 622 | 0.410 |
| `rng.expovariate(lambd=3)` (keyword, int) | 1,636 | 765 | 0.467 |
| `random.expovariate(1.5)` (module function) | 1,463 | 640 | 0.438 |
| `rng.paretovariate(2.0)` | 1,463 | 729 | **0.498** |
| `rng.normalvariate(0.0, 1.0)` | 3,929 | 878 | **0.223** |
| `rng.normalvariate()` (defaults) | 4,000 | 833 | 0.208 |
| `rng.normalvariate(10, 2)` (ints) | 4,088 | 946 | 0.231 |
| `rng.lognormvariate(0.0, 0.5)` | 4,529 | 1,122 | **0.248** |
| `rng.gauss(90, 60)` | 2,662 | 1,339 | **0.503** |
| `random.gauss()` (module function) | 2,569 | 1,222 | 0.476 |
| `sub.expovariate(2.0)` / `sub.gauss(0.0, 1.0)` (subclass, no overrides) | 1,462 / 2,505 | 642 / 1,267 | 0.439 / 0.506 |
| fallbacks, steady state (suspended after 64 consecutive): | | | |
| `own.expovariate/normalvariate/lognormvariate/paretovariate/gauss` (Python `random()`) | 2,092 / 6,275 / 6,804 / 2,098 / 3,221 | same | 1.000 each |
| `sysr.expovariate(2.0)` (`SystemRandom`) | 4,020 | 4,020 | 1.000 |
| `rng.expovariate(numpy.float64(2.0))` | 1,798 | 1,812 | 1.008 |
| `rng.expovariate(Fraction(2))` | 9,270 | 9,270 | 1.000 |
| `rng.expovariate(0)` (`ZeroDivisionError`, caught) | 4,353 | 4,345 | 0.998 |
| `rng.paretovariate(0.03)` (range guard) | 1,463 | 1,463 | 1.000 |
| `rng.lognormvariate(700.0, 1.0)` (overflow guard; the original's `normalvariate` call is accelerated) | 4,546 | 1,496 | 0.329 |
| `odd.gauss()` with `gauss_next = 1` (an int) | 1,376 | 1,376 | 1.000 |
| mixed traffic (stays installed above the §7.2 thresholds): | | | |
| `rng.expovariate` + `own.expovariate` (1:1) | 3,789 | 3,183 | 0.840 |
| `rng.expovariate` + 2 × `own.expovariate` (1:2, at the threshold) | 5,857 | 5,721 | 0.977 |
| `rng.paretovariate` + `rng.paretovariate(numpy.float64)` (1:1) | 3,498 | 3,096 | 0.885 |
| `rng.paretovariate` + 4 × the numpy call (1:4: suspended) | 9,619 | 9,619 | 1.000 |
| `rng.gauss` + `own.gauss` (1:1) / + 3 × (1:3) | 6,241 / 13,261 | 4,998 / 12,334 | 0.801 / 0.930 |
| `rng.normalvariate` + `own.normalvariate` (1:1) | 10,463 | 7,517 | 0.718 |

Every cell ≤ 1.05 (worst 1.008). Under the JIT (`PYTHON_JIT=1`, warmed 30,000 calls,
`utils_cg.py random_cells --jit --only expovariate`): fast paths 0.403–0.459, fallbacks
1.000–1.008, mixed 1:1 0.875, mixed 1:2 1.007.

### 7.4 Eight threads (3.15.1t, `benches/targets/random_ft.py`, CPU time per call, **M**)

The 8-thread rule (DECISIONS §10.9): CPU time per call at 8 threads ≤ 1.05× the original.
`local`: every thread its own `random.Random(5)`; `shared`: one generator used by all
threads (the module-level functions always share `random._inst`). Dev box, 3.15.1t,
`-t 1,8 -r 3`, median of 3 (wall-time efficiencies are noisy on the shared box; CPU time
per call is the metric):

| cell | inputs | enabled/plain, 1 thread (wall ns per call) | enabled/plain CPU per call, 8 threads |
|---|---|---:|---:|
| `R.expovariate(2.0)` | local / shared | 0.62 / 0.76 | **0.58 / 0.36** |
| `R.paretovariate(2.0)` | local / shared | 0.64 / 0.84 | 0.65 / 0.38 |
| `R.normalvariate(0.0, 1.0)` | local / shared | 0.33 / 0.32 | 0.28 / 0.15 |
| `R.lognormvariate(0.0, 0.5)` | local / shared | 0.32 / 0.37 | 0.41 / 0.17 |
| `R.gauss(90, 60)` | local / shared | 0.63 / 0.65 | 0.55 / 0.19 |
| `random.expovariate(2.0)` (module) | — | 0.59–0.91 | 0.31 |
| `random.gauss()` (module) | — | 0.55–0.63 | 0.20–0.31 |
| `R.choices(T4, W4)` (weights) | local / shared | 0.19 / 0.19 | 0.19 / 0.73 |
| `R.choices(W, k=40)` | local / shared | 0.19 / 0.20 | 0.21 / 0.75 |
| `R.sample(W, 5)` | local / shared | 0.11 / 0.12 | 0.07 / 0.45 |
| `R.sample(range(10**6), 20)` | local / shared | 0.19 / 0.19 | 0.19 / 0.47 |
| `R.randint(1, 1000)` (wave 1, reference) | local / shared | 0.48 / 0.50 | 0.49 / 0.98 |

The first version computed inside the generator's critical section: with 8 threads on one
generator the waiters spun in `PyMutex` (`sched_yield`) and CPU per call was 1.4–2.3× the
original's (`expovariate` shared 2.10, module-level `random.expovariate` 1.77). Drawing
under the section and computing outside it brought every row to ≤ 0.98.

### 7.5 Workloads (instructions per steady-state `run()`, `benches/targets/text_wl.py`, 3.15.1, **M**)

`perf stat -e instructions:u`, (I_3 − I_1)/2, median of 2, digests identical. "before" is
`ws2/text` b864a0b (wave-1 random kernels), "after" this branch.

| workload | `--only random` before → after | all groups before → after |
|---|---:|---:|
| `custom/fulfilment_sim` | 0.998 → **0.975** | 0.996 → **0.975** |
| `data/report_render` | 0.758 → **0.746** | 0.348 → **0.337** |

### 7.6 `choices` and `sample` without heap traffic

- `choices` with weights: the weights are read under the list's lock straight into a stack
  buffer of exact `f64` cumulative sums (≤ 64 items; a `Vec` beyond), ints kept as `i64`
  within ±2**53 (no `i128` → `f64` software conversion); no intermediate `Vec<Num>`.
- `sample`: the indices go straight into the result list (no index `Vec`); the pool method
  keeps its index pool on the stack for populations ≤ 512, the set method its selected
  indices in a stack open-addressing table for `k <= 128` (`hashbrown` beyond). Same
  draws in the same order (`cargo test sample_each_matches_reference`: 18 shapes across
  every method and size class, stream position compared).
- Keyword names are matched by identity first (call sites pass interned constants), so
  `weights=`/`k=` no longer compare against every parameter name.

| cell (3.15.1) | CPython Ir | wave 1 | wave 2 | ratio now |
|---|---:|---:|---:|---:|
| `rng.choices(T4, weights=[50,30,15,5])` | 8,347 | 2,584 | 1,850 | **0.222** |
| `rng.choices(L6, cum_weights=[1..6])` | 6,139 | 3,270 | 1,966 | 0.320 |
| `rng.sample(W, 5)` (set method) | 17,263 | 2,263 | 1,656 | **0.096** |
| `rng.sample(range(10**6), 20)` (set method) | 59,225 | 18,065 | 16,824 | 0.284 |

### 7.7 Conformance, install cost

`test.test_random`: 114 pass, 2 skip, **0 differences** with all eleven targets installed on
3.15.1, 3.15.1t, 3.14.9, 3.14.9t; `pytest -q` (whole suite) green on the four builds except
one `copy` test on 3.14.9 (`test_copy_differential.py::test_tool_attached_by_hook_sees_the_rest`,
a `_slotnames` call order under a profiler; it fails alone too and is not this group's). Install (warm lazy install
on `import random`, `utils_install_cost.py`): **+235k Ir** (+21.7% of a 1.08M import) for the
eleven targets, against +148k for the six (budget 100k): the five add ~87k (five code
fingerprints, five sites, seven pins, the probe calibration), a further documented
overage until the framework validates on first call.
