### default-gc on 3.15.1 (65 workloads)

| workload | plain ms | default ms | gc ms | default ÷ plain | gc ÷ plain | gc ÷ default | default (headline run) | tier targets per process |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| web/aiohttp_app | 272.5 | 247.5 | 247.3 | 0.908 | 0.907 | 0.999 | 0.909 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/asyncio_http | 233.6 | 183.8 | 184.1 | 0.787 | 0.788 | 1.002 | 0.796 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/django_blog | 492.0 | 484.9 | 484.7 | 0.986 | 0.985 | 1.000 | 0.988 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/email_mime | 649.8 | 649.8 | 647.6 | 1.000 | 0.997 | 0.997 | 0.989 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/fastapi_pydantic | 294.8 | 283.0 | 285.0 | 0.960 | 0.967 | 1.007 | 0.960 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/flask_jinja | 418.6 | 401.0 | 403.1 | 0.958 | 0.963 | 1.005 | 0.964 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/json_api | 393.7 | 283.3 | 282.2 | 0.720 | 0.717 | 0.996 | 0.714 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/logging_service | 283.3 | 194.7 | 194.0 | 0.687 | 0.685 | 0.997 | 0.691 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/requests_local | 249.0 | 229.6 | 229.1 | 0.922 | 0.920 | 0.997 | 0.921 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/sqlalchemy_orm | 342.7 | 341.3 | 340.6 | 0.996 | 0.994 | 0.998 | 1.000 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/backup_archive | 598.2 | 597.4 | 596.8 | 0.999 | 0.998 | 0.999 | 0.998 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/billing_engine | 892.6 | 850.5 | 739.6 | 0.953 | 0.829 | 0.870 | 0.955 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/config_merge | 539.3 | 536.8 | 537.4 | 0.995 | 0.996 | 1.001 | 0.993 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/csv_etl | 394.6 | 239.0 | 238.1 | 0.606 | 0.603 | 0.996 | 0.611 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/fs_inventory | 315.0 | 227.8 | 227.0 | 0.723 | 0.721 | 0.996 | 0.721 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/log_analytics | 760.6 | 443.2 | 443.1 | 0.583 | 0.583 | 1.000 | 0.578 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/pandas_etl | 528.4 | 526.5 | 526.8 | 0.996 | 0.997 | 1.001 | 0.998 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/report_render | 406.2 | 145.6 | 145.6 | 0.358 | 0.358 | 1.000 | 0.356 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/state_snapshot | 640.6 | 416.6 | 416.3 | 0.650 | 0.650 | 0.999 | 0.650 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/text_corpus | 605.4 | 603.5 | 603.8 | 0.997 | 0.997 | 1.001 | 1.000 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/tz_events | 535.1 | 296.7 | 296.7 | 0.554 | 0.554 | 1.000 | 0.565 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/xml_feeds | 499.7 | 495.7 | 497.3 | 0.992 | 0.995 | 1.003 | 0.998 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/black_format | 1195.7 | 1178.8 | 1176.4 | 0.986 | 0.984 | 0.998 | 0.990 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/coverage_pyflakes_tests | 380.5 | 382.6 | 382.3 | 1.006 | 1.005 | 0.999 | 0.990 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/import_startup | 1884.5 | 1885.6 | 1888.4 | 1.001 | 1.002 | 1.001 | 1.001 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/isort_stdlib | 285.6 | 217.0 | 216.2 | 0.760 | 0.757 | 0.996 | 0.760 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pip_resolve | 561.5 | 526.7 | 527.7 | 0.938 | 0.940 | 1.002 | 0.939 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pycodestyle_stdlib | 1012.3 | 994.2 | 990.4 | 0.982 | 0.978 | 0.996 | 0.967 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pyflakes_stdlib | 1212.6 | 1200.3 | 1201.0 | 0.990 | 0.990 | 1.001 | 0.987 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pylint_stdlib | 1182.5 | 1177.3 | 1179.5 | 0.996 | 0.997 | 1.002 | 0.997 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pytest_networkx | 631.8 | 592.4 | 597.0 | 0.938 | 0.945 | 1.008 | 0.926 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/rich_render | 285.0 | 279.1 | 279.4 | 0.979 | 0.980 | 1.001 | 0.976 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/sphinx_autodoc | 850.0 | 835.0 | 832.1 | 0.982 | 0.979 | 0.997 | 0.981 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/sqlglot_transpile | 877.2 | 849.8 | 850.3 | 0.969 | 0.969 | 1.000 | 0.972 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/django_admin | 996.0 | 972.3 | 973.8 | 0.976 | 0.978 | 1.002 | 0.975 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/drf_api | 1170.0 | 1158.0 | 1160.7 | 0.990 | 0.992 | 1.002 | 0.996 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/falcon_api | 386.8 | 303.4 | 304.8 | 0.784 | 0.788 | 1.005 | 0.788 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/fastapi_sqlalchemy | 1202.3 | 1180.6 | 1190.1 | 0.982 | 0.990 | 1.008 | 1.002 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/flask_marshmallow | 738.8 | 724.1 | 719.1 | 0.980 | 0.973 | 0.993 | 0.975 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/strawberry_graphql | 907.1 | 873.2 | 877.7 | 0.963 | 0.968 | 1.005 | 0.954 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/attrs_cattrs | 800.8 | 689.6 | 694.9 | 0.861 | 0.868 | 1.008 | 0.856 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/boto3_client | 1497.3 | 1454.9 | 1455.0 | 0.972 | 0.972 | 1.000 | 0.971 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/bs4_scrape | 1104.1 | 1086.5 | 1083.2 | 0.984 | 0.981 | 0.997 | 0.978 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/jsonschema_validate | 1362.6 | 1350.4 | 1356.7 | 0.991 | 0.996 | 1.005 | 0.975 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/pydantic_payloads | 649.1 | 640.4 | 645.7 | 0.987 | 0.995 | 1.008 | 0.992 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/yaml_config | 1040.2 | 1025.2 | 1025.5 | 0.986 | 0.986 | 1.000 | 0.997 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/di_plugins | 778.3 | 742.0 | 745.4 | 0.953 | 0.958 | 1.005 | 0.959 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/ecs_arena | 957.8 | 928.3 | 938.0 | 0.969 | 0.979 | 1.010 | 0.990 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/event_ledger | 1363.0 | 1192.0 | 1196.6 | 0.875 | 0.878 | 1.004 | 0.881 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/fulfilment_sim | 1242.6 | 1210.1 | 1211.8 | 0.974 | 0.975 | 1.001 | 0.977 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/mini_interp | 1094.7 | 1087.4 | 1090.2 | 0.993 | 0.996 | 1.003 | 1.003 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/order_domain | 1276.5 | 1228.6 | 1241.9 | 0.962 | 0.973 | 1.011 | 0.969 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/orm_mapper | 798.6 | 752.7 | 756.1 | 0.943 | 0.947 | 1.004 | 0.939 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/report_engine | 930.7 | 801.3 | 799.6 | 0.861 | 0.859 | 0.998 | 0.854 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/rules_engine | 862.7 | 725.6 | 701.6 | 0.841 | 0.813 | 0.967 | 0.849 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/task_planner | 490.5 | 489.8 | 485.3 | 0.999 | 0.990 | 0.991 | 0.993 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/cache_service | 980.5 | 984.0 | 990.2 | 1.004 | 1.010 | 1.006 | 1.003 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/job_worker | 619.7 | 342.2 | 340.9 | 0.552 | 0.550 | 0.996 | 0.552 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/uvicorn_fastapi | 944.4 | 908.6 | 904.3 | 0.962 | 0.958 | 0.995 | 0.959 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/waitress_flask | 1630.8 | 1521.4 | 1504.8 | 0.933 | 0.923 | 0.989 | 0.975 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/compute_pool | 1197.5 | 1165.1 | 1161.4 | 0.973 | 0.970 | 0.997 | 0.974 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/http_server | 631.5 | 574.6 | 575.7 | 0.910 | 0.912 | 1.002 | 0.919 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/pipeline_queue | 588.8 | 305.6 | 306.1 | 0.519 | 0.520 | 1.002 | 0.513 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/request_pool | 450.0 | 247.1 | 247.7 | 0.549 | 0.550 | 1.002 | 0.550 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/shared_cache | 526.8 | 317.9 | 318.3 | 0.604 | 0.604 | 1.001 | 0.606 | gc: {'interp:disabled': 4, 'gc:installed': 1} |

