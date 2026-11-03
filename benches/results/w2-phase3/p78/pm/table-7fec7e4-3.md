build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Pattern.match (compiled) | local | 117 | 1.010 | 7.45 | 7.45 | 1.010 | 0.13 | 0.13 | 1.013 |
| Pattern.match (compiled) | shared | 120 | 1.005 | 7.28 | 7.17 | 0.991 | 0.13 | 0.13 | 0.992 |
| re.match (module cache) | local | 302 | 0.674 | 6.31 | 6.83 | 0.730 | 0.14 | 0.13 | 0.713 |
| re.match (module cache) | shared | 304 | 0.641 | 6.29 | 7.09 | 0.722 | 0.14 | 0.12 | 0.710 |
