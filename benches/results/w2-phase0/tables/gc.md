3.15.1: gc enabled True, threshold [2000, 10, 10], extra callback keys ['candidates', 'duration']
3.15.1t: gc enabled True, threshold [2000, 10, 10], extra callback keys ['candidates', 'duration']

| workload | 3.15.1 collections (per gen) | 3.15.1 GC ms (share of run) | 3.15.1 collected | 3.15.1 peak RSS MiB | 3.15.1t collections (per gen) | 3.15.1t GC ms (share of run) | 3.15.1t collected | 3.15.1t peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| data/backup_archive | 0 (—) | 0.0 (0.0%) | 0 | 41 | 0 (—) | 0.0 (0.0%) | 0 | 49 |
| data/billing_engine | 375 (0:341/1:31/2:3) | 296.5 (33.8%) | 0 | 138 | 4 (0:4) | 104.6 (14.2%) | 0 | 147 |
| data/config_merge | 29 (0:27/1:2) | 7.9 (1.5%) | 18,018 | 36 | 5 (0:5) | 17.3 (2.9%) | 18,488 | 47 |
| data/csv_etl | 0 (—) | 0.0 (0.0%) | 0 | 48 | 0 (—) | 0.0 (0.0%) | 0 | 56 |
| data/fs_inventory | 8 (0:7/1:1) | 0.2 (0.1%) | 0 | 28 | 1 (0:1) | 2.8 (0.9%) | 10 | 31 |
| data/log_analytics | 1 (0:1) | 0.1 (0.0%) | 0 | 54 | 0 (—) | 0.0 (0.0%) | 0 | 66 |
| data/pandas_etl | 0 (—) | 0.0 (0.0%) | 0 | 256 | 0 (—) | 0.0 (0.0%) | 0 | 300 |
| data/report_render | 20 (0:19/1:1) | 2.8 (0.7%) | 0 | 49 | 3 (0:3) | 7.5 (1.7%) | 22 | 56 |
| data/state_snapshot | 103 (0:94/1:8/2:1) | 99.2 (15.4%) | 0 | 138 | 3 (0:3) | 112.2 (9.3%) | 0 | 164 |
| data/text_corpus | 7 (0:7) | 2.0 (0.3%) | 0 | 40 | 1 (0:1) | 2.4 (0.4%) | 22 | 42 |
| data/tz_events | 48 (0:44/1:4) | 0.6 (0.1%) | 0 | 55 | 3 (0:3) | 20.6 (3.5%) | 0 | 63 |
| data/xml_feeds | 72 (0:65/1:6/2:1) | 72.4 (14.2%) | 824 | 100 | 3 (0:3) | 52.4 (9.6%) | 887 | 109 |
| devtools/black_format | 158 (0:144/1:13/2:1) | 132.2 (11.1%) | 208,327 | 76 | 14 (0:14) | 114.1 (8.8%) | 256,697 | 61 |
| devtools/coverage_pyflakes_tests | 164 (0:149/1:14/2:1) | 47.6 (12.7%) | 286,837 | 63 | 17 (0:17) | 115.6 (24.6%) | 289,330 | 72 |
| devtools/import_startup | 0 (—) | 0.0 (0.0%) | 0 | 24 | 0 (—) | 0.0 (0.0%) | 0 | 27 |
| devtools/isort_stdlib | 1 (0:1) | 0.4 (0.2%) | 1,928 | 45 | 0 (—) | 0.0 (0.0%) | 0 | 54 |
| devtools/pip_resolve | 24 (0:22/1:2) | 10.0 (1.8%) | 13,709 | 72 | 1 (0:1) | 17.9 (2.9%) | 14,569 | 84 |
| devtools/pycodestyle_stdlib | 0 (—) | 0.0 (0.0%) | 0 | 30 | 0 (—) | 0.0 (0.0%) | 0 | 35 |
| devtools/pyflakes_stdlib | 125 (0:115/1:10) | 222.5 (19.3%) | 548,028 | 79 | 52 (0:52) | 199.6 (16.6%) | 792,692 | 61 |
| devtools/pylint_stdlib | 167 (0:152/1:14/2:1) | 174.3 (19.8%) | 103,740 | 123 | 3 (0:3) | 109.9 (12.4%) | 103,603 | 139 |
| devtools/pytest_networkx | 42 (0:34/1:2/2:6) | 199.3 (32.4%) | 14,254 | 131 | 6 (2:6) | 129.2 (22.0%) | 14,700 | 152 |
| devtools/rich_render | 36 (0:33/1:3) | 4.9 (1.7%) | 17 | 43 | 1 (0:1) | 4.9 (1.6%) | 26 | 53 |
| devtools/sphinx_autodoc | 70 (0:64/1:6) | 23.6 (2.8%) | 132,971 | 99 | 2 (0:2) | 49.2 (5.7%) | 82,308 | 125 |
| devtools/sqlglot_transpile | 156 (0:142/1:13/2:1) | 73.7 (8.4%) | 332,206 | 49 | 15 (0:15) | 137.4 (13.6%) | 324,168 | 57 |
| web/aiohttp_app | 1 (1:1) | 2.1 (0.8%) | 212 | 49 | 0 (—) | 0.0 (0.0%) | 0 | 57 |
| web/asyncio_http | 2 (0:2) | 0.2 (0.1%) | 0 | 32 | 0 (—) | 0.0 (0.0%) | 0 | 36 |
| web/django_blog | 17 (0:16/1:1) | 8.0 (1.6%) | 29,842 | 62 | 1 (0:1) | 17.7 (3.3%) | 30,745 | 81 |
| web/email_mime | 30 (0:28/1:2) | 14.9 (2.3%) | 71,352 | 34 | 15 (0:15) | 35.9 (5.1%) | 74,584 | 40 |
| web/fastapi_pydantic | 11 (0:10/1:1) | 4.1 (1.4%) | 13,800 | 64 | 0 (—) | 0.0 (0.0%) | 0 | 78 |
| web/flask_jinja | 7 (0:6/1:1) | 2.7 (0.7%) | 14,748 | 50 | 1 (0:1) | 9.3 (2.0%) | 21,917 | 62 |
| web/json_api | 20 (0:18/1:2) | 3.6 (0.9%) | 0 | 45 | 2 (0:2) | 6.1 (1.4%) | 156 | 53 |
| web/logging_service | 0 (—) | 0.0 (0.0%) | 0 | 33 | 0 (—) | 0.0 (0.0%) | 0 | 38 |
| web/requests_local | 0 (—) | 0.0 (0.0%) | 0 | 39 | 0 (—) | 0.0 (0.0%) | 0 | 47 |
| web/sqlalchemy_orm | 31 (0:28/1:3) | 10.7 (3.1%) | 2,008 | 119 | 0 (—) | 0.0 (0.0%) | 0 | 137 |

3.15.1: 34 workloads; GC time 1417 ms of 20334 ms run time (7.0%); median share 1.1%; top: data/billing_engine 33.8%, devtools/pytest_networkx 32.4%, devtools/pylint_stdlib 19.8%, devtools/pyflakes_stdlib 19.3%, data/state_snapshot 15.4%, data/xml_feeds 14.2%
  uncollectable: none; callback vs get_stats() collections agree: True

3.15.1t: 34 workloads; GC time 1266 ms of 23084 ms run time (5.5%); median share 1.6%; top: devtools/coverage_pyflakes_tests 24.6%, devtools/pytest_networkx 22.0%, devtools/pyflakes_stdlib 16.6%, data/billing_engine 14.2%, devtools/sqlglot_transpile 13.6%, devtools/pylint_stdlib 12.4%
  uncollectable: none; callback vs get_stats() collections agree: True
