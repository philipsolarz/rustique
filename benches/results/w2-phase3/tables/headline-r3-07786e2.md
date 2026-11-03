| family (n) | 3.15.1 rep 3 (07786e2) | 3.15.1 rep 2 (7fec7e4) | 3.15.1t rep 3 | 3.15.1t rep 2 |
|---|---:|---:|---:|---:|
| web | **0.885** (10) | 0.882 | **0.901** (10) | 0.903 |
| data | **0.751** (12) | 0.752 | **0.736** (12) | 0.735 |
| devtools | **0.958** (12) | 0.957 | **0.861** (12) | 0.861 |
| frameworks | **0.943** (6) | 0.940 | **0.953** (6) | 0.955 |
| libs | **0.961** (6) | 0.961 | **0.967** (6) | 0.966 |
| custom | **0.942** (10) | 0.936 | **0.974** (10) | 0.973 |
| services | **0.852** (4) | 0.848 | **0.884** (4) | 0.888 |
| threads | **0.687** (5) | 0.685 | **0.784** (5) | 0.792 |
| wave 1's 34 | **0.859** (34) | 0.858 | **0.826** (34) | 0.826 |
| the 31 new | **0.888** (31) | 0.884 | **0.924** (31) | 0.925 |
| all 65 | **0.872** (65) | 0.870 | **0.871** (65) | 0.872 |

3.15.1: n=65; median 0.968; best data/report_render 0.356; worst web/email_mime 1.005; > 1.02: []; mismatches: []; plain rep3/rep2 0.9992; moves > 2% (rep 3, rep 2): [('data/billing_engine', 0.966, 0.989), ('custom/report_engine', 0.861, 0.843)]
3.15.1t: n=65; median 0.971; best devtools/isort_stdlib 0.202; worst custom/mini_interp 1.023; > 1.02: [('custom/mini_interp', 1.023)]; mismatches: []; plain rep3/rep2 1.0001; moves > 2% (rep 3, rep 2): [('data/csv_etl', 0.65, 0.632), ('services/uvicorn_fastapi', 0.972, 0.992), ('threads/request_pool', 0.754, 0.778)]
