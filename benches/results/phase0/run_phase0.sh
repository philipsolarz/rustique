#!/usr/bin/env bash
# Phase 0 ground truth for Rustique on a dedicated benchmark box.
#
# Target: AWS c7a.8xlarge (AMD EPYC 9R14, 32 cores without SMT, 4 L3 domains = cores
# 0-7, 8-15, 16-23, 24-31), fresh Ubuntu 24.04, passwordless sudo. Everything lives under
# $BASE (default ~/phase0); results under $RES (default $BASE/results), which is the
# directory to copy off the box (rsync it after every stage: the box may self-terminate).
#
# Usage (on the box):
#   scp rustique.bundle run_phase0.sh sitecustomize.py analyze_phase0.py box:phase0/
#       (bundle: `git bundle create rustique.bundle main` in a checkout containing $COMMIT)
#   ./run_phase0.sh all                  # everything, in the scheduled order below
#   ./run_phase0.sh <stage> [args]       # one stage (see `stage_*` functions)
#
# Stages (`all` runs them in the schedule of stage_all; jobs in different lanes run
# concurrently on separate L3 domains: never two timing jobs in one domain, callgrind never
# in a timing domain):
#   setup                                apt, rustup (rust-toolchain.toml), uv, CPythons,
#                                        CPython Lib/test checkouts, repo clone
#   facts                                CPU / frequency / kernel / perf counters
#   build <b>                            venv + deps + fat-LTO `maturin develop --uv --release`
#   test <b>                             pytest, examples, downstream rust_extension + test_capi,
#                                        pytest again with rustique_demo built ("pytest-full")
#   rust                                 cargo fmt --check, clippy -D warnings, cargo test
#   conformance <b>                      CPython container suites + native-tier conformance
#   py316 <b>                            3.16 builds (develop, forward-compat env, pip sdist),
#                                        then the tests (py316-test / py316-extra)
#   pyperf-venv <b>                      pyperformance driver venv + benchmark venv + sanity
#   instr <b> <cpus> [impls]             scoreboard callgrind instruction counts, 0-100 elements
#   time <b> <cpu,cpu,..> [impls]        scoreboard pyperf wall time, 10k/1M (one shard per CPU)
#   recheck <b> <cpu> [processes]        notable 10k/1M cells again with 10 pyperf processes
#   report <b> [impls]                   matrices, summaries, analysis, compare with docs/scoreboard
#   pyperformance <b> plain|enabled <cpus> [tag]   pyperformance full default suite
#   pyperformance-compare <b> [tag]      pyperf compare_to plain vs enabled + analysis
#   apps <b> <cpus> [tag]                benches/apps/run_apps.py bench
#   pack                                 gzip big result files
#
# Deploying a changed copy of this script to a box where it is running: copy it to a
# temporary name and `mv` it into place. Never overwrite it in place (bash reads scripts
# incrementally; a running stage would continue at a stale offset of the new file).
#
# Builds: <b> is a python-build-standalone CPython version, `t` suffix = free-threaded
# (3.15.1, 3.15.1t, 3.14.9, 3.14.9t, 3.16.0a1, 3.16.0a1t).

set -uo pipefail

COMMIT=${COMMIT:-5abd54c}
BASE=${BASE:-$HOME/phase0}
RES=${RES:-$BASE/results}
BUNDLE=${BUNDLE:-$BASE/rustique.bundle}
UV_VERSION=${UV_VERSION:-0.13.5}          # pins the python-build-standalone release (20261021)
PYPERFORMANCE_VERSION=${PYPERFORMANCE_VERSION:-1.14.0}
BUILDS=${BUILDS:-"3.15.1 3.15.1t 3.14.9 3.14.9t"}
BUILDS_316=${BUILDS_316:-"3.16.0a1 3.16.0a1t"}
# Test/bench dependencies, pinned to the versions of the development venv.
DEPS=${DEPS:-"maturin==1.15.0 pytest==9.1.1 hypothesis==6.170.9 numpy==2.5.4 pyperf==2.10.0 networkx pyflakes"}
DOMAINS=(0-7 8-15 16-23 24-31)
# One CPU per L3 domain for single-CPU timing shards: on the c7a.8xlarge these had no device
# IRQs (ENA queues sit on 5,7,9,11,16,18,21,26,28; NVMe on 15,31); check /proc/interrupts.
TIMING_CPU=(2 10 20 30)

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export UV_PYTHON_DOWNLOADS=automatic
mkdir -p "$RES"/{logs,machine,tests,conformance,py316,scoreboard,pyperformance,apps}