| family (n) | default ÷ plain | gc ÷ plain | gc ÷ default |
|---|---:|---:|---:|
| web (10) | 0.885 | 0.885 | 1.000 |
| data (12) | 0.748 | 0.739 | 0.988 |
| devtools (12) | 0.958 | 0.958 | 1.000 |
| frameworks (6) | 0.943 | 0.945 | 1.002 |
| libs (6) | 0.962 | 0.965 | 1.003 |
| custom (10) | 0.935 | 0.935 | 0.999 |
| services (4) | 0.840 | 0.837 | 0.997 |
| threads (5) | 0.686 | 0.687 | 1.001 |
| all (65) | 0.870 | 0.868 | 0.998 |

worst vs default: gc custom/order_domain 1.011
over 1.02 vs default: gc: []
tier target states seen (workloads): {('gc', 'interp:disabled'): 65, ('gc', 'gc:installed'): 65}

### default-gc on 3.15.1t (65 workloads)

| workload | plain ms | default ms | gc ms | default ÷ plain | gc ÷ plain | gc ÷ default | default (headline run) | tier targets per process |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| web/aiohttp_app | 303.2 | 283.2 | 282.2 | 0.934 | 0.931 | 0.996 | 0.932 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/asyncio_http | 241.9 | 196.5 | 196.6 | 0.812 | 0.813 | 1.001 | 0.812 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/django_blog | 544.6 | 540.7 | 540.6 | 0.993 | 0.993 | 1.000 | 0.997 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/email_mime | 707.8 | 707.0 | 709.0 | 0.999 | 1.002 | 1.003 | 1.012 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/fastapi_pydantic | 326.1 | 328.7 | 329.9 | 1.008 | 1.011 | 1.004 | 0.969 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/flask_jinja | 462.2 | 446.0 | 446.7 | 0.965 | 0.966 | 1.002 | 0.970 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/json_api | 439.2 | 334.0 | 334.0 | 0.761 | 0.761 | 1.000 | 0.752 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/logging_service | 301.8 | 220.0 | 221.8 | 0.729 | 0.735 | 1.008 | 0.742 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/requests_local | 248.7 | 230.9 | 230.4 | 0.928 | 0.926 | 0.998 | 0.928 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| web/sqlalchemy_orm | 355.2 | 353.7 | 353.2 | 0.996 | 0.994 | 0.999 | 0.996 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/backup_archive | 608.2 | 607.0 | 606.4 | 0.998 | 0.997 | 0.999 | 0.999 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/billing_engine | 752.1 | 711.6 | 643.7 | 0.946 | 0.856 | 0.905 | 0.954 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/config_merge | 603.5 | 609.5 | 606.8 | 1.010 | 1.005 | 0.996 | 0.991 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/csv_etl | 429.4 | 272.6 | 275.3 | 0.635 | 0.641 | 1.010 | 0.637 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/fs_inventory | 320.7 | 230.0 | 230.4 | 0.717 | 0.718 | 1.002 | 0.720 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/log_analytics | 807.2 | 473.4 | 472.2 | 0.586 | 0.585 | 0.998 | 0.586 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/pandas_etl | 587.4 | 590.2 | 589.5 | 1.005 | 1.003 | 0.999 | 0.997 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/report_render | 463.8 | 162.1 | 157.6 | 0.350 | 0.340 | 0.972 | 0.351 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/state_snapshot | 1223.4 | 580.2 | 553.7 | 0.474 | 0.453 | 0.954 | 0.477 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/text_corpus | 630.9 | 632.1 | 630.6 | 1.002 | 1.000 | 0.998 | 1.000 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/tz_events | 614.7 | 365.3 | 365.2 | 0.594 | 0.594 | 1.000 | 0.591 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| data/xml_feeds | 563.2 | 571.7 | 540.8 | 1.015 | 0.960 | 0.946 | 1.000 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/black_format | 1295.7 | 1282.1 | 1289.8 | 0.989 | 0.995 | 1.006 | 0.994 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/coverage_pyflakes_tests | 492.1 | 492.5 | 492.2 | 1.001 | 1.000 | 0.999 | 1.000 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/import_startup | 2236.2 | 2238.0 | 2235.7 | 1.001 | 1.000 | 0.999 | 0.999 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/isort_stdlib | 1147.4 | 229.9 | 230.6 | 0.200 | 0.201 | 1.003 | 0.203 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pip_resolve | 626.5 | 589.0 | 588.6 | 0.940 | 0.940 | 0.999 | 0.940 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pycodestyle_stdlib | 1065.1 | 1069.4 | 1069.2 | 1.004 | 1.004 | 1.000 | 0.995 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pyflakes_stdlib | 1189.5 | 1194.2 | 1192.4 | 1.004 | 1.002 | 0.998 | 0.995 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pylint_stdlib | 913.1 | 896.5 | 893.7 | 0.982 | 0.979 | 0.997 | 0.998 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/pytest_networkx | 631.1 | 593.9 | 596.5 | 0.941 | 0.945 | 1.004 | 0.944 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/rich_render | 314.5 | 308.4 | 308.4 | 0.981 | 0.981 | 1.000 | 0.991 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/sphinx_autodoc | 895.3 | 886.7 | 887.2 | 0.990 | 0.991 | 1.001 | 1.002 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| devtools/sqlglot_transpile | 1027.8 | 997.9 | 1002.6 | 0.971 | 0.975 | 1.005 | 0.975 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/django_admin | 1121.4 | 1100.2 | 1103.4 | 0.981 | 0.984 | 1.003 | 0.982 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/drf_api | 1346.3 | 1350.9 | 1347.6 | 1.003 | 1.001 | 0.998 | 0.993 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/falcon_api | 431.2 | 356.4 | 355.9 | 0.826 | 0.825 | 0.999 | 0.814 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/fastapi_sqlalchemy | 1358.6 | 1356.9 | 1355.2 | 0.999 | 0.997 | 0.999 | 0.997 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/flask_marshmallow | 823.8 | 814.3 | 802.5 | 0.988 | 0.974 | 0.986 | 0.988 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| frameworks/strawberry_graphql | 1086.8 | 1077.2 | 1070.9 | 0.991 | 0.985 | 0.994 | 0.980 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/attrs_cattrs | 879.6 | 773.9 | 770.7 | 0.880 | 0.876 | 0.996 | 0.877 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/boto3_client | 1649.6 | 1570.7 | 1562.4 | 0.952 | 0.947 | 0.995 | 0.947 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/bs4_scrape | 1156.7 | 1157.4 | 1154.7 | 1.001 | 0.998 | 0.998 | 0.991 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/jsonschema_validate | 1478.3 | 1454.7 | 1462.8 | 0.984 | 0.990 | 1.006 | 0.982 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/pydantic_payloads | 747.0 | 737.8 | 741.2 | 0.988 | 0.992 | 1.005 | 0.995 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| libs/yaml_config | 1110.7 | 1114.3 | 1113.6 | 1.003 | 1.003 | 0.999 | 0.996 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/di_plugins | 825.2 | 816.6 | 816.9 | 0.990 | 0.990 | 1.000 | 0.982 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/ecs_arena | 1060.5 | 1064.3 | 1061.7 | 1.004 | 1.001 | 0.998 | 1.000 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/event_ledger | 1554.7 | 1496.4 | 1484.9 | 0.962 | 0.955 | 0.992 | 0.975 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/fulfilment_sim | 1362.7 | 1319.8 | 1321.9 | 0.969 | 0.970 | 1.002 | 0.971 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/mini_interp | 1221.6 | 1237.7 | 1236.6 | 1.013 | 1.012 | 0.999 | 1.015 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/order_domain | 1411.3 | 1370.2 | 1369.3 | 0.971 | 0.970 | 0.999 | 0.986 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/orm_mapper | 858.8 | 812.3 | 807.6 | 0.946 | 0.940 | 0.994 | 0.951 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/report_engine | 1030.0 | 927.5 | 930.2 | 0.901 | 0.903 | 1.003 | 0.901 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/rules_engine | 1074.7 | 1063.4 | 965.4 | 0.990 | 0.898 | 0.908 | 0.972 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| custom/task_planner | 531.8 | 530.3 | 530.7 | 0.997 | 0.998 | 1.001 | 1.001 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/cache_service | 1040.2 | 1038.0 | 1040.7 | 0.998 | 1.000 | 1.003 | 0.998 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/job_worker | 299.8 | 203.0 | 204.9 | 0.677 | 0.683 | 1.009 | 0.682 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/uvicorn_fastapi | 1025.8 | 993.9 | 992.0 | 0.969 | 0.967 | 0.998 | 0.976 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| services/waitress_flask | 184.6 | 170.0 | 170.4 | 0.921 | 0.923 | 1.002 | 0.925 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/compute_pool | 174.0 | 170.3 | 169.7 | 0.978 | 0.975 | 0.997 | 0.983 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/http_server | 87.3 | 80.1 | 80.0 | 0.917 | 0.916 | 0.998 | 0.893 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/pipeline_queue | 266.9 | 190.4 | 191.1 | 0.714 | 0.716 | 1.004 | 0.711 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/request_pool | 142.3 | 110.5 | 94.0 | 0.776 | 0.661 | 0.851 | 0.780 | gc: {'interp:disabled': 4, 'gc:installed': 1} |
| threads/shared_cache | 228.2 | 139.1 | 137.7 | 0.609 | 0.604 | 0.990 | 0.616 | gc: {'interp:disabled': 4, 'gc:installed': 1} |

