### T6, 3.15.1t: tier vs default

workload | GC % | GC work × | est × | instr × | wall × (paired) | largest coll. × | max pause × | peak RSS ×
---|---|---|---|---|---|---|---|---
custom/rules_engine | 11.6 | 0.38 | 0.929 |  | 0.925 | 1.00 | 0.95 | 1.005
data/billing_engine | 14.4 | 0.50 | 0.928 |  | 0.908 | 1.06 | 0.96 | 1.013
data/state_snapshot | 10.3 | 0.26 | 0.924 |  | 0.945 | 1.00 | 0.99 | 1.022
devtools/coverage_pyflakes_tests | 27.0 | 1.00 | 1.000 |  | 0.998 | 1.00 | 1.00 | 1.026
devtools/pyflakes_stdlib | 16.5 | 1.00 | 1.000 |  | 0.999 | 1.00 | 1.01 | 1.038
devtools/pylint_stdlib | 13.2 | 1.00 | 1.000 |  | 1.006 | 1.00 | 1.00 | 1.010
devtools/sqlglot_transpile | 14.1 | 1.00 | 1.000 |  | 1.002 | 1.01 | 1.01 | 1.026
frameworks/strawberry_graphql | 16.6 | 1.00 | 1.000 |  | 0.997 | 1.00 | 0.95 | 1.030
threads/request_pool | 29.5 | 0.27 | 0.785 |  | 0.803 | 1.00 | 1.00 | 1.022

GC > 10%: 9 workloads; geomean wall 0.952, est 0.949; max RSS 1.038; max largest-collection 1.06
others: 52 workloads; worst wall 1.037 (web/fastapi_pydantic), worst est 1.003, max RSS 1.287
