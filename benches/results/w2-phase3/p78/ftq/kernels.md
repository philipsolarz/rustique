build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| json.dumps | local | 1880 | 0.227 | 1.40 | 1.00 | 0.162 | 0.70 | 1.00 | 0.159 |
| json.dumps | shared | 1895 | 0.229 | 3.54 | 10.75 | 0.695 | 0.14 | 0.09 | 0.364 |
| json.loads | local | 1337 | 0.432 | 1.51 | 0.97 | 0.278 | 0.66 | 0.99 | 0.289 |
| json.loads | shared | 1327 | 0.449 | 1.51 | 0.93 | 0.276 | 0.66 | 1.01 | 0.290 |
| copy.deepcopy | local | 3287 | 0.248 | 1.35 | 1.61 | 0.296 | 0.72 | 0.61 | 0.294 |
| copy.deepcopy | shared | 3488 | 0.241 | 1.68 | 6.46 | 0.929 | 0.59 | 0.14 | 0.987 |
| deepcopy(frozenset) | local | 38447 | 0.021 | 1.02 | 1.24 | 0.026 | 0.86 | 0.80 | 0.023 |
| deepcopy(frozenset) | shared | 38711 | 0.023 | 1.24 | 5.38 | 0.098 | 0.55 | 0.18 | 0.071 |
| dataclasses.asdict | local | 1189 | 0.456 | 1.33 | 1.27 | 0.436 | 0.72 | 0.74 | 0.447 |
| dataclasses.asdict | shared | 1267 | 0.464 | 4.12 | 7.59 | 0.854 | 0.24 | 0.12 | 0.900 |
| datetime.strptime | local | 3406 | 0.058 | 2.97 | 1.32 | 0.026 | 0.13 | 0.74 | 0.010 |
| datetime.strptime | shared | 3406 | 0.061 | 3.17 | 1.24 | 0.024 | 0.14 | 0.80 | 0.011 |
| datetime.isoformat | local | 390 | 0.155 | 0.99 | 1.00 | 0.156 | 1.00 | 0.99 | 0.157 |
| datetime.isoformat | shared | 392 | 0.157 | 1.55 | 9.15 | 0.927 | 0.63 | 0.11 | 0.925 |
| date.strftime | local | 1289 | 0.086 | 6.91 | 1.94 | 0.024 | 0.14 | 0.51 | 0.024 |
| date.strftime | shared | 1314 | 0.089 | 7.81 | 10.40 | 0.119 | 0.13 | 0.09 | 0.120 |
| posixpath.join | local | 457 | 0.202 | 1.01 | 1.00 | 0.199 | 0.96 | 0.97 | 0.200 |
| posixpath.join | shared | 458 | 0.199 | 1.01 | 1.01 | 0.199 | 0.96 | 0.93 | 0.205 |
| posixpath.join_fb | local | 287 | 0.303 | 1.63 | 1.00 | 0.186 | 0.60 | 0.97 | 0.188 |
| posixpath.join_fb | shared | 293 | 0.299 | 6.89 | 8.29 | 0.359 | 0.14 | 0.11 | 0.368 |
| fnmatch.fnmatch | local | 304 | 0.135 | 7.77 | 1.00 | 0.017 | 0.12 | 0.98 | 0.017 |
| fnmatch.fnmatch | shared | 308 | 0.132 | 7.65 | 1.01 | 0.017 | 0.13 | 0.96 | 0.017 |
| pathlib.join | local | 848 | 0.244 | 1.00 | 1.00 | 0.244 | 0.98 | 0.86 | 0.277 |
| pathlib.join | shared | 893 | 0.266 | 3.51 | 6.55 | 0.497 | 0.27 | 0.14 | 0.504 |
| logging.format | local | 1839 | 0.401 | 1.19 | 3.02 | 1.016 | 0.83 | 0.29 | 1.129 |
| logging.format | shared | 1871 | 0.417 | 5.55 | 9.09 | 0.683 | 0.17 | 0.10 | 0.746 |
| logging.LogRecord | local | 2683 | 0.493 | 1.60 | 1.01 | 0.310 | 0.61 | 0.98 | 0.304 |
| logging.LogRecord | shared | 2702 | 0.508 | 1.60 | 0.97 | 0.309 | 0.61 | 1.00 | 0.307 |
| urllib.quote | local | 867 | 0.162 | 5.32 | 1.00 | 0.031 | 0.18 | 0.99 | 0.029 |
| urllib.quote | shared | 909 | 0.155 | 5.07 | 1.00 | 0.031 | 0.19 | 0.97 | 0.030 |
| urllib.urlsplit | local | 68 | 1.033 | 10.54 | 10.38 | 1.018 | 0.09 | 0.09 | 1.023 |
| urllib.urlsplit | shared | 77 | 0.996 | 9.29 | 9.48 | 1.017 | 0.10 | 0.10 | 1.017 |
| random.randint | local | 150 | 0.482 | 7.48 | 9.07 | 0.584 | 0.12 | 0.11 | 0.536 |
| random.randint | shared | 150 | 0.485 | 7.47 | 8.99 | 0.583 | 0.12 | 0.11 | 0.536 |
| Random().randint | local | 140 | 0.461 | 0.99 | 1.01 | 0.469 | 1.00 | 0.97 | 0.474 |
| Random().randint | shared | 138 | 0.474 | 6.00 | 9.08 | 0.717 | 0.15 | 0.10 | 0.678 |
| textwrap.wrap | local | 18703 | 0.042 | 1.01 | 1.01 | 0.042 | 0.99 | 0.98 | 0.042 |
| textwrap.wrap | shared | 18416 | 0.044 | 1.02 | 1.01 | 0.043 | 0.94 | 0.91 | 0.045 |
| statistics.mean | local | 5700 | 0.149 | 3.03 | 0.99 | 0.049 | 0.33 | 1.00 | 0.049 |
| statistics.mean | shared | 5764 | 0.145 | 3.00 | 4.29 | 0.208 | 0.33 | 0.22 | 0.215 |
| float.repr | local | 259 | 0.279 | 1.01 | 1.03 | 0.283 | 0.92 | 0.92 | 0.278 |
| float.repr | shared | 257 | 0.313 | 1.02 | 0.93 | 0.285 | 0.93 | 0.99 | 0.295 |
| sorted(50 ints) | local | 810 | 0.735 | 1.00 | 1.00 | 0.733 | 0.98 | 0.98 | 0.735 |
| sorted(50 ints) | shared | 819 | 0.723 | 3.05 | 3.80 | 0.900 | 0.30 | 0.24 | 0.909 |
| list.sort(50 str) | local | 1446 | 0.853 | 0.99 | 1.00 | 0.863 | 1.00 | 0.99 | 0.866 |
| list.sort(50 str) | shared | 1577 | 0.874 | 5.81 | 5.82 | 0.876 | 0.07 | 0.08 | 0.755 |
| x in list(100) | local | 652 | 0.833 | 1.00 | 1.04 | 0.862 | 0.98 | 0.95 | 0.865 |
| x in list(100) | shared | 653 | 0.916 | 1.01 | 1.00 | 0.906 | 0.95 | 1.00 | 0.868 |
| list.index(100) | local | 662 | 0.820 | 1.02 | 1.01 | 0.805 | 0.94 | 0.97 | 0.794 |
| list.index(100) | shared | 683 | 0.876 | 1.01 | 1.00 | 0.866 | 0.95 | 0.95 | 0.871 |
| x in tuple(8) | local | 63 | 0.430 | 1.00 | 1.00 | 0.429 | 0.99 | 1.00 | 0.428 |
| x in tuple(8) | shared | 63 | 0.429 | 1.00 | 1.00 | 0.430 | 0.96 | 0.95 | 0.435 |
| class in tuple(5) | local | 56 | 1.033 | 1.00 | 1.01 | 1.040 | 0.98 | 0.97 | 1.050 |
| class in tuple(5) | shared | 55 | 1.033 | 1.00 | 1.00 | 1.036 | 0.99 | 0.94 | 1.096 |
| pair in list(20) | local | 274 | 0.710 | 1.00 | 1.00 | 0.711 | 1.00 | 0.98 | 0.721 |
| pair in list(20) | shared | 337 | 0.713 | 6.84 | 6.56 | 0.683 | 0.14 | 0.15 | 0.676 |
| deepcopy(UUID) | local | 3376 | 0.221 | 1.82 | 3.38 | 0.410 | 0.55 | 0.29 | 0.417 |
| deepcopy(UUID) | shared | 3538 | 0.219 | 2.49 | 6.58 | 0.581 | 0.40 | 0.14 | 0.608 |
| set.__reduce__ | local | 35007 | 0.021 | 1.02 | 3.56 | 0.072 | 0.87 | 0.28 | 0.064 |
| set.__reduce__ | shared | 35194 | 0.020 | 1.03 | 3.64 | 0.073 | 0.63 | 0.26 | 0.050 |
| sum(list 100) | local | 526 | 0.359 | 1.01 | 1.10 | 0.391 | 0.97 | 0.74 | 0.467 |
| sum(list 100) | shared | 835 | 0.229 | 11.94 | 12.05 | 0.231 | 0.08 | 0.08 | 0.238 |
| max(list 100) | local | 1051 | 0.104 | 1.00 | 1.08 | 0.112 | 0.98 | 0.82 | 0.124 |
| max(list 100) | shared | 1892 | 0.060 | 19.06 | 10.99 | 0.035 | 0.05 | 0.09 | 0.035 |
| min(dict 50) | local | 757 | 0.622 | 1.01 | 1.00 | 0.618 | 0.97 | 0.99 | 0.609 |
| min(dict 50) | shared | 1137 | 0.420 | 10.54 | 11.52 | 0.459 | 0.09 | 0.08 | 0.486 |
| functools.cache hit | local | 36 | 1.002 | 1.01 | 1.00 | 0.997 | 0.96 | 0.98 | 0.987 |
| functools.cache hit | shared | 36 | 1.001 | 5.69 | 5.69 | 1.001 | 0.18 | 0.18 | 1.001 |
| lru_cache(128) hit | local | 43 | 0.997 | 1.01 | 1.01 | 1.002 | 0.98 | 0.98 | 0.997 |
| lru_cache(128) hit | shared | 44 | 0.997 | 9.44 | 9.55 | 1.007 | 0.10 | 0.10 | 1.004 |
| re.match (module cache) | local | 284 | 0.557 | 6.71 | 7.99 | 0.664 | 0.13 | 0.11 | 0.663 |
| re.match (module cache) | shared | 310 | 0.552 | 6.17 | 7.41 | 0.662 | 0.14 | 0.12 | 0.661 |
| isinstance(d, Mapping) | local | 145 | 1.001 | 8.42 | 8.34 | 0.991 | 0.12 | 0.12 | 0.996 |
| isinstance(d, Mapping) | shared | 146 | 0.999 | 8.36 | 8.24 | 0.984 | 0.12 | 0.12 | 0.985 |
| isinstance(f, Number) | local | 144 | 1.013 | 8.52 | 8.36 | 0.994 | 0.11 | 0.12 | 0.993 |
| isinstance(f, Number) | shared | 147 | 1.001 | 8.27 | 8.25 | 0.998 | 0.12 | 0.12 | 0.993 |
| isinstance(d, dict) (ref) | local | 23 | 1.005 | 1.00 | 1.00 | 1.001 | 0.98 | 0.98 | 0.998 |
| isinstance(d, dict) (ref) | shared | 22 | 0.996 | 1.00 | 1.00 | 1.001 | 0.99 | 0.96 | 1.019 |
| Pattern.match (compiled) | local | 112 | 1.047 | 9.32 | 9.53 | 1.071 | 0.10 | 0.10 | 1.040 |
| Pattern.match (compiled) | shared | 118 | 0.961 | 9.36 | 9.84 | 1.011 | 0.11 | 0.10 | 1.006 |
