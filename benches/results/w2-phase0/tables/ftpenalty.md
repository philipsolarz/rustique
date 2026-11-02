### Workloads: plain 3.15.1t / plain 3.15.1 (median steady-state run())

| workload | 3.15.1 plain ms | 3.15.1t plain ms | FT ÷ GIL | wave 1 (phase 3 rep 1) |
|---|---:|---:|---:|---:|
| data/backup_archive | 598.5 | 605.5 | 1.012 | 1.012 |
| data/billing_engine | 932.3 | 748.9 | 0.803 | 0.826 |
| data/config_merge | 540.6 | 611.7 | 1.131 | 1.131 |
| data/csv_etl | 393.0 | 428.8 | 1.091 | 1.114 |
| data/fs_inventory | 316.0 | 321.1 | 1.016 | 1.017 |
| data/log_analytics | 772.2 | 804.5 | 1.042 | 1.053 |
| data/pandas_etl | 528.2 | 590.6 | 1.118 | 1.117 |
| data/report_render | 406.9 | 463.5 | 1.139 | 1.145 |
| data/state_snapshot | 647.3 | 1229.5 | **1.899** | 1.927 |
| data/text_corpus | 604.3 | 628.1 | 1.039 | 1.045 |
| data/tz_events | 530.6 | 609.1 | 1.148 | 1.168 |
| data/xml_feeds | 506.0 | 571.6 | 1.130 | 1.126 |
| devtools/black_format | 1194.6 | 1298.2 | 1.087 | 1.079 |
| devtools/coverage_pyflakes_tests | 380.9 | 491.5 | 1.290 | 1.294 |
| devtools/import_startup | 1888.7 | 2240.4 | 1.186 | 1.183 |
| devtools/isort_stdlib | 284.9 | 1146.9 | **4.025** | 4.186 |
| devtools/pip_resolve | 563.0 | 623.7 | 1.108 | 1.112 |
| devtools/pycodestyle_stdlib | 1013.7 | 1065.7 | 1.051 | 1.055 |
| devtools/pyflakes_stdlib | 1210.5 | 1190.3 | 0.983 | 0.986 |
| devtools/pylint_stdlib | 1184.4 | 894.7 | 0.755 | 0.761 |
| devtools/pytest_networkx | 629.5 | 631.9 | 1.004 | 1.001 |
| devtools/rich_render | 284.8 | 309.2 | 1.086 | 1.089 |
| devtools/sphinx_autodoc | 848.9 | 903.3 | 1.064 | 1.047 |
| devtools/sqlglot_transpile | 878.0 | 1027.1 | 1.170 | 1.171 |
| web/aiohttp_app | 273.3 | 300.6 | 1.100 | 1.114 |
| web/asyncio_http | 231.2 | 242.9 | 1.051 | 1.042 |
| web/django_blog | 489.1 | 544.9 | 1.114 | 1.118 |
| web/email_mime | 656.2 | 706.4 | 1.076 | 1.083 |
| web/fastapi_pydantic | 297.0 | 340.4 | 1.146 | 1.144 |
| web/flask_jinja | 417.3 | 460.2 | 1.103 | 1.105 |
| web/json_api | 396.1 | 440.0 | 1.111 | 1.114 |
| web/logging_service | 283.1 | 300.2 | 1.060 | 1.061 |
| web/requests_local | 250.5 | 250.1 | 0.999 | 1.004 |
| web/sqlalchemy_orm | 341.7 | 354.5 | 1.037 | 1.026 |

geomean data: 1.109
geomean devtools: 1.187
geomean web: 1.079
geomean all (34): 1.127; median 1.089
outliers > 1.3: devtools/isort_stdlib 4.03, data/state_snapshot 1.90
below 1.0: devtools/pylint_stdlib 0.76, data/billing_engine 0.80, devtools/pyflakes_stdlib 0.98, web/requests_local 1.00

### pyperformance: plain 3.15.1t / plain 3.15.1 (mean per benchmark)

replicate A: 122 benchmarks; geomean 1.073; median 1.100; > 1.3: 2; < 1.0: 18
replicate -B: 121 benchmarks; geomean 1.077; median 1.101; > 1.3: 3; < 1.0: 15

replicates averaged: 122 benchmarks, geomean 1.074

