STATUS: wave 1, group `statistics` (workstream utils). Contract: `docs/ACCELERATION.md`.
Code: `crates/rustique-py/src/targets/statistics/`, `crates/rustique-core/src/stats/`
(`cargo test`). Tests: `tests/targets/test_statistics.py`. Benchmarks:
`benches/targets/statistics_cells.py` (callgrind), `utils_pyperf.py statistics`.

# Targets `statistics.mean`, `variance`, `pvariance`, `stdev`, `pstdev`

Vectorcall kernels on the module's function objects (captured references reach them;
introspection unchanged; tools see the original — `test_statistics.py::test_mechanics`).
Side-effect free: `checkable`.

| target | validated | free-threaded | checkable |
|---|---|---|---|
| all five | 3.14, 3.15 | yes | yes |

`fmean` (already `math.fsum` + one division), `median*` (`sorted` + indexing) and
`quantiles` are not targets: their Python glue is a small part of their cost.

## 1. Exactness

CPython computes these as exact fractions (`_sum`, `_ss`: every value through
`as_integer_ratio`, partial sums per denominator, `Fraction` arithmetic) and rounds once.
For `int`/`bool` and finite `float` data every value is `±m·2^e` with `e >= -1074`, so the
kernel adds the values exactly into fixed-scale integers (`FixedAcc`: `Σx` at `2^-1074`,
`Σx²` at `2^-2148`, 64-bit limbs; scale 0 for int-only data) and finishes with Python ints:

- `mean = Σx / n`, `variance = (nΣx² − (Σx)²) / (n(n−1))`, `pvariance = … / n²`, the
  same rationals CPython's `Fraction`s hold;
- `_convert(value, T)`: for int-only data an exact `int` when the division is exact (e.g.
  `mean([1, 2, 3]) == 2`, `variance([1, 2, 3]) == 1`), else `float(value)`; floats come from
  CPython's correctly rounded `int / int` true division — the value `Fraction.__float__`
  computes, independent of the fraction's representation (`OverflowError` → CPython runs);
- `stdev`/`pstdev`: `_float_sqrt_of_frac(n, m)` step for step on the reduced fraction
  (`math.gcd`), with `bit_length`, shifts, `math.isqrt` and the round-to-odd bit, then
  `numerator / denominator`.

The result type is CPython's (`T` from `_coerce`: `int` for int/bool data, `float` if any
float). Check mode and the differential tests compare floats bit for bit (`float.hex`).

## 2. Exact subset and fallbacks

Accelerated: `data` an exact `list` or `tuple` whose items are exact `int` (fitting 64
bits), `bool` or finite `float`; `xbar`/`mu` absent or `None`. Falls back (`args`): any
other container (iterators and generators would be consumed), Decimal/Fraction/float
subclasses (numpy scalars), NaN/inf, big ints, given `xbar`/`mu`, too few points
(`StatisticsError`), overflow. `deps`: a replaced dependency.

## 3. Dependencies

Pinned once for the whole group (`GroupSpec`, flag `STATS_DEPS`; replacing any of them
suspends all five targets until it is back), the module globals read on the accelerated
path: `_sum`, `_ss`, `_convert`,
`_exact_ratio`, `_coerce`, `Fraction`, `Decimal`, `groupby`, `reduce`, `math`, and for
`stdev`/`pstdev` `_float_sqrt_of_frac`, `_integer_sqrt_of_frac_rto`, `_sqrt_bit_width`
(checked = 109); `math.gcd`, `math.isqrt`; the builtins `map`, `sum`, `set`, `type`,
`issubclass`. Names read only on error paths (`StatisticsError`, `_isfinite` for NaN/inf,
`_decimal_sqrt_of_frac`) are not pinned: CPython does not read them for the accepted
inputs. **Private state skipped:** none.

## 4. Version coverage

3.14.9 and 3.15.1 and their free-threaded builds (the five functions and their helpers
are identical in both).

