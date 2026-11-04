build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Pattern.match (compiled) | local | 115 | 1.001 | 7.63 | 7.59 | 0.996 | 0.13 | 0.13 | 0.997 |
| Pattern.match (compiled) | shared | 120 | 1.030 | 7.26 | 7.06 | 1.000 | 0.13 | 0.14 | 1.003 |
| re.match (module cache) | local | 303 | 0.630 | 6.25 | 7.29 | 0.735 | 0.14 | 0.12 | 0.728 |
| re.match (module cache) | shared | 304 | 0.640 | 6.27 | 7.15 | 0.730 | 0.14 | 0.12 | 0.727 |
