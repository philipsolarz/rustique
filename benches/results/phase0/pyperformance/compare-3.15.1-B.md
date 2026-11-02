Benchmarks with tag 'apps':
===========================

+----------------+----------------+-----------------------+
| Benchmark      | plain-3.15.1-B | enabled-3.15.1-B      |
+================+================+=======================+
| 2to3           | 203 ms         | 205 ms: 1.01x slower  |
+----------------+----------------+-----------------------+
| chameleon      | 10.3 ms        | 10.2 ms: 1.01x faster |
+----------------+----------------+-----------------------+
| Geometric mean | (ref)          | 1.00x faster          |
+----------------+----------------+-----------------------+

Benchmark hidden because not significant (5): docutils, fastapi_http, html5lib, sphinx, tornado_http

Benchmarks with tag 'asyncio':
==============================

+------------------+----------------+-----------------------+
| Benchmark        | plain-3.15.1-B | enabled-3.15.1-B      |
+==================+================+=======================+
| async_tree_eager | 74.2 ms        | 73.3 ms: 1.01x faster |
+------------------+----------------+-----------------------+
| asyncio_tcp_ssl  | 883 ms         | 878 ms: 1.01x faster  |
+------------------+----------------+-----------------------+
| coroutines       | 17.7 ms        | 18.0 ms: 1.01x slower |
+------------------+----------------+-----------------------+
| async_generators | 311 ms         | 315 ms: 1.01x slower  |
+------------------+----------------+-----------------------+
| Geometric mean   | (ref)          | 1.00x slower          |
+------------------+----------------+-----------------------+

Benchmark hidden because not significant (16): asyncio_tcp, async_tree_io_tg, async_tree_eager_memoization_tg, async_tree_eager_memoization, async_tree_eager_cpu_io_mixed, async_tree_cpu_io_mixed_tg, async_tree_eager_cpu_io_mixed_tg, async_tree_cpu_io_mixed, async_tree_io, async_tree_memoization_tg, async_tree_eager_io_tg, async_tree_eager_tg, async_tree_none, async_tree_none_tg, async_tree_memoization, async_tree_eager_io

Benchmarks with tag 'math':
===========================

+----------------+----------------+-----------------------+
| Benchmark      | plain-3.15.1-B | enabled-3.15.1-B      |
+================+================+=======================+
| pidigits       | 175 ms         | 175 ms: 1.00x slower  |
+----------------+----------------+-----------------------+
| nbody          | 72.9 ms        | 73.8 ms: 1.01x slower |
+----------------+----------------+-----------------------+
| Geometric mean | (ref)          | 1.01x slower          |
+----------------+----------------+-----------------------+

Benchmark hidden because not significant (1): float

Benchmarks with tag 'regex':
============================

+----------------+----------------+-----------------------+
| Benchmark      | plain-3.15.1-B | enabled-3.15.1-B      |
+================+================+=======================+
| regex_effbot   | 2.12 ms        | 2.15 ms: 1.01x slower |
+----------------+----------------+-----------------------+
| regex_compile  | 74.4 ms        | 75.7 ms: 1.02x slower |
+----------------+----------------+-----------------------+
| Geometric mean | (ref)          | 1.01x slower          |
+----------------+----------------+-----------------------+

Benchmark hidden because not significant (2): regex_v8, regex_dna

Benchmarks with tag 'serialize':
================================

+----------------------+----------------+------------------------+
| Benchmark            | plain-3.15.1-B | enabled-3.15.1-B       |
+======================+================+========================+
| urlsafe_base64_small | 244 us         | 240 us: 1.02x faster   |
+----------------------+----------------+------------------------+
| tomli_loads          | 1.46 sec       | 1.44 sec: 1.01x faster |
+----------------------+----------------+------------------------+
| base85_large         | 5.33 ms        | 5.35 ms: 1.00x slower  |
+----------------------+----------------+------------------------+
| base85_small         | 199 us         | 200 us: 1.01x slower   |
+----------------------+----------------+------------------------+
| xml_etree_process    | 43.3 ms        | 43.7 ms: 1.01x slower  |
+----------------------+----------------+------------------------+
| pickle_list          | 2.53 us        | 2.55 us: 1.01x slower  |
+----------------------+----------------+------------------------+
| pickle_pure_python   | 223 us         | 225 us: 1.01x slower   |
+----------------------+----------------+------------------------+
| unpickle_pure_python | 157 us         | 159 us: 1.01x slower   |
+----------------------+----------------+------------------------+
| base16_small         | 339 us         | 343 us: 1.01x slower   |
+----------------------+----------------+------------------------+
| pickle               | 7.60 us        | 7.82 us: 1.03x slower  |
+----------------------+----------------+------------------------+
| xml_etree_generate   | 59.7 ms        | 61.6 ms: 1.03x slower  |
+----------------------+----------------+------------------------+
| Geometric mean       | (ref)          | 1.00x slower           |
+----------------------+----------------+------------------------+