Private CPython symbols and layouts: none (public C API only).

## 5. Conformance

`python tests/conformance/run_target_suites.py --targets statistics --modules ...` (CPython's own test
modules from `$CPYTHON_SRC`, plain vs `enable(only=["statistics"])` before the tests import the
module, per test id; every target asserted `installed`), on 3.15.1, 3.15.1t, 3.14.9 and
3.14.9t: `test.test_statistics`: 400 pass — **identical outcome sets on all four builds, 0 differences**
(`benches/targets/utils_matrix.sh`, which also runs `tests/targets` and the whole
`pytest -q`). The runner normalizes object addresses in subTest ids (`0x…`), which differ
between processes (`test_urlparse` parametrizes with function reprs).

## 6. Measurements

### 6.1 Instructions per call (callgrind)

`python benches/targets/utils_cg.py statistics_cells [--jit]`, CPython 3.15.1, dev box
(method: random.md §6.1; JIT column measured for the 10-element cells only — warming the
1000-element cells 30,000 times under valgrind is impractical). **M**.

| cell | orig Ir | rq Ir | ratio | JIT orig | JIT rq | JIT ratio |
|---|---:|---:|---:|---:|---:|---:|
| `mean(10 floats)` | 125823 | 13757 | 0.109 | 116761 | 13758 | 0.118 |
| `mean(1000 floats)` | 3791191 | 132034 | 0.035 | | | |
| `mean(10 ints)` | 52166 | 7132 | 0.137 | 48940 | 7133 | 0.146 |
| `mean(1000 ints)` | 1663068 | 127067 | 0.076 | | | |
| `variance(10 floats)` | 296260 | 43053 | 0.145 | 276879 | 43019 | 0.155 |
| `pvariance(1000 floats)` | 5901804 | 239851 | 0.041 | | | |
| `stdev(10 floats)` | 303969 | 81337 | 0.268 | 284008 | 81251 | 0.286 |
| `stdev(1000 floats)` | 5909146 | 280611 | 0.047 | | | |
| `pstdev(10 ints)` | 124757 | 23390 | 0.187 | | | |
| `variance(10 ints)` | 117624 | 15079 | 0.128 | | | |
| fallbacks (steady state): | | | | | | |
| `mean(10 Decimals)` | 175825 | 176210 | 1.002 | | | |
| `stdev(10 Decimals)` | 437915 | 438824 | 1.002 | | | |
| `mean(iterator)` | 125992 | 126349 | 1.003 | | | |
| `variance(floats, xbar)` | 187697 | 188212 | 1.003 | | | |

Every original costs ≥ 49k Ir, far above the vectorcall fallback threshold: the group is
`jit_safe`.

### Wall time, large inputs (pyperf)

`python benches/targets/utils_pyperf.py statistics --fast` (busy shared box: only ≥ 1.5×
claims). **M**.

| cell | plain | rustique | ratio |
|---|---:|---:|---:|
| `mean(10k floats)` | 3521.8 µs | 77.3 µs | 0.022 |
| `mean(10k ints)` | 985.6 µs | 69.3 µs | 0.070 |
| `variance(10k floats)` | 4929.4 µs | 117.4 µs | 0.024 |
| `stdev(10k floats)` | 4961.7 µs | 121.2 µs | 0.024 |
| `pstdev(10k ints)` | 1500.7 µs | 100.9 µs | 0.067 |

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
| `import statistics` plain | 13.53M Ir | 13.59M Ir | |
| lazy install overhead | +139k (+1.0%) | **+106k (+0.8%)** | 680k: within |

Before this pass: +443k cold, +224k warm.

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

`python benches/targets/utils_first64.py` (Ir per call over 64 calls, fresh process):
`statistics.mean([Decimal]*2)`: 48,831 → 49,274 Ir/call (1.009, +443). After 64 consecutive fallbacks the target is suspended and the cells above
measure 1.000 (steady state).
