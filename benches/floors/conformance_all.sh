#!/usr/bin/env bash
# Conformance of all 15 groups at once on the running build (Phase 3's `conformance` stage).
# Usage (dev box, via rbox): bash benches/floors/conformance_all.sh <out.json>
set -euo pipefail
MODS=${CONF_MODULES:-test.test_list,test.test_tuple,test.test_sort,test.test_builtin,test.test_json,test.test_float,test.test_format,test.test_strtod,test.test_fstring,test.test_copy,test.test_copyreg,test.test_dataclasses,test.test_strptime,test.test_datetime,test.test_time,test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,test.test_logging,test.test_random,test.test_textwrap,test.test_urlparse,test.test_urllib,test.test_statistics}
groups=$(python -c 'import rustique; print(",".join(sorted({t["group"] for t in rustique.status()["targets"]})))')
names=$(python -c '
import rustique, sys
v = sys.version_info[:2]
skip = {"pathlib.PurePath.parent","pathlib.PurePath.name","pathlib.PurePath.suffix","pathlib.PurePath.stem","pathlib.PurePath.parts"} if v < (3, 15) else set()
print(",".join(sorted(t["name"] for t in rustique.status()["targets"] if t["name"] not in skip)))')
python tests/conformance/run_target_suites.py --targets "$groups" --modules "$MODS" --expect-installed "$names" --timeout 3600 --json "$1"
