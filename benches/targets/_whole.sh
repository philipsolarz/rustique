#!/bin/bash
# Whole-process Ir (callgrind totals) of `python -c pass` through the startup switch (the
# `.pth` line's loader, run with -c the way site runs it), RUSTIQUE=1 vs RUSTIQUE=1 with the
# paths groups excluded: their share of every process start (posixpath is imported at
# startup and installs eagerly; fnmatch/pathlib stay pending until imported).
L=$(python -c "import ast,sys; ns={}; t=open('tests/test_accel_startup.py').read(); exec(compile(ast.Module([n for n in ast.parse(t).body if isinstance(n, ast.Assign) and getattr(n.targets[0],'id','')=='LOADER'],[]),'x','exec'),ns); print(ns['LOADER'])")
SD=$(python -c "import rustique,os; print(os.path.dirname(os.path.dirname(rustique.__file__)))")
CODE="import os; sitedir='$SD'; os.environ.get('RUSTIQUE','0') != '0' and exec($(python -c "import sys; print(repr(sys.argv[1]))" "$L"))"
run() { d=$(mktemp -d); env "$@" valgrind --tool=callgrind -q --callgrind-out-file=$d/cg.out python -c "$CODE$TAIL" >/dev/null 2>&1; grep -h "^summary:\|^totals:" $d/cg.out* | head -1 | awk '{print $2}'; rm -rf $d; }
# Second set: `os._exit(0)` at the end (no interpreter finalization): the share without
# the teardown costs (dict-watcher events while module dicts are cleared, objects kept
# alive by installed functions).
for TAIL in ${TAILS:-"" "; os._exit(0)"}; do
for i in 1 2 3; do
  z=$(run RUSTIQUE=0); a=$(run RUSTIQUE=1 RUSTIQUE_EXCLUDE=posixpath,fnmatch,pathlib); b=$(run RUSTIQUE=1)
  echo "tail='$TAIL' RUSTIQUE=0: $z  RUSTIQUE=1 excl. paths: $a (+$((a-z)))  RUSTIQUE=1: $b (+$((b-z)))  paths share: $((b-a))"
done
done
RUSTIQUE=1 python -c "$CODE
import rustique; s=rustique.status(); print({t['name']: t['state'] for t in s['targets'] if t['group'] in ('posixpath','fnmatch','pathlib')})" | cut -c1-400
