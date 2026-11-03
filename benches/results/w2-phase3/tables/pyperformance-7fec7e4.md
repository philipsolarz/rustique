### 3.15.1: 122 benchmarks, geomean **0.911** (Phase 0 on the same 122: 0.942, this run 0.911); median 0.994; < 0.95: 15; > 1.02: 5; > 1.05: 2
faster than 0.95: `regex_compile` 0.044, `deepcopy_memo` 0.132, `deepcopy` 0.234, `json_dumps` 0.250, `logging_format` 0.513, `logging_simple` 0.520, `json_loads` 0.578, `logging_silent` 0.622, `sympy_integrate` 0.837, `deepcopy_reduce` 0.922, `meteor_contest` 0.924, `hexiom` 0.932, `pathlib` 0.933, `nqueens` 0.933, `regex_v8` 0.943
slower than 1.02: `python_startup` 1.074 (Phase 0 1.092), `unpack_sequence` 1.068 (Phase 0 1.021), `unpickle_pure_python` 1.024 (Phase 0 0.997), `urlsafe_base64_small` 1.023 (Phase 0 1.007), `bench_mp_pool` 1.023 (Phase 0 1.018)
re-run alone (two pairs, opposite order): `coroutines` 1.014 / 1.019 (full run 1.002), `unpack_sequence` 0.972 / 0.994 (full run 1.068)
in one side only: []
moved > 3% vs Phase 0: `regex_compile` 0.044 (was 1.018), `logging_simple` 0.520 (was 0.677), `logging_format` 0.513 (was 0.663), `regex_v8` 0.943 (was 1.013), `nqueens` 0.933 (was 0.986), `k_core` 0.955 (was 1.005), `mdp` 0.963 (was 1.005), `regex_dna` 0.971 (was 1.004), `sphinx` 0.961 (was 0.993), `deepcopy` 0.234 (was 0.227), `unpack_sequence` 1.068 (was 1.021)

### 3.15.1t: 122 benchmarks, geomean **0.936** (Phase 0 on the same 120: 0.955, this run 0.934); median 0.997; < 0.95: 12; > 1.02: 5; > 1.05: 2
faster than 0.95: `regex_compile` 0.122, `deepcopy_memo` 0.183, `deepcopy` 0.280, `json_dumps` 0.319, `json_loads` 0.613, `logging_format` 0.643, `logging_simple` 0.671, `sympy_integrate` 0.834, `deepcopy_reduce` 0.920, `meteor_contest` 0.926, `pathlib` 0.946, `regex_effbot` 0.949
slower than 1.02: `python_startup` 1.110 (Phase 0 1.100), `coroutines` 1.073 (Phase 0 1.010), `base32_small` 1.034 (Phase 0 1.019), `bench_mp_pool` 1.026 (Phase 0 1.011), `urlsafe_base64_small` 1.021 (Phase 0 1.001)
re-run alone (two pairs, opposite order): `coroutines` 1.073 / 1.080 (full run 1.073), `unpack_sequence` 0.998 / 0.998 (full run 0.998)
in one side only: []
moved > 3% vs Phase 0: `regex_compile` 0.122 (was 1.015), `logging_format` 0.643 (was 0.780), `logging_simple` 0.671 (was 0.786), `regex_effbot` 0.949 (was 0.998), `coroutines` 1.073 (was 1.010)

