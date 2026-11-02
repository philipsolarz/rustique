#!/usr/bin/env bash
# Wave 2, Phase 0: ground truth on the benchmark box (Rustique `wave2` at 70c54fe = the merged
# wave 1 + a one-line change of benches/workloads/run.py's family list).
#
# Same box type and conventions as wave 1's Phase 0 / Phase 3 (benches/results/phase0/run_phase0.sh,
# benches/results/phase3/run_phase3.sh; stage names kept): AWS c7a.8xlarge, 32 cores, 4 L3
# domains (0-7, 8-15, 16-23, 24-31). The box was prepared by setup_box.sh (Rust 1.99.1, uv
# 0.13.6, python-build-standalone CPythons, CPython sources /work/cpython/v<ver>, valgrind,
# perf, hyperfine). Everything of this run lives under $BASE (default /work/w2p0); results
# under $RES (default $BASE/results), the directory to copy off the box after every step.
#
# Usage (on the box):
#   scp rustique.bundle run_w2phase0.sh status_dump.py gcstats.py jit_compare.py sb_isolate.py box:/work/w2p0/
#   (deploy by atomic rename: copy to a temporary name, then `mv` into place; bash reads a
#   running script incrementally. Helper .py files go to $BASE/bin/.)
#   ./run_w2phase0.sh <stage> [args]
#   ./run_w2phase0.sh all            the whole schedule (stage_all at the end of this file)
#
# Stages (jobs in different lanes run concurrently on separate L3 domains: never two timing
# jobs in one domain; callgrind may share a domain with callgrind, never with timing):
#   setup                          clone $COMMIT from the bundle, record toolchain and interpreters
#   facts                          CPU / IRQs / perf counters / JIT availability per build
#   quiet                          stop apt's timers and unattended-upgrades for the run
#   build <b>                      venv + test deps + fat-LTO `maturin develop --uv --release`
#   wheel <b>                      fat-LTO `maturin build --release` wheel (same target dir)
#   status <b>                     rustique.status() after enable(), target modules imported
#   test <b>                       full `pytest -q` (without numpy: --continue-on-collection-errors)
#   rust                           cargo fmt --check, clippy -D warnings, cargo test --workspace
#   conformance <b>                run_target_suites.py: 27 CPython test modules, ALL groups at once
#   wlvenv <b>                     workload venvs, one per family (pins conflict): requirements + the wheel
#   workloads <b> <cpus> [tag]     benches/workloads/run.py compare <family> -n 5 -p 5 (plain vs enable())
#   wlstats-run <b>                counters of one steady-state run() per workload (phase3/wlstats_run.py)
#   whole <b> <cpus> [rounds]      pyperf command, whole process, plain vs RUSTIQUE=1, 6 workloads
#   gcstats <b>                    GC baseline of one steady-state run() per workload, plain (gcstats.py)
#   jit <b> <cpus> [p]             plain workloads, PYTHON_JIT unset vs PYTHON_JIT=1 (jit_compare.py)
#   pyperf-venv <b>                pyperformance driver venv + benchmark venv + sanity
#   pyperformance <b> plain|enabled <cpus> [tag]
#   pyperformance-ports <b> <cpus> asyncio_tcp,asyncio_tcp_ssl,asyncio_websockets alone
#   pyperformance-compare <b> [tag]
#   startup <b> <cpu> [rounds]     benches/dx/startup.py cells: bare venv and venv with another .pth
#   instr <b> <cpus> / time <b> <cpu,..> / report <b>   scoreboard (--impls enabled)
#   isolate-instr <b> <cpus>       every instr cell > 1.05 again, alone in its own callgrind worker, 3 runs
#   isolate-time <b> <cpu,..>      every 10k/1M cell > 1.05 again, alone on one CPU, 10 procs x 5 values, 2 runs
#   pack                           gzip big result files
#
# Builds: <b> = python-build-standalone CPython; 3.15.1, 3.15.1t, 3.14.9, 3.14.9t (release
# 20261021, as wave 1), 3.15.2, 3.15.2t, 3.14.10, 3.14.10t, 3.16.0a1, 3.16.0a1t (20261026).

set -uo pipefail