log() { echo "[$(date -u +%FT%TZ)] $*" | tee -a "$RES/steps.log" >&2; }

# step <name> <command...>: run with output in logs/<name>.log, record the duration.
step() {
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

pyreq() { # uv request for a build: exact python-build-standalone variant (baseline x86_64)
    local b=$1
    if [[ $b == *t ]]; then echo "cpython-${b%t}+freethreaded-linux-x86_64-gnu"; else echo "cpython-$b-linux-x86_64-gnu"; fi
}
pybin() { uv python find --no-project "$(pyreq "$1")"; }
venv() { echo "$BASE/venv/$1"; }
work() { echo "$BASE/w/$1"; }
tgt() { echo "$BASE/target/$1"; }
cpysrc() { local v=${1%t}; echo "$BASE/cpython/v${v}"; }

in_build() { # in_build <b> <command...>: run inside the build's clone with its venv active
    local b=$1; shift
    (cd "$(work "$b")" && VIRTUAL_ENV="$(venv "$b")" PATH="$(venv "$b")/bin:$PATH" \
        CARGO_TARGET_DIR="$(tgt "$b")" "$@")
}

# --------------------------------------------------------------------------------------
# setup
# --------------------------------------------------------------------------------------

do_apt() {
    sudo DEBIAN_FRONTEND=noninteractive apt-get update -q
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -yq build-essential pkg-config valgrind git rsync \
        tmux time jq linux-tools-common "linux-tools-$(uname -r)" linux-tools-aws cpuid util-linux
}

do_rust() {
    if ! command -v rustup >/dev/null; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
    fi
    local chan
    chan=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$BASE/src/rust-toolchain.toml")
    rustup toolchain install "$chan" --profile minimal --component rustfmt,clippy
    (cd "$BASE/src" && rustc -Vv && cargo -V && cargo clippy -V && cargo fmt --version)
}

do_uv() {
    if ! uv --version 2>/dev/null | grep -q "^uv $UV_VERSION"; then
        curl -LsSf "https://astral.sh/uv/$UV_VERSION/install.sh" | sh
    fi
    uv --version
}

do_pythons() {
    local b reqs=()
    for b in $BUILDS $BUILDS_316; do reqs+=("$(pyreq "$b")"); done
    uv python install "${reqs[@]}"
    : >"$RES/machine/pythons.txt"
    for b in $BUILDS $BUILDS_316; do
        local p; p=$(pybin "$b")
        { echo "== $b: $p"; "$p" -VV; cat "$(dirname "$(dirname "$p")")/BUILD" 2>/dev/null; echo; } >>"$RES/machine/pythons.txt"
    done
    cat "$RES/machine/pythons.txt"
}

do_cpython_src() {
    local v
    for v in 3.15.1 3.14.9; do
        local d="$BASE/cpython/v$v"
        [[ -d $d/Lib/test ]] && continue
        git clone -q --depth 1 --branch "v$v" --filter=blob:none --sparse https://github.com/python/cpython.git "$d"
        git -C "$d" sparse-checkout set Lib/test
        git -C "$d" log -1 --format="v$v %H %s"
    done
}

do_repo() {
    [[ -d $BASE/src/.git ]] || git clone -q "$BUNDLE" "$BASE/src"
    git -C "$BASE/src" checkout -q "$COMMIT"
    git -C "$BASE/src" log -1 --format='%H %s'
}

stage_setup() {
    step setup-apt do_apt
    step setup-repo do_repo
    step setup-rust do_rust
    step setup-uv do_uv
    step setup-pythons do_pythons
    step setup-cpython-src do_cpython_src
}

# --------------------------------------------------------------------------------------
# machine facts
# --------------------------------------------------------------------------------------

do_facts() {
    local f=$RES/machine
    uname -a >"$f/uname.txt"
    cat /etc/os-release >"$f/os-release.txt"
    lscpu >"$f/lscpu.txt"
    lscpu -e=CPU,CORE,SOCKET,NODE,CACHE >"$f/lscpu-e.txt" 2>&1
    grep MHz /proc/cpuinfo | sort | uniq -c >"$f/cpuinfo-mhz.txt"
    { ls /sys/devices/system/cpu/cpu0/cpufreq/ 2>&1; cat /sys/devices/system/cpu/cpufreq/boost 2>&1;
      cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>&1;
      cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_{min,max,cur}_freq 2>&1;
      cat /sys/devices/system/cpu/amd_pstate/status 2>&1;
      cat /sys/devices/system/cpu/cpuidle/current_driver 2>&1; } >"$f/cpufreq.txt"
    for c in 0 1 9 17 25; do echo "cpu$c L3 shared: $(cat /sys/devices/system/cpu/cpu$c/cache/index3/shared_cpu_list)"; done >"$f/l3.txt"
    free -g >"$f/free.txt"; df -h / >"$f/df.txt"; nproc >"$f/nproc.txt"
    cat /proc/cmdline >"$f/cmdline.txt"
    cat /proc/interrupts >"$f/interrupts.txt"   # pick timing CPUs without device IRQs
    cat /proc/sys/kernel/perf_event_paranoid >"$f/perf_event_paranoid.txt"
    cat /proc/sys/kernel/randomize_va_space >"$f/aslr.txt"
    sudo cat /sys/kernel/mm/transparent_hugepage/enabled >"$f/thp.txt" 2>&1
    valgrind --version >"$f/valgrind.txt" 2>&1
    perf --version >"$f/perf-version.txt" 2>&1
    sudo perf stat -e instructions,cycles,task-clock true >"$f/perf-stat-true.txt" 2>&1
    sudo perf stat -e instructions:u,cycles:u -- taskset -c 9 python3 -c 'sum(range(10**7))' >"$f/perf-stat-sum.txt" 2>&1
    # sustained frequency under a 1-core busy loop: cycles / task-clock
    sudo perf stat -e cycles,task-clock -- taskset -c 9 timeout 5 sh -c 'while :; do :; done' >"$f/perf-stat-busy.txt" 2>&1
    # and with all 32 cores busy
    sudo perf stat -a -e cycles,task-clock -- sh -c 'for i in $(seq 32); do timeout 5 sh -c "while :; do :; done" & done; wait' >"$f/perf-stat-busy-all.txt" 2>&1
    command -v pyperf >/dev/null && pyperf system show >"$f/pyperf-system-show.txt" 2>&1
    (curl -s -m 2 -H "X-aws-ec2-metadata-token: $(curl -s -m 2 -X PUT http://169.254.169.254/latest/api/token -H 'X-aws-ec2-metadata-token-ttl-seconds: 60')" \
        http://169.254.169.254/latest/meta-data/instance-type; echo) >"$f/instance-type.txt" 2>&1
    head -50 "$f"/*.txt
}
stage_facts() { step facts do_facts; }

# --------------------------------------------------------------------------------------
# build / test
# --------------------------------------------------------------------------------------

do_venv() { # do_venv <b> [deps...]
    local b=$1; shift
    [[ -d $(work "$b") ]] || git clone -q "$BASE/src" "$(work "$b")"
    git -C "$(work "$b")" checkout -q "$COMMIT"
    [[ -x $(venv "$b")/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$(venv "$b")"
    "$(venv "$b")/bin/python" -VV
    if (($#)); then uv pip install -q --python "$(venv "$b")/bin/python" "$@"; fi
}

do_build() {
    local b=$1
    # shellcheck disable=SC2086
    do_venv "$b" $DEPS || return 1
    in_build "$b" maturin develop --uv --release || return 1
    in_build "$b" python -c 'import rustique, sys; print(rustique.__file__, rustique.__version__); rustique.enable(); print("enable ok")'
    ls -la "$(work "$b")/python/rustique/"
    uv pip freeze --python "$(venv "$b")/bin/python" >"$RES/tests/freeze-$b.txt"
}
stage_build() { step "build-$1" do_build "$1"; }

do_pytest() {
    local b=$1
    in_build "$b" python -m pytest -q -p no:cacheprovider --durations=25 --junitxml="$RES/tests/pytest-$b.xml"
}
do_examples() {
    local b=$1 f rc=0
    for f in "$(work "$b")"/examples/*.py; do
        echo "== $f"
        in_build "$b" python "$f" || { echo "FAILED rc=$? $f"; rc=1; }
    done
    return $rc
}
do_downstream() {
    local b=$1
    in_build "$b" maturin develop --uv --release -m examples/rust_extension/Cargo.toml || return 1
    in_build "$b" python -m pytest -q -p no:cacheprovider --junitxml="$RES/tests/downstream-$b.xml" examples/rust_extension tests/test_capi.py
}
do_pytest_full() { # the whole suite again, now that rustique_demo exists (un-skips its tests)
    local b=$1
    in_build "$b" python -m pytest -q -p no:cacheprovider --junitxml="$RES/tests/pytest-full-$b.xml"
}
stage_test() {
    local b=$1
    step "pytest-$b" do_pytest "$b"
    step "examples-$b" do_examples "$b"
    step "downstream-$b" do_downstream "$b"
    stage_pytest_full "$b"
}
stage_pytest_full() { step "pytest-full-$1" do_pytest_full "$1"; }

rust_env() { # PyO3 needs a CPython >= 3.14 and its libpython (as in .github/workflows/ci.yml)
    local py; py=$(pybin 3.15.1)
    export PYO3_PYTHON=$py
    LD_LIBRARY_PATH=$("$py" -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))')
    export LD_LIBRARY_PATH
    export CARGO_TARGET_DIR=$BASE/target/rust-ci
}
do_fmt() { (rust_env && cd "$BASE/src" && cargo fmt --all --check); }
do_clippy() { (rust_env && cd "$BASE/src" && cargo clippy --workspace --all-targets -- -D warnings); }
do_cargo_test() { (rust_env && cd "$BASE/src" && cargo test --workspace); }
stage_rust() {
    step rust-fmt do_fmt
    step rust-clippy do_clippy
    step rust-test do_cargo_test
}

# --------------------------------------------------------------------------------------
# conformance
# --------------------------------------------------------------------------------------

do_conf_suites() {
    local b=$1
    in_build "$b" env CPYTHON_SRC="$(cpysrc "$b")" python tests/conformance/run_cpython_suites.py \
        --json "$RES/conformance/suites-$b.json"
}
do_conf_takeover() {
    local b=$1
    in_build "$b" env CPYTHON_SRC="$(cpysrc "$b")" python tests/test_takeover_conformance.py \
        --json "$RES/conformance/takeover-$b.json"
}
do_conf_takeover_gate() {
    local b=$1
    in_build "$b" env CPYTHON_SRC="$(cpysrc "$b")" python -m pytest -q -p no:cacheprovider \
        --junitxml="$RES/conformance/takeover-gate-$b.xml" tests/test_takeover_conformance.py
}
stage_conformance() {
    local b=$1
    step "conf-suites-$b" do_conf_suites "$b"
    step "conf-takeover-$b" do_conf_takeover "$b"
    step "conf-takeover-gate-$b" do_conf_takeover_gate "$b"
}

# --------------------------------------------------------------------------------------
# 3.16 (PyO3 0.30.1 supports CPython <= 3.15): record what happens, change nothing
# --------------------------------------------------------------------------------------

do_py316_develop() {
    local b=$1
    do_venv "$b" "maturin==1.15.0" || return 1
    in_build "$b" maturin develop --uv --release
}
do_py316_develop_fwd() {
    local b=$1
    in_build "$b" env PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop --uv --release
}
do_py316_sdist() {
    local b=$1 sd
    mkdir -p "$BASE/dist"
    sd=$(ls "$BASE"/dist/rustique-*.tar.gz 2>/dev/null | head -1)
    if [[ -z $sd ]]; then
        in_build "$b" maturin sdist -o "$BASE/dist" || return 1
        sd=$(ls "$BASE"/dist/rustique-*.tar.gz | head -1)
    fi
    echo "sdist: $sd"; tar tzf "$sd" | head -20
    local pv="$BASE/venv/$b-pip"
    [[ -x $pv/bin/python ]] || "$(pybin "$b")" -m venv "$pv"
    "$pv/bin/python" -m pip --version
    # The CPython install's own pip (ensurepip), a PEP 517 build from the sdist, verbose.
    CARGO_TARGET_DIR="$(tgt "$b")-pip" "$pv/bin/python" -m pip install -v --no-cache-dir "$sd"
}
do_py316_testdeps() { # the same test deps where wheels exist (no source builds of numpy on an alpha)
    local b=$1 py; py="$(venv "$b")/bin/python"
    uv pip install --python "$py" pytest==9.1.1 hypothesis==6.170.9 pyperf==2.10.0
    uv pip install --python "$py" --only-binary :all: numpy==2.5.4 || echo "no numpy 2.5.4 wheel for $b"
    uv pip freeze --python "$py" >"$RES/tests/freeze-$b.txt"
}
stage_py316() {
    local b=$1
    # PyO3 0.30.1 accepts max+1 (3.16) as "experimental" (pyo3-ffi build.rs), so this may build.
    step "py316-develop-$b" do_py316_develop "$b"
    step "py316-develop-fwdcompat-$b" do_py316_develop_fwd "$b"
    step "py316-pip-sdist-$b" do_py316_sdist "$b"
    # If it built, run the same tests as the supported builds.
    if in_build "$b" python -c 'import rustique' 2>/dev/null; then stage_py316_test "$b"; fi
    cp "$RES/logs/py316-"*"$b".log "$RES/py316/" 2>/dev/null
}
stage_py316_test() {
    step "py316-testdeps-$1" do_py316_testdeps "$1"
    stage_test "$1"
    # Without numpy, tests/test_takeover_compat.py fails at collection (benches/takeover_compat.py
    # imports numpy unconditionally) and pytest stops: run the rest past that error, then try
    # numpy from source and, if it builds, the whole suite again.
    step "pytest-continue-$1" do_pytest_continue "$1"
    step "py316-numpy-source-$1" do_numpy_source "$1" && stage_pytest_full "$1"
}
do_pytest_continue() {
    local b=$1
    in_build "$b" python -m pytest -q -p no:cacheprovider --continue-on-collection-errors \
        --junitxml="$RES/tests/pytest-continue-$b.xml"
}
do_numpy_source() {
    local b=$1 py; py="$(venv "$b")/bin/python"
    in_build "$b" python -c 'import numpy' 2>/dev/null && { echo "numpy already importable"; return 0; }
    uv pip install --python "$py" --no-binary numpy numpy==2.5.4 || return 1
    in_build "$b" python -c 'import numpy; print(numpy.__version__, numpy.__file__)'
}

# --------------------------------------------------------------------------------------
# scoreboard
# --------------------------------------------------------------------------------------

do_instr() { # instr <b> <cpus> [impls]
    local b=$1 cpus=$2 impls=${3:-enabled}
    local n; n=$(taskset -c "$cpus" nproc)
    in_build "$b" taskset -c "$cpus" python benches/scoreboard.py instr --impls "$impls" --shards "$n" --quiet \
        -o "$RES/scoreboard/instr-$impls-$b.json"
}
stage_instr() { step "instr-${3:-enabled}-$1" do_instr "$@"; }

do_time() { # time <b> <cpu,cpu,...> [impls]
    local b=$1 cpus=$2 impls=${3:-enabled}
    local n; n=$(tr ',' '\n' <<<"$cpus" | wc -l)
    in_build "$b" python benches/scoreboard.py time --size 1e4,1e6 --impls "$impls" --shards "$n" --affinity "$cpus" \
        -o "$RES/scoreboard/time-$impls-$b.json"
}
stage_time() { step "time-${3:-enabled}-$1" do_time "$@"; }

do_report() { # report <b> [impls]
    local b=$1 impls=${2:-enabled} d=$RES/scoreboard
    local files=()
    [[ -f $d/instr-$impls-$b.json ]] && files+=("$d/instr-$impls-$b.json")
    for f in "$d/time-$impls-$b.json".shard*.json; do [[ -f $f ]] && files+=("$f"); done
    ((${#files[@]})) || { echo "no results for $b/$impls"; return 1; }
    in_build "$b" python benches/scoreboard.py report "${files[@]}" --impl "$impls" -o "$d/$impls-$b.md"
    { in_build "$b" python benches/scoreboard.py summary --after "${files[@]}" --impl "$impls"
      [[ -f $d/instr-$impls-$b.json ]] && in_build "$b" python benches/scoreboard.py summary --size \
          --after "$d/instr-$impls-$b.json" --impl "$impls"; } | tee "$d/summary-$impls-$b.md"
    in_build "$b" python "$BASE/analyze_phase0.py" scoreboard --impl "$impls" "${files[@]}" | tee "$d/analysis-$impls-$b.txt"
    # against the matrix committed at $COMMIT (docs/scoreboard/<impl>-<build>.md)
    local committed; committed="$(work "$b")/docs/scoreboard/$impls-$b.md"
    [[ -f $committed ]] && python3 "$BASE/analyze_phase0.py" matrix-compare "$committed" "$d/$impls-$b.md" --top 30 \
        | tee "$d/compare-matrix-$impls-$b.txt"
    return 0
}
stage_report() { step "report-${2:-enabled}-$1" do_report "$@"; }

# Re-measure notable 10k/1M cells (largest deviations from the published matrices, kernel-path
# cells above 1.05, worst unchanged-code cells) with 10 pyperf processes instead of 3.
RECHECK=(
    "tuple|sorted(x)|int,float"  "dict|sum(x)|int"  "dict|min(x)|int"  "list|x.sort()|int,object"
    "list|sorted(x)|int"  "frozenset|all(x)|object,float"  "dict|any(x)|float,str"
    "frozenset|x.isdisjoint(t)|str"  "list|x.clear()|int,mixed"  "dict|tuple(x)|int"  "tuple|reversed|mixed"
    "tuple|x.count(hit)|float"  "list|max(x)|float"  "list|min(x)|int"  "list|sum(x)|int"
)
do_recheck() { # recheck <b> <cpu> [processes]
    local b=$1 cpu=$2 p=${3:-10} i=0 spec t op st
    for spec in "${RECHECK[@]}"; do
        IFS='|' read -r t op st <<<"$spec"
        i=$((i + 1))
        in_build "$b" taskset -c "$cpu" python benches/scoreboard.py time --size 1e4,1e6 --impls enabled --type "$t" --op "$op" \
            --storage "$st" --shards 1 --affinity "$cpu" -p "$p" -o "$RES/scoreboard/recheck-$b-$i.json" || return 1
    done
}
stage_recheck() { step "recheck-$1" do_recheck "$@"; }

# --------------------------------------------------------------------------------------
# pyperformance (plain vs rustique.enable() via sitecustomize)
# --------------------------------------------------------------------------------------

ppdir() { echo "$BASE/pyperf/$1"; }
do_pyperf_venv() {
    local b=$1 d; d=$(ppdir "$b")
    mkdir -p "$d/site"
    cp "$BASE/sitecustomize.py" "$d/site/sitecustomize.py"
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
    # Sanity: rustique is importable and enabled through sitecustomize in the benchmark venv.
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=1 "$bp" -c \
        'import sys; m = sys.modules.get("rustique"); print("enabled: rustique loaded via sitecustomize:", m is not None, m and m.__file__)'
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=0 "$bp" -c \
        'import sys; print("plain: rustique loaded:", "rustique" in sys.modules)'
}
stage_pyperf_venv() { step "pyperf-venv-$1" do_pyperf_venv "$1"; }

do_pyperformance() { # pyperformance <b> plain|enabled <cpus> [tag]  (tag names a replicate, e.g. -B)
    local b=$1 mode=$2 cpus=$3 tag=${4:-} d; d=$(ppdir "$b")
    local en=0; [[ $mode == enabled ]] && en=1
    local out="$RES/pyperformance/$mode-$b$tag.json"
    rm -f "$out"
    (cd "$d" && PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=$en \
        "$d/driver/bin/pyperformance" run --python "$(pybin "$b")" ${PP_BENCHMARKS:+-b "$PP_BENCHMARKS"} \
        --inherit-environ PYTHONPATH,RUSTIQUE_ENABLE --affinity "$cpus" -o "$out")
}
stage_pyperformance() { step "pyperformance-$2-$1${4:-}" do_pyperformance "$@"; }
# asyncio_tcp(_ssl) and asyncio_websockets bind fixed ports (8882, 8001): two pyperformance
# runs that reach them at the same time make one fail with EADDRINUSE. Re-run them alone.
PORT_BENCHMARKS=asyncio_tcp,asyncio_tcp_ssl,asyncio_websockets
stage_pyperformance_ports() { # pyperformance-ports <b> <cpus>: plain then enabled, nothing else running
    PP_BENCHMARKS=$PORT_BENCHMARKS stage_pyperformance "$1" plain "$2" -ports
    PP_BENCHMARKS=$PORT_BENCHMARKS stage_pyperformance "$1" enabled "$2" -ports
}

do_pyperformance_compare() { # pyperformance-compare <b> [tag]
    local b=$1 tag=${2:-} d; d=$(ppdir "$b")
    local p="$RES/pyperformance/plain-$b$tag.json" e="$RES/pyperformance/enabled-$b$tag.json"
    "$d/driver/bin/python" -m pyperf compare_to "$p" "$e" --table -G | tee "$RES/pyperformance/compare-$b$tag.md"
    "$d/driver/bin/python" -m pyperf compare_to "$p" "$e" -G --min-speed=1 >"$RES/pyperformance/compare-$b$tag-full.txt"
    "$d/driver/bin/python" "$BASE/analyze_phase0.py" pyperformance "$p" "$e" | tee "$RES/pyperformance/analysis-$b$tag.txt"
}
stage_pyperformance_compare() { step "pyperformance-compare-$1${2:-}" do_pyperformance_compare "$@"; }

# The published pyperformance/apps numbers were measured at an older commit (BENCHMARKS.md:
# `2789558`). Build that commit in its own clone/venv/target and run the apps there too.
do_build_at() { # build-at <commit> <b>  (no timing; run it on an idle box or a free domain)
    local c=$1 b=$2 w="$BASE/w/$2@$1" v="$BASE/venv/$2@$1"
    [[ -d $w ]] || git clone -q "$BASE/src" "$w"
    git -C "$w" checkout -q "$c" || return 1
    [[ -x $v/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$v"
    uv pip install -q --python "$v/bin/python" maturin==1.15.0 pyperf==2.10.0 networkx pyflakes
    (cd "$w" && VIRTUAL_ENV=$v PATH="$v/bin:$PATH" CARGO_TARGET_DIR="$BASE/target/$b@$c" maturin develop --uv --release)
}
do_apps_at() { # apps-at <commit> <b> <cpus>: default apps + the lint app on test_importlib
    local c=$1 b=$2 cpus=$3 w="$BASE/w/$2@$1" v="$BASE/venv/$2@$1"
    (cd "$w" && taskset -c "$cpus" "$v/bin/python" benches/apps/run_apps.py bench -o "$RES/apps/apps-$b@$c.json") || return 1
    (cd "$w" && taskset -c "$cpus" "$v/bin/python" benches/apps/run_apps.py bench --apps lint \
        --lint-path "$(cpysrc "$b")/Lib/test/test_importlib" -o "$RES/apps/lint-importlib-$b@$c.json")
}

# --------------------------------------------------------------------------------------
# applications
# --------------------------------------------------------------------------------------

do_apps() { # apps <b> <cpus> [tag]
    local b=$1 cpus=$2 tag=${3:-}
    in_build "$b" taskset -c "$cpus" python benches/apps/run_apps.py bench -o "$RES/apps/apps-$b$tag.json"
}
stage_apps() { step "apps-$1${3:-}" do_apps "$@"; }
# The lint app over CPython's Lib/test/test_importlib (89 files): BENCHMARKS.md describes the
# published lint workload as "pyflakes over ~90 files of CPython's tests", while run_apps.py
# defaults to this repo's tests/ + benches/ (37 files).
do_apps_lint_cpython() { # apps-lint-cpython <b> <cpus>
    local b=$1 cpus=$2
    in_build "$b" taskset -c "$cpus" python benches/apps/run_apps.py bench --apps lint \
        --lint-path "$(cpysrc "$b")/Lib/test/test_importlib" -o "$RES/apps/lint-importlib-$b.json"
}

stage_pack() {
    find "$RES" -type f \( -name '*.json' -o -name '*.log' -o -name '*.xml' \) -size +1M ! -name '*.gz' -exec gzip -9f {} +
    du -sh "$RES"
}

# --------------------------------------------------------------------------------------
# all: the scheduled run
# --------------------------------------------------------------------------------------

stage_all() {
    # The schedule actually used for Phase 0 (2026-11-01). Lanes run concurrently; inside a
    # lane steps run in order. Domain 0 = callgrind, then timing; domains 1-3 = timing only.
    local t0; t0=$(date +%s)
    local D0=${DOMAINS[0]} D1=${DOMAINS[1]} D2=${DOMAINS[2]} D3=${DOMAINS[3]}
    local T="${TIMING_CPU[1]},${TIMING_CPU[2]},${TIMING_CPU[3]}"
    local i=0 b
    stage_setup
    stage_facts
    # Wave A (no timing, ~20 min): build + test per interpreter, 3.16 attempts, then Rust
    # checks, conformance and the pyperformance venvs.
    for b in $BUILDS; do
        (CARGO_BUILD_JOBS=8 taskset -c "${DOMAINS[$((i % 4))]}" "$0" build "$b"; "$0" test "$b") &
        i=$((i + 1))
    done
    (for b in $BUILDS_316; do CARGO_BUILD_JOBS=8 "$0" py316 "$b"; done) &
    wait
    (CARGO_BUILD_JOBS=8 taskset -c "$D0" "$0" rust) &
    (taskset -c "$D1" "$0" conformance 3.15.1; taskset -c "$D1" "$0" conformance 3.14.9) &
    (taskset -c "$D2" "$0" conformance 3.15.1t; taskset -c "$D2" "$0" conformance 3.14.9t) &
    (for b in 3.15.1 3.15.1t; do taskset -c "$D3" "$0" pyperf-venv "$b"; done) &
    wait
    # Wave B (~4.5 h).
    (   # lane 0: callgrind (enabled, then the optional Rustique types), then timing jobs
        "$0" instr 3.15.1 "$D0"; "$0" instr 3.15.1t "$D0"
        "$0" instr 3.15.1 "$D0" rustique; "$0" instr 3.15.1t "$D0" rustique
        "$0" pyperformance 3.15.1 plain "$D0"; "$0" pyperformance 3.15.1 enabled "$D0"
        "$0" apps 3.15.1 "$D0"; "$0" apps 3.15.1t "$D0"; "$0" apps 3.15.1 "$D0" -2; "$0" apps 3.15.1t "$D0" -2
        "$0" recheck 3.15.1 "${TIMING_CPU[0]}"; "$0" recheck 3.15.1t "${TIMING_CPU[0]}"
    ) &
    (   # lanes 1-3: scoreboard wall time (one single-CPU shard per domain), then three
        # pyperformance replicates, one per domain, A = plain first, B = enabled first
        "$0" time 3.15.1 "$T"; "$0" time 3.15.1t "$T"
        ("$0" pyperformance 3.15.1t plain "$D1"; "$0" pyperformance 3.15.1t enabled "$D1") &
        ("$0" pyperformance 3.15.1t enabled "$D2" -B; "$0" pyperformance 3.15.1t plain "$D2" -B) &
        ("$0" pyperformance 3.15.1 enabled "$D3" -B; "$0" pyperformance 3.15.1 plain "$D3" -B) &
        wait
    ) &
    wait
    # Wave C (optional, ~1.6 h): the lint app on CPython's tests, then wall time of
    # Rustique's own types, two shards per build.
    ("$0" pyperformance-ports 3.15.1t "$D1"; "$0" pyperformance-ports 3.15.1 "$D1") &
    ("$0" apps-lint-cpython 3.15.1 "$D0"; "$0" apps-lint-cpython 3.15.1t "$D0") &
    wait
    "$0" time 3.15.1 "${TIMING_CPU[0]},${TIMING_CPU[1]}" rustique &
    "$0" time 3.15.1t "${TIMING_CPU[2]},${TIMING_CPU[3]}" rustique &
    wait
    # The apps at the commit the published apps numbers came from.
    for b in 3.15.1 3.15.1t; do CARGO_BUILD_JOBS=16 "$0" build-at 2789558 "$b"; done
    "$0" apps-at 2789558 3.15.1 "$D0" & "$0" apps-at 2789558 3.15.1t "$D1" & wait
    "$0" apps 3.15.1 "$D0" -3 & "$0" apps 3.15.1t "$D1" -3 & wait
    # Reports (box otherwise idle).
    for b in 3.15.1 3.15.1t; do
        "$0" report "$b"; "$0" report "$b" rustique
        "$0" pyperformance-compare "$b"; "$0" pyperformance-compare "$b" -B
    done
    stage_pack
    log "ALL done in $(($(date +%s) - t0))s"
}

# --------------------------------------------------------------------------------------

main() {
    local stage=${1:?stage}; shift
    case $stage in
        setup | facts | rust | all | pack) "stage_$stage" ;;
        build | test | conformance | py316 | instr | time | report | apps | pyperformance) "stage_$stage" "$@" ;;
        pyperf-venv) stage_pyperf_venv "$@" ;;
        py316-test) stage_py316_test "$@" ;;
        pytest-full) stage_pytest_full "$@" ;;
        recheck) stage_recheck "$@" ;;
        build-at) step "build-$2@$1" do_build_at "$@" ;;
        apps-at) step "apps-$2@$1" do_apps_at "$@" ;;
        pyperformance-ports) stage_pyperformance_ports "$@" ;;
        apps-lint-cpython) step "apps-lint-importlib-$1" do_apps_lint_cpython "$@" ;;
        py316-extra) step "pytest-continue-$1" do_pytest_continue "$1"; step "py316-numpy-source-$1" do_numpy_source "$1" && stage_pytest_full "$1" ;;
        pyperformance-compare) stage_pyperformance_compare "$@" ;;
        *) echo "unknown stage $stage" >&2; exit 2 ;;
    esac
}
main "$@"
