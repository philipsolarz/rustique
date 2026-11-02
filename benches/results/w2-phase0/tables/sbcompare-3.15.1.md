
### 3.15.1 — 0-100 elements, callgrind Ir

cells 3814; mismatches/errors: 0 []

| category | cells | ≤ 1.05 phase 3 | ≤ 1.05 phase 0 | geomean phase 3 | geomean phase 0 | median phase 3 | geomean phase3/phase0 per cell |
|---|---:|---:|---:|---:|---:|---:|---:|
| kernel path | 586 | 586 | 583 | 0.6327 | 0.6316 | 0.7164 | 1.0017 |
| wave-1 serial (json.dumps, copy.deepcopy) | 152 | 152 | 152 | 0.1541 | 0.1542 | 0.1753 | 0.9992 |
| other ops | 3076 | 3070 | 3063 | 0.9917 | 0.9920 | 1.0000 | 0.9997 |
| all | 3814 | 3808 | 3798 | 0.8593 | 0.8593 | 1.0000 | 1.0000 |

cells > 1.05: 6
  1.076 (phase 0 1.000)  [other ] set/for v in x/mixed/0
  1.076 (phase 0 1.025)  [other ] frozenset/for v in x/mixed/0
  1.069 (phase 0 1.046)  [other ] frozenset/tuple(x)/str/0
  1.062 (phase 0 1.024)  [other ] tuple/repr(x)/int/0
  1.058 (phase 0 0.986)  [other ] tuple/pickle.dumps/str/8
  1.057 (phase 0 1.052)  [other ] list/insert(0, v)/float/100

ops whose geomean moved > 3% vs phase 0: 0

### 3.15.1 — 10k/1M elements, wall time

cells 1846; mismatches/errors: 0 []

| category | cells | ≤ 1.05 phase 3 | ≤ 1.05 phase 0 | geomean phase 3 | geomean phase 0 | median phase 3 | geomean phase3/phase0 per cell |
|---|---:|---:|---:|---:|---:|---:|---:|
| kernel path | 328 | 326 | 323 | 0.4611 | 0.4621 | 0.5245 | 0.9977 |
| wave-1 serial (json.dumps, copy.deepcopy) | 51 | 51 | 51 | 0.2489 | 0.2425 | 0.2835 | 1.0262 |
| other ops | 1467 | 1396 | 1370 | 0.9845 | 0.9872 | 0.9995 | 0.9973 |
| all | 1846 | 1773 | 1744 | 0.8283 | 0.8299 | 0.9971 | 0.9981 |