COMMIT=${COMMIT:-70c54fe}
BASE=${BASE:-/work/w2p0}
RES=${RES:-$BASE/results}
BUNDLE=${BUNDLE:-$BASE/rustique.bundle}
CPY=${CPY:-/work/cpython}
BIN=$BASE/bin            # this run's helper scripts (status_dump.py, gcstats.py, jit_compare.py, sb_isolate.py)
SRC=$BASE/src            # the reference clone: wave 1's helper scripts (phase0/, phase3/) at $COMMIT
PYPERFORMANCE_VERSION=${PYPERFORMANCE_VERSION:-1.14.0}
DEPS=${DEPS:-"maturin==1.15.0 pytest==9.1.1 hypothesis==6.170.9 pyperf==2.10.0 networkx pyflakes"}
NUMPY=${NUMPY:-numpy==2.5.4}
ALL_BUILDS="3.15.1 3.15.1t 3.14.9 3.14.9t 3.15.2 3.15.2t 3.14.10 3.14.10t 3.16.0a1 3.16.0a1t"
DOMAINS=(0-7 8-15 16-23 24-31)
TIMING_CPU=(2 10 20 30)   # one CPU per L3 domain, as wave 1 (machine/interrupts.txt: 2, 10, 30 had no device IRQ)
CONF_MODULES=${CONF_MODULES:-test.test_list,test.test_tuple,test.test_sort,test.test_builtin,test.test_json,test.test_float,test.test_format,test.test_strtod,test.test_fstring,test.test_copy,test.test_copyreg,test.test_dataclasses,test.test_strptime,test.test_datetime,test.test_time,test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,test.test_logging,test.test_random,test.test_textwrap,test.test_urlparse,test.test_urllib,test.test_statistics}
WHOLE=${WHOLE:-"data/state_snapshot data/log_analytics data/report_render web/json_api devtools/pytest_networkx devtools/import_startup"}
FAMILIES="data devtools web"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
unset RUSTIQUE RUSTIQUE_ONLY RUSTIQUE_EXCLUDE RUSTIQUE_STATS RUSTIQUE_REPORT RUSTIQUE_CHECK RUSTIQUE_ENABLE PYTHON_JIT
mkdir -p "$RES"/{logs,machine,tests,status,conformance,workloads,whole,gc,jit,pyperformance,startup,scoreboard}

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
cpysrc() { echo "$CPY/v${1%t}"; }
ppdir() { echo "$BASE/pyperf/$1"; }

in_build() { # in_build <b> <command...>: inside the build's clone with its venv active
    local b=$1; shift
    (cd "$(work "$b")" && VIRTUAL_ENV="$(venv "$b")" PATH="$(venv "$b")/bin:$PATH" \
        CARGO_TARGET_DIR="$(tgt "$b")" "$@")
}

# ---------------------------------------------------------------------------------------
do_setup() {
    [[ -d $SRC/.git ]] || git clone -q "$BUNDLE" "$SRC"
    git -C "$SRC" checkout -q "$COMMIT" || return 1
    git -C "$SRC" log -1 --format='%H %s'
    (cd "$SRC" && rustc -Vv && cargo -V && cargo clippy -V && cargo fmt --version)
    uv --version; valgrind --version; perf --version 2>&1 | head -1; hyperfine --version
    : >"$RES/machine/pythons.txt"
    local b
    for b in $ALL_BUILDS; do
        local p; p=$(pybin "$b"); { echo "== $b: $p"; "$p" -VV; } >>"$RES/machine/pythons.txt"
    done
    cat "$RES/machine/pythons.txt"
    for v in 3.15.1 3.14.9 3.15.2 3.14.10; do git -C "$CPY/v$v" log -1 --format="v$v %H %s"; done
    uname -a; nproc; free -g
}

