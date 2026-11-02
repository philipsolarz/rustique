
### workloads/stats-3.15.1

| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |
|---|---|---|
| data/backup_archive | random.randint 47,231; tuple.__contains__ 6,304 (fb args 1,560); builtins.max 4,172 (fb args 18); builtins.sum 2,080; builtins.min 1,048; +10 more | 54 / 30 / 0 |
| data/billing_engine | builtins.min 460,654; random.choice 379,963; random.randint 40,000; list.__contains__ 74; tuple.__contains__ 41 (fb args 37); +11 more | 41 / 43 / 0 |
| data/config_merge | random.randint 6,202; list.sort 4,246 (fb args 1,060); datetime.datetime.isoformat 1,454; random.choice 720; builtins.min 565; +12 more | 48 / 36 / 0 |
| data/csv_etl | random.choice 360,000; random.randint 320,000; tuple.__contains__ 80,010; json.dumps 65,381; datetime.date.strftime 65,378; +11 more | 48 / 36 / 0 |
| data/fs_inventory | fnmatch.fnmatch 138,386; tuple.__contains__ 94,192; posixpath.join 77,519; random.randint 34,020; random.choice 26,385; +20 more | 52 / 32 / 0 |
| data/log_analytics | random.choice 264,779; random.randint 150,977; datetime.date.strftime 90,062 (fb args 24); datetime.datetime.strptime 89,999 (fb deps 1); tuple.__contains__ 45,014 (fb args 10); +17 more | 55 / 29 / 0 |
| data/pandas_etl | random.randint 1,455,000; random.choice 1,205,000; random.randrange 480,000; builtins.min 9,112; tuple.__contains__ 3,469 (fb args 854, deopt 1,216); +20 more | 84 / 0 / 0 |
| data/report_render | random.randint 120,000; list.count 80,006; random.sample 80,000; random.choices 80,000; tuple.__contains__ 40,026 (fb args 4); +15 more | 46 / 38 / 0 |
| data/state_snapshot | random.randint 154,540; random.choice 48,000; random.sample 40,632; random.randrange 39,209; random.choices 30,882; +15 more | 52 / 32 / 0 |
| data/text_corpus | builtins.min 110,810; random.choice 82,040; random.randint 48,049; random.choices 45,049; tuple.__contains__ 10; +8 more | 39 / 45 / 0 |
| data/tz_events | datetime.date.strftime 245,248 (fb args 24); builtins.max 174,428 (fb args 57); random.choice 168,000; random.randint 72,000; datetime.datetime.isoformat 48,000; +20 more | 61 / 23 / 0 |
| data/xml_feeds | random.choice 230,000; list.sort 147,668 (fb args 18); random.randint 140,000; tuple.__contains__ 662 (fb args 51); builtins.min 236; +8 more | 57 / 27 / 0 |
| devtools/black_format | tuple.__contains__ 614,809 (fb args 532, deopt 27,667); list.sort 50,152 (fb args 367); builtins.min 17,236; builtins.max 15,349 (fb args 1,049); list.__contains__ 413 (fb args 335,851); +16 more | 77 / 7 / 0 |
| devtools/coverage_pyflakes_tests | tuple.__contains__ 31,204 (fb args 6,539, deopt 8); builtins.min 5,507 (fb args 530); builtins.max 4,006 (fb args 250); list.__contains__ 1,260 (fb args 17); posixpath.dirname 449; +18 more | 77 / 7 / 0 |
| devtools/import_startup | posixpath.dirname 85; list.sort 42; tuple.__contains__ 4; json.dumps 1; posixpath.join 1; +3 more | 39 / 45 / 0 |
| devtools/isort_stdlib | tuple.__contains__ 500,488 (fb args 237); copy.deepcopy 52,554 (fb deopt 2,060); list.__contains__ 23,607 (fb args 5); pathlib.Path.__new__ 2,235; pathlib.PurePath.__init__ 2,235; +20 more | 79 / 5 / 0 |
| devtools/pip_resolve | tuple.__contains__ 61,157 (fb args 1,411); posixpath.basename 38,366; builtins.max 31,690 (fb args 784, deopt 610); builtins.all 25,360; posixpath.splitext 17,471; +37 more | 84 / 0 / 0 |
| devtools/pycodestyle_stdlib | list.__contains__ 1,311,699; tuple.__contains__ 634,930 (fb args 252, deopt 212); builtins.min 518; builtins.max 80 (fb args 20); posixpath.join 33; +5 more | 39 / 45 / 0 |
| devtools/pyflakes_stdlib | tuple.__contains__ 270,036 (fb args 135,740); list.sort 987 (fb args 122); list.__contains__ 977 (fb args 112); posixpath.join 67; builtins.min 58; +6 more | 39 / 45 / 0 |
| devtools/pylint_stdlib | tuple.__contains__ 62,591 (fb args 47,421); list.__contains__ 42,318 (fb args 26,180); list.index 7,316 (fb args 4,426); builtins.min 2,245 (fb args 2,790); posixpath.join 1,562; +29 more | 79 / 5 / 0 |
| devtools/pytest_networkx | tuple.__contains__ 1,357,390 (fb args 2,858, deopt 1,484); pathlib.PurePath.__eq__ 39,037; pathlib.PurePath.__hash__ 35,491; pathlib.PurePath.__init__ 16,564 (fb args 594); pathlib.Path.__new__ 14,120; +39 more | 84 / 0 / 0 |
| devtools/rich_render | builtins.max 150,948 (fb args 14,841); builtins.min 92,575; tuple.__contains__ 30,265 (fb args 527, deopt 219); list.sort 26,503 (fb args 5,199); logging.Logger.level 6,150 (fb deopt 10); +14 more | 84 / 0 / 0 |
| devtools/sphinx_autodoc | builtins.min 144,800; tuple.__contains__ 53,523 (fb args 1,284, deopt 3,246); list.__contains__ 24,489 (fb args 285); list.sort 8,235 (fb args 5,417); builtins.max 7,613 (fb args 958, deopt 280); +38 more | 84 / 0 / 0 |
| devtools/sqlglot_transpile | copy.deepcopy 73,858 (fb deopt 20,742); list.sort 26,321 (fb args 230); tuple.__contains__ 11,877 (fb args 6,462, deopt 14,103); list.__contains__ 2,621 (fb args 39); random.choice 1,154; +16 more | 77 / 7 / 0 |
| web/aiohttp_app | list.__contains__ 656,567 (fb args 16, deopt 16); tuple.__contains__ 10,480 (fb args 754); builtins.max 4,805 (fb args 294); json.dumps 3,827; json.loads 3,826; +13 more | 84 / 0 / 0 |
| web/asyncio_http | tuple.__contains__ 17,585 (fb args 29,115); json.dumps 11,949; json.loads 11,948 (fb deopt 1,704); builtins.max 2,622 (fb args 62); list.__contains__ 259 (fb args 6); +8 more | 66 / 18 / 0 |
| web/django_blog | random.choice 64,387 (fb args 320); random.randint 6,656; urllib.parse.quote 6,112; datetime.datetime.isoformat 4,736; tuple.__contains__ 4,621 (fb args 5,063, deopt 5,526); +33 more | 84 / 0 / 0 |
| web/email_mime | tuple.__contains__ 165,508; builtins.min 3,132; random.randint 1,676; random.choice 1,018; list.__contains__ 961; +14 more | 57 / 27 / 0 |
| web/fastapi_pydantic | tuple.__contains__ 17,275 (fb args 6,801, deopt 1,714); copy.deepcopy 15,540 (fb deopt 3,830); random.choice 9,696; random.randint 7,812; builtins.max 6,514 (fb args 632); +29 more | 84 / 0 / 0 |
| web/flask_jinja | random.choice 49,998; tuple.__contains__ 41,046 (fb args 1,030); urllib.parse.quote 26,446; urllib.parse.quote_plus 8,160; random.randint 6,608; +28 more | 84 / 0 / 0 |
| web/json_api | random.randint 318,740; list.count 149,286; random.choice 125,770; copy.deepcopy 72,702 (fb deopt 10,346); datetime.datetime.isoformat 50,260; +13 more | 52 / 32 / 0 |
| web/logging_service | logging.Formatter.formatTime 30,340; tuple.__contains__ 29,528 (fb args 175); logging.Logger.findCaller 28,308; logging.LogRecord.__init__ 28,306 (fb args 1, deopt 2); logging.Formatter.format 26,984 (fb args 2,032); +17 more | 66 / 18 / 0 |
| web/requests_local | list.__contains__ 12,275 (fb args 34); urllib.parse._urlsplit 12,003; tuple.__contains__ 9,306 (fb args 15,972); builtins.min 1,949; json.loads 1,200; +24 more | 82 / 2 / 0 |
| web/sqlalchemy_orm | random.randint 12,326; random.randrange 11,926; random.choice 6,000; tuple.__contains__ 5,833 (fb args 20,607, deopt 4); list.sort 5,262 (fb args 1,936); +15 more | 84 / 0 / 0 |