| family (n) | default ÷ plain | gc ÷ plain | gc ÷ default |
|---|---:|---:|---:|
| web (10) | 0.907 | 0.907 | 1.001 |
| data (12) | 0.737 | 0.723 | 0.981 |
| devtools (12) | 0.860 | 0.861 | 1.001 |
| frameworks (6) | 0.963 | 0.959 | 0.996 |
| libs (6) | 0.967 | 0.967 | 1.000 |
| custom (10) | 0.974 | 0.963 | 0.989 |
| services (4) | 0.881 | 0.884 | 1.003 |
| threads (5) | 0.788 | 0.761 | 0.966 |
| all (65) | 0.873 | 0.866 | 0.992 |

worst vs default: gc data/csv_etl 1.010
over 1.02 vs default: gc: []
tier target states seen (workloads): {('gc', 'interp:disabled'): 65, ('gc', 'gc:installed'): 65}

### default-gc+interp on 3.15.1 (28 workloads)

| workload | plain ms | default ms | gc+interp ms | default ÷ plain | gc+interp ÷ plain | gc+interp ÷ default | default (headline run) | tier targets per process |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| devtools/black_format | 1196.2 | 1180.2 | 1165.7 | 0.987 | 0.975 | 0.988 | 0.990 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/coverage_pyflakes_tests | 381.5 | 380.6 | 378.1 | 0.998 | 0.991 | 0.994 | 0.990 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/import_startup | 1881.3 | 1881.9 | 1883.3 | 1.000 | 1.001 | 1.001 | 1.001 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/isort_stdlib | 285.9 | 215.6 | 217.3 | 0.754 | 0.760 | 1.008 | 0.760 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/pip_resolve | 566.0 | 527.8 | 529.4 | 0.932 | 0.935 | 1.003 | 0.939 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/pycodestyle_stdlib | 1012.6 | 989.3 | 996.2 | 0.977 | 0.984 | 1.007 | 0.967 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/pyflakes_stdlib | 1205.8 | 1199.8 | 1198.0 | 0.995 | 0.994 | 0.998 | 0.987 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/pylint_stdlib | 1191.3 | 1184.3 | 1155.4 | 0.994 | 0.970 | 0.976 | 0.997 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/pytest_networkx | 637.3 | 600.3 | 582.6 | 0.942 | 0.914 | 0.970 | 0.926 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/rich_render | 285.4 | 279.9 | 273.0 | 0.981 | 0.957 | 0.975 | 0.976 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/sphinx_autodoc | 854.2 | 834.4 | 829.7 | 0.977 | 0.971 | 0.994 | 0.981 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| devtools/sqlglot_transpile | 880.0 | 849.7 | 780.9 | 0.966 | 0.887 | 0.919 | 0.972 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| frameworks/django_admin | 998.4 | 972.8 | 957.3 | 0.974 | 0.959 | 0.984 | 0.975 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| frameworks/drf_api | 1164.1 | 1161.1 | 1157.1 | 0.997 | 0.994 | 0.997 | 0.996 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| frameworks/falcon_api | 386.6 | 304.7 | 307.3 | 0.788 | 0.795 | 1.009 | 0.788 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| frameworks/fastapi_sqlalchemy | 1197.5 | 1204.5 | 1181.1 | 1.006 | 0.986 | 0.981 | 1.002 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| frameworks/flask_marshmallow | 740.3 | 728.3 | 711.8 | 0.984 | 0.961 | 0.977 | 0.975 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| frameworks/strawberry_graphql | 919.6 | 865.9 | 859.5 | 0.942 | 0.935 | 0.993 | 0.954 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/di_plugins | 775.7 | 740.4 | 753.9 | 0.955 | 0.972 | 1.018 | 0.959 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/ecs_arena | 950.8 | 905.7 | 915.0 | 0.953 | 0.962 | 1.010 | 0.990 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/event_ledger | 1353.5 | 1194.4 | 1193.0 | 0.882 | 0.881 | 0.999 | 0.881 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/fulfilment_sim | 1239.9 | 1205.9 | 1194.9 | 0.973 | 0.964 | 0.991 | 0.977 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/mini_interp | 1081.4 | 1094.5 | 1076.3 | 1.012 | 0.995 | 0.983 | 1.003 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/order_domain | 1265.3 | 1245.8 | 1129.5 | 0.985 | 0.893 | 0.907 | 0.969 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/orm_mapper | 794.9 | 755.6 | 725.7 | 0.951 | 0.913 | 0.960 | 0.939 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/report_engine | 936.3 | 801.3 | 794.1 | 0.856 | 0.848 | 0.991 | 0.854 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/rules_engine | 855.6 | 724.4 | 701.2 | 0.847 | 0.820 | 0.968 | 0.849 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |
| custom/task_planner | 490.7 | 487.4 | 488.1 | 0.993 | 0.995 | 1.001 | 0.993 | gc+interp: {'interp:installed': 4, 'gc:installed': 1} |