cells > 1.05: 73
  1.544 (phase 0 0.912)  [other ] dict/miss in x/mixed/10000  rel.std builtin 0.060 enabled 0.307
  1.495 (phase 0 1.001)  [other ] dict/hit in x/mixed/10000  rel.std builtin 0.073 enabled 0.219
  1.485 (phase 0 0.964)  [other ] list/x[i] = v/str/1000000  rel.std builtin 0.011 enabled 0.357
  1.430 (phase 0 1.018)  [other ] dict/x[k] = v/object/10000  rel.std builtin 0.027 enabled 0.288
  1.425 (phase 0 1.343)  [other ] frozenset/x <= t/object/10000  rel.std builtin 0.013 enabled 0.315
  1.365 (phase 0 1.017)  [other ] dict/miss in x/object/10000  rel.std builtin 0.005 enabled 0.298
  1.317 (phase 0 1.046)  [other ] dict/tuple(x)/float/10000  rel.std builtin 0.149 enabled 0.456
  1.294 (phase 0 0.999)  [other ] tuple/copy C(x)/float/10000  rel.std builtin 0.002 enabled 0.250
  1.270 (phase 0 0.952)  [other ] dict/tuple(x)/mixed/1000000  rel.std builtin 0.007 enabled 0.132
  1.247 (phase 0 1.227)  [other ] frozenset/x == t/object/1000000  rel.std builtin 0.201 enabled 0.232
  1.244 (phase 0 0.870)  [other ] set/x.isdisjoint(t)/mixed/1000000  rel.std builtin 0.051 enabled 0.188
  1.230 (phase 0 1.006)  [other ] frozenset/list(x)/object/10000  rel.std builtin 0.013 enabled 0.205
  1.194 (phase 0 1.355)  [other ] tuple/for v in x/int/10000  rel.std builtin 0.002 enabled 0.155
  1.187 (phase 0 0.814)  [other ] dict/tuple(x)/str/1000000  rel.std builtin 0.020 enabled 0.177
  1.181 (phase 0 0.905)  [other ] dict/tuple(x)/mixed/10000  rel.std builtin 0.011 enabled 0.266
  1.180 (phase 0 0.987)  [other ] tuple/for v in x/str/10000  rel.std builtin 0.002 enabled 0.156
  1.177 (phase 0 0.982)  [other ] dict/tuple(x)/int/1000000  rel.std builtin 0.192 enabled 0.010
  1.173 (phase 0 1.152)  [other ] frozenset/x <= t/mixed/10000  rel.std builtin 0.021 enabled 0.181
  1.172 (phase 0 1.033)  [other ] set/miss in x/int/10000  rel.std builtin 0.023 enabled 0.087
  1.170 (phase 0 0.837)  [other ] frozenset/x <= t/object/1000000  rel.std builtin 0.005 enabled 0.028
  1.164 (phase 0 1.004)  [other ] tuple/len(x)/float/10000  rel.std builtin 0.022 enabled 0.300
  1.156 (phase 0 0.920)  [other ] dict/x[k] = v/str/1000000  rel.std builtin 0.001 enabled 0.148
  1.150 (phase 0 0.601)  [other ] dict/x.get(k)/mixed/10000  rel.std builtin 0.031 enabled 0.159
  1.146 (phase 0 1.073)  [other ] set/x == y/int/10000  rel.std builtin 0.015 enabled 0.077
  1.140 (phase 0 1.141)  [other ] tuple/for v in x/str/1000000  rel.std builtin 0.022 enabled 0.099
  1.139 (phase 0 0.746)  [other ] dict/x[k]/mixed/10000  rel.std builtin 0.028 enabled 0.086
  1.134 (phase 0 1.013)  [other ] frozenset/tuple(x)/str/10000  rel.std builtin 0.013 enabled 0.025
  1.130 (phase 0 0.933)  [other ] dict/x == t/object/10000  rel.std builtin 0.047 enabled 0.135
  1.130 (phase 0 0.956)  [other ] dict/for v in x.values()/float/10000  rel.std builtin 0.043 enabled 0.153
  1.126 (phase 0 0.788)  [other ] dict/x.get(miss)/mixed/10000  rel.std builtin 0.016 enabled 0.050
  1.126 (phase 0 1.023)  [other ] set/copy C(x)/mixed/10000  rel.std builtin 0.030 enabled 0.042
  1.119 (phase 0 0.939)  [other ] dict/x[k]/int/10000  rel.std builtin 0.064 enabled 0.069
  1.118 (phase 0 1.001)  [other ] list/x[i] = v/str/10000  rel.std builtin 0.058 enabled 0.049
  1.118 (phase 0 0.938)  [other ] set/x <= t/mixed/1000000  rel.std builtin 0.001 enabled 0.043
  1.114 (phase 0 1.021)  [other ] dict/enumerate/float/1000000  rel.std builtin 0.015 enabled 0.066
  1.102 (phase 0 1.013)  [other ] frozenset/hash(x)/int/10000  rel.std builtin 0.001 enabled 0.103
  1.097 (phase 0 1.079)  [other ] set/x == t/object/10000  rel.std builtin 0.010 enabled 0.090
  1.096 (phase 0 0.939)  [other ] set/miss in x/mixed/10000  rel.std builtin 0.030 enabled 0.117
  1.096 (phase 0 0.974)  [other ] dict/enumerate/int/1000000  rel.std builtin 0.012 enabled 0.104
  1.089 (phase 0 1.032)  [other ] dict/reversed/float/10000  rel.std builtin 0.129 enabled 0.029
  1.087 (phase 0 0.926)  [other ] list/x[i] = v/mixed/10000  rel.std builtin 0.079 enabled 0.059
  1.084 (phase 0 1.022)  [other ] frozenset/x.isdisjoint(t)/object/10000  rel.std builtin 0.043 enabled 0.069
  1.081 (phase 0 1.016)  [other ] set/x <= t/str/1000000  rel.std builtin 0.012 enabled 0.061
  1.079 (phase 0 0.990)  [other ] frozenset/x == t/str/1000000  rel.std builtin 0.004 enabled 0.008
  1.079 (phase 0 1.002)  [other ] tuple/copy C(x)/mixed/1000000  rel.std builtin 0.001 enabled 0.080
  1.077 (phase 0 1.012)  [other ] set/tuple(x)/mixed/10000  rel.std builtin 0.017 enabled 0.081
  1.077 (phase 0 0.868)  [other ] tuple/for v in x/float/1000000  rel.std builtin 0.069 enabled 0.169
  1.076 (phase 0 1.216)  [other ] list/pickle.dumps/str/1000000  rel.std builtin 0.023 enabled 0.018
  1.076 (phase 0 0.970)  [other ] dict/x == t/object/1000000  rel.std builtin 0.003 enabled 0.081
  1.075 (phase 0 1.087)  [other ] frozenset/tuple(x)/object/10000  rel.std builtin 0.010 enabled 0.073
  1.074 (phase 0 1.011)  [other ] dict/x[k] = v/str/10000  rel.std builtin 0.026 enabled 0.058
  1.073 (phase 0 1.138)  [other ] list/pickle round trip/str/1000000  rel.std builtin 0.015 enabled 0.010
  1.071 (phase 0 0.965)  [other ] dict/x[k] = v/int/1000000  rel.std builtin 0.027 enabled 0.059
  1.067 (phase 0 0.993)  [other ] dict/x.setdefault(k)/mixed/1000000  rel.std builtin 0.014 enabled 0.077
  1.067 (phase 0 0.948)  [other ] tuple/from list/mixed/10000  rel.std builtin 0.001 enabled 0.064
  1.067 (phase 0 1.061)  [other ] list/repr(x)/object/1000000  rel.std builtin 0.013 enabled 0.015
  1.066 (phase 0 1.002)  [other ] list/x[i] = v/float/10000  rel.std builtin 0.058 enabled 0.047
  1.064 (phase 0 1.166)  [other ] set/x.isdisjoint(t)/str/10000  rel.std builtin 0.085 enabled 0.097
  1.063 (phase 0 0.983)  [other ] dict/for v in x.values()/mixed/10000  rel.std builtin 0.030 enabled 0.061
  1.063 (phase 0 1.016)  [other ] dict/miss in x/mixed/1000000  rel.std builtin 0.038 enabled 0.066
  1.061 (phase 0 1.033)  [other ] dict/repr(x)/str/10000  rel.std builtin 0.003 enabled 0.048
  1.061 (phase 0 1.073)  [kernel] set/all(x)/str/10000  rel.std builtin 0.024 enabled 0.073
  1.061 (phase 0 1.056)  [other ] list/len(x)/mixed/10000  rel.std builtin 0.002 enabled 0.049
  1.059 (phase 0 0.906)  [other ] set/miss in x/int/1000000  rel.std builtin 0.118 enabled 0.074
  1.057 (phase 0 1.060)  [other ] dict/pickle.dumps/object/1000000  rel.std builtin 0.007 enabled 0.011
  1.057 (phase 0 1.008)  [other ] list/x.clear()/mixed/1000000  rel.std builtin 0.041 enabled 0.027
  1.056 (phase 0 1.036)  [other ] dict/reversed/float/1000000  rel.std builtin 0.020 enabled 0.070
  1.055 (phase 0 1.030)  [other ] set/list(x)/mixed/10000  rel.std builtin 0.005 enabled 0.021
  1.053 (phase 0 1.006)  [other ] dict/x.get(k)/int/1000000  rel.std builtin 0.003 enabled 0.059
  1.052 (phase 0 1.099)  [other ] dict/hit in x/object/1000000  rel.std builtin 0.006 enabled 0.062
  1.052 (phase 0 1.054)  [kernel] frozenset/min(x)/str/10000  rel.std builtin 0.060 enabled 0.005
  1.051 (phase 0 0.998)  [other ] frozenset/x == y/int/10000  rel.std builtin 0.086 enabled 0.072
  1.051 (phase 0 0.960)  [other ] tuple/reversed/int/1000000  rel.std builtin 0.014 enabled 0.086

