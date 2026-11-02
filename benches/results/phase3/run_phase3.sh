#!/usr/bin/env bash
# Phase 3: integrate and prove. Final measurements of wave 1 on the benchmark box.
#
# Same box and conventions as Phase 0 (benches/results/phase0/run_phase0.sh, docs/atlas/phase0.md):
# AWS c7a.8xlarge, 32 cores, 4 L3 domains (0-7, 8-15, 16-23, 24-31). Reuses what Phase 0 left
# under $P0 (default ~/phase0): the uv-managed CPythons, rustup 1.99.1, the CPython Lib/test
# checkouts ($P0/cpython/v<ver>), the pyperformance driver/benchmark venvs ($P0/pyperf/<b>)
# and sitecustomize.py. Everything new lives under $BASE (default ~/phase3); results under
# $RES (default $BASE/results), the directory to copy off the box (rsync it after every step).
#
# Usage (on the box):
#   scp rustique.bundle run_phase3.sh whole.py box:phase3/   (bundle containing $COMMIT)
#   ./run_phase3.sh <stage> [args]
#   ./run_phase3.sh all              the whole schedule (stage_all at the end of this file)
#
# Stages (jobs in different lanes run concurrently on separate L3 domains: never two timing
# jobs in one domain; callgrind may share a domain with callgrind, never with timing):
#   setup                          clone $COMMIT, record toolchain
#   build <b>                      venv + test deps + fat-LTO `maturin develop --uv --release`
#   wheel <b>                      fat-LTO `maturin build --release` wheel (same target dir)
#   test <b>                       full `pytest -q`
#   pytest-repeat <b> <i>          full `pytest -q` again (run i), for intermittent crashes
#   rust                           cargo fmt --check, clippy -D warnings, cargo test --workspace
#   conformance <b>                run_target_suites.py: 27 CPython test modules, ALL groups at once
#   wlvenv <b>                     workload venvs, one per family (pins conflict): requirements + the wheel
#   workloads <b> <cpus> [tag]     benches/workloads/run.py compare all -n 5 -p 5 (plain vs enable())
#   wlstats <b>                    every workload once with enable() + RUSTIQUE_REPORT (JSON; whole process)
#   wlstats-run <b>                counters of one steady-state run() per workload (wlstats_run.py)
#   whole <b> <cpus> [rounds]      pyperf command, whole process, plain vs RUSTIQUE=1, 6 workloads
#   pyperformance <b> plain|enabled <cpus> [tag]
#   pyperformance-ports <b> <cpus> asyncio_tcp,asyncio_tcp_ssl,asyncio_websockets alone
#   pyperformance-compare <b> [tag]
#   startup <b> <cpu> [rounds]     benches/dx/startup.py cells: bare venv and venv with another .pth
#   instr <b> <cpus> / time <b> <cpu,..> / report <b>   scoreboard (--impls enabled)
#   isolate <b> <cpus> <cell>...   re-measure instr cells alone (3 runs each)
#   spot <b> <cpus>                per-target callgrind benches (step 7)
#   pack                           gzip big result files
#
# Builds: <b> = 3.15.1, 3.15.1t, 3.14.9, 3.14.9t (python-build-standalone 20261021, as Phase 0).

set -uo pipefail

