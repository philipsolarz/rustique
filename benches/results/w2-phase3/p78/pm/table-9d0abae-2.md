build 3.15.1t; ns = wall time per call per thread; eff = time(1 thread) / time(8 threads) with every thread doing the same work (1.00 = perfect scaling, 0.12 = serialized); enabled/plain < 1 = Rustique faster

CPU ns = CPU time per call (sum over threads / calls): contention (spinning, cache-line bouncing) raises it at 8 threads; a lock that parks the thread raises wall time only. Wall time is sensitive to other processes on the CPU set; CPU time is not.

| cell | inputs | plain ns (1 thr) | enabled/plain (1 thr) | plain CPU ns x8/x1 | enabled CPU ns x8/x1 | enabled/plain CPU (8 thr) | plain eff | enabled eff | enabled/plain wall (8 thr) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Pattern.match (compiled) | local | 120 | 0.948 | 7.26 | 7.67 | 1.002 | 0.13 | 0.12 | 1.005 |
| Pattern.match (compiled) | shared | 123 | 0.974 | 7.08 | 7.26 | 0.999 | 0.13 | 0.13 | 0.998 |
| re.match (module cache) | local | 314 | 0.611 | 6.20 | 7.25 | 0.715 | 0.14 | 0.12 | 0.710 |
| re.match (module cache) | shared | 315 | 0.615 | 6.13 | 7.18 | 0.721 | 0.14 | 0.12 | 0.717 |
