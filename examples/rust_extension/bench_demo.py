"""C-API paths versus PyO3's element-wise conversions (best of N, wall time).

Build both extensions optimised first:
    maturin develop --uv --profile dev-release
    maturin develop --uv --release -m examples/rust_extension/Cargo.toml
    python examples/rust_extension/bench_demo.py
"""

import sys
import time

import rustique_demo as demo
from rustique import List

N = 10_000_000
REPEAT = 7


def best(f, *args):
    times = []
    for _ in range(REPEAT):
        start = time.perf_counter()
        result = f(*args)
        times.append(time.perf_counter() - start)
        del result
    return min(times)


def row(label, seconds, base=None):
    ratio = f"  ({seconds / base:5.2f}x)" if base else ""
    print(f"  {label:<52} {seconds * 1e3:9.2f} ms{ratio}")


def main():
    print(f"Python {sys.version.split()[0]}{' (free-threaded)' if not getattr(sys, '_is_gil_enabled', lambda: True)() else ''}")
    count = len(demo.primes(N))
    assert count == len(demo.primes_pylist(N)) == demo.count_primes(N) == 664_579
    print(f"primes below {N:,} ({count:,} values)")
    sieve = best(demo.count_primes, N)
    pylist = best(demo.primes_pylist, N)
    row("sieve only (count_primes)", sieve)
    row("-> builtin list via PyO3 Vec<i64> (primes_pylist)", pylist, pylist)
    row("-> rustique.List[int] via list_from_slice (primes)", best(demo.primes, N), pylist)
    print(f"  conversion alone: PyO3 {(pylist - sieve) * 1e3:.2f} ms vs C API "
          f"{(best(demo.primes, N) - sieve) * 1e3:.2f} ms")

    print(f"sum of {N:,} floats")
    floats = [i * 0.5 for i in range(N)]
    rs_floats = List[float](floats)
    expected = sum(floats)
    assert demo.fsum_borrowed(rs_floats) == demo.fsum_extracted(floats) == expected
    extracted = best(demo.fsum_extracted, floats)
    row("PyO3 Vec<f64> extraction from a builtin list", extracted, extracted)
    row("PyO3 Vec<f64> extraction from a rustique List", best(demo.fsum_extracted, rs_floats), extracted)
    row("borrow_list::<f64> + sum (zero-copy)", best(demo.fsum_borrowed, rs_floats), extracted)
    row("rustique List.sum() (for reference)", best(rs_floats.sum), extracted)


if __name__ == "__main__":
    main()
