build | workload | policy | run ms | GC ms/run | examined/run | RSS first → last MB | RSS growth 2nd−1st half MB | peak RSS MB
---|---|---|---|---|---|---|---|---
3.15.1 | services/cache_service | default | 962 | 0.0 | 0 | 56 → 59 | +0.5/+1.0 | 60
3.15.1 | services/cache_service | tier | 957 | 0.0 | 0 | 58 → 62 | +0.7/+0.9 | 63
3.15.1 | services/job_worker | default | 621 | 16.3 | 139,722 | 42 → 46 | +2.5/+1.2 | 46
3.15.1 | services/job_worker | tier | 607 | 16.4 | 139,634 | 44 → 48 | +4.3/+0.5 | 50
3.15.1 | services/uvicorn_fastapi | default | 931 | 61.6 | 694,838 | 72 → 74 | +1.2/+0.9 | 74
3.15.1 | services/uvicorn_fastapi | tier | 916 | 46.9 | 611,162 | 74 → 77 | +1.0/+0.7 | 78
3.15.1 | services/waitress_flask | default | 1608 | 5.8 | 36,093 | 51 → 52 | +0.4/+0.6 | 52
3.15.1 | services/waitress_flask | tier | 1651 | 5.9 | 35,987 | 53 → 55 | +0.5/+0.4 | 55
3.15.1t | services/cache_service | default | 1039 | 0.0 | 0 | 62 → 63 | +0.1/+0.1 | 63
3.15.1t | services/cache_service | tier | 1035 | 0.0 | 0 | 65 → 65 | +0.1/+0.0 | 65
3.15.1t | services/job_worker | default | 297 | 63.9 | 438,500 | 54 → 56 | +1.0/+0.7 | 57
3.15.1t | services/job_worker | tier | 298 | 63.2 | 440,309 | 56 → 58 | +0.8/+0.7 | 59
3.15.1t | services/uvicorn_fastapi | default | 1040 | 21.9 | 131,218 | 96 → 99 | +0.6/+1.2 | 101
3.15.1t | services/uvicorn_fastapi | tier | 1026 | 21.9 | 132,502 | 97 → 101 | +0.3/+1.0 | 102
3.15.1t | services/waitress_flask | default | 186 | 19.0 | 136,522 | 74 → 78 | +1.1/+1.2 | 78
3.15.1t | services/waitress_flask | tier | 186 | 19.1 | 136,776 | 76 → 80 | +1.0/+1.4 | 80