Benchmark hidden because not significant (14): unpickle_list, ascii85_large, pickle_dict, base16_large, xml_etree_parse, base32_large, base64_large, json_dumps, base64_small, ascii85_small, xml_etree_iterparse, base32_small, json_loads, unpickle

Benchmarks with tag 'startup':
==============================

+------------------------+----------------+-----------------------+
| Benchmark              | plain-3.15.1-B | enabled-3.15.1-B      |
+========================+================+=======================+
| python_startup_no_site | 7.32 ms        | 7.38 ms: 1.01x slower |
+------------------------+----------------+-----------------------+
| python_startup         | 10.5 ms        | 12.3 ms: 1.18x slower |
+------------------------+----------------+-----------------------+
| Geometric mean         | (ref)          | 1.09x slower          |
+------------------------+----------------+-----------------------+

Benchmarks with tag 'template':
===============================

Benchmark hidden because not significant (2): django_template, mako

All benchmarks:
===============

+-------------------------+----------------+------------------------+
| Benchmark               | plain-3.15.1-B | enabled-3.15.1-B       |
+=========================+================+========================+
| meteor_contest          | 75.9 ms        | 71.1 ms: 1.07x faster  |
+-------------------------+----------------+------------------------+
| hexiom                  | 4.37 ms        | 4.11 ms: 1.06x faster  |
+-------------------------+----------------+------------------------+
| urlsafe_base64_small    | 244 us         | 240 us: 1.02x faster   |
+-------------------------+----------------+------------------------+
| pyflate                 | 319 ms         | 314 ms: 1.01x faster   |
+-------------------------+----------------+------------------------+
| chameleon               | 10.3 ms        | 10.2 ms: 1.01x faster  |
+-------------------------+----------------+------------------------+
| tomli_loads             | 1.46 sec       | 1.44 sec: 1.01x faster |
+-------------------------+----------------+------------------------+
| async_tree_eager        | 74.2 ms        | 73.3 ms: 1.01x faster  |
+-------------------------+----------------+------------------------+
| deltablue               | 2.43 ms        | 2.41 ms: 1.01x faster  |
+-------------------------+----------------+------------------------+
| sympy_expand            | 273 ms         | 271 ms: 1.01x faster   |
+-------------------------+----------------+------------------------+
| bench_thread_pool       | 851 us         | 846 us: 1.01x faster   |
+-------------------------+----------------+------------------------+
| sqlglot_v2_optimize     | 36.5 ms        | 36.3 ms: 1.01x faster  |
+-------------------------+----------------+------------------------+
| asyncio_tcp_ssl         | 883 ms         | 878 ms: 1.01x faster   |
+-------------------------+----------------+------------------------+
| sqlglot_v2_normalize    | 73.2 ms        | 72.9 ms: 1.00x faster  |
+-------------------------+----------------+------------------------+
| sympy_str               | 157 ms         | 157 ms: 1.00x faster   |
+-------------------------+----------------+------------------------+
| sympy_integrate         | 13.1 ms        | 13.1 ms: 1.00x faster  |
+-------------------------+----------------+------------------------+
| pidigits                | 175 ms         | 175 ms: 1.00x slower   |
+-------------------------+----------------+------------------------+
| dulwich_log             | 30.1 ms        | 30.2 ms: 1.00x slower  |
+-------------------------+----------------+------------------------+
| many_optionals          | 396 us         | 398 us: 1.00x slower   |
+-------------------------+----------------+------------------------+
| base85_large            | 5.33 ms        | 5.35 ms: 1.00x slower  |
+-------------------------+----------------+------------------------+
| sqlalchemy_declarative  | 76.7 ms        | 77.0 ms: 1.00x slower  |
+-------------------------+----------------+------------------------+
| scimark_fft             | 192 ms         | 193 ms: 1.00x slower   |
+-------------------------+----------------+------------------------+
| richards                | 32.6 ms        | 32.8 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| base85_small            | 199 us         | 200 us: 1.01x slower   |
+-------------------------+----------------+------------------------+
| sqlglot_v2_parse        | 816 us         | 821 us: 1.01x slower   |
+-------------------------+----------------+------------------------+
| subparsers              | 7.42 ms        | 7.47 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| deepcopy_memo           | 20.5 us        | 20.7 us: 1.01x slower  |
+-------------------------+----------------+------------------------+
| sympy_sum               | 87.0 ms        | 87.6 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| deepcopy_reduce         | 1.85 us        | 1.86 us: 1.01x slower  |
+-------------------------+----------------+------------------------+
| 2to3                    | 203 ms         | 205 ms: 1.01x slower   |
+-------------------------+----------------+------------------------+
| python_startup_no_site  | 7.32 ms        | 7.38 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| xml_etree_process       | 43.3 ms        | 43.7 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| pickle_list             | 2.53 us        | 2.55 us: 1.01x slower  |
+-------------------------+----------------+------------------------+
| logging_format          | 4.97 us        | 5.02 us: 1.01x slower  |
+-------------------------+----------------+------------------------+
| telco                   | 4.66 ms        | 4.71 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| scimark_sparse_mat_mult | 3.31 ms        | 3.34 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| pickle_pure_python      | 223 us         | 225 us: 1.01x slower   |
+-------------------------+----------------+------------------------+
| unpickle_pure_python    | 157 us         | 159 us: 1.01x slower   |
+-------------------------+----------------+------------------------+
| sqlite_synth            | 1.53 us        | 1.55 us: 1.01x slower  |
+-------------------------+----------------+------------------------+
| connected_components    | 471 ms         | 476 ms: 1.01x slower   |
+-------------------------+----------------+------------------------+
| pprint_safe_repr        | 540 ms         | 546 ms: 1.01x slower   |
+-------------------------+----------------+------------------------+
| nbody                   | 72.9 ms        | 73.8 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| base16_small            | 339 us         | 343 us: 1.01x slower   |
+-------------------------+----------------+------------------------+
| crypto_pyaes            | 54.4 ms        | 55.0 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| regex_effbot            | 2.12 ms        | 2.15 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| coroutines              | 17.7 ms        | 18.0 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| async_generators        | 311 ms         | 315 ms: 1.01x slower   |
+-------------------------+----------------+------------------------+
| chaos                   | 40.2 ms        | 40.8 ms: 1.01x slower  |
+-------------------------+----------------+------------------------+
| gc_traversal            | 3.87 ms        | 3.93 ms: 1.02x slower  |
+-------------------------+----------------+------------------------+
| regex_compile           | 74.4 ms        | 75.7 ms: 1.02x slower  |
+-------------------------+----------------+------------------------+
| shortest_path           | 490 ms         | 500 ms: 1.02x slower   |
+-------------------------+----------------+------------------------+
| k_core                  | 2.43 sec       | 2.48 sec: 1.02x slower |
+-------------------------+----------------+------------------------+
| pickle                  | 7.60 us        | 7.82 us: 1.03x slower  |
+-------------------------+----------------+------------------------+
| create_gc_cycles        | 1.61 ms        | 1.66 ms: 1.03x slower  |
+-------------------------+----------------+------------------------+
| xml_etree_generate      | 59.7 ms        | 61.6 ms: 1.03x slower  |
+-------------------------+----------------+------------------------+
| scimark_lu              | 79.0 ms        | 82.0 ms: 1.04x slower  |
+-------------------------+----------------+------------------------+
| unpack_sequence         | 33.1 ns        | 35.9 ns: 1.09x slower  |
+-------------------------+----------------+------------------------+
| python_startup          | 10.5 ms        | 12.3 ms: 1.18x slower  |
+-------------------------+----------------+------------------------+
| Geometric mean          | (ref)          | 1.00x slower           |
+-------------------------+----------------+------------------------+

