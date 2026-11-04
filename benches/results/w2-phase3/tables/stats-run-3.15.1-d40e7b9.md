
### w2-phase3/workloads/stats-run-3.15.1

| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |
|---|---|---|
| custom/di_plugins | tuple.__contains__ 408,188 (fb args 197,364); enum.property.__get__ 190,387; enum.EnumType.__call__ 68,171; +4 more | 17 / 53 / 1 |
| custom/ecs_arena | dataclasses.__init__ 468,927; list.sort 187,589 (fb args 1,600); builtins.max 16,501; +8 more | 18 / 53 / 1 |
| custom/event_ledger | functools.singledispatch 1,001,652; functools.singledispatchmethod.__get__ 808,764; dataclasses.__init__ 184,239; +9 more | 24 / 51 / 1 |
| custom/fulfilment_sim | random.lognormvariate 81,730; tuple.__contains__ 74,532; random.expovariate 74,127; +7 more | 12 / 57 / 1 |
| custom/mini_interp | tuple.__contains__ 9,539; collections.namedtuple 1,984; re.sub 0 (fb args 28); +2 more | 16 / 57 / 1 |
| custom/order_domain | enum.Flag.operators 92,515; enum.property.__get__ 89,877; dataclasses.__init__ 82,237; +8 more | 23 / 44 / 1 |
| custom/orm_mapper | enum.EnumType.__call__ 105,100; datetime.date.isoformat 42,692; enum.property.__get__ 26,648; +5 more | 17 / 46 / 1 |
| custom/report_engine | dataclasses.__init__ 921,306; tuple.__contains__ 107,393; datetime.date.strftime 71,768; +8 more | 23 / 44 / 1 |
| custom/rules_engine | contextlib.contextmanager 329,053; dataclasses.__init__ 264,240; enum.Flag.operators 168,653; +9 more | 22 / 43 / 1 |
| custom/task_planner | collections.namedtuple 28,498; builtins.min 16,300 (fb deopt 12,800); builtins.max 13,105 (fb args 12,804, deopt 12,804); +5 more | 18 / 57 / 1 |
| data/backup_archive | tuple.__contains__ 3,136 (fb args 780); builtins.max 2,086; builtins.sum 1,040; +6 more | 20 / 39 / 1 |
| data/billing_engine | builtins.min 230,327; enum.Flag.operators 120,000; enum.EnumType.__call__ 80,000; +7 more | 24 / 48 / 1 |
| data/config_merge | datetime.datetime.isoformat 727; re.compile 241; json.loads 240; +4 more | 19 / 47 / 1 |
| data/csv_etl | tuple.__contains__ 40,000; json.dumps 32,690; datetime.date.strftime 32,689; +5 more | 21 / 45 / 1 |
| data/fs_inventory | fnmatch.fnmatch 69,193; tuple.__contains__ 47,092; posixpath.join 24,590; +12 more | 32 / 40 / 1 |
| data/log_analytics | datetime.datetime.strptime 45,000; datetime.date.strftime 45,000; urllib.parse._urlsplit 19,513; +7 more | 27 / 38 / 1 |
| data/pandas_etl | tuple.__contains__ 892 (fb args 34); re.search 84; datetime.date.strftime 19; +19 more | 40 / 5 / 1 |
| data/report_render | random.randint 60,000; list.count 40,000; random.sample 40,000; +12 more | 23 / 50 / 1 |
| data/state_snapshot | datetime.datetime.isoformat 3,924; enum.property.__get__ 1,840; float.repr 1,200; +10 more | 25 / 42 / 1 |
| data/text_corpus | builtins.min 55,395; re.sub 6,000; re.compile 2; +3 more | 17 / 57 / 1 |
| data/tz_events | datetime.date.strftime 122,593; builtins.max 87,192; datetime.datetime.isoformat 24,000; +10 more | 31 / 31 / 1 |
| data/xml_feeds | list.sort 73,832 (fb args 4); contextlib.contextmanager 2; tuple.__contains__ 1; +2 more | 17 / 35 / 1 |
| devtools/black_format | tuple.__contains__ 302,188 (fb args 13,442); list.__contains__ 167,883 (fb args 62); enum.Enum.__hash__ 49,064; +9 more | 33 / 12 / 1 |
| devtools/coverage_pyflakes_tests | tuple.__contains__ 18,122; builtins.min 2,378 (fb args 265); builtins.max 1,969 (fb args 17); +19 more | 34 / 12 / 1 |
| devtools/import_startup | posixpath.dirname 42; collections.namedtuple 42; list.sort 21; +3 more | 10 / 67 / 1 |
| devtools/isort_stdlib | tuple.__contains__ 226,383; copy.deepcopy 26,265 (fb deopt 1,030); list.__contains__ 11,768; +13 more | 36 / 22 / 1 |
| devtools/pip_resolve | tuple.__contains__ 24,459; posixpath.basename 19,180; builtins.all 12,680; +29 more | 55 / 5 / 1 |
| devtools/pycodestyle_stdlib | list.__contains__ 655,813; tuple.__contains__ 317,553 (fb deopt 106); collections.namedtuple 183,255; +6 more | 15 / 64 / 1 |
| devtools/pyflakes_stdlib | tuple.__contains__ 202,865; contextlib.contextmanager 50,409; list.__contains__ 516; +3 more | 15 / 67 / 1 |
| devtools/pylint_stdlib | tuple.__contains__ 53,605 (fb args 54); list.__contains__ 34,075; collections.namedtuple 16,609; +31 more | 44 / 11 / 1 |
| devtools/pytest_networkx | tuple.__contains__ 21,862 (fb args 36, deopt 1); pathlib.PurePath.__eq__ 19,434; pathlib.PurePath.__hash__ 17,743; +42 more | 58 / 5 / 1 |
| devtools/rich_render | collections.namedtuple 129,406; builtins.max 74,814 (fb args 7,283); builtins.min 44,212; +12 more | 29 / 5 / 1 |
| devtools/sphinx_autodoc | builtins.min 67,669; tuple.__contains__ 26,223 (fb args 1,621); collections.namedtuple 19,820; +38 more | 59 / 5 / 1 |
| devtools/sqlglot_transpile | copy.deepcopy 36,909 (fb deopt 10,361); tuple.__contains__ 8,533 (fb args 6,908); enum.Enum.__hash__ 3,588; +11 more | 28 / 15 / 1 |
| frameworks/django_admin | list.__contains__ 19,805 (fb args 1,976, deopt 3,046); contextlib.contextmanager 11,653; urllib.parse.quote_plus 8,836; +25 more | 55 / 0 / 1 |
| frameworks/drf_api | copy.deepcopy 19,004 (fb deopt 5,960); tuple.__contains__ 17,896 (fb args 5,320); collections.namedtuple 7,429; +20 more | 45 / 0 / 1 |
| frameworks/falcon_api | tuple.__contains__ 26,254; json.dumps 15,612; list.__contains__ 13,533; +9 more | 33 / 5 / 1 |
| frameworks/fastapi_sqlalchemy | tuple.__contains__ 28,247 (fb args 6,060, deopt 450); builtins.max 22,798; logging.Logger.level 16,334; +18 more | 48 / 5 / 1 |
| frameworks/flask_marshmallow | tuple.__contains__ 20,120 (fb args 2,640); list.__contains__ 12,617; urllib.parse.quote 6,480; +17 more | 50 / 5 / 1 |
| frameworks/strawberry_graphql | collections.namedtuple 326,849; dataclasses.__init__ 22,170; enum.EnumType.__call__ 12,819; +8 more | 29 / 15 / 1 |
| libs/attrs_cattrs | tuple.__contains__ 64,417 (fb args 28,092); datetime.datetime.isoformat 29,831; enum.property.__get__ 28,092; +9 more | 27 / 28 / 1 |
| libs/boto3_client | tuple.__contains__ 618,000; logging.Logger.level 39,840; list.__contains__ 30,220; +19 more | 42 / 5 / 1 |
| libs/bs4_scrape | tuple.__contains__ 379,076; list.__contains__ 40,460; list.sort 5,554 (fb args 2,924); +4 more | 23 / 17 / 1 |
| libs/jsonschema_validate | re.search 57,059; urllib.parse.unquote 46,222; list.count 3,000; +4 more | 34 / 15 / 1 |
| libs/pydantic_payloads | enum.property.__get__ 58,441; collections.namedtuple 49,299; copy.copy 28,800; +4 more | 37 / 18 / 1 |
| libs/yaml_config | builtins.sorted 560 (fb args 25,601); float.repr 320; datetime.datetime.isoformat 160; +5 more | 17 / 48 / 1 |
| services/cache_service | tuple.__contains__ 39,134 (fb args 9,856); builtins.max 2,897; builtins.sum 1,920 (fb args 25,662); +2 more | 30 / 5 / 1 |
| services/job_worker | tuple.__contains__ 43,400; json.dumps 25,464; logging.Logger.findCaller 12,264; +13 more | 35 / 12 / 1 |
| services/uvicorn_fastapi | tuple.__contains__ 31,040 (fb args 1,920); dataclasses.__init__ 12,177; copy.copy 6,400; +11 more | 47 / 5 / 1 |
| services/waitress_flask | tuple.__contains__ 4,950; logging.Logger.findCaller 3,479; logging.LogRecord.__init__ 3,479; +8 more | 48 / 5 / 1 |
| threads/compute_pool | builtins.min 7,679,072; tuple.__contains__ 615; builtins.sorted 300 (fb args 300); +3 more | 18 / 25 / 1 |
| threads/http_server | tuple.__contains__ 7,208 (fb args 34,800); json.dumps 3,600; builtins.min 3,309; +9 more | 22 / 38 / 1 |
| threads/pipeline_queue | urllib.parse.unquote 35,963; urllib.parse.parse_qsl 35,963; datetime.datetime.strptime 35,963; +7 more | 26 / 41 / 1 |
| threads/request_pool | tuple.__contains__ 24,039; json.dumps 12,000; urllib.parse.parse_qs 12,000; +11 more | 32 / 8 / 1 |
| threads/shared_cache | json.dumps 36,000; datetime.datetime.isoformat 36,000; tuple.__contains__ 29,051; +6 more | 22 / 21 / 1 |
| web/aiohttp_app | list.__contains__ 327,003; tuple.__contains__ 4,240; builtins.max 2,345; +7 more | 26 / 5 / 1 |
| web/asyncio_http | tuple.__contains__ 8,533 (fb args 14,507); json.loads 5,974 (fb deopt 852); json.dumps 5,974; +5 more | 21 / 37 / 1 |
| web/django_blog | contextlib.contextmanager 3,719; urllib.parse.quote 3,056; re.search 3,028; +19 more | 48 / 0 / 1 |
| web/email_mime | tuple.__contains__ 82,749; list.__contains__ 480; random.randrange 360; +9 more | 21 / 39 / 1 |
| web/fastapi_pydantic | copy.deepcopy 7,770 (fb deopt 1,915); tuple.__contains__ 6,480 (fb args 1,200, deopt 800); re.finditer 4,483; +18 more | 45 / 5 / 1 |
| web/flask_jinja | tuple.__contains__ 17,600; urllib.parse.quote 13,200; urllib.parse.quote_plus 4,080; +11 more | 44 / 5 / 1 |
| web/json_api | enum.property.__get__ 265,378; list.count 49,680; datetime.datetime.isoformat 25,130; +8 more | 21 / 42 / 1 |
| web/logging_service | logging.Formatter.formatTime 15,170; tuple.__contains__ 14,586; json.dumps 14,154; +9 more | 29 / 23 / 1 |
| web/requests_local | collections.namedtuple 7,620 (fb args 540); urllib.parse._urlsplit 6,000; list.__contains__ 5,940; +14 more | 37 / 9 / 1 |
| web/sqlalchemy_orm | tuple.__contains__ 6,211 (fb args 1,698); list.__contains__ 2,528; datetime.datetime.isoformat 1,500; +7 more | 31 / 8 / 1 |
