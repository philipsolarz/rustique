Benchmarks with tag 'asyncio':
==============================

+-----------------+---------------------+------------------------+
| Benchmark       | plain-3.15.1t-ports | enabled-3.15.1t-ports  |
+=================+=====================+========================+
| asyncio_tcp_ssl | 1.09 sec            | 1.10 sec: 1.01x slower |
+-----------------+---------------------+------------------------+
| Geometric mean  | (ref)               | 1.00x slower           |
+-----------------+---------------------+------------------------+

Benchmark hidden because not significant (2): asyncio_tcp, asyncio_websockets

All benchmarks:
===============

+-----------------+---------------------+------------------------+
| Benchmark       | plain-3.15.1t-ports | enabled-3.15.1t-ports  |
+=================+=====================+========================+
| asyncio_tcp_ssl | 1.09 sec            | 1.10 sec: 1.01x slower |
+-----------------+---------------------+------------------------+
| Geometric mean  | (ref)               | 1.00x slower           |
+-----------------+---------------------+------------------------+

Benchmark hidden because not significant (2): asyncio_websockets, asyncio_tcp