Benchmark hidden because not significant (64): logging_simple, asyncio_tcp, async_tree_io_tg, pprint_pformat, unpickle_list, async_tree_eager_memoization_tg, tornado_http, async_tree_eager_memoization, typing_runtime_protocols, async_tree_eager_cpu_io_mixed, raytrace, async_tree_cpu_io_mixed_tg, richards_super, scimark_sor, ascii85_large, pickle_dict, scimark_monte_carlo, sqlalchemy_imperative, base16_large, bpe_tokeniser, regex_v8, xml_etree_parse, django_template, pathlib, base32_large, base64_large, mdp, comprehensions, go, async_tree_eager_cpu_io_mixed_tg, sphinx, nqueens, sqlglot_v2_transpile, dask, async_tree_cpu_io_mixed, html5lib, async_tree_io, json_dumps, base64_small, generators, regex_dna, ascii85_small, spectral_norm, xml_etree_iterparse, docutils, base32_small, bench_mp_pool, json_loads, async_tree_memoization_tg, deepcopy, fastapi_http, async_tree_eager_io_tg, unpickle, async_tree_eager_tg, coverage, async_tree_none, async_tree_none_tg, async_tree_memoization, logging_silent, fannkuch, mako, float, async_tree_eager_io, xdsl_constant_fold
Ignored benchmarks (1) of /home/ubuntu/phase0/results/pyperformance/enabled-3.15.1-B.json: asyncio_websockets
