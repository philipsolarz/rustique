### T6, 3.15.1: tier:all=1 vs rustique

workload | GC % | GC work × | est × | instr × | wall × (paired) | largest coll. × | max pause × | peak RSS ×
---|---|---|---|---|---|---|---|---
data/billing_engine | 34.5 | 0.79 | 0.929 |  | 0.870 | 1.02 | 1.01 | 0.999
data/state_snapshot | 10.4 | 1.00 | 1.000 |  | 1.018 | 1.00 | 1.05 | 0.998
data/xml_feeds | 14.2 | 1.00 | 1.000 |  | 0.998 | 1.00 | 1.01 | 1.002
devtools/black_format | 11.3 | 1.00 | 1.000 |  | 0.999 | 1.00 | 0.99 | 0.995
devtools/coverage_pyflakes_tests | 14.4 | 1.00 | 1.000 |  | 1.003 | 1.00 | 0.97 | 1.000
devtools/pyflakes_stdlib | 22.7 | 1.00 | 1.000 |  | 1.002 | 1.00 | 1.00 | 0.999
devtools/pylint_stdlib | 39.6 | 1.00 | 1.000 |  | 1.003 | 1.00 | 1.00 | 1.000
frameworks/strawberry_graphql | 10.3 | 1.00 | 1.000 |  | 1.006 | 1.00 | 1.01 | 1.001
libs/bs4_scrape | 10.5 | 1.00 | 1.000 |  | 1.005 | 1.00 | 1.00 | 0.998

GC > 10%: 9 workloads; geomean wall 0.988, est 0.992; max RSS 1.002; max largest-collection 1.02
others: 1 workloads; worst wall 0.989 (threads/request_pool), worst est 0.997, max RSS 0.979
