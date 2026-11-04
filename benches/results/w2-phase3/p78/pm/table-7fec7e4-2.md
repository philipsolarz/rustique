build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Pattern.match (compiled) | local | 115 | 1.028 | 7.58 | 7.40 | 1.004 | 0.13 | 0.13 | 1.002 |
| Pattern.match (compiled) | shared | 122 | 1.014 | 7.13 | 7.08 | 1.006 | 0.13 | 0.14 | 1.002 |
| re.match (module cache) | local | 303 | 0.655 | 6.31 | 7.12 | 0.739 | 0.14 | 0.12 | 0.740 |
| re.match (module cache) | shared | 302 | 0.652 | 6.23 | 7.19 | 0.753 | 0.14 | 0.12 | 0.751 |
