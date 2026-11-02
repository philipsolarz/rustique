| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio | values |
|---|---|---:|---:|---:|---:|
| 3.15.1 | data/log_analytics | 0.968 ± 0.006 | 0.625 ± 0.015 | 0.645 | 20/20 |
| 3.15.1 | data/report_render | 0.450 ± 0.002 | 0.222 ± 0.002 | 0.494 | 20/20 |
| 3.15.1 | data/state_snapshot | 0.891 ± 0.005 | 0.659 ± 0.005 | 0.740 | 20/20 |
| 3.15.1 | devtools/import_startup | 1.907 ± 0.004 | 1.941 ± 0.007 | 1.018 | 20/20 |
| 3.15.1 | devtools/pytest_networkx | 1.077 ± 0.007 | 1.043 ± 0.007 | 0.969 | 20/20 |
| 3.15.1 | web/json_api | 0.656 ± 0.009 | 0.518 ± 0.002 | 0.790 | 20/20 |
| 3.15.1 | **geomean** | | | **0.754** | |
<!-- 3.15.1 data__log_analytics: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 data__report_render: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 data__state_snapshot: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 devtools__import_startup: digests DIFFER {'plain': {'80b60af41db2a724ae01bcc21d64a029a5cedc5326cc91f8a0f08e503fe9870c'}, 'enabled': {'1d82b0f355eb258d1d9478ec28b04ad61290cdba7bec6c7e99326c37b277e1c4'}}; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 devtools__pytest_networkx: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 web__json_api: digests same; rustique loaded plain=['False'] enabled=['True'] -->
| 3.15.1t | data/log_analytics | 1.028 ± 0.006 | 0.669 ± 0.005 | 0.651 | 20/20 |
| 3.15.1t | data/report_render | 0.502 ± 0.002 | 0.241 ± 0.001 | 0.479 | 20/20 |
| 3.15.1t | data/state_snapshot | 1.446 ± 0.008 | 0.754 ± 0.003 | 0.522 | 20/20 |
| 3.15.1t | devtools/import_startup | 2.261 ± 0.005 | 2.311 ± 0.003 | 1.022 | 20/20 |
| 3.15.1t | devtools/pytest_networkx | 1.214 ± 0.005 | 1.181 ± 0.006 | 0.973 | 20/20 |
| 3.15.1t | web/json_api | 0.706 ± 0.002 | 0.573 ± 0.002 | 0.812 | 20/20 |
| 3.15.1t | **geomean** | | | **0.713** | |
<!-- 3.15.1t data__log_analytics: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t data__report_render: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t data__state_snapshot: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t devtools__import_startup: digests DIFFER {'plain': {'80b60af41db2a724ae01bcc21d64a029a5cedc5326cc91f8a0f08e503fe9870c'}, 'enabled': {'1d82b0f355eb258d1d9478ec28b04ad61290cdba7bec6c7e99326c37b277e1c4'}}; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t devtools__pytest_networkx: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t web__json_api: digests same; rustique loaded plain=['False'] enabled=['True'] -->
