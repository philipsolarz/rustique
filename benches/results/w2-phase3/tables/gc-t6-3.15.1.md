### T6, 3.15.1: tier vs default

workload | GC % | GC work × | est × | instr × | wall × (paired) | largest coll. × | max pause × | peak RSS ×
---|---|---|---|---|---|---|---|---
data/billing_engine | 33.4 | 0.79 | 0.931 |  | 0.880 | 1.02 | 1.01 | 1.017
data/state_snapshot | 12.4 | 1.04 | 1.005 |  | 1.016 | 1.00 | 1.30 | 1.016
data/xml_feeds | 14.6 | 1.00 | 1.000 |  | 0.991 | 1.00 | 0.98 | 1.022
devtools/black_format | 11.2 | 1.00 | 1.000 |  | 0.993 | 1.00 | 0.96 | 1.032
devtools/coverage_pyflakes_tests | 14.4 | 1.00 | 1.000 |  | 0.991 | 1.00 | 0.98 | 1.032
devtools/pyflakes_stdlib | 22.6 | 1.00 | 1.000 |  | 0.990 | 1.00 | 1.01 | 1.028
devtools/pylint_stdlib | 39.8 | 1.00 | 1.000 |  | 1.003 | 1.00 | 0.98 | 1.012
libs/bs4_scrape | 11.3 | 1.00 | 1.000 |  | 1.003 | 1.00 | 0.64 | 1.029

GC > 10%: 8 workloads; geomean wall 0.982, est 0.992; max RSS 1.032; max largest-collection 1.02
others: 53 workloads; worst wall 1.021 (libs/pydantic_payloads), worst est 1.000, max RSS 1.114
