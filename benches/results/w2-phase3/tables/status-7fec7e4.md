| interpreter | default targets installed | refused (category) | interp tier (RUSTIQUE_TIERS=interp) | gc tier | with RUSTIQUE_UNVALIDATED_PATCH=1 |
|---|---:|---|---|---|---:|
| 3.14.0 | 0 / 111 | 111 `unvalidated-patch` | 4 refused | 1 refused | 87 |
| 3.14.1 | 105 / 111 | 5 `version`, 1 `not-needed` | 4 refused | 1 refused | 105 |
| 3.14.10 | 105 / 111 | 5 `version`, 1 `not-needed` | 4 refused | 1 installed | 105 |
| 3.14.10t | 99 / 111 | 7 `build`, 5 `version` | 4 refused | 1 installed | 99 |
| 3.14.5 | 105 / 111 | 5 `version`, 1 `not-needed` | 4 refused | 1 refused | 105 |
| 3.14.6 | 105 / 111 | 5 `version`, 1 `not-needed` | 4 refused | 1 installed | 105 |
| 3.14.8 | 105 / 111 | 5 `version`, 1 `not-needed` | 4 refused | 1 installed | 105 |
| 3.14.9 | 105 / 111 | 5 `version`, 1 `not-needed` | 4 refused | 1 installed | — |
| 3.14.9t | 99 / 111 | 7 `build`, 5 `version` | 4 refused | 1 installed | — |
| 3.15.0 | 108 / 111 | 2 `fingerprint`, 1 `not-needed` | 4 refused | 1 installed | 108 |
| 3.15.1 | 110 / 111 | 1 `not-needed` | 4 installed | 1 installed | — |
| 3.15.1t | 104 / 111 | 7 `build` | 3 installed, 1 refused | 1 installed | — |
| 3.15.2 | 110 / 111 | 1 `not-needed` | 4 installed | 1 installed | 110 |
| 3.15.2t | 104 / 111 | 7 `build` | 3 installed, 1 refused | 1 installed | 104 |
| 3.16.0a1 | 0 / 111 | 111 `version` | — | — | — |

- 3.14.0: 111 × `unvalidated-patch`: CPython 3.14.0 is not a patch release this Rustique build has validated (validated: 3.14.1, 3.14.5, 3.14.6, 3.14.8, 3.14.9, 3.14.10); RUSTIQUE_UNVALIDATED_PATCH=1 installs the targets whose code matches a validated release
- 3.14.1: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.1: 1 × `not-needed`: only free-threaded builds need it
- 3.14.10: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.10: 1 × `not-needed`: only free-threaded builds need it
- 3.14.10t: 7 × `build`: not validated on free-threaded builds
- 3.14.10t: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.5: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.5: 1 × `not-needed`: only free-threaded builds need it
- 3.14.6: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.6: 1 × `not-needed`: only free-threaded builds need it
- 3.14.8: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.8: 1 × `not-needed`: only free-threaded builds need it
- 3.14.9: 5 × `version`: CPython 3.14 not validated for this target
- 3.14.9: 1 × `not-needed`: only free-threaded builds need it
- 3.14.9t: 7 × `build`: not validated on free-threaded builds
- 3.14.9t: 5 × `version`: CPython 3.14 not validated for this target
- 3.15.0: 2 × `fingerprint`: copy.deepcopy: code fingerprint db53303f37e0e7df differs from every one recorded for CPython 3.15
- 3.15.0: 1 × `not-needed`: only free-threaded builds need it
- 3.15.1: 1 × `not-needed`: only free-threaded builds need it
- 3.15.1t: 7 × `build`: not validated on free-threaded builds
- 3.15.2: 1 × `not-needed`: only free-threaded builds need it
- 3.15.2t: 7 × `build`: not validated on free-threaded builds
- 3.16.0a1: 111 × `version`: CPython 3.16 pre-release (0x31000a1)
