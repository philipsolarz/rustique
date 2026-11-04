| run | tests | passed | failed | errors | skipped | time s |
|---|---:|---:|---:|---:|---:|---:|
| pytest-3.14.9.xml | 5831 | 5808 | 0 | 0 | 23 | 238.5 |
| pytest-3.14.9t.xml | 5831 | 5811 | 0 | 0 | 20 | 280.0 |
| pytest-3.14.10.xml | 5831 | 5808 | 0 | 0 | 23 | 237.8 |
| pytest-3.14.10t.xml | 5831 | 5811 | 0 | 0 | 20 | 288.0 |
| pytest-3.15.1.xml | 5831 | 5807 | 0 | 0 | 24 | 228.2 |
| pytest-3.15.1t.xml | 5831 | 5810 | 0 | 0 | 21 | 307.9 |
| pytest-3.15.2.xml | 5831 | 5807 | 0 | 0 | 24 | 223.9 |
| pytest-3.15.2t.xml | 5831 | 5810 | 0 | 0 | 21 | 307.6 |
| pytest-3.16.0a1.xml.gz | 5831 | 4025 | 278 | 1504 | 24 | 181.0 |
| pytest-3.16.0a1t.xml.gz | 5831 | 4028 | 278 | 1504 | 21 | 222.1 |

Failures and errors on 3.16.0a1/a1t: every one asserts an installed target that the version gate refused ("CPython 3.16 pre-release (0x31000a1)"); details per test in tests/pytest-3.16.0a1*.xml.gz (docs/atlas/w2-phase0.md §2).
