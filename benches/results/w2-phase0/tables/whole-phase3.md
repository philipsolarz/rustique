| build | workload | plain s (mean ± std) | RUSTIQUE=1 s | ratio | values |
|---|---|---:|---:|---:|---:|
| 3.15.1 | data/log_analytics | 0.969 ± 0.008 | 0.619 ± 0.003 | 0.639 | 20/20 |
| 3.15.1 | data/report_render | 0.451 ± 0.002 | 0.223 ± 0.001 | 0.494 | 20/20 |
| 3.15.1 | data/state_snapshot | 0.893 ± 0.009 | 0.669 ± 0.005 | 0.749 | 20/20 |
| 3.15.1 | devtools/import_startup | 1.917 ± 0.006 | 1.959 ± 0.005 | 1.022 | 20/20 |
| 3.15.1 | devtools/pytest_networkx | 1.081 ± 0.006 | 1.051 ± 0.014 | 0.972 | 20/20 |
| 3.15.1 | web/json_api | 0.655 ± 0.002 | 0.520 ± 0.006 | 0.794 | 20/20 |
| 3.15.1 | **geomean** | | | **0.756** | |
<!-- 3.15.1 data__log_analytics: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 data__report_render: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 data__state_snapshot: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 devtools__import_startup: digests DIFFER {'plain': {'80b60af41db2a724ae01bcc21d64a029a5cedc5326cc91f8a0f08e503fe9870c'}, 'enabled': {'1d82b0f355eb258d1d9478ec28b04ad61290cdba7bec6c7e99326c37b277e1c4'}}; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1 devtools__pytest_networkx: digests same; rustique loaded plain=['True'] enabled=['True'] -->
<!-- 3.15.1 web__json_api: digests same; rustique loaded plain=['False'] enabled=['True'] -->
| 3.15.1t | data/log_analytics | 1.029 ± 0.009 | 0.664 ± 0.002 | 0.645 | 20/20 |
| 3.15.1t | data/report_render | 0.504 ± 0.002 | 0.242 ± 0.001 | 0.481 | 20/20 |
| 3.15.1t | data/state_snapshot | 1.471 ± 0.006 | 0.765 ± 0.004 | 0.520 | 20/20 |
| 3.15.1t | devtools/import_startup | 2.278 ± 0.006 | 2.326 ± 0.005 | 1.021 | 20/20 |
| 3.15.1t | devtools/pytest_networkx | 1.225 ± 0.008 | 1.191 ± 0.005 | 0.973 | 20/20 |
| 3.15.1t | web/json_api | 0.710 ± 0.003 | 0.575 ± 0.003 | 0.810 | 20/20 |
| 3.15.1t | **geomean** | | | **0.712** | |
<!-- 3.15.1t data__log_analytics: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t data__report_render: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t data__state_snapshot: digests same; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t devtools__import_startup: digests DIFFER {'plain': {'80b60af41db2a724ae01bcc21d64a029a5cedc5326cc91f8a0f08e503fe9870c'}, 'enabled': {'1d82b0f355eb258d1d9478ec28b04ad61290cdba7bec6c7e99326c37b277e1c4'}}; rustique loaded plain=['False'] enabled=['True'] -->
<!-- 3.15.1t devtools__pytest_networkx: digests same; rustique loaded plain=['True'] enabled=['True'] -->
<!-- 3.15.1t web__json_api: digests same; rustique loaded plain=['False'] enabled=['True'] -->