### workloads/stats-3.15.1t

| workload | targets hit (hits, fallbacks by reason) | installed / pending / refused |
|---|---|---|
| data/backup_archive | random.randint 47,231; tuple.__contains__ 6,304 (fb args 1,560); builtins.max 4,172 (fb args 18); builtins.sum 2,080; builtins.min 1,048; +10 more | 54 / 30 / 0 |
| data/billing_engine | builtins.min 460,654; random.choice 379,963; random.randint 40,000; list.__contains__ 74; tuple.__contains__ 41 (fb args 37); +11 more | 41 / 43 / 0 |
| data/config_merge | random.randint 6,202; list.sort 4,246 (fb args 1,060); datetime.datetime.isoformat 1,454; random.choice 720; builtins.min 565; +12 more | 48 / 36 / 0 |
| data/csv_etl | random.choice 360,000; random.randint 320,000; tuple.__contains__ 80,010; json.dumps 65,381; datetime.date.strftime 65,378; +11 more | 48 / 36 / 0 |
| data/fs_inventory | fnmatch.fnmatch 138,386; tuple.__contains__ 94,192; posixpath.join 77,519; random.randint 34,020; random.choice 26,385; +20 more | 52 / 32 / 0 |
| data/log_analytics | random.choice 264,779; random.randint 150,977; datetime.date.strftime 90,062 (fb args 24); datetime.datetime.strptime 89,999 (fb deps 1); tuple.__contains__ 45,014 (fb args 10); +17 more | 55 / 29 / 0 |
| data/pandas_etl | random.randint 1,455,000; random.choice 1,205,000; random.randrange 480,000; builtins.min 9,112; tuple.__contains__ 3,469 (fb args 854, deopt 1,216); +20 more | 84 / 0 / 0 |
| data/report_render | random.randint 120,000; list.count 80,006; random.sample 80,000; random.choices 80,000; tuple.__contains__ 40,026 (fb args 4); +15 more | 46 / 38 / 0 |
| data/state_snapshot | random.randint 154,540; random.choice 48,000; random.sample 40,632; random.randrange 39,209; random.choices 30,882; +15 more | 52 / 32 / 0 |
| data/text_corpus | builtins.min 110,810; random.choice 82,040; random.randint 48,049; random.choices 45,049; tuple.__contains__ 10; +8 more | 39 / 45 / 0 |
| data/tz_events | datetime.date.strftime 245,248 (fb args 24); builtins.max 174,428 (fb args 57); random.choice 168,000; random.randint 72,000; datetime.datetime.isoformat 48,000; +20 more | 61 / 23 / 0 |
| data/xml_feeds | random.choice 230,000; list.sort 147,668 (fb args 18); random.randint 140,000; tuple.__contains__ 662 (fb args 51); builtins.min 236; +8 more | 57 / 27 / 0 |
| devtools/black_format | tuple.__contains__ 614,809 (fb args 532, deopt 27,667); list.sort 50,152 (fb args 367); builtins.min 17,236; builtins.max 15,349 (fb args 1,049); list.__contains__ 413 (fb args 335,851); +16 more | 77 / 7 / 0 |
| devtools/coverage_pyflakes_tests | tuple.__contains__ 31,204 (fb args 6,539, deopt 8); builtins.min 5,507 (fb args 530); builtins.max 4,006 (fb args 250); list.__contains__ 1,260 (fb args 17); posixpath.dirname 449; +18 more | 77 / 7 / 0 |
| devtools/import_startup | posixpath.dirname 85; list.sort 42; tuple.__contains__ 4; json.dumps 1; posixpath.join 1; +3 more | 39 / 45 / 0 |
| devtools/isort_stdlib | tuple.__contains__ 501,908 (fb args 237); copy.deepcopy 52,554 (fb deopt 2,060); list.__contains__ 23,607 (fb args 5); pathlib.Path.__new__ 2,215; pathlib.PurePath.__init__ 2,215; +20 more | 79 / 5 / 0 |
| devtools/pip_resolve | tuple.__contains__ 61,157 (fb args 1,411); posixpath.basename 38,366; builtins.max 31,690 (fb args 784, deopt 610); builtins.all 25,360; posixpath.splitext 17,471; +37 more | 84 / 0 / 0 |
| devtools/pycodestyle_stdlib | list.__contains__ 1,311,699; tuple.__contains__ 634,930 (fb args 252, deopt 212); builtins.min 518; builtins.max 80 (fb args 20); posixpath.join 33; +5 more | 39 / 45 / 0 |
| devtools/pyflakes_stdlib | tuple.__contains__ 270,036 (fb args 135,740); list.sort 987 (fb args 122); list.__contains__ 977 (fb args 112); posixpath.join 67; builtins.min 58; +6 more | 39 / 45 / 0 |
| devtools/pylint_stdlib | tuple.__contains__ 61,653 (fb args 47,421); list.__contains__ 42,340 (fb args 26,188); list.index 7,316 (fb args 4,426); builtins.min 2,245 (fb args 2,790); posixpath.join 1,350; +29 more | 79 / 5 / 0 |
| devtools/pytest_networkx | tuple.__contains__ 1,357,390 (fb args 2,858, deopt 1,484); pathlib.PurePath.__eq__ 39,037; pathlib.PurePath.__hash__ 35,491; pathlib.PurePath.__init__ 16,564 (fb args 594); pathlib.Path.__new__ 14,120; +39 more | 84 / 0 / 0 |
| devtools/rich_render | builtins.max 150,948 (fb args 14,841); builtins.min 92,575; tuple.__contains__ 30,265 (fb args 527, deopt 219); list.sort 26,503 (fb args 5,199); logging.Logger.level 6,150 (fb deopt 10); +14 more | 84 / 0 / 0 |
| devtools/sphinx_autodoc | builtins.min 144,800; tuple.__contains__ 53,523 (fb args 1,284, deopt 3,246); list.__contains__ 24,489 (fb args 285); list.sort 8,235 (fb args 5,417); builtins.max 7,613 (fb args 958, deopt 280); +38 more | 84 / 0 / 0 |
| devtools/sqlglot_transpile | copy.deepcopy 73,858 (fb deopt 20,742); list.sort 26,321 (fb args 230); tuple.__contains__ 11,877 (fb args 6,462, deopt 14,103); list.__contains__ 2,621 (fb args 39); random.choice 1,154; +16 more | 77 / 7 / 0 |
| web/aiohttp_app | list.__contains__ 656,567 (fb args 32); tuple.__contains__ 10,480 (fb args 754); builtins.max 4,805 (fb args 294); json.dumps 3,827; json.loads 3,826; +13 more | 84 / 0 / 0 |
| web/asyncio_http | tuple.__contains__ 17,585 (fb args 29,115); json.dumps 11,949; json.loads 11,948 (fb deopt 1,704); builtins.max 2,622 (fb args 62); list.__contains__ 259 (fb args 6); +8 more | 66 / 18 / 0 |
| web/django_blog | random.choice 64,387 (fb args 320); random.randint 6,656; urllib.parse.quote 6,112; datetime.datetime.isoformat 4,736; tuple.__contains__ 4,621 (fb args 5,063, deopt 5,526); +33 more | 84 / 0 / 0 |
| web/email_mime | tuple.__contains__ 165,508; builtins.min 3,132; random.randint 1,676; random.choice 1,018; list.__contains__ 961; +14 more | 57 / 27 / 0 |
| web/fastapi_pydantic | tuple.__contains__ 17,275 (fb args 7,016, deopt 1,714); copy.deepcopy 15,540 (fb deopt 3,830); random.choice 9,696; random.randint 7,812; builtins.max 6,514 (fb args 632); +29 more | 84 / 0 / 0 |
| web/flask_jinja | random.choice 49,998; tuple.__contains__ 41,046 (fb args 1,030); urllib.parse.quote 26,446; urllib.parse.quote_plus 8,160; random.randint 6,608; +28 more | 84 / 0 / 0 |
| web/json_api | random.randint 318,740; list.count 149,286; random.choice 125,770; copy.deepcopy 72,702 (fb deopt 10,346); datetime.datetime.isoformat 50,260; +13 more | 52 / 32 / 0 |
| web/logging_service | logging.Formatter.formatTime 30,340; tuple.__contains__ 29,528 (fb args 175); logging.Logger.findCaller 28,308; logging.LogRecord.__init__ 28,306 (fb args 1, deopt 2); logging.Formatter.format 26,984 (fb args 2,032); +17 more | 66 / 18 / 0 |
| web/requests_local | list.__contains__ 12,275 (fb args 34); urllib.parse._urlsplit 12,003; tuple.__contains__ 9,306 (fb args 15,972); builtins.min 1,949; json.loads 1,200; +24 more | 82 / 2 / 0 |
| web/sqlalchemy_orm | random.randint 12,326; random.randrange 11,926; random.choice 6,000; tuple.__contains__ 5,628 (fb args 20,315, deopt 4); list.sort 5,212 (fb args 1,786); +15 more | 84 / 0 / 0 |