COMMIT=${COMMIT:-11bad29}
BASE=${BASE:-$HOME/phase3}
P0=${P0:-$HOME/phase0}
RES=${RES:-$BASE/results}
BUNDLE=${BUNDLE:-$BASE/rustique.bundle}
PYPERFORMANCE_VERSION=${PYPERFORMANCE_VERSION:-1.14.0}
DEPS=${DEPS:-"maturin==1.15.0 pytest==9.1.1 hypothesis==6.170.9 numpy==2.5.4 pyperf==2.10.0 networkx pyflakes"}
DOMAINS=(0-7 8-15 16-23 24-31)
TIMING_CPU=(2 10 20 30)   # one IRQ-free CPU per L3 domain (Phase 0, /proc/interrupts)
# The CPython test modules of every target group (docs/targets/*.md, conformance sections).
CONF_MODULES=${CONF_MODULES:-test.test_list,test.test_tuple,test.test_sort,test.test_builtin,test.test_json,test.test_float,test.test_format,test.test_strtod,test.test_fstring,test.test_copy,test.test_copyreg,test.test_dataclasses,test.test_strptime,test.test_datetime,test.test_time,test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,test.test_logging,test.test_random,test.test_textwrap,test.test_urlparse,test.test_urllib,test.test_statistics}
# Whole-process workloads (one `setup()` + `run()` per process).
WHOLE=${WHOLE:-"data/state_snapshot data/log_analytics data/report_render web/json_api devtools/pytest_networkx devtools/import_startup"}

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
unset RUSTIQUE RUSTIQUE_ONLY RUSTIQUE_EXCLUDE RUSTIQUE_STATS RUSTIQUE_REPORT RUSTIQUE_CHECK RUSTIQUE_ENABLE
mkdir -p "$RES"/{logs,machine,tests,conformance,workloads,whole,pyperformance,startup,scoreboard,spot}

log() { echo "[$(date -u +%FT%TZ)] $*" | tee -a "$RES/steps.log" >&2; }

step() { # step <name> <command...>: output in logs/<name>.log, duration in durations.tsv
    local name=$1; shift
    local logf="$RES/logs/$name.log" t0 t1 rc
    t0=$(date +%s)
    log "START $name"
    "$@" >"$logf" 2>&1
    rc=$?
    t1=$(date +%s)
    printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$(date -u -d "@$t0" +%FT%TZ)" "$(date -u -d "@$t1" +%FT%TZ)" \
        "$((t1 - t0))" "$rc" >>"$RES/durations.tsv"
    log "END   $name rc=$rc $((t1 - t0))s"
    return $rc
}

pyreq() { local b=$1; if [[ $b == *t ]]; then echo "cpython-${b%t}+freethreaded-linux-x86_64-gnu"; else echo "cpython-$b-linux-x86_64-gnu"; fi; }
pybin() { uv python find --no-project "$(pyreq "$1")"; }
venv() { echo "$BASE/venv/$1"; }
work() { echo "$BASE/w/$1"; }
tgt() { echo "$BASE/target/$1"; }
wl() { echo "$BASE/wl/$1-$2"; }   # wl <b> <family>: one workload venv per family (their pins conflict)
FAMILIES="data devtools web"
cpysrc() { echo "$P0/cpython/v${1%t}"; }

in_build() { # in_build <b> <command...>: inside the build's clone with its venv active
    local b=$1; shift
    (cd "$(work "$b")" && VIRTUAL_ENV="$(venv "$b")" PATH="$(venv "$b")/bin:$PATH" \
        CARGO_TARGET_DIR="$(tgt "$b")" "$@")
}

# ---------------------------------------------------------------------------------------
do_setup() {
    [[ -d $BASE/src/.git ]] || git clone -q "$BUNDLE" "$BASE/src"
    git -C "$BASE/src" checkout -q "$COMMIT" || return 1
    git -C "$BASE/src" log -1 --format='%H %s'
    (cd "$BASE/src" && rustc -Vv && cargo -V && cargo clippy -V && cargo fmt --version)
    uv --version; valgrind --version; perf --version 2>&1 | head -1
    : >"$RES/machine/pythons.txt"
    for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
        local p; p=$(pybin "$b"); { echo "== $b: $p"; "$p" -VV; } >>"$RES/machine/pythons.txt"
    done
    cat "$RES/machine/pythons.txt"
    for v in 3.15.1 3.14.9; do git -C "$P0/cpython/v$v" log -1 --format="v$v %H %s"; done
    uname -a; nproc; free -g
}

