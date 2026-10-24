# Rustique benchmarks

Produced by `benches/bench.py --sizes 1e5,1e6 --repeat 7` against a `maturin develop
--release` build (fat LTO, one codegen unit) on an otherwise idle 8-CPU WSL2 machine
(Ryzen 5600X). Ratios are rustique time ÷ reference time (the `(ref)` row, usually the
builtin); lower is faster. Every measurement rebuilds its inputs and checks its result
against the reference result. Absolute times on this VM are noisy; compare ratios.
Reproduce with:

```bash
.venv/bin/maturin develop --uv --release
.venv/bin/python benches/bench.py --sizes 1e5,1e6 --repeat 7
.venv/bin/python benches/list_hotpaths.py        # per-element hot paths, incl. 3.15t
```

`3.15.1` GIL=on, rustique 0.2.0, numpy 2.5.4, 8 CPUs, git a6be77a (code; docs-only commits since); best of 7, fresh inputs per repetition, results verified.

  construct: from list[int] n=100,000: list=74.6 µs, List=229.4 µs, List[int]=248.6 µs, array('q')=1.16 ms, numpy=1.63 ms
  construct: from list[int] n=1,000,000: list=2.80 ms, List=2.59 ms, List[int]=2.80 ms, array('q')=11.61 ms, numpy=16.35 ms
  construct: from range n=100,000: list=625.7 µs, List=62.7 µs, arange=60.7 µs, array('q')=2.82 ms, numpy=50.0 µs
  construct: from range n=1,000,000: list=11.96 ms, List=656.8 µs, arange=658.2 µs, array('q')=28.59 ms, numpy=649.9 µs
  construct: from generator n=100,000: list=1.39 ms, List=1.78 ms, List[int]=2.96 ms, array('q')=3.52 ms
  construct: from generator n=1,000,000: list=14.88 ms, List=20.11 ms, List[int]=29.76 ms, array('q')=35.21 ms
  construct: from list[float] n=100,000: list=81.7 µs, List=97.4 µs, List[float]=104.0 µs, numpy=1.55 ms
  construct: from list[float] n=1,000,000: list=2.95 ms, List=2.23 ms, List[float]=2.34 ms, numpy=15.60 ms
  construct: from list[str] n=100,000: list=110.2 µs, List=1.03 ms, List[str]=1.01 ms
  construct: from list[str] n=1,000,000: list=5.28 ms, List=10.41 ms, List[str]=10.44 ms
  index: x[i] (100k random) n=100,000: list=5.03 ms, List=5.29 ms, array('q')=4.85 ms, numpy=6.44 ms
  index: x[i] (100k random) n=1,000,000: list=18.73 ms, List=11.01 ms, array('q')=11.14 ms, numpy=11.92 ms
  iterate: for x in c n=100,000: list=2.44 ms, List=3.75 ms, array('q')=3.08 ms, numpy=4.40 ms
  iterate: for x in c n=1,000,000: list=25.07 ms, List=38.02 ms, array('q')=30.77 ms, numpy=44.10 ms
  append: n x c.append(v) n=100,000: list=986.9 µs, List=3.43 ms, List[int]=4.04 ms, array('q')=3.39 ms
  append: n x c.append(v) n=1,000,000: list=9.58 ms, List=34.45 ms, List[int]=40.95 ms, array('q')=34.09 ms
  contains: v in c n=100,000: list=49.03 ms, List=1.14 ms, array('q')=101.06 ms
  contains: v in c n=1,000,000: list=49.98 ms, List=2.13 ms, array('q')=101.49 ms
  sum: int n=100,000: list=344.7 µs, List.sum()=18.6 µs, sum(List)=1.33 ms, array('q')=747.4 µs, numpy.sum=31.2 µs
  sum: int n=1,000,000: list=3.55 ms, List.sum()=417.5 µs, sum(List)=14.88 ms, array('q')=7.51 ms, numpy.sum=452.4 µs
  min: int n=100,000: list=849.3 µs, List.min()=23.2 µs, min(List)=1.86 ms, array('q')=1.28 ms, numpy.min=33.0 µs
  min: int n=1,000,000: list=8.63 ms, List.min()=400.7 µs, min(List)=20.17 ms, array('q')=12.91 ms, numpy.min=441.6 µs
  max: int n=100,000: list=905.3 µs, List.max()=24.7 µs, max(List)=1.91 ms, array('q')=1.33 ms, numpy.max=32.2 µs
  max: int n=1,000,000: list=9.13 ms, List.max()=403.0 µs, max(List)=20.06 ms, array('q')=13.31 ms, numpy.max=448.4 µs
  sum: float (compensated) n=100,000: list=402.1 µs, List.sum()=107.3 µs
  sum: float (compensated) n=1,000,000: list=4.11 ms, List.sum()=1.05 ms
  sort: int (in place) n=100,000: list=10.56 ms, List=539.9 µs, List[int]=561.9 µs, numpy=769.7 µs
  sort: int (in place) n=1,000,000: list=173.82 ms, List=4.42 ms, List[int]=4.22 ms, numpy=9.00 ms
  sort: float (in place) n=100,000: list=10.20 ms, List=636.9 µs, numpy=678.6 µs
  sort: float (in place) n=1,000,000: list=163.93 ms, List=5.59 ms, numpy=7.70 ms
  sort: str (in place) n=100,000: list=15.74 ms, List=2.04 ms
  sort: str (in place) n=1,000,000: list=290.71 ms, List=20.48 ms
  slice: c[n//4 : 3n//4] n=100,000: list=44.1 µs, List=23.6 µs, array('q')=19.1 µs, numpy (copy)=28.0 µs
  slice: c[n//4 : 3n//4] n=1,000,000: list=904.4 µs, List=437.2 µs, array('q')=388.5 µs, numpy (copy)=423.0 µs
  slice: c[::3] n=100,000: list=51.0 µs, List=24.5 µs, numpy (copy)=33.8 µs
  slice: c[::3] n=1,000,000: list=2.18 ms, List=548.3 µs, numpy (copy)=509.1 µs
  map: x*2+1 n=100,000: list comp=4.38 ms, List.map(X*2+1)=256.3 µs, List.map(lambda)=9.08 ms, numpy=177.7 µs
  map: x*2+1 n=1,000,000: list comp=41.13 ms, List.map(X*2+1)=2.42 ms, List.map(lambda)=101.07 ms, numpy=1.81 ms
  filter: x%3==0 n=100,000: list comp=2.41 ms, List.filter(X%3==0)=663.0 µs, List.filter(lambda)=6.48 ms, numpy=814.4 µs
  filter: x%3==0 n=1,000,000: list comp=27.17 ms, List.filter(X%3==0)=2.83 ms, List.filter(lambda)=68.29 ms, numpy=7.92 ms
  map: float x*0.5-1 n=100,000: list comp=3.39 ms, List.map(X*0.5-1)=183.2 µs, numpy=170.9 µs
  map: float x*0.5-1 n=1,000,000: list comp=32.15 ms, List.map(X*0.5-1)=2.38 ms, numpy=1.88 ms
  dict build: int keys n=100,000: dict=9.90 ms, Dict=13.92 ms
  dict build: int keys n=1,000,000: dict=122.52 ms, Dict=267.50 ms
  dict lookup: int keys (100k) n=100,000: dict=13.70 ms, Dict=16.51 ms
  dict lookup: int keys (100k) n=1,000,000: dict=16.57 ms, Dict=22.23 ms
  dict build: str keys n=100,000: dict=5.22 ms, Dict=9.27 ms
  dict build: str keys n=1,000,000: dict=125.48 ms, Dict=246.57 ms
  dict lookup: str keys (100k) n=100,000: dict=6.69 ms, Dict=9.33 ms
  dict lookup: str keys (100k) n=1,000,000: dict=13.71 ms, Dict=23.93 ms
  set | n=100,000: set=1.94 ms, Set=2.42 ms
  set | n=1,000,000: set=110.21 ms, Set=92.50 ms
  set & n=100,000: set=1.71 ms, Set=2.26 ms
  set & n=1,000,000: set=60.00 ms, Set=95.50 ms
  set - n=100,000: set=1.96 ms, Set=2.59 ms
  set - n=1,000,000: set=62.88 ms, Set=102.71 ms
  set ^ n=100,000: set=2.07 ms, Set=4.96 ms
  set ^ n=1,000,000: set=115.59 ms, Set=179.63 ms
  set build: int n=100,000: set=2.68 ms, Set=4.63 ms
  set build: int n=1,000,000: set=74.58 ms, Set=180.07 ms
| case | n | implementation | best | median | ratio vs builtin |
|---|---:|---|---:|---:|---:|
| construct: from list[int] | 100,000 | list | 74.6 µs | 87.2 µs | 1.00 (ref) |
| construct: from list[int] | 100,000 | List | 229.4 µs | 237.7 µs | 3.07 |
| construct: from list[int] | 100,000 | List[int] | 248.6 µs | 258.3 µs | 3.33 |
| construct: from list[int] | 100,000 | array('q') | 1.16 ms | 1.19 ms | 15.56 |
| construct: from list[int] | 100,000 | numpy | 1.63 ms | 1.64 ms | 21.81 |
| construct: from list[int] | 1,000,000 | list | 2.80 ms | 3.08 ms | 1.00 (ref) |
| construct: from list[int] | 1,000,000 | List | 2.59 ms | 2.71 ms | 0.93 |
| construct: from list[int] | 1,000,000 | List[int] | 2.80 ms | 2.84 ms | 1.00 |
| construct: from list[int] | 1,000,000 | array('q') | 11.61 ms | 11.73 ms | 4.15 |
| construct: from list[int] | 1,000,000 | numpy | 16.35 ms | 16.46 ms | 5.84 |
| construct: from range | 100,000 | list | 625.7 µs | 666.5 µs | 1.00 (ref) |
| construct: from range | 100,000 | List | 62.7 µs | 66.8 µs | 0.10 |
| construct: from range | 100,000 | arange | 60.7 µs | 63.1 µs | 0.10 |
| construct: from range | 100,000 | array('q') | 2.82 ms | 2.87 ms | 4.50 |
| construct: from range | 100,000 | numpy | 50.0 µs | 58.4 µs | 0.08 |
| construct: from range | 1,000,000 | list | 11.96 ms | 15.01 ms | 1.00 (ref) |
| construct: from range | 1,000,000 | List | 656.8 µs | 677.4 µs | 0.05 |
| construct: from range | 1,000,000 | arange | 658.2 µs | 679.9 µs | 0.06 |
| construct: from range | 1,000,000 | array('q') | 28.59 ms | 29.25 ms | 2.39 |
| construct: from range | 1,000,000 | numpy | 649.9 µs | 684.7 µs | 0.05 |
| construct: from generator | 100,000 | list | 1.39 ms | 1.44 ms | 1.00 (ref) |
| construct: from generator | 100,000 | List | 1.78 ms | 1.82 ms | 1.28 |
| construct: from generator | 100,000 | List[int] | 2.96 ms | 2.97 ms | 2.12 |
| construct: from generator | 100,000 | array('q') | 3.52 ms | 3.57 ms | 2.52 |
| construct: from generator | 1,000,000 | list | 14.88 ms | 15.33 ms | 1.00 (ref) |
| construct: from generator | 1,000,000 | List | 20.11 ms | 22.18 ms | 1.35 |
| construct: from generator | 1,000,000 | List[int] | 29.76 ms | 30.05 ms | 2.00 |
| construct: from generator | 1,000,000 | array('q') | 35.21 ms | 35.40 ms | 2.37 |
| construct: from list[float] | 100,000 | list | 81.7 µs | 94.2 µs | 1.00 (ref) |
| construct: from list[float] | 100,000 | List | 97.4 µs | 110.9 µs | 1.19 |
| construct: from list[float] | 100,000 | List[float] | 104.0 µs | 108.4 µs | 1.27 |
| construct: from list[float] | 100,000 | numpy | 1.55 ms | 1.56 ms | 18.99 |
| construct: from list[float] | 1,000,000 | list | 2.95 ms | 3.03 ms | 1.00 (ref) |
| construct: from list[float] | 1,000,000 | List | 2.23 ms | 2.38 ms | 0.75 |
| construct: from list[float] | 1,000,000 | List[float] | 2.34 ms | 2.39 ms | 0.79 |
| construct: from list[float] | 1,000,000 | numpy | 15.60 ms | 16.00 ms | 5.29 |
| construct: from list[str] | 100,000 | list | 110.2 µs | 120.0 µs | 1.00 (ref) |
| construct: from list[str] | 100,000 | List | 1.03 ms | 1.05 ms | 9.36 |
| construct: from list[str] | 100,000 | List[str] | 1.01 ms | 1.02 ms | 9.13 |
| construct: from list[str] | 1,000,000 | list | 5.28 ms | 5.71 ms | 1.00 (ref) |
| construct: from list[str] | 1,000,000 | List | 10.41 ms | 10.58 ms | 1.97 |
| construct: from list[str] | 1,000,000 | List[str] | 10.44 ms | 20.52 ms | 1.98 |
| index: x[i] (100k random) | 100,000 | list | 5.03 ms | 5.40 ms | 1.00 (ref) |
| index: x[i] (100k random) | 100,000 | List | 5.29 ms | 5.67 ms | 1.05 |
| index: x[i] (100k random) | 100,000 | array('q') | 4.85 ms | 4.93 ms | 0.96 |
| index: x[i] (100k random) | 100,000 | numpy | 6.44 ms | 6.56 ms | 1.28 |
| index: x[i] (100k random) | 1,000,000 | list | 18.73 ms | 21.47 ms | 1.00 (ref) |
| index: x[i] (100k random) | 1,000,000 | List | 11.01 ms | 12.62 ms | 0.59 |
| index: x[i] (100k random) | 1,000,000 | array('q') | 11.14 ms | 11.95 ms | 0.59 |
| index: x[i] (100k random) | 1,000,000 | numpy | 11.92 ms | 13.48 ms | 0.64 |
| iterate: for x in c | 100,000 | list | 2.44 ms | 2.53 ms | 1.00 (ref) |
| iterate: for x in c | 100,000 | List | 3.75 ms | 3.88 ms | 1.54 |
| iterate: for x in c | 100,000 | array('q') | 3.08 ms | 3.22 ms | 1.26 |
| iterate: for x in c | 100,000 | numpy | 4.40 ms | 4.48 ms | 1.80 |
| iterate: for x in c | 1,000,000 | list | 25.07 ms | 25.41 ms | 1.00 (ref) |
| iterate: for x in c | 1,000,000 | List | 38.02 ms | 39.99 ms | 1.52 |
| iterate: for x in c | 1,000,000 | array('q') | 30.77 ms | 31.27 ms | 1.23 |
| iterate: for x in c | 1,000,000 | numpy | 44.10 ms | 45.61 ms | 1.76 |
| append: n x c.append(v) | 100,000 | list | 986.9 µs | 1.53 ms | 1.00 (ref) |
| append: n x c.append(v) | 100,000 | List | 3.43 ms | 3.48 ms | 3.47 |
| append: n x c.append(v) | 100,000 | List[int] | 4.04 ms | 4.11 ms | 4.09 |
| append: n x c.append(v) | 100,000 | array('q') | 3.39 ms | 3.48 ms | 3.43 |
| append: n x c.append(v) | 1,000,000 | list | 9.58 ms | 15.25 ms | 1.00 (ref) |
| append: n x c.append(v) | 1,000,000 | List | 34.45 ms | 35.09 ms | 3.60 |
| append: n x c.append(v) | 1,000,000 | List[int] | 40.95 ms | 41.22 ms | 4.27 |
| append: n x c.append(v) | 1,000,000 | array('q') | 34.09 ms | 34.47 ms | 3.56 |
| contains: v in c | 100,000 | list | 49.03 ms | 49.99 ms | 1.00 (ref) |
| contains: v in c | 100,000 | List | 1.14 ms | 1.16 ms | 0.02 |
| contains: v in c | 100,000 | array('q') | 101.06 ms | 101.80 ms | 2.06 |
| contains: v in c | 1,000,000 | list | 49.98 ms | 52.54 ms | 1.00 (ref) |
| contains: v in c | 1,000,000 | List | 2.13 ms | 2.29 ms | 0.04 |
| contains: v in c | 1,000,000 | array('q') | 101.49 ms | 102.31 ms | 2.03 |
| sum: int | 100,000 | list | 344.7 µs | 351.1 µs | 1.00 (ref) |
| sum: int | 100,000 | List.sum() | 18.6 µs | 23.3 µs | 0.05 |
| sum: int | 100,000 | sum(List) | 1.33 ms | 1.34 ms | 3.86 |
| sum: int | 100,000 | array('q') | 747.4 µs | 754.5 µs | 2.17 |
| sum: int | 100,000 | numpy.sum | 31.2 µs | 38.2 µs | 0.09 |
| sum: int | 1,000,000 | list | 3.55 ms | 3.79 ms | 1.00 (ref) |
| sum: int | 1,000,000 | List.sum() | 417.5 µs | 469.1 µs | 0.12 |
| sum: int | 1,000,000 | sum(List) | 14.88 ms | 15.79 ms | 4.19 |
| sum: int | 1,000,000 | array('q') | 7.51 ms | 7.56 ms | 2.11 |
| sum: int | 1,000,000 | numpy.sum | 452.4 µs | 466.3 µs | 0.13 |
| min: int | 100,000 | list | 849.3 µs | 862.8 µs | 1.00 (ref) |
| min: int | 100,000 | List.min() | 23.2 µs | 24.4 µs | 0.03 |
| min: int | 100,000 | min(List) | 1.86 ms | 1.91 ms | 2.19 |
| min: int | 100,000 | array('q') | 1.28 ms | 1.30 ms | 1.51 |
| min: int | 100,000 | numpy.min | 33.0 µs | 46.8 µs | 0.04 |
| min: int | 1,000,000 | list | 8.63 ms | 8.89 ms | 1.00 (ref) |
| min: int | 1,000,000 | List.min() | 400.7 µs | 483.5 µs | 0.05 |
| min: int | 1,000,000 | min(List) | 20.17 ms | 20.82 ms | 2.34 |
| min: int | 1,000,000 | array('q') | 12.91 ms | 13.02 ms | 1.49 |
| min: int | 1,000,000 | numpy.min | 441.6 µs | 474.7 µs | 0.05 |
| max: int | 100,000 | list | 905.3 µs | 913.0 µs | 1.00 (ref) |
| max: int | 100,000 | List.max() | 24.7 µs | 32.9 µs | 0.03 |
| max: int | 100,000 | max(List) | 1.91 ms | 1.98 ms | 2.11 |
| max: int | 100,000 | array('q') | 1.33 ms | 1.34 ms | 1.47 |
| max: int | 100,000 | numpy.max | 32.2 µs | 42.7 µs | 0.04 |
| max: int | 1,000,000 | list | 9.13 ms | 9.42 ms | 1.00 (ref) |
| max: int | 1,000,000 | List.max() | 403.0 µs | 504.7 µs | 0.04 |
| max: int | 1,000,000 | max(List) | 20.06 ms | 20.97 ms | 2.20 |
| max: int | 1,000,000 | array('q') | 13.31 ms | 13.39 ms | 1.46 |
| max: int | 1,000,000 | numpy.max | 448.4 µs | 467.1 µs | 0.05 |
| sum: float (compensated) | 100,000 | list | 402.1 µs | 418.6 µs | 1.00 (ref) |
| sum: float (compensated) | 100,000 | List.sum() | 107.3 µs | 112.3 µs | 0.27 |
| sum: float (compensated) | 1,000,000 | list | 4.11 ms | 4.37 ms | 1.00 (ref) |
| sum: float (compensated) | 1,000,000 | List.sum() | 1.05 ms | 1.06 ms | 0.26 |
| sort: int (in place) | 100,000 | list | 10.56 ms | 10.73 ms | 1.00 (ref) |
| sort: int (in place) | 100,000 | List | 539.9 µs | 622.9 µs | 0.05 |
| sort: int (in place) | 100,000 | List[int] | 561.9 µs | 672.2 µs | 0.05 |
| sort: int (in place) | 100,000 | numpy | 769.7 µs | 780.6 µs | 0.07 |
| sort: int (in place) | 1,000,000 | list | 173.82 ms | 181.53 ms | 1.00 (ref) |
| sort: int (in place) | 1,000,000 | List | 4.42 ms | 4.70 ms | 0.03 |
| sort: int (in place) | 1,000,000 | List[int] | 4.22 ms | 4.51 ms | 0.02 |
| sort: int (in place) | 1,000,000 | numpy | 9.00 ms | 9.15 ms | 0.05 |
| sort: float (in place) | 100,000 | list | 10.20 ms | 10.28 ms | 1.00 (ref) |
| sort: float (in place) | 100,000 | List | 636.9 µs | 801.1 µs | 0.06 |
| sort: float (in place) | 100,000 | numpy | 678.6 µs | 707.3 µs | 0.07 |
| sort: float (in place) | 1,000,000 | list | 163.93 ms | 171.23 ms | 1.00 (ref) |
| sort: float (in place) | 1,000,000 | List | 5.59 ms | 5.92 ms | 0.03 |
| sort: float (in place) | 1,000,000 | numpy | 7.70 ms | 7.92 ms | 0.05 |
| sort: str (in place) | 100,000 | list | 15.74 ms | 15.86 ms | 1.00 (ref) |
| sort: str (in place) | 100,000 | List | 2.04 ms | 2.10 ms | 0.13 |
| sort: str (in place) | 1,000,000 | list | 290.71 ms | 325.89 ms | 1.00 (ref) |
| sort: str (in place) | 1,000,000 | List | 20.48 ms | 20.86 ms | 0.07 |
| slice: c[n//4 : 3n//4] | 100,000 | list | 44.1 µs | 47.5 µs | 1.00 (ref) |
| slice: c[n//4 : 3n//4] | 100,000 | List | 23.6 µs | 29.0 µs | 0.53 |
| slice: c[n//4 : 3n//4] | 100,000 | array('q') | 19.1 µs | 26.5 µs | 0.43 |
| slice: c[n//4 : 3n//4] | 100,000 | numpy (copy) | 28.0 µs | 40.2 µs | 0.64 |
| slice: c[n//4 : 3n//4] | 1,000,000 | list | 904.4 µs | 1.20 ms | 1.00 (ref) |
| slice: c[n//4 : 3n//4] | 1,000,000 | List | 437.2 µs | 523.2 µs | 0.48 |
| slice: c[n//4 : 3n//4] | 1,000,000 | array('q') | 388.5 µs | 464.6 µs | 0.43 |
| slice: c[n//4 : 3n//4] | 1,000,000 | numpy (copy) | 423.0 µs | 499.5 µs | 0.47 |
| slice: c[::3] | 100,000 | list | 51.0 µs | 57.2 µs | 1.00 (ref) |
| slice: c[::3] | 100,000 | List | 24.5 µs | 36.3 µs | 0.48 |
| slice: c[::3] | 100,000 | numpy (copy) | 33.8 µs | 43.4 µs | 0.66 |
| slice: c[::3] | 1,000,000 | list | 2.18 ms | 2.47 ms | 1.00 (ref) |
| slice: c[::3] | 1,000,000 | List | 548.3 µs | 777.7 µs | 0.25 |
| slice: c[::3] | 1,000,000 | numpy (copy) | 509.1 µs | 538.3 µs | 0.23 |
| map: x*2+1 | 100,000 | list comp | 4.38 ms | 4.77 ms | 1.00 (ref) |
| map: x*2+1 | 100,000 | List.map(X*2+1) | 256.3 µs | 271.1 µs | 0.06 |
| map: x*2+1 | 100,000 | List.map(lambda) | 9.08 ms | 9.75 ms | 2.08 |
| map: x*2+1 | 100,000 | numpy | 177.7 µs | 184.4 µs | 0.04 |
| map: x*2+1 | 1,000,000 | list comp | 41.13 ms | 51.87 ms | 1.00 (ref) |
| map: x*2+1 | 1,000,000 | List.map(X*2+1) | 2.42 ms | 2.49 ms | 0.06 |
| map: x*2+1 | 1,000,000 | List.map(lambda) | 101.07 ms | 107.58 ms | 2.46 |
| map: x*2+1 | 1,000,000 | numpy | 1.81 ms | 1.94 ms | 0.04 |
| filter: x%3==0 | 100,000 | list comp | 2.41 ms | 2.46 ms | 1.00 (ref) |
| filter: x%3==0 | 100,000 | List.filter(X%3==0) | 663.0 µs | 673.1 µs | 0.27 |
| filter: x%3==0 | 100,000 | List.filter(lambda) | 6.48 ms | 6.87 ms | 2.68 |
| filter: x%3==0 | 100,000 | numpy | 814.4 µs | 827.0 µs | 0.34 |
| filter: x%3==0 | 1,000,000 | list comp | 27.17 ms | 28.71 ms | 1.00 (ref) |
| filter: x%3==0 | 1,000,000 | List.filter(X%3==0) | 2.83 ms | 3.03 ms | 0.10 |
| filter: x%3==0 | 1,000,000 | List.filter(lambda) | 68.29 ms | 75.64 ms | 2.51 |
| filter: x%3==0 | 1,000,000 | numpy | 7.92 ms | 8.23 ms | 0.29 |
| map: float x*0.5-1 | 100,000 | list comp | 3.39 ms | 4.27 ms | 1.00 (ref) |
| map: float x*0.5-1 | 100,000 | List.map(X*0.5-1) | 183.2 µs | 188.1 µs | 0.05 |
| map: float x*0.5-1 | 100,000 | numpy | 170.9 µs | 179.3 µs | 0.05 |
| map: float x*0.5-1 | 1,000,000 | list comp | 32.15 ms | 40.82 ms | 1.00 (ref) |
| map: float x*0.5-1 | 1,000,000 | List.map(X*0.5-1) | 2.38 ms | 2.50 ms | 0.07 |
| map: float x*0.5-1 | 1,000,000 | numpy | 1.88 ms | 1.99 ms | 0.06 |
| dict build: int keys | 100,000 | dict | 9.90 ms | 11.17 ms | 1.00 (ref) |
| dict build: int keys | 100,000 | Dict | 13.92 ms | 14.78 ms | 1.41 |
| dict build: int keys | 1,000,000 | dict | 122.52 ms | 136.42 ms | 1.00 (ref) |
| dict build: int keys | 1,000,000 | Dict | 267.50 ms | 290.79 ms | 2.18 |
| dict lookup: int keys (100k) | 100,000 | dict | 13.70 ms | 14.79 ms | 1.00 (ref) |
| dict lookup: int keys (100k) | 100,000 | Dict | 16.51 ms | 16.96 ms | 1.21 |
| dict lookup: int keys (100k) | 1,000,000 | dict | 16.57 ms | 21.17 ms | 1.00 (ref) |
| dict lookup: int keys (100k) | 1,000,000 | Dict | 22.23 ms | 26.55 ms | 1.34 |
| dict build: str keys | 100,000 | dict | 5.22 ms | 6.10 ms | 1.00 (ref) |
| dict build: str keys | 100,000 | Dict | 9.27 ms | 11.45 ms | 1.78 |
| dict build: str keys | 1,000,000 | dict | 125.48 ms | 140.37 ms | 1.00 (ref) |
| dict build: str keys | 1,000,000 | Dict | 246.57 ms | 277.35 ms | 1.97 |
| dict lookup: str keys (100k) | 100,000 | dict | 6.69 ms | 6.93 ms | 1.00 (ref) |
| dict lookup: str keys (100k) | 100,000 | Dict | 9.33 ms | 9.47 ms | 1.39 |
| dict lookup: str keys (100k) | 1,000,000 | dict | 13.71 ms | 16.00 ms | 1.00 (ref) |
| dict lookup: str keys (100k) | 1,000,000 | Dict | 23.93 ms | 24.86 ms | 1.75 |
| set \| | 100,000 | set | 1.94 ms | 3.49 ms | 1.00 (ref) |
| set \| | 100,000 | Set | 2.42 ms | 3.89 ms | 1.25 |
| set \| | 1,000,000 | set | 110.21 ms | 128.00 ms | 1.00 (ref) |
| set \| | 1,000,000 | Set | 92.50 ms | 93.74 ms | 0.84 |
| set & | 100,000 | set | 1.71 ms | 2.02 ms | 1.00 (ref) |
| set & | 100,000 | Set | 2.26 ms | 2.88 ms | 1.32 |
| set & | 1,000,000 | set | 60.00 ms | 61.08 ms | 1.00 (ref) |
| set & | 1,000,000 | Set | 95.50 ms | 101.47 ms | 1.59 |
| set - | 100,000 | set | 1.96 ms | 2.32 ms | 1.00 (ref) |
| set - | 100,000 | Set | 2.59 ms | 2.69 ms | 1.32 |
| set - | 1,000,000 | set | 62.88 ms | 65.95 ms | 1.00 (ref) |
| set - | 1,000,000 | Set | 102.71 ms | 112.80 ms | 1.63 |
| set ^ | 100,000 | set | 2.07 ms | 2.64 ms | 1.00 (ref) |
| set ^ | 100,000 | Set | 4.96 ms | 5.49 ms | 2.39 |
| set ^ | 1,000,000 | set | 115.59 ms | 117.77 ms | 1.00 (ref) |
| set ^ | 1,000,000 | Set | 179.63 ms | 188.22 ms | 1.55 |
| set build: int | 100,000 | set | 2.68 ms | 2.85 ms | 1.00 (ref) |
| set build: int | 100,000 | Set | 4.63 ms | 5.26 ms | 1.73 |
| set build: int | 1,000,000 | set | 74.58 ms | 76.67 ms | 1.00 (ref) |
| set build: int | 1,000,000 | Set | 180.07 ms | 207.69 ms | 2.41 |

Notes:
- sum: float (compensated): Python's float sum is compensated (Neumaier); List.sum() must return the identical value. numpy.sum (pairwise) and math.fsum (exact) differ in the last bits, so they are not listed.
