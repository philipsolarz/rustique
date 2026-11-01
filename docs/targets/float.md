STATUS: wave 1, workstream `json` (group `float` ships with it). 2026-11-01.

# Target group `float`

One target, `float.repr`: CPython's `float_repr` (`repr`/`str` of a float) computed by a
Rust shortest-round-trip core instead of David Gay's bignum `_Py_dg_dtoa`.

## Sites

| site | kind | reached by |
|---|---|---|
| `PyFloat_Type.tp_repr` | static type slot (process-global) | `repr(x)`, `str(x)` (`object_str` calls `tp_repr`), `f"{x!r}"`, `'%r'`/`'%s' % x`, `print`, logging, container reprs, `_json`'s float encoding (`PyFloat_Type.tp_repr(obj)`) |
| `float.__format__` (`ml_meth`, `METH_O`) | static method table (process-global) | `f"{x}"`, `format(x)`, `"{}".format(x)` — an **empty** format spec only |

Measured reachability (`tests/targets/test_float.py::test_reachability`): every row above
hits the kernel once per call. Not reached, and still exact: `float.__repr__(x)` (the
wrapper descriptor captured the original when the type was readied), Python subclasses of
`float` (their `tp_repr` is set from that wrapper's `d_wrapped`, the original).

## Exact subset and fallbacks

- `tp_repr`: receiver of the exact type `float`, finite value. Subclass receivers reached
  through C subtypes, `inf`, `-inf`, `nan` → the original `float_repr` (+3 Ir).
- `__format__`: exact `float` receiver, exact empty `str` spec, finite value. Any other
  spec (`.2f`, `>10`, …), a `str`-subclass spec, subclass receivers (their `__str__` may
  be user code), non-finite values → the original (+3 Ir).
- Result objects are built like CPython's: a new compact ASCII `str` (`PyUnicode_New(n,
  127)` + copy = `_PyUnicode_FromASCII`; a float repr is never one character, so no
  cached singleton is involved). For `__format__('')` CPython returns the very string
  `str(x)` produced (writer read-only shortcut), so the kernel returns its fresh repr.
- The kernels hold no Python objects (interpreter-agnostic, safe on every interpreter and
  free-threaded builds) and stay valid after uninstall.
- Private state skipped: none.

## The core: `rustique_core::float_repr`

CPython prints the shortest digit string that reads back exactly (dtoa mode 0: the
closest such string, ties to even), fixed notation when the leading digit's decimal
exponent is in `-4..=15` (`.0` appended to integers), exponent notation otherwise
(`e±XX`, at least two exponent digits). The digits come from the `zmij` crate (MIT,
Schubfach/Żmij); its own layout differs from CPython's only for exponent `-5` (it prints
`0.00001234`) and single-digit exponents (`1e+7`), both rewritten in `relayout`.

Identity with CPython (all **measured**):
- `cargo test -p rustique-core`: 1.62 M values in 9 families (random bit patterns,
  subnormals, all powers of two ± 2 ulps, powers of ten ± 2 ulps, dyadic m·2^e (halfway
  cases), short decimal strings, uniform·10^k, large integers, ±2000 ulps around the
  layout cut-offs) hashed against CPython 3.15.1's `repr` (`tests/float_repr_cpython.rs`,
  generator `float_repr_cpython_gen.py`; the oracle hashes are identical on 3.14.9), the
  5,315-case CPython table of `pyfmt`, and 60,000 proptest cases against the slow exact
  `pyfmt` implementation.
- In process against `PyOS_double_to_string(x, 'r', 0, Py_DTSF_ADD_DOT_0)` — the very
  function `float_repr` calls (`benches/targets/float_selfcheck.py`): **464,304,767 values,
  0 mismatches** on 3.15.1 (200 M random bit patterns, 50 M subnormals, 100 M dyadic,
  20 M decimal strings, 100 M consecutive bit patterns across 1e16, 1e-4, 1e-5, 1e15, 1.0,
  2^53, the smallest subnormals, the smallest normal and the largest finite value);
  23.2 M values on each of 3.14.9, 3.15.1t, 3.14.9t, 0 mismatches.
- Install-time self-test: 44 probe values (both signs) through the core vs
  `PyOS_double_to_string` and the original `float.__format__('')`; any mismatch refuses
  the target.

## Guards and install

- Version gate: CPython 3.14 and 3.15 final, GIL and free-threaded, non-debug
  (`accel::version::supported`).
- Registered as `TargetSpec` `float.repr` (group `float`, installed at `enable()`).
- Original check: `PyFloat_Type.tp_repr` must equal the `d_wrapped` of the `float.__repr__`
  wrapper descriptor (i.e. CPython's own function, nobody patched it; `Prepare::slot`);
  `float.__format__` fingerprint (`ml_name`, `ml_flags`, `ml_doc`) recorded for 3.14 and
  3.15 (`c80efea6ae8f6b2c` on both).
- Install is all-or-nothing (both sites), compare-and-swap; the registry clears the JIT
  caches after the `ml_meth` swap and on `disable()` (contract §3.8). Counters: `hits`,
  `fallback.args`.
- Test/benchmark helpers: `rustique._rustique._accel._t_float` (`core_repr`,
  `core_reprs`, `_selfcheck`).

## Conformance (measured, `tests/targets/conformance.py --groups float`)

Same outcome for every test id with and without the target, on 3.15.1, 3.15.1t, 3.14.9,
3.14.9t: `test_float` (52 pass, 2 skip), `test_format` (17/2), `test_strtod` (8/1),
`test_fstring` (95 on 3.15, 94 on 3.14), `test_json` (232/1 on 3.15, 220/1 on 3.14).
The kernel was hit in each run (e.g. 1,253 times in `test_float`).

## Benchmarks

callgrind, region-instrumented, Ir per call, 3.15.1 (`benches/targets/json_cg.py --only
float`), plain → installed:

| cell | plain | installed | ratio |
|---|---:|---:|---:|
| `repr(1.23)` | 2,031 | 753 | 0.371 |
| `repr(123.456789)` | 3,248 | 791 | 0.244 |
| `repr(1.2345678901234567e+300)` | 20,550 | 719 | 0.035 |
| `str(2.5e-07)` | 2,813 | 690 | 0.245 |
| `f'{x}'` | 2,435 | 931 | 0.382 |
| `f'v={x} t={xs}ms'` | 6,855 | 2,668 | 0.389 |
| `'%s' % (x,)` | 1,962 | 684 | 0.349 |
| `f'{x:.2f}'` (fallback: spec) | 1,824 | 1,827 | 1.002 |
| `repr(inf)` (fallback: value) | 794 | 798 | 1.005 |
| `repr(F(1.5))` (subclass: not reached) | 1,914 | 1,900 | 0.993 |

## Versions

Validated: 3.14, 3.15 (GIL and free-threaded). Anything else refuses with the reason in
`status()`.