do_build() {
    local b=$1
    [[ -d $(work "$b") ]] || git clone -q "$BASE/src" "$(work "$b")"
    git -C "$(work "$b")" checkout -q "$COMMIT" || return 1
    [[ -x $(venv "$b")/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$(venv "$b")"
    "$(venv "$b")/bin/python" -VV
    # shellcheck disable=SC2086
    uv pip install -q --python "$(venv "$b")/bin/python" $DEPS || return 1
    in_build "$b" maturin develop --uv --release || return 1
    in_build "$b" python -c 'import rustique, sys; print(rustique.__file__, rustique.__version__); rustique.enable(); print("enable ok")'
    uv pip freeze --python "$(venv "$b")/bin/python" >"$RES/tests/freeze-$b.txt"
}
do_wheel() {
    local b=$1
    mkdir -p "$BASE/dist/$b"; rm -f "$BASE/dist/$b"/*.whl
    in_build "$b" maturin build --release -i "$(venv "$b")/bin/python" -o "$BASE/dist/$b" || return 1
    ls -la "$BASE/dist/$b"
    python3 -c 'import sys, zipfile; [print(i.file_size, i.filename) for i in zipfile.ZipFile(sys.argv[1]).infolist() if i.filename.endswith((".pth", ".so"))]' "$BASE"/dist/"$b"/*.whl
}
do_pytest() {
    local b=$1 tag=${2:-}
    in_build "$b" python -m pytest -q -p no:cacheprovider --durations=25 --junitxml="$RES/tests/pytest-$b$tag.xml"
}
rust_env() {
    local py; py=$(pybin 3.15.1)
    export PYO3_PYTHON=$py
    LD_LIBRARY_PATH=$("$py" -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')
    export LD_LIBRARY_PATH
    export CARGO_TARGET_DIR=$BASE/target/rust-ci
}
do_fmt() { (rust_env && cd "$BASE/src" && cargo fmt --all --check); }
do_clippy() { (rust_env && cd "$BASE/src" && cargo clippy --workspace --all-targets -- -D warnings); }
do_cargo_test() { (rust_env && cd "$BASE/src" && cargo test --workspace); }

# ---------------------------------------------------------------------------------------
# conformance: every group at once
do_conformance() {
    local b=$1 groups names
    groups=$(in_build "$b" python -c 'import rustique; print(",".join(sorted({t["group"] for t in rustique.status()["targets"]})))') || return 1
    names=$(in_build "$b" python -c 'import rustique; print(",".join(sorted(t["name"] for t in rustique.status()["targets"])))') || return 1
    echo "groups: $groups"; echo "targets: $names"
    in_build "$b" env CPYTHON_SRC="$(cpysrc "$b")" python tests/conformance/run_target_suites.py \
        --targets "$groups" --modules "$CONF_MODULES" --expect-installed "$names" --timeout 3600 \
        --json "$RES/conformance/all-groups-$b.json"
}

# ---------------------------------------------------------------------------------------
# workloads
do_wlvenv() { # one venv per family: data pins python-dateutil 4.0.0, devtools' boto3 needs < 3
    local b=$1 f v rc=0
    for f in $FAMILIES; do
        v=$(wl "$b" "$f"); echo "== $f: $v"
        [[ -x $v/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$v"
        uv pip install --python "$v/bin/python" -r "$(work "$b")/benches/workloads/$f/requirements.txt" pyperf==2.10.0 || { rc=1; continue; }
        uv pip install --python "$v/bin/python" --reinstall --no-deps "$BASE"/dist/"$b"/*.whl || { rc=1; continue; }
        uv pip freeze --python "$v/bin/python" >"$RES/workloads/freeze-$b-$f.txt"
        ls "$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')"/*.pth
        # sanity: RUSTIQUE=1 reaches the extension through the shipped .pth; unset loads nothing
        env RUSTIQUE=1 "$v/bin/python" -c 'import sys; print("RUSTIQUE=1:", "rustique._rustique" in sys.modules); import rustique; s=rustique.status(); print(s["process"]["switch"]); print({k: sum(1 for t in s["targets"] if t["state"] == k) for k in ("installed","pending-import","refused","disabled")})'
        "$v/bin/python" -c 'import sys; print("unset:", "rustique._rustique" in sys.modules)'
    done
    return $rc
}
do_workloads() { # workloads <b> <cpus> [tag]: run.py compare per family, each in its family's venv
    local b=$1 cpus=$2 tag=${3:-} f rc=0
    for f in $FAMILIES; do
        (cd "$(work "$b")" && taskset -c "$cpus" "$(wl "$b" "$f")/bin/python" benches/workloads/run.py compare "$f" \
            -n 5 -p 5 -o "$RES/workloads/compare-$b-$f$tag.json") || rc=1
    done
    return $rc
}
do_wlstats() {
    local b=$1 f w d rc=0
    d="$RES/workloads/stats-$b"; mkdir -p "$d"; rm -f "$d"/*.json
    for f in $FAMILIES; do
        for w in $(cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" benches/workloads/run.py list | grep "^$f/"); do
            echo "== $w"
            (cd "$(work "$b")" && RUSTIQUE_REPORT="$d/${w//\//__}.json" "$(wl "$b" "$f")/bin/python" \
                benches/workloads/run.py one "$w" --enable -n 1) || { echo "FAILED $w"; rc=1; }
        done
    done
    return $rc
}
do_wlstats_run() { # counters of one steady-state run() per workload (wlstats_run.py)
    local b=$1 f w d rc=0
    d="$RES/workloads/stats-run-$b"; mkdir -p "$d"; rm -f "$d"/*.json
    for f in $FAMILIES; do
        for w in $(cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" benches/workloads/run.py list | grep "^$f/"); do
            echo "== $w"
            (cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" "$BASE/wlstats_run.py" "$w" "$d/${w//\//__}.json") \
                || { echo "FAILED $w"; rc=1; }
        done
    done
    return $rc
}
do_whole() { # whole <b> <cpus> [rounds]: plain / RUSTIQUE=1 alternate, 5 processes per variant per round
    local b=$1 cpus=$2 rounds=${3:-4} py w r v out
    out="$RES/whole/whole-$b.json"; rm -f "$out"
    local dd="$RES/whole/digests-$b"; rm -rf "$dd"; mkdir -p "$dd"
    for w in $WHOLE; do
        py=$(wl "$b" "${w%%/*}")/bin/python
        for ((r = 1; r <= rounds; r++)); do
            local order="plain enabled"; ((r % 2 == 0)) && order="enabled plain"
            for v in $order; do
                local en=(); [[ $v == enabled ]] && en=(RUSTIQUE=1)
                (cd "$(work "$b")" && env "${en[@]}" WHOLE_DIGEST="$dd/${w//\//__}-$v.txt" "$py" -m pyperf command \
                    --affinity "$cpus" -p 5 -n 1 -w 1 --inherit-environ RUSTIQUE,WHOLE_DIGEST \
                    --name "$w/$v" --append "$out" --quiet -- "$py" "$BASE/whole.py" "$w") || return 1
            done
        done
        echo "$w: digests $(sort -u "$dd/${w//\//__}"-*.txt | tr '\n' ' ')"
    done
}

# ---------------------------------------------------------------------------------------
# pyperformance (Phase 0's venvs and sitecustomize: plain vs rustique.enable())
ppdir() { echo "$P0/pyperf/$1"; }
do_pyperformance() { # <b> plain|enabled <cpus> [tag]
    local b=$1 mode=$2 cpus=$3 tag=${4:-} d; d=$(ppdir "$b")
    local en=0; [[ $mode == enabled ]] && en=1
    local out="$RES/pyperformance/$mode-$b$tag.json"
    rm -f "$out"
    # sanity: which rustique the benchmark venv sees
    local bp; bp=$(ls "$d"/venv/*/bin/python | head -1)
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=$en "$bp" -c \
        'import sys; m = sys.modules.get("rustique"); print("rustique loaded:", m and m.__file__, "installed:", m and sum(t["state"]=="installed" for t in m.status()["targets"]))'
    (cd "$d" && PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=$en \
        "$d/driver/bin/pyperformance" run --python "$(pybin "$b")" ${PP_BENCHMARKS:+-b "$PP_BENCHMARKS"} \
        --inherit-environ PYTHONPATH,RUSTIQUE_ENABLE --affinity "$cpus" -o "$out")
}
PORT_BENCHMARKS=asyncio_tcp,asyncio_tcp_ssl,asyncio_websockets
do_pyperformance_compare() {
    local b=$1 tag=${2:-} d; d=$(ppdir "$b")
    local p="$RES/pyperformance/plain-$b$tag.json" e="$RES/pyperformance/enabled-$b$tag.json"
    "$d/driver/bin/python" -m pyperf compare_to "$p" "$e" --table -G | tee "$RES/pyperformance/compare-$b$tag.md"
    "$d/driver/bin/python" -m pyperf compare_to "$p" "$e" -G --min-speed=1 >"$RES/pyperformance/compare-$b$tag-full.txt"
    "$d/driver/bin/python" "$BASE/analyze_phase0.py" pyperformance "$p" "$e" | tee "$RES/pyperformance/analysis-$b$tag.txt"
}

# ---------------------------------------------------------------------------------------
# startup cells: release wheel pip-installed into a fresh venv vs an identical venv without it
do_startup() { # <b> <cpu> [rounds]
    local b=$1 cpu=$2 rounds=${3:-300} kind base; base=$(pybin "$b")
    for kind in bare pth; do
        local pv="$BASE/su/$b/plain-$kind" rv="$BASE/su/$b/rq-$kind"
        rm -rf "$pv" "$rv"
        "$base" -m venv "$pv" && "$base" -m venv "$rv" || return 1
        "$rv/bin/python" -m pip install -q --no-deps --no-index "$BASE"/dist/"$b"/*.whl || return 1
        if [[ $kind == pth ]]; then
            for v in "$pv" "$rv"; do
                echo "# stands for any other .pth" >"$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')/aaa_other.pth"
            done
        fi
        for v in "$pv" "$rv"; do ls "$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')"; done
        for i in 1 2 3; do "$pv/bin/python" -c pass; "$rv/bin/python" -c pass; RUSTIQUE=1 "$rv/bin/python" -c pass
            "$rv/bin/python" -c 'import rustique; rustique.enable()'; done
        (cd "$(work "$b")" && taskset -c "$cpu" "$(venv "$b")/bin/python" benches/dx/startup.py cells "$pv" "$rv" \
            -r "$rounds" --ir-runs 3 -o "$RES/startup/cells-$b-$kind.json") || return 1
    done
}

# ---------------------------------------------------------------------------------------
# scoreboard (as Phase 0)
do_instr() {
    local b=$1 cpus=$2 impls=${3:-enabled} n; n=$(taskset -c "$cpus" nproc)
    in_build "$b" taskset -c "$cpus" python benches/scoreboard.py instr --impls "$impls" --shards "$n" --quiet \
        -o "$RES/scoreboard/instr-$impls-$b.json"
}
do_time() {
    local b=$1 cpus=$2 impls=${3:-enabled} n; n=$(tr ',' '\n' <<<"$cpus" | wc -l)
    in_build "$b" python benches/scoreboard.py time --size 1e4,1e6 --impls "$impls" --shards "$n" --affinity "$cpus" \
        -o "$RES/scoreboard/time-$impls-$b.json"
}
do_report() {
    local b=$1 impls=${2:-enabled} d=$RES/scoreboard files=()
    [[ -f $d/instr-$impls-$b.json ]] && files+=("$d/instr-$impls-$b.json")
    for f in "$d/time-$impls-$b.json".shard*.json; do [[ -f $f ]] && files+=("$f"); done
    ((${#files[@]})) || { echo "no results for $b/$impls"; return 1; }
    in_build "$b" python benches/scoreboard.py report "${files[@]}" --impl "$impls" -o "$d/$impls-$b.md"
    { in_build "$b" python benches/scoreboard.py summary --after "${files[@]}" --impl "$impls"
      [[ -f $d/instr-$impls-$b.json ]] && in_build "$b" python benches/scoreboard.py summary --size \
          --after "$d/instr-$impls-$b.json" --impl "$impls"; } | tee "$d/summary-$impls-$b.md"
    in_build "$b" python "$BASE/analyze_phase0.py" scoreboard --impl "$impls" "${files[@]}" | tee "$d/analysis-$impls-$b.txt"
    local p0m="$P0/results/scoreboard/$impls-$b.md"
    [[ -f $p0m ]] && python3 "$BASE/analyze_phase0.py" matrix-compare "$p0m" "$d/$impls-$b.md" --top 40 \
        | tee "$d/compare-phase0-$impls-$b.txt"
    return 0
}

# Re-measure single instr cells, each alone in its own callgrind worker (Phase 0's isolate.sh):
# the cells above 1.05 at 0-100 elements, to separate allocator state left by the previous
# cell in a shared worker from a real cost.
do_isolate() { # isolate <b> <cpus> <type/op/storage/size>...
    local b=$1 cpus=$2 cell t op st n j; shift 2
    local out="$RES/scoreboard/isolated-instr-$b.txt"
    for cell in "$@"; do
        IFS=/ read -r t op st n <<<"$cell"
        for rep in 1 2 3; do
            j=$(mktemp --suffix=.json)
            in_build "$b" taskset -c "$cpus" python benches/scoreboard.py instr --impls enabled --type "$t" --op "$op" \
                --storage "$st" --size "$n" --shards 1 --quiet -o "$j" >/dev/null 2>&1
            python3 - "$j" "$cell" "$rep" <<'PY' | tee -a "$out"
import json, sys
d = json.load(open(sys.argv[1]))
for c in d["cells"]:
    if c["key"] == sys.argv[2]:
        print(f"{c['key']}: builtin {c.get('builtin', 0):.1f} enabled {c.get('enabled', 0):.1f} "
              f"ratio {c.get('enabled', 0) / c['builtin']:.3f} (isolated, run {sys.argv[3]})")
PY
            rm -f "$j"
        done
    done
}

# ---------------------------------------------------------------------------------------
# step 7: per-target callgrind benches, as each target doc's reproduce section runs them
do_spot() {
    local b=$1 cpus=$2 f rc=0
    for f in ${SPOT:-json_cg copy_cells datetime_cg paths_cg logging_cg utils_cg:random_cells}; do
        echo "== $f"
        local cmd=("benches/targets/$f.py")
        [[ $f == utils_cg:* ]] && cmd=(benches/targets/utils_cg.py "${f#utils_cg:}")
        in_build "$b" taskset -c "$cpus" python "${cmd[@]}" -o "$RES/spot/${f#utils_cg:}-$b.json" >"$RES/spot/${f#utils_cg:}-$b.txt" 2>&1 || { echo "FAILED $f"; rc=1; }
        tail -60 "$RES/spot/${f#utils_cg:}-$b.txt"
    done
    return $rc
}

stage_pack() {
    find "$RES" -type f \( -name '*.json' -o -name '*.log' -o -name '*.xml' \) -size +1M ! -name '*.gz' -exec gzip -9f {} +
    du -sh "$RES"
}

# ---------------------------------------------------------------------------------------
# all: the schedule used on 2026-11-01 (lanes run concurrently; inside a lane, in order).
# Domain 0 = untimed and callgrind jobs; domains 1-3 = at most one timing job each.
stage_all() {
    local t0; t0=$(date +%s)
    "$0" setup
    # Wave A (no timing): per build, on its own domain: build, wheel, workload venvs, pytest
    local lane=0 b
    for b in 3.15.1 3.15.1t 3.14.9 3.14.9t; do
        ( d=${DOMAINS[$lane]}
          CARGO_BUILD_JOBS=8 taskset -c "$d" "$0" build "$b" && CARGO_BUILD_JOBS=8 taskset -c "$d" "$0" wheel "$b"
          case $b in 3.15.1|3.15.1t) taskset -c "$d" "$0" wlvenv "$b" ;; esac
          taskset -c "$d" "$0" test "$b" ) &
        lane=$((lane + 1))
    done
    wait
    CARGO_BUILD_JOBS=16 "$0" rust
    taskset -c 8-15 "$0" conformance 3.15.1 & taskset -c 16-23 "$0" conformance 3.15.1t &
    taskset -c 24-31 "$0" conformance 3.14.9 & wait
    # Wave B (timing)
    ( taskset -c 0-7 "$0" wlstats 3.15.1; taskset -c 0-7 "$0" wlstats 3.15.1t
      "$0" instr 3.15.1 0-7; "$0" instr 3.15.1t 0-7
      "$0" spot 3.15.1 0-7
      "$0" pyperformance 3.15.1t enabled 0-7 -B; "$0" pyperformance 3.15.1t plain 0-7 -B ) &
    ( "$0" workloads 3.15.1 8-15; "$0" pyperformance 3.15.1 plain 8-15; "$0" pyperformance 3.15.1 enabled 8-15 ) &
    ( "$0" workloads 3.15.1t 16-23; "$0" pyperformance 3.15.1t plain 16-23; "$0" pyperformance 3.15.1t enabled 16-23 ) &
    ( "$0" whole 3.15.1 24-31; "$0" whole 3.15.1t 24-31; "$0" startup 3.15.1 30; "$0" startup 3.14.9 30
      "$0" pyperformance 3.15.1 enabled 24-31 -B; "$0" pyperformance 3.15.1 plain 24-31 -B ) &
    wait
    # Wave C: scoreboard wall time on one IRQ-free CPU per domain, reports, port re-runs
    "$0" time 3.15.1 2,10,20,30; "$0" time 3.15.1t 2,10,20,30
    for b in 3.15.1 3.15.1t; do "$0" report "$b"; "$0" pyperformance-compare "$b"; "$0" pyperformance-compare "$b" -B; done
    "$0" pack
    log "ALL done in $(($(date +%s) - t0))s"
}

main() {
    local stage=${1:?stage}; shift
    case $stage in
        setup) step setup do_setup ;;
        build) step "build-$1" do_build "$1" ;;
        wheel) step "wheel-$1" do_wheel "$1" ;;
        test) step "pytest-$1" do_pytest "$1" ;;
        pytest-repeat) step "pytest-repeat-$1-$2" do_pytest "$1" "-repeat$2" ;;
        rust) step rust-fmt do_fmt; step rust-clippy do_clippy; step rust-test do_cargo_test ;;
        conformance) step "conformance-$1" do_conformance "$1" ;;
        wlvenv) step "wlvenv-$1" do_wlvenv "$1" ;;
        workloads) step "workloads-$1${3:-}" do_workloads "$@" ;;
        wlstats) step "wlstats-$1" do_wlstats "$1" ;;
        wlstats-run) step "wlstats-run-$1" do_wlstats_run "$1" ;;
        whole) step "whole-$1" do_whole "$@" ;;
        pyperformance) step "pyperformance-$2-$1${4:-}" do_pyperformance "$@" ;;
        pyperformance-ports)
            PP_BENCHMARKS=$PORT_BENCHMARKS step "pyperformance-plain-$1-ports" do_pyperformance "$1" plain "$2" -ports
            PP_BENCHMARKS=$PORT_BENCHMARKS step "pyperformance-enabled-$1-ports" do_pyperformance "$1" enabled "$2" -ports ;;
        pyperformance-compare) step "pyperformance-compare-$1${2:-}" do_pyperformance_compare "$@" ;;
        startup) step "startup-$1" do_startup "$@" ;;
        instr) step "instr-$1" do_instr "$@" ;;
        time) step "time-$1" do_time "$@" ;;
        report) step "report-$1" do_report "$@" ;;
        spot) step "spot-$1" do_spot "$@" ;;
        isolate) step "isolate-$1" do_isolate "$@" ;;
        pack) stage_pack ;;
        all) stage_all ;;
        *) echo "unknown stage $stage" >&2; exit 2 ;;
    esac
}
main "$@"
