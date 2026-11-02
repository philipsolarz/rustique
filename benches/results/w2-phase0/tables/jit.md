| workload | no JIT ms | PYTHON_JIT=1 ms | ratio | digest |
|---|---:|---:|---:|---|
| data/backup_archive | 599.2 | 598.8 | 0.999 | same |
| data/billing_engine | 914.0 | 899.7 | 0.984 | same |
| data/config_merge | 538.8 | 545.7 | 1.013 | same |
| data/csv_etl | 392.8 | 389.4 | 0.991 | same |
| data/fs_inventory | 315.2 | 307.1 | 0.974 | same |
| data/log_analytics | 769.3 | 758.0 | 0.985 | same |
| data/pandas_etl | 535.0 | 530.0 | 0.991 | same |
| data/report_render | 404.8 | 388.2 | 0.959 | same |
| data/state_snapshot | 637.0 | 625.2 | 0.982 | same |
| data/text_corpus | 603.3 | 608.1 | 1.008 | same |
| data/tz_events | 528.6 | 532.2 | 1.007 | same |
| data/xml_feeds | 503.5 | 488.1 | 0.970 | same |
| devtools/black_format | 1196.0 | 1233.4 | 1.031 | same |
| devtools/coverage_pyflakes_tests | 383.0 | 376.8 | 0.984 | same |
| devtools/import_startup | 1881.9 | 1885.9 | 1.002 | same |
| devtools/isort_stdlib | 283.6 | 272.6 | 0.961 | same |
| devtools/pip_resolve | 564.3 | 573.2 | 1.016 | same |
| devtools/pycodestyle_stdlib | 1011.1 | 950.6 | 0.940 | same |
| devtools/pyflakes_stdlib | 1217.1 | 1221.4 | 1.004 | same |
| devtools/pylint_stdlib | 1184.8 | 1239.7 | 1.046 | same |
| devtools/pytest_networkx | 641.0 | 654.2 | 1.021 | same |
| devtools/rich_render | 283.9 | 294.3 | 1.037 | same |
| devtools/sphinx_autodoc | 847.8 | 842.7 | 0.994 | same |
| devtools/sqlglot_transpile | 862.5 | 913.1 | 1.059 | same |
| web/aiohttp_app | 269.8 | 283.6 | 1.051 | same |
| web/asyncio_http | 232.3 | 246.0 | 1.059 | same |
| web/django_blog | 489.5 | 517.9 | 1.058 | same |
| web/email_mime | 659.2 | 700.8 | 1.063 | same |
| web/fastapi_pydantic | 296.2 | 328.4 | 1.109 | same |
| web/flask_jinja | 419.2 | 436.3 | 1.041 | same |
| web/json_api | 396.3 | 434.7 | 1.097 | same |
| web/logging_service | 282.2 | 280.4 | 0.994 | same |
| web/requests_local | 250.2 | 270.9 | 1.083 | same |
| web/sqlalchemy_orm | 341.6 | 351.6 | 1.029 | same |

geomean data (12): 0.988
geomean devtools (12): 1.007
geomean web (10): 1.058
geomean all (34): 1.015; median 1.007; best devtools/pycodestyle_stdlib 0.940; worst web/fastapi_pydantic 1.109; > 1.05: 8; < 0.95: 1; JIT state as requested in every child: True
