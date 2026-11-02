
### workloads/stats-run-3.15.1

| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |
|---|---|---|
| data/backup_archive | tuple.__contains__ 3,136 (fb args 780); builtins.max 2,086; builtins.sum 1,040; +6 more | 49 / 35 / 0 |
| data/billing_engine | builtins.min 230,327; list.sort 1 (fb args 2); statistics.stdev 1; +3 more | 38 / 46 / 0 |
| data/config_merge | list.sort 2,123 (fb args 530); datetime.datetime.isoformat 727; json.loads 240; +3 more | 40 / 44 / 0 |
| data/csv_etl | tuple.__contains__ 40,000; json.dumps 32,690; datetime.date.strftime 32,689; +5 more | 43 / 41 / 0 |
| data/fs_inventory | fnmatch.fnmatch 69,193; tuple.__contains__ 47,092; posixpath.join 24,590; +12 more | 47 / 37 / 0 |
| data/log_analytics | datetime.datetime.strptime 45,000; datetime.date.strftime 45,000; urllib.parse._urlsplit 19,513; +5 more | 50 / 34 / 0 |
| data/pandas_etl | tuple.__contains__ 866 (fb args 55, deopt 5); datetime.date.strftime 19; list.__contains__ 17; +11 more | 79 / 5 / 0 |
| data/report_render | random.randint 60,000; list.count 40,000; random.sample 40,000; +11 more | 38 / 46 / 0 |
| data/state_snapshot | datetime.datetime.isoformat 3,924; list.sort 1,200; float.repr 1,200; +7 more | 44 / 40 / 0 |
| data/text_corpus | builtins.min 55,395; list.sort 0 (fb args 3,003); builtins.sorted 0 (fb args 3,001); +1 more | 31 / 53 / 0 |
| data/tz_events | datetime.date.strftime 122,593; builtins.max 87,192; datetime.datetime.isoformat 24,000; +9 more | 56 / 28 / 0 |
| data/xml_feeds | list.sort 73,832 (fb args 4); tuple.__contains__ 1; builtins.sorted 0 (fb args 73,836); +1 more | 52 / 32 / 0 |
| devtools/black_format | tuple.__contains__ 302,188 (fb deopt 13,442); list.sort 24,488 (fb args 8); builtins.min 8,105; +5 more | 72 / 12 / 0 |
| devtools/coverage_pyflakes_tests | tuple.__contains__ 15,058 (fb args 3,064); builtins.min 2,378 (fb args 265); builtins.max 1,969 (fb args 17); +12 more | 72 / 12 / 0 |
| devtools/import_startup | posixpath.dirname 42; list.sort 21; list.count 0 (fb args 21); +1 more | 25 / 59 / 0 |
| devtools/isort_stdlib | tuple.__contains__ 226,383; copy.deepcopy 26,265 (fb deopt 1,030); list.__contains__ 11,768; +7 more | 68 / 16 / 0 |
| devtools/pip_resolve | tuple.__contains__ 24,423 (fb args 36); posixpath.basename 19,180; builtins.all 12,680; +25 more | 79 / 5 / 0 |
| devtools/pycodestyle_stdlib | list.__contains__ 655,813; tuple.__contains__ 317,434 (fb args 119, deopt 106); posixpath.join 2; +4 more | 28 / 56 / 0 |
| devtools/pyflakes_stdlib | tuple.__contains__ 135,002 (fb args 67,863); list.sort 493 (fb args 61); list.__contains__ 460 (fb args 56); +2 more | 25 / 59 / 0 |
| devtools/pylint_stdlib | tuple.__contains__ 30,909 (fb args 23,318); list.__contains__ 20,966 (fb args 13,069); list.index 3,658 (fb args 2,213); +26 more | 74 / 10 / 0 |
| devtools/pytest_networkx | tuple.__contains__ 21,337 (fb args 561, deopt 1); pathlib.PurePath.__eq__ 19,518; pathlib.PurePath.__hash__ 17,748; +31 more | 79 / 5 / 0 |
| devtools/rich_render | builtins.max 74,814 (fb args 7,283); builtins.min 44,212; tuple.__contains__ 14,722 (fb deopt 100); +7 more | 79 / 5 / 0 |
| devtools/sphinx_autodoc | builtins.min 67,669; tuple.__contains__ 26,138 (fb args 85, deopt 1,621); list.__contains__ 11,738 (fb args 123); +30 more | 79 / 5 / 0 |
| devtools/sqlglot_transpile | copy.deepcopy 36,909 (fb deopt 10,361); list.sort 12,931 (fb args 112); tuple.__contains__ 5,418 (fb args 3,115, deopt 6,908); +7 more | 69 / 15 / 0 |
| web/aiohttp_app | list.__contains__ 327,003; tuple.__contains__ 4,240; builtins.max 2,345; +4 more | 79 / 5 / 0 |
| web/asyncio_http | tuple.__contains__ 8,533 (fb args 14,507); json.loads 5,974 (fb deopt 852); json.dumps 5,974; +3 more | 52 / 32 / 0 |
| web/django_blog | urllib.parse.quote 3,056; datetime.datetime.isoformat 1,604; tuple.__contains__ 1,440 (fb args 2,392, deopt 2,702); +13 more | 84 / 0 / 0 |
| web/email_mime | tuple.__contains__ 82,749; list.__contains__ 480; random.randrange 360; +8 more | 49 / 35 / 0 |
| web/fastapi_pydantic | copy.deepcopy 7,770 (fb deopt 1,915); tuple.__contains__ 6,320 (fb args 1,360, deopt 800); builtins.max 3,206; +12 more | 79 / 5 / 0 |
| web/flask_jinja | tuple.__contains__ 17,600; urllib.parse.quote 13,200; urllib.parse.quote_plus 4,080; +8 more | 79 / 5 / 0 |
| web/json_api | list.count 49,680; copy.deepcopy 36,351 (fb deopt 5,173); datetime.datetime.isoformat 25,130; +7 more | 44 / 40 / 0 |
| web/logging_service | logging.Formatter.formatTime 15,170; tuple.__contains__ 14,586; logging.Logger.findCaller 14,154; +8 more | 61 / 23 / 0 |
| web/requests_local | urllib.parse._urlsplit 6,000; list.__contains__ 5,940; tuple.__contains__ 4,263 (fb args 7,800); +11 more | 77 / 7 / 0 |
| web/sqlalchemy_orm | list.sort 2,604 (fb args 864); tuple.__contains__ 2,019 (fb args 5,890); datetime.datetime.isoformat 1,500; +3 more | 76 / 8 / 0 |

