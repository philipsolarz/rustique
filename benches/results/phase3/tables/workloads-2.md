| workload | 3.15.1 plain ms | 3.15.1 enabled ms | 3.15.1 ratio | 3.15.1t plain ms | 3.15.1t enabled ms | 3.15.1t ratio |
|---|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 600.2 | 597.6 | 0.996 | 606.2 | 605.9 | 0.999 |
| data/billing_engine | 909.1 | 883.7 | 0.972 | 755.1 | 733.3 | 0.971 |
| data/config_merge | 537.6 | 539.1 | 1.003 | 610.2 | 605.8 | 0.993 |
| data/csv_etl | 390.3 | 242.1 | 0.620 | 430.9 | 277.7 | 0.645 |
| data/fs_inventory | 316.1 | 234.5 | 0.742 | 320.9 | 235.6 | 0.734 |
| data/log_analytics | 772.4 | 457.0 | 0.592 | 806.7 | 491.7 | 0.610 |
| data/pandas_etl | 529.7 | 529.8 | 1.000 | 591.5 | 590.5 | 0.998 |
| data/report_render | 405.7 | 177.6 | 0.438 | 464.7 | 195.5 | 0.421 |
| data/state_snapshot | 643.3 | 421.4 | 0.655 | 1241.6 | 590.3 | 0.475 |
| data/text_corpus | 610.1 | 604.7 | 0.991 | 631.6 | 631.7 | 1.000 |
| data/tz_events | 529.4 | 308.5 | 0.583 | 613.2 | 376.7 | 0.614 |
| data/xml_feeds | 504.9 | 508.7 | 1.007 | 568.8 | 573.2 | 1.008 |
| devtools/black_format | 1191.8 | 1186.6 | 0.996 | 1290.7 | 1294.1 | 1.003 |
| devtools/coverage_pyflakes_tests | 379.7 | 375.6 | 0.989 | 493.4 | 496.4 | 1.006 |
| devtools/import_startup | 1891.0 | 1894.6 | 1.002 | 2246.8 | 2244.7 | 0.999 |
| devtools/isort_stdlib | 284.5 | 218.0 | 0.766 | 1185.3 | 233.1 | 0.197 |
| devtools/pip_resolve | 562.3 | 537.3 | 0.955 | 625.4 | 593.9 | 0.950 |
| devtools/pycodestyle_stdlib | 1012.3 | 1007.2 | 0.995 | 1064.3 | 1063.4 | 0.999 |
| devtools/pyflakes_stdlib | 1207.7 | 1208.6 | 1.001 | 1197.8 | 1201.6 | 1.003 |
| devtools/pylint_stdlib | 1169.8 | 1175.7 | 1.005 | 897.4 | 898.2 | 1.001 |
| devtools/pytest_networkx | 628.9 | 602.6 | 0.958 | 630.8 | 601.6 | 0.954 |
| devtools/rich_render | 286.7 | 284.5 | 0.992 | 312.3 | 309.8 | 0.992 |
| devtools/sphinx_autodoc | 849.0 | 839.6 | 0.989 | 897.7 | 887.2 | 0.988 |
| devtools/sqlglot_transpile | 873.7 | 848.9 | 0.972 | 1031.8 | 1004.5 | 0.974 |
| web/aiohttp_app | 272.2 | 246.3 | 0.905 | 302.8 | 281.3 | 0.929 |
| web/asyncio_http | 229.9 | 182.4 | 0.793 | 241.1 | 196.4 | 0.815 |
| web/django_blog | 487.4 | 487.0 | 0.999 | 546.3 | 544.9 | 0.997 |
| web/email_mime | 656.4 | 648.4 | 0.988 | 708.3 | 714.4 | 1.009 |
| web/fastapi_pydantic | 296.6 | 289.7 | 0.977 | 338.1 | 334.2 | 0.989 |
| web/flask_jinja | 417.6 | 413.3 | 0.990 | 461.4 | 457.9 | 0.992 |
| web/json_api | 394.7 | 331.2 | 0.839 | 443.3 | 370.4 | 0.835 |
| web/logging_service | 284.8 | 231.2 | 0.812 | 304.4 | 259.6 | 0.853 |
| web/requests_local | 251.1 | 232.9 | 0.927 | 251.2 | 236.9 | 0.943 |
| web/sqlalchemy_orm | 343.9 | 342.7 | 0.996 | 354.5 | 357.0 | 1.007 |

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| geomean data | 0.771 (12) | 0.755 (12) |
| geomean devtools | 0.966 (12) | 0.863 (12) |
| geomean web | 0.919 (10) | 0.934 (10) |
| **geomean all** | **0.879** (34) | **0.843** (34) |
| median | 0.982 | 0.988 |
| best | data/report_render 0.438 | devtools/isort_stdlib 0.197 |
| worst | data/xml_feeds 1.007 | web/email_mime 1.009 |

3.15.1: 34 workloads, digest mismatches: none; ≤ 0.95: 12, > 1.05: []

3.15.1t: 34 workloads, digest mismatches: none; ≤ 0.95: 13, > 1.05: []
