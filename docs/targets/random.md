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
