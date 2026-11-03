build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Pattern.match (compiled) | local | 116 | 1.040 | 7.56 | 7.25 | 0.998 | 0.13 | 0.13 | 0.997 |
| Pattern.match (compiled) | shared | 120 | 1.014 | 7.23 | 7.18 | 1.006 | 0.13 | 0.13 | 1.004 |
| re.match (module cache) | local | 297 | 0.656 | 6.40 | 7.13 | 0.731 | 0.14 | 0.12 | 0.726 |
| re.match (module cache) | shared | 304 | 0.650 | 6.24 | 7.03 | 0.733 | 0.14 | 0.13 | 0.731 |
