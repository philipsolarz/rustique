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

Module globals and builtins the originals read at call time, pinned with
`cx.pin`/`cx.pin_global` (LOAD_GLOBAL semantics: a module global shadowing the builtin
counts): `_index` (randint, randrange), `_ONE` (randrange), `len` (choice, shuffle,
sample, choices), `range`/`reversed` (shuffle), `_Sequence`, `_ceil`, `_log`,
`_accumulate`, `_bisect`, `isinstance`, `list`, `set`, `int` (sample), `_floor`, `_repeat`,
`_accumulate`, `_bisect`, `_isfinite` (choices), plus the error-path builtins. The class
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

See §6 of this file (filled from `benches/targets/random_cells.py` and
`utils_pyperf.py random`).