do_facts() {
    local f=$RES/machine b
    uname -a >"$f/uname.txt"
    cat /etc/os-release >"$f/os-release.txt"
    lscpu >"$f/lscpu.txt"
    lscpu -e=CPU,CORE,SOCKET,NODE,CACHE >"$f/lscpu-e.txt" 2>&1
    for c in 0 8 16 24; do echo "cpu$c L3 shared: $(cat /sys/devices/system/cpu/cpu$c/cache/index3/shared_cpu_list)"; done >"$f/l3.txt"
    free -g >"$f/free.txt"; df -h / >"$f/df.txt"; nproc >"$f/nproc.txt"
    cat /proc/cmdline >"$f/cmdline.txt"
    cat /proc/interrupts >"$f/interrupts.txt"
    systemctl is-active irqbalance >"$f/irqbalance.txt" 2>&1
    cat /proc/sys/kernel/perf_event_paranoid >"$f/perf_event_paranoid.txt"
    cat /proc/sys/kernel/randomize_va_space >"$f/aslr.txt"
    cat /sys/kernel/mm/transparent_hugepage/enabled >"$f/thp.txt" 2>&1
    perf stat -e instructions,cycles,task-clock true >"$f/perf-stat-true.txt" 2>&1
    perf stat -e cycles,task-clock -- taskset -c 9 timeout 5 sh -c 'while :; do :; done' >"$f/perf-stat-busy.txt" 2>&1
    (curl -s -m 2 -H "X-aws-ec2-metadata-token: $(curl -s -m 2 -X PUT http://169.254.169.254/latest/api/token -H 'X-aws-ec2-metadata-token-ttl-seconds: 60')" \
        http://169.254.169.254/latest/meta-data/instance-type; echo) >"$f/instance-type.txt" 2>&1
    for b in $ALL_BUILDS; do
        "$(pybin "$b")" -c 'import sys, sysconfig; j = getattr(sys, "_jit", None)
print(sys.argv[1], "jit available:", bool(j and j.is_available()), "enabled:", bool(j and j.is_enabled()),
      "| PY_CORE_CFLAGS has jit:", "_Py_JIT" in (sysconfig.get_config_var("PY_CORE_CFLAGS") or ""),
      "| CONFIG_ARGS:", [a for a in (sysconfig.get_config_var("CONFIG_ARGS") or "").split() if "jit" in a.lower()])' "$b"
    done >"$f/jit.txt" 2>&1
    head -30 "$f"/*.txt
}

do_quiet() { # nothing on a timer may wake up during the run
    systemctl list-timers --all --no-pager
    sudo systemctl stop apt-daily.timer apt-daily-upgrade.timer unattended-upgrades.service motd-news.timer 2>&1
    systemctl list-timers --all --no-pager
    systemctl is-active unattended-upgrades.service || true
}

do_build() {
    local b=$1
    [[ -d $(work "$b") ]] || git clone -q "$SRC" "$(work "$b")"
    git -C "$(work "$b")" checkout -q "$COMMIT" || return 1
    [[ -x $(venv "$b")/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$(venv "$b")"
    "$(venv "$b")/bin/python" -VV
    # shellcheck disable=SC2086
    uv pip install -q --python "$(venv "$b")/bin/python" $DEPS || return 1
    # numpy separately: no cp316 wheel may exist (wave 1: built from source); not fatal
    uv pip install --python "$(venv "$b")/bin/python" "$NUMPY" || echo "NUMPY-FAILED: $NUMPY did not install on $b"
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
do_status() {
    local b=$1
    in_build "$b" python "$BIN/status_dump.py" "$RES/status/status-$b.json"
}
do_pytest() {
    local b=$1 tag=${2:-} extra=()
    if ! in_build "$b" python -c 'import numpy' 2>/dev/null; then
        echo "numpy is not importable on $b: --continue-on-collection-errors (wave 1 Phase 0 §5)"
        extra=(--continue-on-collection-errors)
    fi
    in_build "$b" python -m pytest -q -p no:cacheprovider --durations=25 "${extra[@]}" --junitxml="$RES/tests/pytest-$b$tag.xml"
}
rust_env() {
    local py; py=$(pybin 3.15.1)
    export PYO3_PYTHON=$py
    LD_LIBRARY_PATH=$("$py" -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')
    export LD_LIBRARY_PATH
    export CARGO_TARGET_DIR=$BASE/target/rust-ci
}
do_fmt() { (rust_env && cd "$SRC" && cargo fmt --all --check); }
do_clippy() { (rust_env && cd "$SRC" && cargo clippy --workspace --all-targets -- -D warnings); }
do_cargo_test() { (rust_env && cd "$SRC" && cargo test --workspace); }

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
do_wlstats_run() { # counters of one steady-state run() per workload (wave 1's wlstats_run.py)
    local b=$1 f w d rc=0
    d="$RES/workloads/stats-run-$b"; mkdir -p "$d"; rm -f "$d"/*.json
    for f in $FAMILIES; do
        for w in $(cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" benches/workloads/run.py list | grep "^$f/"); do
            echo "== $w"
            (cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" "$SRC/benches/results/phase3/wlstats_run.py" "$w" "$d/${w//\//__}.json") \
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
                    --name "$w/$v" --append "$out" --quiet -- "$py" "$SRC/benches/results/phase3/whole.py" "$w") || return 1
            done
        done
        echo "$w: digests $(sort -u "$dd/${w//\//__}"-*.txt | tr '\n' ' ')"
    done
}
do_gcstats() { # GC baseline: one process per workload, plain, one steady-state run() (untimed lane)
    local b=$1 f w rc=0 out
    out="$RES/gc/gc-$b.jsonl"
    rm -f "$out"
    for f in $FAMILIES; do
        for w in $(cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" benches/workloads/run.py list | grep "^$f/"); do
            (cd "$(work "$b")" && "$(wl "$b" "$f")/bin/python" "$BIN/gcstats.py" "$w" "$out") || { echo "FAILED $w"; rc=1; }
        done
    done
    return $rc
}
do_jit() { # jit <b> <cpus> [p]: plain, PYTHON_JIT unset vs =1, per family
    local b=$1 cpus=$2 p=${3:-3} f rc=0
    for f in $FAMILIES; do
        (cd "$(work "$b")" && taskset -c "$cpus" "$(wl "$b" "$f")/bin/python" "$BIN/jit_compare.py" compare "$f" \
            -n 5 -p "$p" -o "$RES/jit/jit-$b-$f.json") || rc=1
    done
    return $rc
}

# ---------------------------------------------------------------------------------------
# pyperformance (wave 1's method: plain vs rustique.enable() through sitecustomize)
do_pyperf_venv() {
    local b=$1 d; d=$(ppdir "$b")
    mkdir -p "$d/site"
    cp "$SRC/benches/results/phase0/sitecustomize.py" "$d/site/sitecustomize.py"
    [[ -x $d/driver/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$d/driver"
    uv pip install -q --python "$d/driver/bin/python" "pyperformance==$PYPERFORMANCE_VERSION"
    uv pip freeze --python "$d/driver/bin/python"
    # pyperformance 1.14.0 has no `run --venv`: `venv create` and `run` share the default
    # venv under the current directory ($d/venv/cpython<ver>-<hash>-compat-<hash>).
    (cd "$d" && env -u PYTHONPATH -u RUSTIQUE_ENABLE "$d/driver/bin/pyperformance" venv create \
        --python "$(pybin "$b")") || return 1
    local bp; bp=$(ls "$d"/venv/*/bin/python | head -1)
    echo "benchmark venv python: $bp"
    uv pip freeze --python "$bp" >"$RES/pyperformance/bench-venv-freeze-$b.txt" 2>&1 || true
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=1 "$bp" -c \
        'import sys; m = sys.modules.get("rustique"); print("enabled: rustique loaded via sitecustomize:", m is not None, m and m.__file__, "installed:", m and sum(t["state"]=="installed" for t in m.status()["targets"]))'
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=0 "$bp" -c \
        'import sys; print("plain: rustique loaded:", "rustique" in sys.modules)'
}
do_pyperformance() { # <b> plain|enabled <cpus> [tag]
    local b=$1 mode=$2 cpus=$3 tag=${4:-} d; d=$(ppdir "$b")
    local en=0; [[ $mode == enabled ]] && en=1
    local out="$RES/pyperformance/$mode-$b$tag.json"
    rm -f "$out"
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
    "$d/driver/bin/python" "$SRC/benches/results/phase0/analyze_phase0.py" pyperformance "$p" "$e" | tee "$RES/pyperformance/analysis-$b$tag.txt"
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
# scoreboard (as wave 1)
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
    in_build "$b" python "$SRC/benches/results/phase0/analyze_phase0.py" scoreboard --impl "$impls" "${files[@]}" | tee "$d/analysis-$impls-$b.txt"
    # against wave 1's Phase 3 matrix (benches/results/phase3/scoreboard/<impl>-<build>.md at $COMMIT)
    local p3m="$SRC/benches/results/phase3/scoreboard/$impls-$b.md"
    [[ -f $p3m ]] && python3 "$SRC/benches/results/phase0/analyze_phase0.py" matrix-compare "$p3m" "$d/$impls-$b.md" --top 40 \
        | tee "$d/compare-phase3-$impls-$b.txt"
    return 0
}
do_isolate_instr() { # every 0-100 cell > 1.05, alone in its own callgrind worker, 3 runs
    local b=$1 cpus=$2
    in_build "$b" taskset -c "$cpus" python "$BIN/sb_isolate.py" instr --results "$RES/scoreboard/instr-enabled-$b.json" --cpus "$cpus" \
        --reps 3 -o "$RES/scoreboard/isolated-instr-$b.txt"
}
do_isolate_time() { # every 10k/1M cell > 1.05, alone on one CPU, 10 processes x 5 values, 2 runs
    local b=$1 cpus=$2
    in_build "$b" python "$BIN/sb_isolate.py" time --results "$RES/scoreboard/time-enabled-$b.json".shard*.json \
        --cpus "$cpus" --reps 2 -o "$RES/scoreboard/isolated-time-$b.txt"
}

