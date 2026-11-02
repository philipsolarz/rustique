Benchmarks with tag 'apps':
===========================

+----------------+--------------+-----------------------+
| Benchmark      | plain-3.15.1 | enabled-3.15.1        |
+================+==============+=======================+
| 2to3           | 203 ms       | 205 ms: 1.01x slower  |
+----------------+--------------+-----------------------+
| chameleon      | 10.3 ms      | 10.2 ms: 1.01x faster |
+----------------+--------------+-----------------------+
| fastapi_http   | 218 ms       | 217 ms: 1.00x faster  |
+----------------+--------------+-----------------------+
| Geometric mean | (ref)        | 1.00x faster          |
+----------------+--------------+-----------------------+

Benchmark hidden because not significant (4): docutils, html5lib, sphinx, tornado_http

Benchmarks with tag 'asyncio':
==============================

+--------------------+--------------+-----------------------+
| Benchmark          | plain-3.15.1 | enabled-3.15.1        |
+====================+==============+=======================+
| async_tree_eager   | 74.5 ms      | 73.4 ms: 1.02x faster |
+--------------------+--------------+-----------------------+
| asyncio_tcp_ssl    | 879 ms       | 873 ms: 1.01x faster  |
+--------------------+--------------+-----------------------+
| asyncio_websockets | 438 ms       | 436 ms: 1.00x faster  |
+--------------------+--------------+-----------------------+
| async_generators   | 311 ms       | 314 ms: 1.01x slower  |
+--------------------+--------------+-----------------------+
| Geometric mean     | (ref)        | 1.00x faster          |
+--------------------+--------------+-----------------------+

Benchmark hidden because not significant (17): asyncio_tcp, async_tree_io, async_tree_eager_memoization, async_tree_eager_cpu_io_mixed, async_tree_eager_io, async_tree_eager_memoization_tg, async_tree_cpu_io_mixed, async_tree_eager_io_tg, async_tree_cpu_io_mixed_tg, async_tree_io_tg, async_tree_memoization_tg, coroutines, async_tree_memoization, async_tree_eager_cpu_io_mixed_tg, async_tree_eager_tg, async_tree_none_tg, async_tree_none

Benchmarks with tag 'math':
===========================

+----------------+--------------+-----------------------+
| Benchmark      | plain-3.15.1 | enabled-3.15.1        |
+================+==============+=======================+
| nbody          | 74.5 ms      | 73.6 ms: 1.01x faster |
+----------------+--------------+-----------------------+
| pidigits       | 175 ms       | 175 ms: 1.00x slower  |
+----------------+--------------+-----------------------+
| Geometric mean | (ref)        | 1.00x faster          |
+----------------+--------------+-----------------------+

Benchmark hidden because not significant (1): float

Benchmarks with tag 'regex':
============================

+----------------+--------------+-----------------------+
| Benchmark      | plain-3.15.1 | enabled-3.15.1        |
+================+==============+=======================+
| regex_compile  | 74.7 ms      | 75.6 ms: 1.01x slower |
+----------------+--------------+-----------------------+
| Geometric mean | (ref)        | 1.00x slower          |
+----------------+--------------+-----------------------+

Benchmark hidden because not significant (3): regex_dna, regex_v8, regex_effbot

Benchmarks with tag 'serialize':
================================

+----------------------+--------------+------------------------+
| Benchmark            | plain-3.15.1 | enabled-3.15.1         |
+======================+==============+========================+
| unpickle_list        | 2.86 us      | 2.81 us: 1.02x faster  |
+----------------------+--------------+------------------------+
| base32_large         | 2.56 ms      | 2.56 ms: 1.00x faster  |
+----------------------+--------------+------------------------+
| pickle_dict          | 15.6 us      | 15.6 us: 1.00x slower  |
+----------------------+--------------+------------------------+
| tomli_loads          | 1.44 sec     | 1.45 sec: 1.00x slower |
+----------------------+--------------+------------------------+
| json_loads           | 17.6 us      | 17.7 us: 1.00x slower  |
+----------------------+--------------+------------------------+
| json_dumps           | 6.02 ms      | 6.05 ms: 1.00x slower  |
+----------------------+--------------+------------------------+
| pickle_pure_python   | 223 us       | 224 us: 1.00x slower   |
+----------------------+--------------+------------------------+
| xml_etree_process    | 43.3 ms      | 43.6 ms: 1.01x slower  |
+----------------------+--------------+------------------------+
| pickle_list          | 2.53 us      | 2.54 us: 1.01x slower  |
+----------------------+--------------+------------------------+
| xml_etree_generate   | 59.9 ms      | 60.4 ms: 1.01x slower  |
+----------------------+--------------+------------------------+
| base85_small         | 199 us       | 201 us: 1.01x slower   |
+----------------------+--------------+------------------------+
| pickle               | 7.61 us      | 7.72 us: 1.01x slower  |
+----------------------+--------------+------------------------+
| unpickle_pure_python | 157 us       | 159 us: 1.02x slower   |
+----------------------+--------------+------------------------+
| Geometric mean       | (ref)        | 1.00x slower           |
+----------------------+--------------+------------------------+

