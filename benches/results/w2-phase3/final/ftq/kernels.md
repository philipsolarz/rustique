build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| json.dumps | local | 1850 | 0.220 | 1.41 | 1.00 | 0.155 | 0.70 | 1.00 | 0.154 |
| json.dumps | shared | 1917 | 0.212 | 3.58 | 11.09 | 0.658 | 0.14 | 0.08 | 0.359 |
| json.loads | local | 1352 | 0.425 | 1.56 | 0.97 | 0.263 | 0.64 | 1.00 | 0.271 |
| json.loads | shared | 1412 | 0.401 | 1.49 | 0.97 | 0.261 | 0.66 | 0.98 | 0.268 |
| copy.deepcopy | local | 3317 | 0.232 | 1.34 | 1.67 | 0.290 | 0.73 | 0.58 | 0.294 |
| copy.deepcopy | shared | 3497 | 0.235 | 1.66 | 6.58 | 0.931 | 0.59 | 0.14 | 0.996 |
| deepcopy(frozenset) | local | 38481 | 0.021 | 1.02 | 1.24 | 0.026 | 0.87 | 0.79 | 0.023 |
| deepcopy(frozenset) | shared | 38682 | 0.021 | 1.25 | 5.66 | 0.097 | 0.54 | 0.17 | 0.070 |
| dataclasses.asdict | local | 1170 | 0.426 | 1.34 | 1.37 | 0.437 | 0.71 | 0.64 | 0.475 |
| dataclasses.asdict | shared | 1248 | 0.439 | 4.18 | 7.83 | 0.823 | 0.24 | 0.12 | 0.875 |
| datetime.strptime | local | 3420 | 0.056 | 3.08 | 1.38 | 0.025 | 0.13 | 0.71 | 0.011 |
| datetime.strptime | shared | 3451 | 0.056 | 3.32 | 1.34 | 0.022 | 0.15 | 0.74 | 0.011 |
| datetime.isoformat | local | 387 | 0.154 | 1.01 | 1.01 | 0.154 | 0.98 | 0.97 | 0.155 |
| datetime.isoformat | shared | 402 | 0.157 | 1.51 | 9.06 | 0.941 | 0.65 | 0.11 | 0.939 |
| date.strftime | local | 1323 | 0.083 | 6.70 | 1.95 | 0.024 | 0.15 | 0.51 | 0.024 |
| date.strftime | shared | 1355 | 0.086 | 7.55 | 10.18 | 0.116 | 0.13 | 0.10 | 0.118 |
| posixpath.join | local | 469 | 0.193 | 0.99 | 1.00 | 0.195 | 0.99 | 0.98 | 0.195 |
| posixpath.join | shared | 468 | 0.192 | 0.99 | 1.00 | 0.194 | 0.99 | 0.97 | 0.197 |
| posixpath.join_fb | local | 284 | 1.008 | 1.60 | 1.45 | 0.911 | 0.62 | 0.69 | 0.912 |
| posixpath.join_fb | shared | 296 | 0.986 | 6.89 | 8.21 | 1.175 | 0.14 | 0.11 | 1.215 |
| fnmatch.fnmatch | local | 291 | 0.141 | 8.21 | 0.99 | 0.017 | 0.12 | 1.00 | 0.017 |
| fnmatch.fnmatch | shared | 323 | 0.126 | 7.42 | 1.00 | 0.017 | 0.13 | 0.96 | 0.017 |
| pathlib.join | local | 844 | 0.247 | 1.01 | 1.00 | 0.246 | 0.97 | 0.96 | 0.249 |
| pathlib.join | shared | 918 | 0.229 | 3.26 | 7.48 | 0.526 | 0.29 | 0.13 | 0.523 |
| logging.format | local | 1863 | 0.402 | 1.17 | 2.90 | 0.995 | 0.84 | 0.31 | 1.099 |
| logging.format | shared | 1915 | 0.417 | 5.57 | 8.84 | 0.662 | 0.17 | 0.10 | 0.737 |
| logging.LogRecord | local | 2714 | 0.493 | 1.59 | 1.01 | 0.312 | 0.61 | 0.98 | 0.308 |
| logging.LogRecord | shared | 2689 | 0.511 | 1.63 | 0.98 | 0.307 | 0.60 | 1.01 | 0.301 |
| urllib.quote | local | 879 | 0.161 | 5.44 | 1.00 | 0.030 | 0.17 | 1.00 | 0.028 |
| urllib.quote | shared | 907 | 0.156 | 5.29 | 1.00 | 0.030 | 0.18 | 0.94 | 0.029 |
| urllib.urlsplit | local | 68 | 0.991 | 10.60 | 10.55 | 0.987 | 0.09 | 0.09 | 0.985 |
| urllib.urlsplit | shared | 74 | 1.015 | 9.75 | 9.46 | 0.985 | 0.10 | 0.10 | 0.980 |
| random.randint | local | 148 | 0.511 | 7.57 | 8.59 | 0.579 | 0.12 | 0.11 | 0.558 |
| random.randint | shared | 148 | 0.510 | 7.62 | 8.73 | 0.584 | 0.12 | 0.11 | 0.558 |
| Random().randint | local | 138 | 0.478 | 1.01 | 1.00 | 0.475 | 0.99 | 0.98 | 0.482 |
| Random().randint | shared | 137 | 0.484 | 6.05 | 9.10 | 0.728 | 0.15 | 0.10 | 0.689 |
| textwrap.wrap | local | 18317 | 0.043 | 1.02 | 1.00 | 0.043 | 0.96 | 0.97 | 0.043 |
| textwrap.wrap | shared | 18670 | 0.043 | 1.00 | 1.00 | 0.043 | 0.98 | 0.96 | 0.044 |
| statistics.mean | local | 5779 | 0.146 | 2.99 | 1.00 | 0.049 | 0.33 | 0.98 | 0.049 |
| statistics.mean | shared | 5833 | 0.142 | 2.97 | 4.47 | 0.214 | 0.33 | 0.21 | 0.221 |
| float.repr | local | 259 | 0.286 | 0.99 | 0.99 | 0.286 | 0.99 | 0.97 | 0.292 |
| float.repr | shared | 257 | 0.299 | 1.01 | 0.96 | 0.283 | 0.90 | 1.00 | 0.270 |
| sorted(50 ints) | local | 808 | 0.731 | 1.01 | 1.01 | 0.735 | 0.97 | 0.97 | 0.730 |
| sorted(50 ints) | shared | 819 | 0.733 | 3.07 | 3.76 | 0.898 | 0.30 | 0.24 | 0.900 |
| list.sort(50 str) | local | 1438 | 0.845 | 1.00 | 1.00 | 0.848 | 0.98 | 0.98 | 0.847 |
| list.sort(50 str) | shared | 1575 | 0.861 | 5.76 | 5.95 | 0.889 | 0.07 | 0.08 | 0.728 |
| x in list(100) | local | 650 | 1.001 | 1.00 | 0.98 | 0.978 | 0.98 | 1.00 | 0.982 |
| x in list(100) | shared | 651 | 1.040 | 1.01 | 1.00 | 1.034 | 0.97 | 0.98 | 1.029 |
| list.index(100) | local | 661 | 0.883 | 1.02 | 1.02 | 0.885 | 0.95 | 0.98 | 0.857 |
| list.index(100) | shared | 686 | 0.957 | 1.00 | 1.00 | 0.953 | 0.96 | 0.95 | 0.969 |
| x in tuple(8) | local | 63 | 0.431 | 1.00 | 1.01 | 0.435 | 1.00 | 0.98 | 0.439 |
| x in tuple(8) | shared | 63 | 0.433 | 1.00 | 1.00 | 0.433 | 1.00 | 0.94 | 0.458 |
| class in tuple(5) | local | 56 | 1.036 | 1.00 | 1.01 | 1.040 | 0.98 | 0.97 | 1.044 |
| class in tuple(5) | shared | 55 | 1.034 | 1.00 | 1.00 | 1.036 | 0.99 | 0.95 | 1.073 |
| pair in list(20) | local | 274 | 0.708 | 1.00 | 1.01 | 0.713 | 1.00 | 0.96 | 0.737 |
| pair in list(20) | shared | 339 | 0.704 | 6.76 | 5.78 | 0.601 | 0.15 | 0.17 | 0.596 |
| deepcopy(UUID) | local | 3377 | 0.218 | 1.81 | 3.37 | 0.407 | 0.55 | 0.29 | 0.409 |
| deepcopy(UUID) | shared | 3548 | 0.218 | 2.48 | 6.62 | 0.581 | 0.40 | 0.14 | 0.608 |
| set.__reduce__ | local | 35242 | 0.020 | 1.01 | 3.61 | 0.072 | 0.87 | 0.28 | 0.064 |
| set.__reduce__ | shared | 35332 | 0.020 | 1.06 | 3.68 | 0.070 | 0.61 | 0.26 | 0.047 |
| sum(list 100) | local | 529 | 0.360 | 1.01 | 1.14 | 0.405 | 0.98 | 0.74 | 0.476 |
| sum(list 100) | shared | 864 | 0.218 | 11.34 | 12.13 | 0.234 | 0.09 | 0.08 | 0.243 |
| max(list 100) | local | 1053 | 0.103 | 1.00 | 1.08 | 0.111 | 0.99 | 0.82 | 0.125 |
| max(list 100) | shared | 1886 | 0.062 | 19.03 | 10.57 | 0.035 | 0.05 | 0.09 | 0.035 |
| min(dict 50) | local | 759 | 0.636 | 1.00 | 1.00 | 0.631 | 0.99 | 0.98 | 0.643 |
| min(dict 50) | shared | 1146 | 0.415 | 10.54 | 11.78 | 0.464 | 0.09 | 0.08 | 0.492 |
| functools.cache hit | local | 36 | 1.002 | 1.01 | 1.00 | 0.998 | 0.97 | 0.95 | 1.030 |
| functools.cache hit | shared | 36 | 0.998 | 5.70 | 5.69 | 0.995 | 0.18 | 0.18 | 0.996 |
| lru_cache(128) hit | local | 43 | 1.010 | 1.01 | 1.00 | 1.000 | 0.99 | 0.98 | 1.023 |
| lru_cache(128) hit | shared | 44 | 0.985 | 9.38 | 9.55 | 1.003 | 0.10 | 0.10 | 0.999 |
| re.match (module cache) | local | 280 | 0.590 | 6.82 | 7.79 | 0.674 | 0.13 | 0.11 | 0.682 |
| re.match (module cache) | shared | 316 | 0.572 | 6.07 | 7.10 | 0.670 | 0.15 | 0.12 | 0.679 |
| isinstance(d, Mapping) | local | 144 | 1.007 | 8.50 | 8.39 | 0.993 | 0.11 | 0.11 | 0.998 |
| isinstance(d, Mapping) | shared | 145 | 1.000 | 8.38 | 8.21 | 0.979 | 0.12 | 0.12 | 0.990 |
| isinstance(f, Number) | local | 145 | 0.991 | 8.41 | 8.60 | 1.013 | 0.11 | 0.11 | 1.009 |
| isinstance(f, Number) | shared | 150 | 0.969 | 8.09 | 8.41 | 1.008 | 0.12 | 0.11 | 0.999 |
| isinstance(d, dict) (ref) | local | 23 | 1.005 | 1.01 | 1.00 | 0.997 | 0.97 | 0.97 | 1.002 |
| isinstance(d, dict) (ref) | shared | 22 | 0.999 | 1.00 | 1.00 | 1.002 | 0.96 | 0.99 | 0.968 |
| Pattern.match (compiled) | local | 115 | 0.961 | 8.98 | 8.07 | 0.863 | 0.11 | 0.12 | 0.866 |
| Pattern.match (compiled) | shared | 128 | 0.858 | 8.00 | 8.17 | 0.877 | 0.12 | 0.12 | 0.873 |