stage_pack() {
    find "$RES" -type f \( -name '*.json' -o -name '*.jsonl' -o -name '*.log' -o -name '*.xml' -o -name '*.txt' \) -size +1M ! -name '*.gz' -exec gzip -9f {} +
    du -sh "$RES"
}

# ---------------------------------------------------------------------------------------
# all: the schedule. Lanes run concurrently; inside a lane, in order. Every step is a fresh
# "$0 <stage>" process, so a fix deployed by atomic rename reaches the steps not yet started.
lane_builds() { # lane_builds <domain> <build>...: build, wheel, status, test per build (no timing)
    local d=$1 b; shift
    for b in "$@"; do
        if CARGO_BUILD_JOBS=8 taskset -c "$d" "$0" build "$b"; then
            CARGO_BUILD_JOBS=8 taskset -c "$d" "$0" wheel "$b"
            taskset -c "$d" "$0" status "$b"
            case $b in 3.15.1|3.15.1t) taskset -c "$d" "$0" wlvenv "$b" ;; esac
        fi
        taskset -c "$d" "$0" test "$b"
    done
}
stage_all() {
    local t0; t0=$(date +%s)
    local D0=${DOMAINS[0]} D1=${DOMAINS[1]} D2=${DOMAINS[2]} D3=${DOMAINS[3]}
    "$0" setup; "$0" facts; "$0" quiet
    # Wave A (no timing): builds, wheels, status, tests; workload and pyperformance venvs
    lane_builds "$D0" 3.15.1 3.16.0a1 3.14.10 &
    lane_builds "$D1" 3.15.1t 3.16.0a1t 3.14.10t &
    (lane_builds "$D2" 3.14.9 3.15.2; taskset -c "$D2" "$0" pyperf-venv 3.15.1) &
    (lane_builds "$D3" 3.14.9t 3.15.2t; taskset -c "$D3" "$0" pyperf-venv 3.15.1t) &
    wait
    CARGO_BUILD_JOBS=16 "$0" rust
    taskset -c "$D1" "$0" conformance 3.15.1 & taskset -c "$D2" "$0" conformance 3.15.1t &
    taskset -c "$D3" "$0" conformance 3.14.9 &
    (taskset -c "$D0" "$0" conformance 3.15.2; taskset -c "$D0" "$0" conformance 3.14.10) &
    wait
    log "WAVE-A done in $(($(date +%s) - t0))s"
    # Wave B (timing): at most one timing job per domain; domain 0 untimed/callgrind first
    ( taskset -c "$D0" "$0" wlstats-run 3.15.1; taskset -c "$D0" "$0" wlstats-run 3.15.1t
      taskset -c "$D0" "$0" gcstats 3.15.1; taskset -c "$D0" "$0" gcstats 3.15.1t
      "$0" instr 3.15.1 "$D0"; "$0" instr 3.15.1t "$D0"
      "$0" isolate-instr 3.15.1 "$D0"; "$0" isolate-instr 3.15.1t "$D0"
      "$0" pyperformance 3.15.1t enabled "$D0" -B; "$0" pyperformance 3.15.1t plain "$D0" -B ) &
    ( "$0" workloads 3.15.1 "$D1"; "$0" pyperformance 3.15.1 plain "$D1"; "$0" pyperformance 3.15.1 enabled "$D1" ) &
    ( "$0" workloads 3.15.1t "$D2"; "$0" pyperformance 3.15.1t plain "$D2"; "$0" pyperformance 3.15.1t enabled "$D2" ) &
    ( "$0" whole 3.15.1 "$D3"; "$0" whole 3.15.1t "$D3"
      "$0" startup 3.15.1 "${TIMING_CPU[3]}"; "$0" startup 3.14.9 "${TIMING_CPU[3]}"
      "$0" jit 3.15.1 "$D3"
      "$0" pyperformance 3.15.1 enabled "$D3" -B; "$0" pyperformance 3.15.1 plain "$D3" -B ) &
    wait
    log "WAVE-B done in $(($(date +%s) - t0))s"
    # Wave C: port benchmarks alone, scoreboard wall time on one CPU per domain, isolation, reports
    "$0" pyperformance-ports 3.15.1 "$D1"; "$0" pyperformance-ports 3.15.1t "$D1"
    local T="${TIMING_CPU[0]},${TIMING_CPU[1]},${TIMING_CPU[2]},${TIMING_CPU[3]}"
    "$0" time 3.15.1 "$T"; "$0" isolate-time 3.15.1 "$T"
    "$0" time 3.15.1t "$T"; "$0" isolate-time 3.15.1t "$T"
    for b in 3.15.1 3.15.1t; do "$0" report "$b"; "$0" pyperformance-compare "$b"; "$0" pyperformance-compare "$b" -B; done
    "$0" pack
    log "ALL done in $(($(date +%s) - t0))s"
}