Benchmark hidden because not significant (12): urlsafe_base64_small, base16_large, base85_large, base16_small, base64_large, unpickle, ascii85_small, ascii85_large, xml_etree_parse, base64_small, xml_etree_iterparse, base32_small

Benchmarks with tag 'startup':
==============================

+------------------------+--------------+-----------------------+
| Benchmark              | plain-3.15.1 | enabled-3.15.1        |
+========================+==============+=======================+
| python_startup_no_site | 7.29 ms      | 7.30 ms: 1.00x slower |
+------------------------+--------------+-----------------------+
| python_startup         | 10.4 ms      | 12.2 ms: 1.17x slower |
+------------------------+--------------+-----------------------+
| Geometric mean         | (ref)        | 1.08x slower          |
+------------------------+--------------+-----------------------+

Benchmarks with tag 'template':
===============================

Benchmark hidden because not significant (2): django_template, mako

All benchmarks:
===============

+--------------------------+--------------+------------------------+
| Benchmark                | plain-3.15.1 | enabled-3.15.1         |
+==========================+==============+========================+
| meteor_contest           | 76.8 ms      | 71.2 ms: 1.08x faster  |
+--------------------------+--------------+------------------------+
| hexiom                   | 4.41 ms      | 4.11 ms: 1.07x faster  |
+--------------------------+--------------+------------------------+
| pyflate                  | 320 ms       | 314 ms: 1.02x faster   |
+--------------------------+--------------+------------------------+
| unpickle_list            | 2.86 us      | 2.81 us: 1.02x faster  |
+--------------------------+--------------+------------------------+
| shortest_path            | 503 ms       | 495 ms: 1.02x faster   |
+--------------------------+--------------+------------------------+
| async_tree_eager         | 74.5 ms      | 73.4 ms: 1.02x faster  |
+--------------------------+--------------+------------------------+
| unpack_sequence          | 36.3 ns      | 35.8 ns: 1.02x faster  |
+--------------------------+--------------+------------------------+
| sympy_str                | 159 ms       | 157 ms: 1.01x faster   |
+--------------------------+--------------+------------------------+
| chameleon                | 10.3 ms      | 10.2 ms: 1.01x faster  |
+--------------------------+--------------+------------------------+
| nbody                    | 74.5 ms      | 73.6 ms: 1.01x faster  |
+--------------------------+--------------+------------------------+
| deltablue                | 2.43 ms      | 2.41 ms: 1.01x faster  |
+--------------------------+--------------+------------------------+
| asyncio_tcp_ssl          | 879 ms       | 873 ms: 1.01x faster   |
+--------------------------+--------------+------------------------+
| richards                 | 32.7 ms      | 32.5 ms: 1.01x faster  |
+--------------------------+--------------+------------------------+
| dulwich_log              | 30.3 ms      | 30.2 ms: 1.01x faster  |
+--------------------------+--------------+------------------------+
| nqueens                  | 56.3 ms      | 56.0 ms: 1.01x faster  |
+--------------------------+--------------+------------------------+
| sqlglot_v2_parse         | 821 us       | 818 us: 1.00x faster   |
+--------------------------+--------------+------------------------+
| fastapi_http             | 218 ms       | 217 ms: 1.00x faster   |
+--------------------------+--------------+------------------------+
| sqlite_synth             | 1.53 us      | 1.52 us: 1.00x faster  |
+--------------------------+--------------+------------------------+
| asyncio_websockets       | 438 ms       | 436 ms: 1.00x faster   |
+--------------------------+--------------+------------------------+
| sqlglot_v2_transpile     | 1.03 ms      | 1.03 ms: 1.00x faster  |
+--------------------------+--------------+------------------------+
| sympy_expand             | 272 ms       | 271 ms: 1.00x faster   |
+--------------------------+--------------+------------------------+
| base32_large             | 2.56 ms      | 2.56 ms: 1.00x faster  |
+--------------------------+--------------+------------------------+
| pidigits                 | 175 ms       | 175 ms: 1.00x slower   |
+--------------------------+--------------+------------------------+
| python_startup_no_site   | 7.29 ms      | 7.30 ms: 1.00x slower  |
+--------------------------+--------------+------------------------+
| pickle_dict              | 15.6 us      | 15.6 us: 1.00x slower  |
+--------------------------+--------------+------------------------+
| bench_thread_pool        | 846 us       | 849 us: 1.00x slower   |
+--------------------------+--------------+------------------------+
| sqlalchemy_declarative   | 76.6 ms      | 76.9 ms: 1.00x slower  |
+--------------------------+--------------+------------------------+
| logging_format           | 5.01 us      | 5.03 us: 1.00x slower  |
+--------------------------+--------------+------------------------+
| tomli_loads              | 1.44 sec     | 1.45 sec: 1.00x slower |
+--------------------------+--------------+------------------------+
| many_optionals           | 396 us       | 398 us: 1.00x slower   |
+--------------------------+--------------+------------------------+
| json_loads               | 17.6 us      | 17.7 us: 1.00x slower  |
+--------------------------+--------------+------------------------+
| scimark_sparse_mat_mult  | 3.31 ms      | 3.33 ms: 1.00x slower  |
+--------------------------+--------------+------------------------+
| json_dumps               | 6.02 ms      | 6.05 ms: 1.00x slower  |
+--------------------------+--------------+------------------------+
| pickle_pure_python       | 223 us       | 224 us: 1.00x slower   |
+--------------------------+--------------+------------------------+
| pprint_safe_repr         | 543 ms       | 545 ms: 1.00x slower   |
+--------------------------+--------------+------------------------+
| xml_etree_process        | 43.3 ms      | 43.6 ms: 1.01x slower  |
+--------------------------+--------------+------------------------+
| deepcopy_memo            | 20.6 us      | 20.7 us: 1.01x slower  |
+--------------------------+--------------+------------------------+
| pickle_list              | 2.53 us      | 2.54 us: 1.01x slower  |
+--------------------------+--------------+------------------------+
| scimark_sor              | 80.1 ms      | 80.6 ms: 1.01x slower  |
+--------------------------+--------------+------------------------+
| async_generators         | 311 ms       | 314 ms: 1.01x slower   |
+--------------------------+--------------+------------------------+
| xml_etree_generate       | 59.9 ms      | 60.4 ms: 1.01x slower  |
+--------------------------+--------------+------------------------+
| pprint_pformat           | 1.11 sec     | 1.12 sec: 1.01x slower |
+--------------------------+--------------+------------------------+
| 2to3                     | 203 ms       | 205 ms: 1.01x slower   |
+--------------------------+--------------+------------------------+
| regex_compile            | 74.7 ms      | 75.6 ms: 1.01x slower  |
+--------------------------+--------------+------------------------+
| typing_runtime_protocols | 110 us       | 111 us: 1.01x slower   |
+--------------------------+--------------+------------------------+
| gc_traversal             | 3.88 ms      | 3.93 ms: 1.01x slower  |
+--------------------------+--------------+------------------------+
| base85_small             | 199 us       | 201 us: 1.01x slower   |
+--------------------------+--------------+------------------------+
| pickle                   | 7.61 us      | 7.72 us: 1.01x slower  |
+--------------------------+--------------+------------------------+
| unpickle_pure_python     | 157 us       | 159 us: 1.02x slower   |
+--------------------------+--------------+------------------------+
| create_gc_cycles         | 1.62 ms      | 1.65 ms: 1.02x slower  |
+--------------------------+--------------+------------------------+
| python_startup           | 10.4 ms      | 12.2 ms: 1.17x slower  |
+--------------------------+--------------+------------------------+
| Geometric mean           | (ref)        | 1.00x faster           |
+--------------------------+--------------+------------------------+

