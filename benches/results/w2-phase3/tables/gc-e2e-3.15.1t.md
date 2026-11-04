### T6, 3.15.1t: tier:all=1 vs rustique

workload | GC % | GC work × | est × | instr × | wall × (paired) | largest coll. × | max pause × | peak RSS ×
---|---|---|---|---|---|---|---|---
data/billing_engine | 14.8 | 0.50 | 0.926 |  | 0.911 | 1.06 | 1.13 | 1.000
data/state_snapshot | 14.4 | 0.40 | 0.915 |  | 0.944 | 1.00 | 1.08 | 1.020
devtools/coverage_pyflakes_tests | 26.8 | 1.00 | 0.999 |  | 0.994 | 1.00 | 1.00 | 1.002
devtools/pyflakes_stdlib | 16.6 | 1.00 | 1.000 |  | 0.999 | 1.00 | 1.01 | 0.995
devtools/pylint_stdlib | 13.4 | 1.00 | 1.000 |  | 0.997 | 1.00 | 0.99 | 0.999
frameworks/strawberry_graphql | 17.1 | 1.00 | 1.000 |  | 1.004 | 1.00 | 1.00 | 1.001
threads/request_pool | 34.1 | 0.18 | 0.722 |  | 0.780 | 1.01 | 1.02 | 1.013

GC > 10%: 7 workloads; geomean wall 0.944, est 0.932; max RSS 1.020; max largest-collection 1.06
others: 3 workloads; worst wall 0.998 (libs/bs4_scrape), worst est 1.000, max RSS 1.007
