
### 3.15.1t — 0-100 elements, callgrind Ir

cells 3814; mismatches/errors: 0 []

| category | cells | ≤ 1.05 phase 3 | ≤ 1.05 phase 0 | geomean phase 3 | geomean phase 0 | median phase 3 | geomean phase3/phase0 per cell |
|---|---:|---:|---:|---:|---:|---:|---:|
| kernel path | 586 | 586 | 583 | 0.6492 | 0.6491 | 0.7378 | 1.0002 |
| wave-1 serial (json.dumps, copy.deepcopy) | 152 | 152 | 152 | 0.1614 | 0.1613 | 0.1831 | 1.0000 |
| other ops | 3076 | 3074 | 3074 | 0.9915 | 0.9915 | 1.0000 | 1.0000 |
| all | 3814 | 3812 | 3809 | 0.8642 | 0.8642 | 1.0000 | 1.0000 |

cells > 1.05: 2
  1.074 (phase 0 1.074)  [other ] list/x + y/str/8
  1.068 (phase 0 1.068)  [other ] list/x * 2/str/8

ops whose geomean moved > 3% vs phase 0: 0
