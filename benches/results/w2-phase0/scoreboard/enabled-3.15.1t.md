| build | type | cells at parity |
|---|---|---:|
| 3.15.1t | list | 888/890 |
| 3.15.1t | tuple | 719/719 |
| 3.15.1t | dict | 819/819 |
| 3.15.1t | set | 743/743 |
| 3.15.1t | frozenset | 643/643 |

Ratio = builtin types under `rustique.enable()` ÷ plain CPython (lower is faster); **bold** = above the 1.05 parity bar; `—` = missing; `err` = raised; `≠` = result differs from the builtin. Sizes 0–100 show instruction-count ratios (callgrind) when measured, larger sizes pyperf median wall-time ratios.

### list — 3.15.1t  (888/890 cells at parity)

| op | storage | 0 | 1 | 8 | 100 | 10k | 1M |
|---|---|---:|---:|---:|---:|---:|---:|
| literal/display | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| from list | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| from list | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| copy C(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| append loop | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| append loop | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| append loop | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| append loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| append loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| extend | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| extend | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| extend | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| extend | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| extend | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | str | 1.00 | 1.00 | **1.07** | 1.00 |  |  |
| x + y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | str | 1.00 | 1.00 | **1.07** | 1.00 |  |  |
| x * 2 | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| [*x, *y] | int | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| [*x, *y] | float | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| [*x, *y] | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| [*x, *y] | mixed | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| [*x, *y] | object | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| len(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] = v | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] = v | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] = v | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] = v | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] = v | object |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | int |  | 0.76 | 0.54 | 0.21 |  |  |
| hit in x | float |  | 0.76 | 0.51 | 0.16 |  |  |
| hit in x | str |  | 0.76 | 0.68 | 0.40 |  |  |
| hit in x | mixed |  | 0.76 | 0.44 | 0.19 |  |  |
| hit in x | object |  | 0.76 | 1.01 | 1.00 |  |  |
| miss in x | int | 0.87 | 0.94 | 0.38 | 0.19 |  |  |
| miss in x | float | 0.87 | 0.93 | 0.34 | 0.14 |  |  |
| miss in x | str | 0.87 | 0.99 | 0.37 | 0.15 |  |  |
| miss in x | mixed | 0.87 | 0.88 | 0.31 | 0.13 |  |  |
| miss in x | object | 0.87 | 1.04 | 1.01 | 1.00 |  |  |
| x[a:b] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x.index(hit) | int |  | 0.92 | 0.65 | 0.22 |  |  |
| x.index(hit) | float |  | 0.92 | 0.63 | 0.18 |  |  |
| x.index(hit) | str |  | 0.92 | 0.74 | 0.40 |  |  |
| x.index(hit) | mixed |  | 0.92 | 0.55 | 0.21 |  |  |
| x.index(hit) | object |  | 0.92 | 1.02 | 1.00 |  |  |
| x.count(hit) | int |  | 0.93 | 0.50 | 0.20 |  |  |
| x.count(hit) | float |  | 0.93 | 0.47 | 0.15 |  |  |
| x.count(hit) | str |  | 0.94 | 0.61 | 0.38 |  |  |
| x.count(hit) | mixed |  | 0.94 | 0.40 | 0.19 |  |  |
| x.count(hit) | object |  | 1.03 | 1.00 | 1.00 |  |  |
| unpack exact | int |  | 1.00 | 1.00 |  |  |  |
| unpack exact | float |  | 1.00 | 1.00 |  |  |  |
| unpack exact | str |  | 1.00 | 1.00 |  |  |  |
| unpack exact | mixed |  | 1.00 | 1.00 |  |  |  |
| unpack exact | object |  | 1.00 | 1.00 |  |  |  |
| a, *b = x | int |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | float |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | str |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | object |  | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| list(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| sorted(x) | int | 0.98 | 0.98 | 0.83 | 0.52 |  |  |
| sorted(x) | float | 0.98 | 0.99 | 0.77 | 0.55 |  |  |
| sorted(x) | str | 0.98 | 0.98 | 0.90 | 0.74 |  |  |
| sorted(x) | object | 0.98 | 0.98 | 1.03 | 1.00 |  |  |
| sum(x) | int | 0.83 | 0.87 | 0.62 | 0.33 |  |  |
| sum(x) | float | 0.83 | 0.96 | 0.70 | 0.35 |  |  |
| min(x) | int |  | 0.92 | 0.36 | 0.13 |  |  |
| min(x) | float |  | 0.92 | 0.34 | 0.08 |  |  |
| min(x) | str |  | 0.92 | 0.51 | 0.38 |  |  |
| min(x) | object |  | 0.92 | 0.72 | 0.68 |  |  |
| max(x) | int |  | 0.93 | 0.36 | 0.13 |  |  |
| max(x) | float |  | 0.92 | 0.33 | 0.08 |  |  |
| max(x) | str |  | 0.93 | 0.51 | 0.38 |  |  |
| max(x) | object |  | 0.93 | 0.72 | 0.68 |  |  |
| any(x) | int | 0.87 | 0.88 | 0.88 | 0.88 |  |  |
| any(x) | float | 0.87 | 0.88 | 0.88 | 0.88 |  |  |
| any(x) | str | 0.87 | 0.88 | 0.88 | 0.88 |  |  |
| any(x) | mixed | 0.87 | 0.88 | 0.88 | 0.88 |  |  |
| any(x) | object | 0.87 | 0.89 | 0.89 | 0.89 |  |  |
| all(x) | int | 0.86 | 0.81 | 0.48 | 0.23 |  |  |
| all(x) | float | 0.86 | 0.82 | 0.49 | 0.24 |  |  |
| all(x) | str | 0.86 | 0.81 | 0.49 | 0.25 |  |  |
| all(x) | mixed | 0.86 | 0.81 | 0.65 | 0.65 |  |  |
| all(x) | object | 0.85 | 0.82 | 0.52 | 0.27 |  |  |
| x.sort() | int | 0.90 | 0.90 | 0.68 | 0.46 |  |  |
| x.sort() | float | 0.90 | 0.90 | 0.59 | 0.48 |  |  |
| x.sort() | str | 0.90 | 0.90 | 0.85 | 0.72 |  |  |
| x.sort() | object | 0.90 | 0.90 | 1.03 | 1.00 |  |  |
| x.reverse() | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| x.reverse() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.reverse() | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| x.reverse() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.reverse() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pop() loop | int | 0.98 | 1.03 | 1.00 | 1.00 |  |  |
| pop() loop | float | 1.00 | 1.01 | 1.00 | 1.00 |  |  |
| pop() loop | str | 1.00 | 1.03 | 1.00 | 1.00 |  |  |
| pop() loop | mixed | 1.00 | 1.01 | 1.00 | 1.00 |  |  |
| pop() loop | object | 1.00 | 1.01 | 1.00 | 1.00 |  |  |
| insert(0, v) | int | 0.99 | 1.00 | 1.04 | 1.00 |  |  |
| insert(0, v) | float | 1.00 | 1.00 | 1.03 | 1.00 |  |  |
| insert(0, v) | str | 1.00 | 1.00 | 1.04 | 1.00 |  |  |
| insert(0, v) | mixed | 0.98 | 1.00 | 1.03 | 1.00 |  |  |
| insert(0, v) | object | 1.00 | 1.00 | 1.02 | 1.00 |  |  |
| x.clear() | int | 0.99 | 1.02 | 1.00 | 1.01 |  |  |
| x.clear() | float | 1.00 | 1.02 | 1.00 | 1.00 |  |  |
| x.clear() | str | 1.00 | 1.02 | 0.99 | 1.00 |  |  |
| x.clear() | mixed | 1.00 | 1.02 | 1.00 | 1.00 |  |  |
| x.clear() | object | 1.00 | 1.02 | 1.00 | 1.00 |  |  |
| repr(x) | int | 0.99 | 1.00 | 0.99 | 1.00 |  |  |
| repr(x) | float | 1.00 | 0.32 | 0.18 | 0.14 |  |  |
| repr(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | mixed | 1.00 | 1.00 | 0.53 | 0.44 |  |  |
| repr(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| json.dumps | int | 0.08 | 0.11 | 0.16 | 0.30 |  |  |
| json.dumps | float | 0.08 | 0.09 | 0.08 | 0.07 |  |  |
| json.dumps | str | 0.08 | 0.12 | 0.18 | 0.43 |  |  |
| json.dumps | mixed | 0.08 | 0.11 | 0.13 | 0.18 |  |  |
| json.dumps | object | 0.08 | 0.13 | 0.21 | 0.31 |  |  |
| pickle.dumps | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | str | 1.00 | 1.00 | 1.02 | 1.00 |  |  |
| pickle.dumps | mixed | 1.00 | 1.00 | 0.99 | 0.99 |  |  |
| pickle.dumps | object | 1.00 | 1.00 | 0.99 | 0.99 |  |  |
| pickle round trip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | str | 1.00 | 1.00 | 1.02 | 1.00 |  |  |
| pickle round trip | mixed | 1.00 | 1.00 | 1.00 | 1.01 |  |  |
| pickle round trip | object | 1.00 | 1.00 | 1.01 | 1.00 |  |  |
| copy.deepcopy | int | 0.29 | 0.28 | 0.22 | 0.16 |  |  |
| copy.deepcopy | float | 0.29 | 0.28 | 0.22 | 0.17 |  |  |
| copy.deepcopy | str | 0.29 | 0.28 | 0.22 | 0.16 |  |  |
| copy.deepcopy | mixed | 0.28 | 0.28 | 0.22 | 0.16 |  |  |
| copy.deepcopy | object | 0.28 | 0.18 | 0.10 | 0.09 |  |  |

### tuple — 3.15.1t  (719/719 cells at parity)

| op | storage | 0 | 1 | 8 | 100 | 10k | 1M |
|---|---|---:|---:|---:|---:|---:|---:|
| literal/display | int | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x + y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x * 2 | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| (*x, *y) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| (*x, *y) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| (*x, *y) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| (*x, *y) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| (*x, *y) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[i] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[-1] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | int |  | 0.91 | 0.47 | 0.24 |  |  |
| hit in x | float |  | 0.91 | 0.43 | 0.20 |  |  |
| hit in x | str |  | 0.91 | 0.58 | 0.39 |  |  |
| hit in x | mixed |  | 0.91 | 0.36 | 0.18 |  |  |
| hit in x | object |  | 0.91 | 1.01 | 1.00 |  |  |
| miss in x | int | 0.89 | 0.83 | 0.36 | 0.23 |  |  |
| miss in x | float | 0.89 | 0.78 | 0.31 | 0.18 |  |  |
| miss in x | str | 0.89 | 0.85 | 0.28 | 0.13 |  |  |
| miss in x | mixed | 0.89 | 0.75 | 0.23 | 0.12 |  |  |
| miss in x | object | 0.89 | 1.04 | 1.01 | 1.00 |  |  |
| x[a:b] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[a:b] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x.index(hit) | int |  | 0.97 | 0.63 | 0.27 |  |  |
| x.index(hit) | float |  | 0.97 | 0.61 | 0.24 |  |  |
| x.index(hit) | str |  | 0.97 | 0.70 | 0.42 |  |  |
| x.index(hit) | mixed |  | 0.97 | 0.52 | 0.20 |  |  |
| x.index(hit) | object |  | 0.97 | 1.02 | 1.00 |  |  |
| x.count(hit) | int |  | 0.97 | 0.47 | 0.22 |  |  |
| x.count(hit) | float |  | 0.96 | 0.44 | 0.19 |  |  |
| x.count(hit) | str |  | 0.97 | 0.55 | 0.38 |  |  |
| x.count(hit) | mixed |  | 0.97 | 0.36 | 0.17 |  |  |
| x.count(hit) | object |  | 1.03 | 1.00 | 1.00 |  |  |
| unpack exact | int |  | 1.00 | 1.00 |  |  |  |
| unpack exact | float |  | 1.00 | 1.00 |  |  |  |
| unpack exact | str |  | 1.00 | 1.00 |  |  |  |
| unpack exact | mixed |  | 1.00 | 1.00 |  |  |  |
| unpack exact | object |  | 1.00 | 1.00 |  |  |  |
| a, *b = x | int |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | float |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | str |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| a, *b = x | object |  | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | float | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| list(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x < y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | str | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| hash(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| sorted(x) | int | 0.98 | 0.98 | 0.83 | 0.52 |  |  |
| sorted(x) | float | 0.98 | 0.98 | 0.77 | 0.55 |  |  |
| sorted(x) | str | 0.98 | 0.98 | 0.90 | 0.74 |  |  |
| sorted(x) | object | 0.98 | 0.98 | 1.03 | 1.00 |  |  |
| sum(x) | int | 0.80 | 0.86 | 0.69 | 0.44 |  |  |
| sum(x) | float | 0.80 | 0.96 | 0.76 | 0.45 |  |  |
| min(x) | int |  | 0.92 | 0.37 | 0.14 |  |  |
| min(x) | float |  | 0.91 | 0.35 | 0.09 |  |  |
| min(x) | str |  | 0.91 | 0.54 | 0.41 |  |  |
| min(x) | object |  | 0.91 | 0.75 | 0.71 |  |  |
| max(x) | int |  | 0.92 | 0.37 | 0.14 |  |  |
| max(x) | float |  | 0.91 | 0.34 | 0.09 |  |  |
| max(x) | str |  | 0.92 | 0.54 | 0.41 |  |  |
| max(x) | object |  | 0.92 | 0.75 | 0.71 |  |  |
| any(x) | int | 0.82 | 0.84 | 0.84 | 0.84 |  |  |
| any(x) | float | 0.82 | 0.84 | 0.84 | 0.84 |  |  |
| any(x) | str | 0.82 | 0.84 | 0.84 | 0.84 |  |  |
| any(x) | mixed | 0.82 | 0.84 | 0.84 | 0.84 |  |  |
| any(x) | object | 0.82 | 0.85 | 0.85 | 0.85 |  |  |
| all(x) | int | 0.80 | 0.80 | 0.54 | 0.29 |  |  |
| all(x) | float | 0.80 | 0.80 | 0.55 | 0.31 |  |  |
| all(x) | str | 0.80 | 0.80 | 0.55 | 0.31 |  |  |
| all(x) | mixed | 0.80 | 0.80 | 0.68 | 0.68 |  |  |
| all(x) | object | 0.80 | 0.81 | 0.57 | 0.35 |  |  |
| repr(x) | int | 1.00 | 1.00 | 1.01 | 1.00 |  |  |
| repr(x) | float | 1.00 | 0.32 | 0.18 | 0.14 |  |  |
| repr(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | mixed | 1.00 | 1.00 | 0.53 | 0.44 |  |  |
| repr(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| json.dumps | int | 0.08 | 0.09 | 0.14 | 0.30 |  |  |
| json.dumps | float | 0.08 | 0.08 | 0.07 | 0.07 |  |  |
| json.dumps | str | 0.08 | 0.09 | 0.17 | 0.42 |  |  |
| json.dumps | mixed | 0.08 | 0.09 | 0.12 | 0.17 |  |  |
| json.dumps | object | 0.08 | 0.11 | 0.20 | 0.30 |  |  |
| pickle.dumps | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | str | 1.00 | 1.00 | 1.02 | 0.98 |  |  |
| pickle.dumps | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | object | 1.00 | 1.00 | 1.01 | 1.01 |  |  |
| pickle round trip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | mixed | 1.00 | 1.00 | 1.00 | 1.01 |  |  |
| pickle round trip | object | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| copy.deepcopy | int | 0.21 | 0.17 | 0.09 | 0.02 |  |  |
| copy.deepcopy | float | 0.21 | 0.17 | 0.09 | 0.02 |  |  |
| copy.deepcopy | str | 0.21 | 0.17 | 0.09 | 0.02 |  |  |
| copy.deepcopy | mixed | 0.21 | 0.17 | 0.09 | 0.02 |  |  |
| copy.deepcopy | object | 0.21 | 0.14 | 0.09 | 0.08 |  |  |

### dict — 3.15.1t  (819/819 cells at parity)

| op | storage | 0 | 1 | 8 | 100 | 10k | 1M |
|---|---|---:|---:|---:|---:|---:|---:|
| literal/display | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from dict | int | 1.00 | 0.98 | 1.00 | 1.00 |  |  |
| from dict | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from dict | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from dict | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from dict | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from pairs | int | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| from pairs | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from pairs | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from pairs | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from pairs | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | int | 1.00 | 0.98 | 1.00 | 1.00 |  |  |
| copy C(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| setitem loop | int | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| setitem loop | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| setitem loop | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| setitem loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| setitem loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| {**x, **t} | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| {**x, **t} | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| {**x, **t} | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| {**x, **t} | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| {**x, **t} | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x[k] | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] = v | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] = v | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] = v | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] = v | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x[k] = v | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x.get(k) | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x.get(k) | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x.get(k) | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x.get(k) | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x.get(k) | object |  | 1.00 | 1.00 | 1.00 |  |  |
| x.get(miss) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.get(miss) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.get(miss) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.get(miss) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.get(miss) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.setdefault(k) | int |  | 1.00 | 1.00 | 1.00 |  |  |
| x.setdefault(k) | float |  | 1.00 | 1.00 | 1.00 |  |  |
| x.setdefault(k) | str |  | 1.00 | 1.00 | 1.00 |  |  |
| x.setdefault(k) | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| x.setdefault(k) | object |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | int |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | float |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | str |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | object |  | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(**x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for k, v in x.items() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for k, v in x.items() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for k, v in x.items() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for k, v in x.items() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for k, v in x.items() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x.values() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x.values() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x.values() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x.values() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x.values() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | int | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| list(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | mixed | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | int | 1.00 | 1.00 | 1.04 | 1.00 |  |  |
| tuple(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| reversed | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.keys() & t.keys() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.keys() & t.keys() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.keys() & t.keys() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.keys() & t.keys() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.keys() & t.keys() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| sorted(x) | int | 0.98 | 0.99 | 0.85 | 0.55 |  |  |
| sorted(x) | float | 0.98 | 0.99 | 0.85 | 0.61 |  |  |
| sorted(x) | str | 0.98 | 0.99 | 0.90 | 0.75 |  |  |
| sorted(x) | object | 0.98 | 0.99 | 1.02 | 1.00 |  |  |
| sum(x) | int | 0.73 | 0.76 | 0.74 | 0.72 |  |  |
| sum(x) | float | 0.73 | 0.85 | 0.79 | 0.71 |  |  |
| min(x) | int |  | 0.83 | 0.50 | 0.34 |  |  |
| min(x) | float |  | 0.83 | 0.49 | 0.32 |  |  |
| min(x) | str |  | 0.83 | 0.60 | 0.52 |  |  |
| min(x) | object |  | 0.83 | 0.77 | 0.74 |  |  |
| max(x) | int |  | 0.83 | 0.50 | 0.34 |  |  |
| max(x) | float |  | 0.83 | 0.49 | 0.32 |  |  |
| max(x) | str |  | 0.83 | 0.60 | 0.52 |  |  |
| max(x) | object |  | 0.83 | 0.76 | 0.74 |  |  |
| any(x) | int | 1.03 | 1.03 | 1.03 | 1.03 |  |  |
| any(x) | float | 1.03 | 1.03 | 1.03 | 1.03 |  |  |
| any(x) | str | 1.03 | 1.03 | 1.03 | 1.03 |  |  |
| any(x) | mixed | 1.03 | 1.03 | 1.03 | 1.03 |  |  |
| any(x) | object | 1.03 | 1.03 | 1.03 | 1.03 |  |  |
| all(x) | int | 0.71 | 0.70 | 0.64 | 0.59 |  |  |
| all(x) | float | 0.71 | 0.70 | 0.65 | 0.61 |  |  |
| all(x) | str | 0.71 | 0.70 | 0.64 | 0.60 |  |  |
| all(x) | mixed | 0.71 | 0.70 | 0.64 | 0.60 |  |  |
| all(x) | object | 0.71 | 0.71 | 0.66 | 0.63 |  |  |
| popitem() loop | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| popitem() loop | float | 0.98 | 1.00 | 1.00 | 1.00 |  |  |
| popitem() loop | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| popitem() loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| popitem() loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| del x[k] loop | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| del x[k] loop | float | 0.98 | 1.00 | 1.00 | 1.00 |  |  |
| del x[k] loop | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| del x[k] loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| del x[k] loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | float | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.clear() | int | 1.00 | 1.03 | 1.00 | 1.00 |  |  |
| x.clear() | float | 0.99 | 1.03 | 1.00 | 1.00 |  |  |
| x.clear() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.clear() | mixed | 1.00 | 1.05 | 0.99 | 1.00 |  |  |
| x.clear() | object | 1.00 | 1.03 | 1.00 | 1.00 |  |  |
| repr(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | float | 0.99 | 0.51 | 0.35 | 0.31 |  |  |
| repr(x) | str | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | mixed | 1.00 | 1.00 | 0.55 | 0.53 |  |  |
| repr(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| json.dumps | int | 0.08 | 0.12 | 0.20 | 0.32 |  |  |
| json.dumps | float | 0.08 | 0.11 | 0.14 | 0.16 |  |  |
| json.dumps | str | 0.08 | 0.12 | 0.22 | 0.42 |  |  |
| pickle.dumps | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | str | 1.00 | 1.00 | 1.02 | 1.00 |  |  |
| pickle.dumps | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | object | 1.00 | 1.00 | 1.01 | 1.01 |  |  |
| pickle round trip | int | 1.00 | 1.00 | 1.01 | 1.00 |  |  |
| pickle round trip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy.deepcopy | int | 0.22 | 0.22 | 0.23 | 0.25 |  |  |
| copy.deepcopy | float | 0.22 | 0.23 | 0.25 | 0.28 |  |  |
| copy.deepcopy | str | 0.22 | 0.22 | 0.22 | 0.24 |  |  |
| copy.deepcopy | mixed | 0.22 | 0.22 | 0.24 | 0.26 |  |  |
| copy.deepcopy | object | 0.22 | 0.16 | 0.12 | 0.12 |  |  |

### set — 3.15.1t  (743/743 cells at parity)

| op | storage | 0 | 1 | 8 | 100 | 10k | 1M |
|---|---|---:|---:|---:|---:|---:|---:|
| literal/display | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| comprehension | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| add loop | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| add loop | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| add loop | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| add loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| add loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | int |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | float |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | str |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | object |  | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| sorted(x) | int | 0.98 | 0.99 | 0.79 | 0.57 |  |  |
| sorted(x) | float | 0.98 | 0.99 | 0.78 | 0.62 |  |  |
| sorted(x) | str | 0.98 | 0.99 | 0.93 | 0.77 |  |  |
| sorted(x) | object | 0.98 | 0.99 | 1.03 | 1.00 |  |  |
| sum(x) | int | 0.82 | 0.77 | 0.70 | 0.71 |  |  |
| sum(x) | float | 0.82 | 0.86 | 0.74 | 0.71 |  |  |
| min(x) | int |  | 0.78 | 0.43 | 0.33 |  |  |
| min(x) | float |  | 0.77 | 0.43 | 0.32 |  |  |
| min(x) | str |  | 0.77 | 0.56 | 0.52 |  |  |
| min(x) | object |  | 0.78 | 0.73 | 0.74 |  |  |
| max(x) | int |  | 0.78 | 0.44 | 0.33 |  |  |
| max(x) | float |  | 0.78 | 0.42 | 0.32 |  |  |
| max(x) | str |  | 0.78 | 0.56 | 0.51 |  |  |
| max(x) | object |  | 0.78 | 0.74 | 0.74 |  |  |
| any(x) | int | 0.74 | 0.69 | 0.71 | 0.72 |  |  |
| any(x) | float | 0.74 | 0.69 | 0.71 | 0.73 |  |  |
| any(x) | str | 0.74 | 0.69 | 0.69 | 0.78 |  |  |
| any(x) | mixed | 0.74 | 0.74 | 0.73 | 0.68 |  |  |
| any(x) | object | 0.74 | 0.72 | 0.70 | 0.72 |  |  |
| all(x) | int | 0.74 | 0.66 | 0.54 | 0.54 |  |  |
| all(x) | float | 0.74 | 0.66 | 0.55 | 0.55 |  |  |
| all(x) | str | 0.74 | 0.66 | 0.55 | 0.55 |  |  |
| all(x) | mixed | 0.73 | 0.67 | 0.55 | 0.55 |  |  |
| all(x) | object | 0.73 | 0.67 | 0.57 | 0.57 |  |  |
| pop() loop | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pop() loop | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pop() loop | str | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| pop() loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pop() loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| discard loop | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| discard loop | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| discard loop | str | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| discard loop | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| discard loop | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | str | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| x.update(t) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.update(t) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.clear() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.clear() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.clear() | str | 1.00 | 0.99 | 1.00 | 1.00 |  |  |
| x.clear() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.clear() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | float | 1.00 | 0.61 | 0.32 | 0.23 |  |  |
| repr(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | mixed | 1.00 | 1.00 | 0.52 | 0.43 |  |  |
| repr(x) | object | 1.00 | 1.00 | 1.00 | 0.98 |  |  |
| pickle.dumps | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | str | 1.00 | 1.00 | 0.98 | 1.00 |  |  |
| pickle.dumps | mixed | 1.00 | 1.00 | 1.00 | 1.01 |  |  |
| pickle.dumps | object | 1.00 | 1.00 | 0.98 | 0.99 |  |  |
| pickle round trip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | object | 1.00 | 0.99 | 1.01 | 1.00 |  |  |
| copy.deepcopy | int | 0.18 | 0.20 | 0.23 | 0.33 |  |  |
| copy.deepcopy | float | 0.18 | 0.20 | 0.25 | 0.37 |  |  |
| copy.deepcopy | str | 0.18 | 0.20 | 0.23 | 0.31 |  |  |
| copy.deepcopy | mixed | 0.18 | 0.20 | 0.24 | 0.34 |  |  |
| copy.deepcopy | object | 0.18 | 0.17 | 0.14 | 0.12 |  |  |

### frozenset — 3.15.1t  (643/643 cells at parity)

| op | storage | 0 | 1 | 8 | 100 | 10k | 1M |
|---|---|---:|---:|---:|---:|---:|---:|
| literal/display | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| literal/display | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| genexpr ingest | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| from list | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| copy C(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.copy() | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| len(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| bool(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | int |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | float |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | str |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | mixed |  | 1.00 | 1.00 | 1.00 |  |  |
| hit in x | object |  | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| miss in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| g(*x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | float | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| for v in x | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | float | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| list(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | float | 0.99 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| tuple(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| enumerate | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| zip | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == y | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x == t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | float | 1.00 | 0.99 | 0.99 | 0.99 |  |  |
| hash(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| hash(x) | mixed | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| hash(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x \| t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x & t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x - t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x ^ t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x <= t | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| x.isdisjoint(t) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| sorted(x) | int | 0.98 | 0.99 | 0.79 | 0.57 |  |  |
| sorted(x) | float | 0.98 | 0.99 | 0.78 | 0.62 |  |  |
| sorted(x) | str | 0.98 | 0.99 | 0.93 | 0.77 |  |  |
| sorted(x) | object | 0.98 | 0.99 | 1.03 | 1.00 |  |  |
| sum(x) | int | 0.76 | 0.73 | 0.67 | 0.71 |  |  |
| sum(x) | float | 0.76 | 0.82 | 0.72 | 0.70 |  |  |
| min(x) | int |  | 0.73 | 0.42 | 0.33 |  |  |
| min(x) | float |  | 0.73 | 0.41 | 0.32 |  |  |
| min(x) | str |  | 0.73 | 0.55 | 0.51 |  |  |
| min(x) | object |  | 0.73 | 0.72 | 0.74 |  |  |
| max(x) | int |  | 0.74 | 0.42 | 0.33 |  |  |
| max(x) | float |  | 0.73 | 0.40 | 0.31 |  |  |
| max(x) | str |  | 0.73 | 0.55 | 0.51 |  |  |
| max(x) | object |  | 0.73 | 0.73 | 0.74 |  |  |
| any(x) | int | 0.68 | 0.63 | 0.65 | 0.66 |  |  |
| any(x) | float | 0.68 | 0.63 | 0.64 | 0.67 |  |  |
| any(x) | str | 0.68 | 0.63 | 0.62 | 0.72 |  |  |
| any(x) | mixed | 0.68 | 0.67 | 0.67 | 0.61 |  |  |
| any(x) | object | 0.68 | 0.66 | 0.64 | 0.65 |  |  |
| all(x) | int | 0.67 | 0.60 | 0.51 | 0.52 |  |  |
| all(x) | float | 0.67 | 0.61 | 0.52 | 0.53 |  |  |
| all(x) | str | 0.67 | 0.60 | 0.52 | 0.53 |  |  |
| all(x) | mixed | 0.67 | 0.61 | 0.51 | 0.53 |  |  |
| all(x) | object | 0.67 | 0.61 | 0.53 | 0.56 |  |  |
| repr(x) | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | float | 1.00 | 0.62 | 0.33 | 0.23 |  |  |
| repr(x) | str | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| repr(x) | mixed | 1.00 | 1.00 | 0.52 | 0.43 |  |  |
| repr(x) | object | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| pickle.dumps | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle.dumps | object | 1.00 | 0.99 | 0.98 | 0.99 |  |  |
| pickle round trip | int | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | float | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | str | 1.00 | 1.00 | 0.99 | 1.00 |  |  |
| pickle round trip | mixed | 1.00 | 1.00 | 1.00 | 1.00 |  |  |
| pickle round trip | object | 1.00 | 1.01 | 1.00 | 1.00 |  |  |
| copy.deepcopy | int | 0.18 | 0.20 | 0.24 | 0.34 |  |  |
| copy.deepcopy | float | 0.18 | 0.20 | 0.25 | 0.39 |  |  |
| copy.deepcopy | str | 0.18 | 0.20 | 0.23 | 0.33 |  |  |
| copy.deepcopy | mixed | 0.18 | 0.20 | 0.24 | 0.36 |  |  |
| copy.deepcopy | object | 0.18 | 0.17 | 0.14 | 0.13 |  |  |