main() {
    local stage=${1:?stage}; shift
    # A step can be dropped from a running schedule: touch $BASE/skip/<stage>-<first arg>
    # (2026-11-02: the 3.15.1t scoreboard wall time, to free the box by 23:30Z).
    if [[ -e $BASE/skip/$stage-${1:-} ]]; then log "SKIP  $stage ${1:-} ($BASE/skip/$stage-${1:-})"; exit 0; fi
    # Wall-time shards never start while an untimed GC-baseline job still runs (it uses domain 0).
    if [[ $stage == time ]]; then while pgrep -f "run_w2phase0.sh gcstats" >/dev/null; do sleep 5; done; fi
    case $stage in
        setup) step setup do_setup ;;
        facts) step facts do_facts ;;
        quiet) step quiet do_quiet ;;
        build) step "build-$1" do_build "$1" ;;
        wheel) step "wheel-$1" do_wheel "$1" ;;
        status) step "status-$1" do_status "$1" ;;
        test) step "pytest-$1" do_pytest "$1" ;;
        pytest-repeat) step "pytest-repeat-$1-$2" do_pytest "$1" "-repeat$2" ;;
        rust) step rust-fmt do_fmt; step rust-clippy do_clippy; step rust-test do_cargo_test ;;
        conformance) step "conformance-$1" do_conformance "$1" ;;
        wlvenv) step "wlvenv-$1" do_wlvenv "$1" ;;
        workloads) step "workloads-$1${3:-}" do_workloads "$@" ;;
        wlstats-run) step "wlstats-run-$1" do_wlstats_run "$1" ;;
        whole) step "whole-$1" do_whole "$@" ;;
        gcstats) step "gcstats-$1" do_gcstats "$1" ;;
        jit) step "jit-$1" do_jit "$@" ;;
        pyperf-venv) step "pyperf-venv-$1" do_pyperf_venv "$1" ;;
        pyperformance) step "pyperformance-$2-$1${4:-}" do_pyperformance "$@" ;;
        pyperformance-ports)
            PP_BENCHMARKS=$PORT_BENCHMARKS step "pyperformance-plain-$1-ports" do_pyperformance "$1" plain "$2" -ports
            PP_BENCHMARKS=$PORT_BENCHMARKS step "pyperformance-enabled-$1-ports" do_pyperformance "$1" enabled "$2" -ports ;;
        pyperformance-compare) step "pyperformance-compare-$1${2:-}" do_pyperformance_compare "$@" ;;
        startup) step "startup-$1" do_startup "$@" ;;
        instr) step "instr-$1" do_instr "$@" ;;
        time) step "time-$1" do_time "$@" ;;
        report) step "report-$1" do_report "$@" ;;
        isolate-instr) step "isolate-instr-$1" do_isolate_instr "$@" ;;
        isolate-time) step "isolate-time-$1" do_isolate_time "$@" ;;
        pack) stage_pack ;;
        all) stage_all ;;
        *) echo "unknown stage $stage" >&2; exit 2 ;;
    esac
}
main "$@"