### workloads/stats-run-3.15.1t

| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |
|---|---|---|
| data/backup_archive | tuple.__contains__ 3,136 (fb args 780); builtins.max 2,086; builtins.sum 1,040; +6 more | 49 / 35 / 0 |
| data/billing_engine | builtins.min 230,327; list.sort 1 (fb args 2); statistics.stdev 1; +3 more | 38 / 46 / 0 |
| data/config_merge | list.sort 2,123 (fb args 530); datetime.datetime.isoformat 727; json.loads 240; +3 more | 40 / 44 / 0 |
| data/csv_etl | tuple.__contains__ 40,000; json.dumps 32,690; datetime.date.strftime 32,689; +5 more | 43 / 41 / 0 |
| data/fs_inventory | fnmatch.fnmatch 69,193; tuple.__contains__ 47,092; posixpath.join 24,590; +12 more | 47 / 37 / 0 |
| data/log_analytics | datetime.datetime.strptime 45,000; datetime.date.strftime 45,000; urllib.parse._urlsplit 19,513; +5 more | 50 / 34 / 0 |
| data/pandas_etl | tuple.__contains__ 866 (fb args 55, deopt 5); datetime.date.strftime 19; list.__contains__ 17; +11 more | 79 / 5 / 0 |
| data/report_render | random.randint 60,000; list.count 40,000; random.sample 40,000; +11 more | 38 / 46 / 0 |
| data/state_snapshot | datetime.datetime.isoformat 3,924; list.sort 1,200; float.repr 1,200; +7 more | 44 / 40 / 0 |
| data/text_corpus | builtins.min 55,395; list.sort 0 (fb args 3,003); builtins.sorted 0 (fb args 3,001); +1 more | 31 / 53 / 0 |
| data/tz_events | datetime.date.strftime 122,593; builtins.max 87,192; datetime.datetime.isoformat 24,000; +9 more | 56 / 28 / 0 |
| data/xml_feeds | list.sort 73,832 (fb args 4); tuple.__contains__ 1; builtins.sorted 0 (fb args 73,836); +1 more | 52 / 32 / 0 |
| devtools/black_format | tuple.__contains__ 302,188 (fb deopt 13,442); list.sort 24,488 (fb args 8); builtins.min 8,105; +5 more | 72 / 12 / 0 |
| devtools/coverage_pyflakes_tests | tuple.__contains__ 15,058 (fb args 3,064); builtins.min 2,378 (fb args 265); builtins.max 1,969 (fb args 17); +12 more | 72 / 12 / 0 |
| devtools/import_startup | posixpath.dirname 42; list.sort 21; list.count 0 (fb args 21); +1 more | 25 / 59 / 0 |
| devtools/isort_stdlib | tuple.__contains__ 226,383; copy.deepcopy 26,265 (fb deopt 1,030); list.__contains__ 11,768; +7 more | 68 / 16 / 0 |
| devtools/pip_resolve | tuple.__contains__ 24,423 (fb args 36); posixpath.basename 19,180; builtins.all 12,680; +25 more | 79 / 5 / 0 |
| devtools/pycodestyle_stdlib | list.__contains__ 655,813; tuple.__contains__ 317,434 (fb args 119, deopt 106); posixpath.join 2; +4 more | 28 / 56 / 0 |
| devtools/pyflakes_stdlib | tuple.__contains__ 135,002 (fb args 67,863); list.sort 493 (fb args 61); list.__contains__ 460 (fb args 56); +2 more | 25 / 59 / 0 |
| devtools/pylint_stdlib | tuple.__contains__ 30,903 (fb args 23,318); list.__contains__ 21,035 (fb args 13,085); list.index 3,658 (fb args 2,213); +26 more | 74 / 10 / 0 |
| devtools/pytest_networkx | tuple.__contains__ 21,337 (fb args 561, deopt 1); pathlib.PurePath.__eq__ 19,518; pathlib.PurePath.__hash__ 17,748; +31 more | 79 / 5 / 0 |
| devtools/rich_render | builtins.max 74,814 (fb args 7,283); builtins.min 44,212; tuple.__contains__ 14,722 (fb deopt 100); +7 more | 79 / 5 / 0 |
| devtools/sphinx_autodoc | builtins.min 67,669; tuple.__contains__ 26,138 (fb args 85, deopt 1,621); list.__contains__ 11,738 (fb args 123); +30 more | 79 / 5 / 0 |
| devtools/sqlglot_transpile | copy.deepcopy 36,909 (fb deopt 10,361); list.sort 12,931 (fb args 112); tuple.__contains__ 5,418 (fb args 3,115, deopt 6,908); +7 more | 69 / 15 / 0 |
| web/aiohttp_app | list.__contains__ 327,003; tuple.__contains__ 4,240; builtins.max 2,345; +4 more | 79 / 5 / 0 |
| web/asyncio_http | tuple.__contains__ 8,533 (fb args 14,507); json.loads 5,974 (fb deopt 852); json.dumps 5,974; +3 more | 52 / 32 / 0 |
| web/django_blog | urllib.parse.quote 3,056; datetime.datetime.isoformat 1,604; tuple.__contains__ 1,440 (fb args 2,392, deopt 2,702); +13 more | 84 / 0 / 0 |
| web/email_mime | tuple.__contains__ 82,749; list.__contains__ 480; random.randrange 360; +8 more | 49 / 35 / 0 |
| web/fastapi_pydantic | copy.deepcopy 7,770 (fb deopt 1,915); tuple.__contains__ 6,320 (fb args 1,360, deopt 800); builtins.max 3,206; +12 more | 79 / 5 / 0 |
| web/flask_jinja | tuple.__contains__ 17,600; urllib.parse.quote 13,200; urllib.parse.quote_plus 4,080; +8 more | 79 / 5 / 0 |
| web/json_api | list.count 49,680; copy.deepcopy 36,351 (fb deopt 5,173); datetime.datetime.isoformat 25,130; +7 more | 44 / 40 / 0 |
| web/logging_service | logging.Formatter.formatTime 15,170; tuple.__contains__ 14,586; logging.Logger.findCaller 14,154; +8 more | 61 / 23 / 0 |
| web/requests_local | urllib.parse._urlsplit 6,000; list.__contains__ 5,940; tuple.__contains__ 4,263 (fb args 7,800); +11 more | 77 / 7 / 0 |
| web/sqlalchemy_orm | list.sort 2,580 (fb args 792); tuple.__contains__ 1,921 (fb args 5,749); datetime.datetime.isoformat 1,500; +3 more | 76 / 8 / 0 |