ops whose geomean moved > 3% vs phase 0: 26
  tuple     (*x, *y)                     cells   5  phase 3 0.499  phase 0 1.000  change 0.499
  list      sum(x)                       cells   4  phase 3 0.303  phase 0 0.330  change 0.917
  frozenset g(*x)                        cells   5  phase 3 0.950  phase 0 1.026  change 0.926
  set       g(*x)                        cells   5  phase 3 0.968  phase 0 1.041  change 0.931
  frozenset x <= t                       cells  10  phase 3 1.048  phase 0 1.113  change 0.942
  tuple     hit in x                     cells  10  phase 3 0.308  phase 0 0.326  change 0.942
  set       x == t                       cells  10  phase 3 0.970  phase 0 1.021  change 0.950
  set       x.isdisjoint(t)              cells  10  phase 3 0.986  phase 0 1.034  change 0.954
  list      x < y                        cells   8  phase 3 0.954  phase 0 0.998  change 0.955
  frozenset x.isdisjoint(t)              cells  10  phase 3 0.974  phase 0 1.013  change 0.961
  frozenset x == t                       cells  10  phase 3 1.022  phase 0 1.061  change 0.964
  frozenset copy C(x)                    cells  10  phase 3 0.967  phase 0 1.002  change 0.965
  dict      for v in x.values()          cells  10  phase 3 1.000  phase 0 1.033  change 0.969
  dict      reversed                     cells  10  phase 3 1.022  phase 0 0.989  change 1.034
  tuple     a, *b = x                    cells   5  phase 3 1.001  phase 0 0.966  change 1.036
  dict      x.get(miss)                  cells  10  phase 3 0.983  phase 0 0.944  change 1.041
  dict      enumerate                    cells  10  phase 3 1.024  phase 0 0.983  change 1.042
  tuple     x.count(hit)                 cells  10  phase 3 0.314  phase 0 0.301  change 1.042
  dict      sum(x)                       cells   4  phase 3 0.704  phase 0 0.671  change 1.049
  dict      x[k] = v                     cells  10  phase 3 1.057  phase 0 0.997  change 1.061
  set       sum(x)                       cells   4  phase 3 0.698  phase 0 0.657  change 1.062
  list      x[i] = v                     cells  10  phase 3 1.068  phase 0 0.997  change 1.071
  dict      miss in x                    cells  10  phase 3 1.017  phase 0 0.943  change 1.079
  dict      x.get(k)                     cells  10  phase 3 1.007  phase 0 0.890  change 1.131
  dict      tuple(x)                     cells  10  phase 3 1.063  phase 0 0.932  change 1.140
  tuple     copy.deepcopy                cells   5  phase 3 0.026  phase 0 0.022  change 1.175
