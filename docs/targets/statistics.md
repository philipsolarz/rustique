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

Pinned module globals read on the accelerated path: `_sum`, `_ss`, `_convert`,
`_exact_ratio`, `_coerce`, `Fraction`, `Decimal`, `groupby`, `reduce`, `math`, and for
`stdev`/`pstdev` `_float_sqrt_of_frac`, `_integer_sqrt_of_frac_rto`, `_sqrt_bit_width`
(checked = 109); `math.gcd`, `math.isqrt`; the builtins `map`, `sum`, `set`, `type`,
`issubclass`. Names read only on error paths (`StatisticsError`, `_isfinite` for NaN/inf,
`_decimal_sqrt_of_frac`) are not pinned: CPython does not read them for the accepted
inputs. **Private state skipped:** none.

## 4. Version coverage

3.14.9 and 3.15.1 and their free-threaded builds (the five functions and their helpers
are identical in both).
