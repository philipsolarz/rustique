| workload | 3.15.1 plain ms | 3.15.1 enabled ms | 3.15.1 ratio | 3.15.1t plain ms | 3.15.1t enabled ms | 3.15.1t ratio |
|---|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 598.5 | 596.8 | 0.997 | 605.5 | 605.4 | 1.000 |
| data/billing_engine | 932.3 | 900.3 | 0.966 | 748.9 | 730.5 | 0.975 |
| data/config_merge | 540.6 | 541.3 | 1.001 | 611.7 | 605.9 | 0.990 |
| data/csv_etl | 393.0 | 242.9 | 0.618 | 428.8 | 276.4 | 0.644 |
| data/fs_inventory | 316.0 | 233.4 | 0.739 | 321.1 | 236.2 | 0.736 |
| data/log_analytics | 772.2 | 456.1 | 0.591 | 804.5 | 490.2 | 0.609 |
| data/pandas_etl | 528.2 | 527.3 | 0.998 | 590.6 | 587.7 | 0.995 |
| data/report_render | 406.9 | 178.1 | 0.438 | 463.5 | 196.0 | 0.423 |
| data/state_snapshot | 647.3 | 414.7 | 0.641 | 1229.5 | 582.3 | 0.474 |
| data/text_corpus | 604.3 | 603.5 | 0.999 | 628.1 | 632.7 | 1.007 |
| data/tz_events | 530.6 | 312.1 | 0.588 | 609.1 | 378.6 | 0.622 |
| data/xml_feeds | 506.0 | 500.9 | 0.990 | 571.6 | 572.2 | 1.001 |
| devtools/black_format | 1194.6 | 1190.1 | 0.996 | 1298.2 | 1289.4 | 0.993 |
| devtools/coverage_pyflakes_tests | 380.9 | 378.1 | 0.993 | 491.5 | 491.4 | 1.000 |
| devtools/import_startup | 1888.7 | 1889.1 | 1.000 | 2240.4 | 2243.0 | 1.001 |
| devtools/isort_stdlib | 284.9 | 219.2 | 0.769 | 1146.9 | 233.4 | 0.203 |
| devtools/pip_resolve | 563.0 | 530.7 | 0.943 | 623.7 | 593.5 | 0.952 |
| devtools/pycodestyle_stdlib | 1013.7 | 991.0 | 0.978 | 1065.7 | 1060.7 | 0.995 |
| devtools/pyflakes_stdlib | 1210.5 | 1203.1 | 0.994 | 1190.3 | 1202.4 | 1.010 |
| devtools/pylint_stdlib | 1184.4 | 1183.7 | 0.999 | 894.7 | 890.0 | 0.995 |
| devtools/pytest_networkx | 629.5 | 600.3 | 0.954 | 631.9 | 600.9 | 0.951 |
| devtools/rich_render | 284.8 | 283.0 | 0.994 | 309.2 | 308.1 | 0.997 |
| devtools/sphinx_autodoc | 848.9 | 838.5 | 0.988 | 903.3 | 884.9 | 0.980 |
| devtools/sqlglot_transpile | 878.0 | 844.7 | 0.962 | 1027.1 | 1002.1 | 0.976 |
| web/aiohttp_app | 273.3 | 247.5 | 0.906 | 300.6 | 280.1 | 0.932 |
| web/asyncio_http | 231.2 | 182.5 | 0.790 | 242.9 | 196.1 | 0.807 |
| web/django_blog | 489.1 | 487.1 | 0.996 | 544.9 | 542.2 | 0.995 |
| web/email_mime | 656.2 | 645.2 | 0.983 | 706.4 | 707.1 | 1.001 |
| web/fastapi_pydantic | 297.0 | 290.4 | 0.978 | 340.4 | 332.5 | 0.977 |
| web/flask_jinja | 417.3 | 416.0 | 0.997 | 460.2 | 457.3 | 0.994 |
| web/json_api | 396.1 | 330.6 | 0.835 | 440.0 | 369.3 | 0.839 |
| web/logging_service | 283.1 | 230.1 | 0.813 | 300.2 | 259.6 | 0.865 |
| web/requests_local | 250.5 | 232.9 | 0.930 | 250.1 | 236.0 | 0.944 |
| web/sqlalchemy_orm | 341.7 | 341.4 | 0.999 | 354.5 | 353.4 | 0.997 |

| | 3.15.1 | 3.15.1t |
|---|---:|---:|
| geomean data | 0.768 (12) | 0.756 (12) |
| geomean devtools | 0.962 (12) | 0.865 (12) |
| geomean web | 0.919 (10) | 0.932 (10) |
| **geomean all** | **0.877** (34) | **0.843** (34) |
| median | 0.978 | 0.978 |
| best | data/report_render 0.438 | devtools/isort_stdlib 0.203 |
| worst | data/config_merge 1.001 | devtools/pyflakes_stdlib 1.010 |

3.15.1: 34 workloads, digest mismatches: none; ≤ 0.95: 13, > 1.05: []

3.15.1t: 34 workloads, digest mismatches: none; ≤ 0.95: 12, > 1.05: []