| family (n) | default ÷ plain | gc+interp ÷ plain | gc+interp ÷ default |
|---|---:|---:|---:|
| devtools (12) | 0.956 | 0.942 | 0.986 |
| frameworks (6) | 0.945 | 0.936 | 0.990 |
| custom (10) | 0.939 | 0.922 | 0.982 |
| all (28) | 0.948 | 0.934 | 0.985 |

worst vs default: gc+interp custom/di_plugins 1.018
over 1.02 vs default: gc+interp: []
tier target states seen (workloads): {('gc+interp', 'interp:installed'): 28, ('gc+interp', 'gc:installed'): 28}

### default-interp on 3.15.1 (65 workloads)

| workload | plain ms | default ms | interp ms | default ÷ plain | interp ÷ plain | interp ÷ default | default (headline run) | tier targets per process |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| web/aiohttp_app | 272.2 | 249.7 | 244.7 | 0.917 | 0.899 | 0.980 | 0.909 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/asyncio_http | 233.1 | 183.6 | 182.3 | 0.787 | 0.782 | 0.993 | 0.796 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/django_blog | 491.1 | 483.7 | 479.9 | 0.985 | 0.977 | 0.992 | 0.988 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/email_mime | 651.9 | 648.8 | 659.1 | 0.995 | 1.011 | 1.016 | 0.989 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/fastapi_pydantic | 295.1 | 282.9 | 280.7 | 0.959 | 0.951 | 0.992 | 0.960 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/flask_jinja | 416.9 | 401.8 | 401.8 | 0.964 | 0.964 | 1.000 | 0.964 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/json_api | 395.7 | 282.4 | 272.3 | 0.714 | 0.688 | 0.964 | 0.714 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/logging_service | 283.7 | 194.6 | 193.5 | 0.686 | 0.682 | 0.994 | 0.691 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/requests_local | 249.0 | 228.0 | 228.0 | 0.916 | 0.916 | 1.000 | 0.921 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| web/sqlalchemy_orm | 341.4 | 341.1 | 339.5 | 0.999 | 0.994 | 0.995 | 1.000 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/backup_archive | 598.2 | 597.0 | 596.9 | 0.998 | 0.998 | 1.000 | 0.998 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/billing_engine | 886.8 | 847.1 | 844.4 | 0.955 | 0.952 | 0.997 | 0.955 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/config_merge | 537.2 | 537.6 | 516.3 | 1.001 | 0.961 | 0.960 | 0.993 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/csv_etl | 393.5 | 240.0 | 239.9 | 0.610 | 0.610 | 1.000 | 0.611 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/fs_inventory | 314.3 | 227.3 | 227.1 | 0.723 | 0.723 | 0.999 | 0.721 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/log_analytics | 768.7 | 444.8 | 441.5 | 0.579 | 0.574 | 0.992 | 0.578 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/pandas_etl | 527.5 | 526.1 | 524.0 | 0.997 | 0.993 | 0.996 | 0.998 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/report_render | 406.6 | 144.7 | 138.8 | 0.356 | 0.341 | 0.959 | 0.356 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/state_snapshot | 642.3 | 413.5 | 409.6 | 0.644 | 0.638 | 0.990 | 0.650 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/text_corpus | 604.0 | 606.3 | 606.5 | 1.004 | 1.004 | 1.000 | 1.000 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/tz_events | 529.8 | 295.8 | 295.2 | 0.558 | 0.557 | 0.998 | 0.565 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| data/xml_feeds | 496.0 | 499.2 | 490.7 | 1.006 | 0.989 | 0.983 | 0.998 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/black_format | 1202.3 | 1175.1 | 1158.8 | 0.977 | 0.964 | 0.986 | 0.990 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/coverage_pyflakes_tests | 380.3 | 380.7 | 377.4 | 1.001 | 0.993 | 0.992 | 0.990 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/import_startup | 1880.5 | 1879.6 | 1879.3 | 1.000 | 0.999 | 1.000 | 1.001 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/isort_stdlib | 285.9 | 215.0 | 214.9 | 0.752 | 0.752 | 1.000 | 0.760 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/pip_resolve | 563.3 | 528.1 | 527.3 | 0.937 | 0.936 | 0.999 | 0.939 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/pycodestyle_stdlib | 1015.5 | 989.0 | 997.1 | 0.974 | 0.982 | 1.008 | 0.967 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/pyflakes_stdlib | 1209.5 | 1204.7 | 1189.5 | 0.996 | 0.983 | 0.987 | 0.987 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/pylint_stdlib | 1175.4 | 1187.5 | 1157.2 | 1.010 | 0.985 | 0.974 | 0.997 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/pytest_networkx | 638.1 | 597.6 | 578.7 | 0.937 | 0.907 | 0.968 | 0.926 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/rich_render | 285.3 | 279.0 | 273.7 | 0.978 | 0.959 | 0.981 | 0.976 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/sphinx_autodoc | 847.9 | 834.1 | 826.6 | 0.984 | 0.975 | 0.991 | 0.981 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| devtools/sqlglot_transpile | 880.5 | 850.1 | 779.7 | 0.965 | 0.886 | 0.917 | 0.972 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| frameworks/django_admin | 996.8 | 967.5 | 957.2 | 0.971 | 0.960 | 0.989 | 0.975 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| frameworks/drf_api | 1173.0 | 1157.2 | 1161.3 | 0.987 | 0.990 | 1.003 | 0.996 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| frameworks/falcon_api | 387.2 | 307.2 | 305.4 | 0.793 | 0.789 | 0.994 | 0.788 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| frameworks/fastapi_sqlalchemy | 1190.4 | 1192.5 | 1170.2 | 1.002 | 0.983 | 0.981 | 1.002 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| frameworks/flask_marshmallow | 733.4 | 722.3 | 708.9 | 0.985 | 0.967 | 0.981 | 0.975 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| frameworks/strawberry_graphql | 914.8 | 875.9 | 847.4 | 0.957 | 0.926 | 0.967 | 0.954 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| libs/attrs_cattrs | 800.2 | 690.3 | 687.3 | 0.863 | 0.859 | 0.996 | 0.856 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| libs/boto3_client | 1504.8 | 1467.7 | 1448.8 | 0.975 | 0.963 | 0.987 | 0.971 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| libs/bs4_scrape | 1102.5 | 1077.0 | 1059.0 | 0.977 | 0.961 | 0.983 | 0.978 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| libs/jsonschema_validate | 1360.2 | 1344.7 | 1309.4 | 0.989 | 0.963 | 0.974 | 0.975 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| libs/pydantic_payloads | 648.7 | 641.8 | 638.5 | 0.989 | 0.984 | 0.995 | 0.992 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| libs/yaml_config | 1042.9 | 1027.5 | 979.4 | 0.985 | 0.939 | 0.953 | 0.997 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/di_plugins | 776.9 | 745.7 | 758.3 | 0.960 | 0.976 | 1.017 | 0.959 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/ecs_arena | 963.8 | 930.3 | 900.6 | 0.965 | 0.934 | 0.968 | 0.990 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/event_ledger | 1364.1 | 1191.1 | 1190.0 | 0.873 | 0.872 | 0.999 | 0.881 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/fulfilment_sim | 1241.4 | 1182.2 | 1189.3 | 0.952 | 0.958 | 1.006 | 0.977 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/mini_interp | 1099.0 | 1092.8 | 1086.1 | 0.994 | 0.988 | 0.994 | 1.003 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/order_domain | 1276.4 | 1220.6 | 1132.6 | 0.956 | 0.887 | 0.928 | 0.969 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/orm_mapper | 799.0 | 752.1 | 726.8 | 0.941 | 0.910 | 0.966 | 0.939 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/report_engine | 931.5 | 802.9 | 798.6 | 0.862 | 0.857 | 0.995 | 0.854 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/rules_engine | 856.5 | 723.2 | 726.7 | 0.844 | 0.848 | 1.005 | 0.849 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| custom/task_planner | 491.2 | 484.5 | 484.5 | 0.986 | 0.986 | 1.000 | 0.993 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| services/cache_service | 967.9 | 988.5 | 974.8 | 1.021 | 1.007 | 0.986 | 1.003 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| services/job_worker | 620.5 | 342.6 | 341.7 | 0.552 | 0.551 | 0.997 | 0.552 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| services/uvicorn_fastapi | 949.9 | 914.7 | 909.8 | 0.963 | 0.958 | 0.995 | 0.959 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| services/waitress_flask | 1570.0 | 1511.6 | 1398.1 | 0.963 | 0.890 | 0.925 | 0.975 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| threads/compute_pool | 1200.1 | 1162.0 | 1162.9 | 0.968 | 0.969 | 1.001 | 0.974 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| threads/http_server | 631.6 | 575.2 | 574.2 | 0.911 | 0.909 | 0.998 | 0.919 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| threads/pipeline_queue | 588.8 | 304.9 | 298.2 | 0.518 | 0.506 | 0.978 | 0.513 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| threads/request_pool | 454.5 | 246.1 | 246.4 | 0.541 | 0.542 | 1.002 | 0.550 | interp: {'interp:installed': 4, 'gc:disabled': 1} |
| threads/shared_cache | 525.6 | 318.8 | 319.4 | 0.607 | 0.608 | 1.002 | 0.606 | interp: {'interp:installed': 4, 'gc:disabled': 1} |

| family (n) | default ÷ plain | interp ÷ plain | interp ÷ default |
|---|---:|---:|---:|
| web (10) | 0.884 | 0.878 | 0.993 |
| data (12) | 0.749 | 0.742 | 0.989 |
| devtools (12) | 0.957 | 0.941 | 0.983 |
| frameworks (6) | 0.946 | 0.933 | 0.986 |
| libs (6) | 0.962 | 0.944 | 0.981 |
| custom (10) | 0.932 | 0.920 | 0.987 |
| services (4) | 0.850 | 0.829 | 0.975 |
| threads (5) | 0.684 | 0.682 | 0.996 |
| all (65) | 0.870 | 0.859 | 0.987 |

worst vs default: interp custom/di_plugins 1.017
over 1.02 vs default: interp: []
tier target states seen (workloads): {('interp', 'interp:installed'): 65, ('interp', 'gc:disabled'): 65}

