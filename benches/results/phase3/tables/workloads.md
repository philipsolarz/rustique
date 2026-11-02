| workload | 3.15.1 plain ms | 3.15.1 enabled ms | 3.15.1 ratio | 3.15.1t plain ms | 3.15.1t enabled ms | 3.15.1t ratio |
|---|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 599.9 | 597.6 | 0.996 | 607.0 | 606.3 | 0.999 |
| data/billing_engine | 918.1 | 890.4 | 0.970 | 758.1 | 738.9 | 0.975 |
| data/config_merge | 536.8 | 543.6 | 1.013 | 607.3 | 607.9 | 1.001 |
| data/csv_etl | 388.1 | 242.4 | 0.625 | 432.5 | 276.9 | 0.640 |
| data/fs_inventory | 315.3 | 234.2 | 0.743 | 320.6 | 235.6 | 0.735 |
| data/log_analytics | 766.7 | 455.1 | 0.594 | 807.1 | 491.2 | 0.609 |
| data/pandas_etl | 530.6 | 530.8 | 1.000 | 592.5 | 592.5 | 1.000 |
| data/report_render | 405.6 | 176.6 | 0.436 | 464.3 | 196.3 | 0.423 |
| data/state_snapshot | 648.1 | 422.0 | 0.651 | 1248.7 | 593.8 | 0.476 |
| data/text_corpus | 604.5 | 604.5 | 1.000 | 631.7 | 632.0 | 1.001 |
| data/tz_events | 525.7 | 310.8 | 0.591 | 614.0 | 377.3 | 0.615 |
| data/xml_feeds | 505.8 | 505.7 | 1.000 | 569.6 | 572.6 | 1.005 |
| devtools/black_format | 1197.9 | 1186.2 | 0.990 | 1293.1 | 1292.0 | 0.999 |
| devtools/coverage_pyflakes_tests | 382.2 | 380.1 | 0.995 | 494.5 | 493.9 | 0.999 |
| devtools/import_startup | 1899.3 | 1898.6 | 1.000 | 2246.2 | 2248.2 | 1.001 |
| devtools/isort_stdlib | 283.7 | 218.8 | 0.771 | 1187.4 | 232.8 | 0.196 |
| devtools/pip_resolve | 562.0 | 533.6 | 0.949 | 624.9 | 591.8 | 0.947 |
| devtools/pycodestyle_stdlib | 1012.7 | 997.0 | 0.985 | 1067.9 | 1067.0 | 0.999 |
| devtools/pyflakes_stdlib | 1211.5 | 1208.2 | 0.997 | 1194.4 | 1200.1 | 1.005 |
| devtools/pylint_stdlib | 1181.5 | 1184.2 | 1.002 | 899.0 | 898.1 | 0.999 |
| devtools/pytest_networkx | 632.3 | 599.6 | 0.948 | 632.7 | 603.7 | 0.954 |
| devtools/rich_render | 285.0 | 284.4 | 0.998 | 310.3 | 310.3 | 1.000 |
| devtools/sphinx_autodoc | 858.9 | 839.3 | 0.977 | 898.9 | 888.2 | 0.988 |
| devtools/sqlglot_transpile | 879.4 | 851.4 | 0.968 | 1030.0 | 1003.2 | 0.974 |
| web/aiohttp_app | 270.9 | 246.7 | 0.910 | 301.7 | 279.1 | 0.925 |
| web/asyncio_http | 230.9 | 182.6 | 0.791 | 240.5 | 196.0 | 0.815 |
| web/django_blog | 487.3 | 485.6 | 0.997 | 544.6 | 541.7 | 0.995 |
| web/email_mime | 654.5 | 651.4 | 0.995 | 708.8 | 709.8 | 1.001 |
| web/fastapi_pydantic | 296.2 | 291.1 | 0.983 | 338.8 | 334.0 | 0.986 |
| web/flask_jinja | 419.3 | 415.0 | 0.990 | 463.5 | 457.2 | 0.986 |
| web/json_api | 396.8 | 331.3 | 0.835 | 442.0 | 369.2 | 0.835 |
| web/logging_service | 283.5 | 231.6 | 0.817 | 300.8 | 259.8 | 0.864 |
| web/requests_local | 250.3 | 235.5 | 0.941 | 251.3 | 237.2 | 0.944 |
| web/sqlalchemy_orm | 343.6 | 342.4 | 0.997 | 352.7 | 354.3 | 1.005 |

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| geomean data | 0.773 (12) | 0.756 (12) |
| geomean devtools | 0.963 (12) | 0.863 (12) |
| geomean web | 0.922 (10) | 0.933 (10) |
| **geomean all** | **0.880** (34) | **0.843** (34) |
| median | 0.980 | 0.986 |
| best | data/report_render 0.436 | devtools/isort_stdlib 0.196 |
| worst | data/config_merge 1.013 | data/xml_feeds 1.005 |

3.15.1: 34 workloads, digest mismatches: none; ≤ 0.95: 14, > 1.05: []

3.15.1t: 34 workloads, digest mismatches: none; ≤ 0.95: 13, > 1.05: []
