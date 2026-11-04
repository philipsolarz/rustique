| workload | build | mode | 1 thr ms | 2 thr ms | 4 thr ms | 8 thr ms | x2 | x4 | x8 |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| threads/compute_pool | 3.15.1 | plain | 1181 | 1185 | 1189 | 1193 | 1.00 | 0.99 | 0.99 |
| threads/compute_pool | 3.15.1 | enable | 1153 | 1159 | 1167 | 1165 | 0.99 | 0.99 | 0.99 |
| threads/compute_pool | 3.15.1t | plain | 1301 | 661 | 337 | 173 | 1.97 | 3.86 | 7.51 |
| threads/compute_pool | 3.15.1t | enable | 1282 | 648 | 330 | 170 | 1.98 | 3.89 | 7.54 |
| threads/http_server | 3.15.1 | plain | 492 | 525 | 561 | 628 | 0.94 | 0.88 | 0.78 |
| threads/http_server | 3.15.1 | enable | 433 | 474 | 510 | 574 | 0.91 | 0.85 | 0.76 |
| threads/http_server | 3.15.1t | plain | 448 | 236 | 128 | 89 | 1.90 | 3.51 | 5.04 |
| threads/http_server | 3.15.1t | enable | 391 | 206 | 109 | 79 | 1.90 | 3.58 | 4.97 |
| threads/pipeline_queue | 3.15.1 | plain | 593 | 589 | 588 | 592 | 1.01 | 1.01 | 1.00 |
| threads/pipeline_queue | 3.15.1 | enable | 308 | 306 | 304 | 303 | 1.01 | 1.01 | 1.02 |
| threads/pipeline_queue | 3.15.1t | plain | 672 | 463 | 328 | 265 | 1.45 | 2.04 | 2.54 |
| threads/pipeline_queue | 3.15.1t | enable | 357 | 281 | 221 | 189 | 1.27 | 1.62 | 1.89 |
| threads/request_pool | 3.15.1 | plain | 429 | 443 | 443 | 451 | 0.97 | 0.97 | 0.95 |
| threads/request_pool | 3.15.1 | enable | 239 | 241 | 242 | 246 | 0.99 | 0.99 | 0.97 |
| threads/request_pool | 3.15.1t | plain | 468 | 287 | 186 | 139 | 1.63 | 2.52 | 3.38 |
| threads/request_pool | 3.15.1t | enable | 270 | 177 | 129 | 107 | 1.53 | 2.10 | 2.52 |
| threads/shared_cache | 3.15.1 | plain | 490 | 497 | 513 | 526 | 0.99 | 0.96 | 0.93 |
| threads/shared_cache | 3.15.1 | enable | 290 | 295 | 307 | 315 | 0.98 | 0.95 | 0.92 |
| threads/shared_cache | 3.15.1t | plain | 576 | 408 | 282 | 228 | 1.41 | 2.05 | 2.53 |
| threads/shared_cache | 3.15.1t | enable | 366 | 262 | 180 | 141 | 1.40 | 2.03 | 2.61 |

enabled / plain (lower is better):

| workload | build | 1 thr | 2 thr | 4 thr | 8 thr |
|---|---|---:|---:|---:|---:|
| threads/compute_pool | 3.15.1 | 0.976 | 0.978 | 0.981 | 0.977 |
| threads/compute_pool | 3.15.1t | 0.985 | 0.981 | 0.980 | 0.981 |
| threads/http_server | 3.15.1 | 0.881 | 0.902 | 0.910 | 0.914 |
| threads/http_server | 3.15.1t | 0.872 | 0.872 | 0.854 | 0.884 |
| threads/pipeline_queue | 3.15.1 | 0.520 | 0.520 | 0.517 | 0.512 |
| threads/pipeline_queue | 3.15.1t | 0.531 | 0.608 | 0.671 | 0.714 |
| threads/request_pool | 3.15.1 | 0.557 | 0.545 | 0.547 | 0.545 |
| threads/request_pool | 3.15.1t | 0.577 | 0.615 | 0.693 | 0.773 |
| threads/shared_cache | 3.15.1 | 0.591 | 0.593 | 0.597 | 0.599 |
| threads/shared_cache | 3.15.1t | 0.636 | 0.643 | 0.640 | 0.617 |

t build / GIL build (3.15.1t / 3.15.1), same mode and thread count:

| workload | mode | 1 thr | 2 thr | 4 thr | 8 thr |
|---|---|---:|---:|---:|---:|
| threads/compute_pool | plain | 1.101 | 0.558 | 0.283 | 0.145 |
| threads/compute_pool | enable | 1.112 | 0.559 | 0.283 | 0.146 |
| threads/http_server | plain | 0.911 | 0.449 | 0.228 | 0.142 |
| threads/http_server | enable | 0.902 | 0.434 | 0.214 | 0.137 |
| threads/pipeline_queue | plain | 1.133 | 0.786 | 0.559 | 0.447 |
| threads/pipeline_queue | enable | 1.159 | 0.919 | 0.726 | 0.623 |
| threads/request_pool | plain | 1.090 | 0.648 | 0.420 | 0.307 |
| threads/request_pool | enable | 1.130 | 0.732 | 0.531 | 0.435 |
| threads/shared_cache | plain | 1.176 | 0.821 | 0.549 | 0.433 |
| threads/shared_cache | enable | 1.265 | 0.891 | 0.588 | 0.446 |