| benchmark | 3.15.1 plain | 3.15.1t plain | FT ÷ GIL (A) | FT ÷ GIL (B) | averaged |
|---|---:|---:|---:|---:|---:|
| bench_thread_pool | 847 µs | 1.16 ms | 1.369 | 1.379 | **1.374** |
| dask | 421 ms | 570 ms | 1.353 | 1.359 | **1.356** |
| connected_components | 472 ms | 608 ms | 1.288 | 1.318 | **1.303** |
| nbody | 75.4 ms | 94.2 ms | 1.249 | 1.297 | 1.273 |
| shortest_path | 498 ms | 629 ms | 1.263 | 1.261 | 1.262 |
| python_startup_no_site | 7.31 ms | 9.06 ms | 1.240 | 1.258 | 1.249 |
| asyncio_tcp_ssl | 887 ms | 1.1 s | 1.238 | 1.252 | 1.245 |
| docutils | 1.68 s | 2.09 s | 1.245 | 1.243 | 1.244 |
| async_tree_none | 260 ms | 321 ms | 1.234 | 1.241 | 1.237 |
| python_startup | 10.5 ms | 12.8 ms | 1.224 | 1.226 | 1.225 |
| sqlalchemy_imperative | 7.05 ms | 8.59 ms | 1.218 | 1.231 | 1.225 |
| scimark_sparse_mat_mult | 3.32 ms | 4.05 ms | 1.222 | 1.222 | 1.222 |
| sqlglot_v2_parse | 816 µs | 991 µs | 1.214 | 1.228 | 1.221 |
| deltablue | 2.43 ms | 2.96 ms | 1.218 | 1.208 | 1.213 |
| unpickle_list | 2.86 µs | 3.45 µs | 1.208 | 1.195 | 1.201 |
| scimark_fft | 194 ms | 232 ms | 1.192 | 1.198 | 1.195 |
| async_tree_eager_memoization | 173 ms | 208 ms | 1.197 | 1.190 | 1.194 |
| sqlglot_v2_transpile | 1.03 ms | 1.23 ms | 1.190 | 1.197 | 1.194 |
| async_tree_eager | 74.1 ms | 89.1 ms | 1.203 | 1.182 | 1.193 |
| chameleon | 10.3 ms | 12.1 ms | 1.175 | 1.175 | 1.175 |
| comprehensions | 10.8 µs | 12.7 µs | 1.172 | 1.172 | 1.172 |
| scimark_sor | 80.8 ms | 94.5 ms | 1.170 | 1.168 | 1.169 |
| coverage | 52.4 ms | 60.9 ms | 1.162 | 1.172 | 1.167 |
| sphinx | 716 ms | 832 ms | 1.162 | 1.169 | 1.166 |
| async_tree_memoization | 346 ms | 402 ms | 1.161 | 1.168 | 1.164 |
| async_tree_eager_cpu_io_mixed_tg | 433 ms | 503 ms | 1.162 | 1.167 | 1.164 |
| async_tree_cpu_io_mixed | 473 ms | 550 ms | 1.163 | 1.165 | 1.164 |
| scimark_monte_carlo | 45 ms | 52 ms | 1.155 | 1.170 | 1.162 |
| regex_effbot | 2.12 ms | 2.45 ms | 1.154 | 1.159 | 1.157 |
| telco | 4.7 ms | 5.4 ms | 1.149 | 1.154 | 1.152 |
| spectral_norm | 71.6 ms | 82.1 ms | 1.147 | 1.156 | 1.152 |
| xml_etree_process | 44 ms | 50.6 ms | 1.150 | 1.151 | 1.151 |
| typing_runtime_protocols | 112 µs | 128 µs | 1.148 | 1.152 | 1.150 |
| crypto_pyaes | 54.8 ms | 62.8 ms | 1.147 | 1.145 | 1.146 |
| richards_super | 37.1 ms | 42.4 ms | 1.144 | 1.146 | 1.145 |
| sqlalchemy_declarative | 77.2 ms | 88.1 ms | 1.141 | 1.141 | 1.141 |
| raytrace | 188 ms | 213 ms | 1.132 | 1.140 | 1.136 |
| regex_dna | 131 ms | 149 ms | 1.136 | 1.134 | 1.135 |
| pickle_pure_python | 221 µs | 251 µs | 1.137 | 1.130 | 1.134 |
| chaos | 40.3 ms | 45.4 ms | 1.126 | 1.128 | 1.127 |
| nqueens | 56.9 ms | 64 ms | 1.123 | 1.130 | 1.126 |
| base64_small | 215 µs | 240 µs | 1.117 | 1.123 | 1.120 |
| meteor_contest | 76.9 ms | 85.9 ms | 1.118 | 1.117 | 1.117 |
| sympy_expand | 271 ms | 303 ms | 1.117 | 1.117 | 1.117 |
| sympy_str | 157 ms | 176 ms | 1.120 | 1.113 | 1.117 |
| subparsers | 7.43 ms | 8.3 ms | 1.117 | 1.116 | 1.116 |
| html5lib | 37.7 ms | 42 ms | 1.114 | 1.117 | 1.115 |
| async_tree_eager_memoization_tg | 310 ms | 345 ms | 1.112 | 1.118 | 1.115 |
| sympy_sum | 86.7 ms | 96.8 ms | 1.116 | 1.111 | 1.113 |
| deepcopy_memo | 20.7 µs | 23 µs | 1.111 | 1.114 | 1.113 |
| many_optionals | 396 µs | 441 µs | 1.116 | 1.108 | 1.112 |
| richards | 32.8 ms | 36.2 ms | 1.103 | 1.118 | 1.111 |
| sympy_integrate | 13.1 ms | 14.5 ms | 1.111 | 1.107 | 1.109 |
| scimark_lu | 81.3 ms | 89.2 ms | 1.097 | 1.121 | 1.109 |
| logging_simple | 4.63 µs | 5.12 µs | 1.106 | 1.111 | 1.108 |
| sqlglot_v2_optimize | 36.6 ms | 40.5 ms | 1.107 | 1.103 | 1.105 |
| logging_format | 5.01 µs | 5.51 µs | 1.101 | 1.106 | 1.103 |
| mdp | 949 ms | 1.04 s | 1.099 | 1.106 | 1.103 |
| pyflate | 318 ms | 350 ms | 1.102 | 1.102 | 1.102 |
| xdsl_constant_fold | 27.5 ms | 30.8 ms | 1.121 | 1.080 | 1.101 |
| async_tree_memoization_tg | 339 ms | 371 ms | 1.095 | 1.104 | 1.100 |
| regex_compile | 74.7 ms | 81.8 ms | 1.094 | 1.100 | 1.097 |
| async_tree_none_tg | 274 ms | 299 ms | 1.091 | 1.101 | 1.096 |
| fannkuch | 269 ms | 298 ms | 1.107 | 1.085 | 1.096 |
| sqlglot_v2_normalize | 73.7 ms | 80.5 ms | 1.092 | 1.098 | 1.095 |
| go | 85.5 ms | 94 ms | 1.100 | 1.090 | 1.095 |
| unpickle | 10.1 µs | 11.1 µs | 1.099 | 1.091 | 1.095 |
| deepcopy | 164 µs | 180 µs | 1.097 | 1.093 | 1.095 |
| pprint_pformat | 1.11 s | 1.21 s | 1.093 | 1.093 | 1.093 |
| pickle_list | 2.53 µs | 2.77 µs | 1.094 | 1.091 | 1.092 |
| deepcopy_reduce | 1.87 µs | 2.04 µs | 1.093 | 1.092 | 1.092 |
| async_tree_eager_tg | 231 ms | 251 ms | 1.087 | 1.094 | 1.091 |
| async_tree_eager_cpu_io_mixed | 324 ms | 353 ms | 1.089 | 1.091 | 1.090 |
| hexiom | 4.43 ms | 4.87 ms | 1.100 | 1.077 | 1.089 |
| 2to3 | 203 ms | 220 ms | 1.086 | 1.089 | 1.088 |
| xml_etree_generate | 60.7 ms | 65.6 ms | 1.081 | 1.092 | 1.087 |
| unpickle_pure_python | 158 µs | 171 µs | 1.083 | 1.087 | 1.085 |
| pprint_safe_repr | 541 ms | 584 ms | 1.080 | 1.086 | 1.083 |
| generators | 24.9 ms | 26.7 ms | 1.074 | 1.079 | 1.077 |
| urlsafe_base64_small | 243 µs | 262 µs | 1.078 | 1.070 | 1.074 |
| logging_silent | 68.3 ns | 72.6 ns | 1.063 | 1.065 | 1.064 |
| django_template | 26.6 ms | 28.1 ms | 1.057 | 1.059 | 1.058 |
| async_tree_cpu_io_mixed_tg | 494 ms | 519 ms | 1.050 | 1.053 | 1.052 |
| float | 59.7 ms | 62.7 ms | 1.051 | 1.047 | 1.049 |
| k_core | 2.46 s | 2.57 s | 1.044 | 1.054 | 1.049 |
| pickle_dict | 15.6 µs | 16.3 µs | 1.045 | 1.043 | 1.044 |
| json_loads | 17.7 µs | 18.4 µs | 1.039 | 1.046 | 1.042 |
| tomli_loads | 1.45 s | 1.49 s | 1.028 | 1.032 | 1.030 |
| async_generators | 310 ms | 318 ms | 1.026 | 1.029 | 1.027 |
| dulwich_log | 30.1 ms | 30.8 ms | 1.022 | 1.026 | 1.024 |
| json_dumps | 6.09 ms | 6.23 ms | 1.023 | 1.021 | 1.022 |
| unpack_sequence | 35.9 ns | 35.8 ns | 0.997 | 1.045 | 1.021 |
| regex_v8 | 17.2 ms | 17.5 ms | 1.017 | 1.025 | 1.021 |
| mako | 9.98 ms | 10.2 ms | 1.022 | 1.013 | 1.018 |
| asyncio_tcp | 258 ms | 261 ms | 1.013 | 1.023 | 1.018 |
| ascii85_small | 492 µs | 496 µs | 1.007 | 1.017 | 1.012 |
| bpe_tokeniser | 3.2 s | 3.24 s | 1.012 | 1.006 | 1.009 |
| base32_small | 211 µs | 205 µs | 0.974 | 1.042 | 1.008 |
| base85_small | 199 µs | 200 µs | 1.004 | 1.010 | 1.007 |
| base85_large | 5.31 ms | 5.35 ms | 1.006 | 1.004 | 1.005 |
| pickle | 7.67 µs | 7.72 µs | 1.007 | 1.003 | 1.005 |
| pathlib | 17.3 ms | 17.3 ms | 1.003 | 1.005 | 1.004 |
| base16_small | 339 µs | 340 µs | 1.002 | 1.002 | 1.002 |
| ascii85_large | 13.1 ms | 13.1 ms | 1.002 | 1.002 | 1.002 |
| base64_large | 2.02 ms | 2.02 ms | 1.001 | 1.001 | 1.001 |
| base32_large | 2.56 ms | 2.56 ms | 1.000 | 1.001 | 1.000 |
| tornado_http | 80.5 ms | 80.4 ms | 0.999 | 0.998 | 0.998 |
| pidigits | 175 ms | 172 ms | 0.986 | 0.984 | 0.985 |
| coroutines | 17.8 ms | 17.4 ms | 0.978 | 0.974 | 0.976 |
| async_tree_io | 690 ms | 670 ms | 0.971 | 0.978 | 0.974 |
| xml_etree_iterparse | 74.5 ms | 71.5 ms | 0.959 | 0.965 | 0.962 |
| xml_etree_parse | 121 ms | 116 ms | 0.959 | 0.959 | 0.959 |
| asyncio_websockets | 437 ms | 405 ms | 0.926 | — | 0.926 |
| fastapi_http | 217 ms | 192 ms | 0.885 | 0.886 | 0.886 |
| async_tree_io_tg | 732 ms | 641 ms | 0.876 | 0.884 | 0.880 |
| sqlite_synth | 1.52 µs | 1.29 µs | 0.850 | 0.852 | 0.851 |
| async_tree_eager_io | 740 ms | 622 ms | 0.841 | 0.856 | 0.848 |
| async_tree_eager_io_tg | 776 ms | 593 ms | 0.764 | 0.772 | 0.768 |
| bench_mp_pool | 7.72 ms | 5.72 ms | 0.742 | 0.737 | 0.739 |
| create_gc_cycles | 1.62 ms | 1.14 ms | 0.703 | 0.704 | 0.704 |
| base16_large | 7.93 ms | 5.18 ms | 0.654 | 0.654 | 0.654 |
| gc_traversal | 3.87 ms | 1.96 ms | 0.507 | 0.510 | 0.508 |

outliers > 1.3: bench_thread_pool 1.37, dask 1.36, connected_components 1.30