Benchmark hidden because not significant (71): xdsl_constant_fold, asyncio_tcp, regex_dna, async_tree_io, async_tree_eager_memoization, urlsafe_base64_small, k_core, async_tree_eager_cpu_io_mixed, deepcopy, async_tree_eager_io, scimark_monte_carlo, async_tree_eager_memoization_tg, tornado_http, async_tree_cpu_io_mixed, comprehensions, django_template, base16_large, mdp, bpe_tokeniser, sqlglot_v2_normalize, async_tree_eager_io_tg, base85_large, deepcopy_reduce, go, raytrace, sympy_integrate, base16_small, pathlib, generators, async_tree_cpu_io_mixed_tg, async_tree_io_tg, regex_v8, async_tree_memoization_tg, telco, fannkuch, sqlglot_v2_optimize, sqlalchemy_imperative, sympy_sum, coverage, coroutines, async_tree_memoization, scimark_fft, bench_mp_pool, sphinx, connected_components, base64_large, spectral_norm, async_tree_eager_cpu_io_mixed_tg, richards_super, unpickle, logging_silent, html5lib, async_tree_eager_tg, logging_simple, async_tree_none_tg, ascii85_small, dask, ascii85_large, regex_effbot, xml_etree_parse, crypto_pyaes, subparsers, mako, scimark_lu, float, docutils, base64_small, xml_etree_iterparse, async_tree_none, base32_small, chaos
