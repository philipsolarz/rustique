#!/usr/bin/env bash
# Wave 2, Phase 3: integrate and prove, on the benchmark box (Rustique `wave2` at $COMMIT).
#
# Wave 2 Phase 0's runner (benches/results/w2-phase0/run_w2phase0.sh) with the wave-2 changes:
# all 8 workload families (65 workloads), the opt-in tiers (interp, gc), the repacked release
# wheel (top-level `_rustique`, tools/repack_wheel.py), the interp tier's handler sets
# (clang-20 + llvm-profdata-20, RUSTIQUE_INTERP_REQUIRE=1), status on every interpreter the box
# has (T10), the workstreams' quiet-box addenda (gc tier, threads scaling, estimator).
# Same box type and conventions: AWS c7a.8xlarge, 32 cores, 4 L3 domains (0-7, 8-15, 16-23,
# 24-31). Everything lives under $BASE (default /work/w2p3); results under $RES, the directory
# copied off the box after every step.
#
# Usage (on the box): ./run_w2phase3.sh <stage> [args]     (deploy by atomic rename: bash reads
# a running script incrementally; helper .py files go to $BASE/bin/)
#
# "plain" = the family venv without Rustique enabled; "default" = rustique.enable() (every
# default group); "+interp"/"+gc"/"+both" = the default tier plus RUSTIQUE_TIERS=interp|gc|gc,interp.
# RUSTIQUE_VALIDATE=eager only for harnesses that read states right after enable (status,
# conformance); timed runs keep the shipped first-call validation.
#
# Stages:
#   setup | facts | quiet               as Phase 0 (clone $COMMIT from the bundle, toolchain, machine)
#   build <b>                           venv + test deps + fat-LTO `maturin develop --uv --release`
#   wheel <b>                           fat-LTO `maturin build --release` + tools/repack_wheel.py
#   status <b>                          status_dump.py in the build venv (VALIDATE=eager, every target module)
#   statusx <b> <wheel-build>           a fresh venv on interpreter <b> with <wheel-build>'s wheel; status + reach.py
#   status316 <b>                       3.16.0a1(t): the wheel (must not install), then `uv pip install <src>`
#   test <b> | rust                     full `pytest -q`; cargo fmt --check, clippy -D warnings, cargo test
#   conformance <b>                     run_target_suites.py, every default group at once, $CONF_MODULES
#   wlvenv <b> [nowheel]                one venv per family: requirements (+ --no-deps where complete) + pyperf + wheel
#   wlcheck <b> [enabled]               every workload once (`run.py one -n 1`), plain [and enabled]: imports + digest
#   workloads <b> <cpus> [tag] [fams]   run.py compare <family> -n 5 -p 5, default tier (the headline)
#   t7 <b> <cpus>                       T7: isort_stdlib, state_snapshot with RUSTIQUE_EXCLUDE=copy (after the 3.15.1t headline)
#   tiers <b> <cpus> <sides> [fams]     tiers_compare.py: plain / default / +tier interleaved, -n 5 -p 5
#   wlstats-run <b>                     counters of one steady-state run() per workload
#   whole <b> <cpus> [rounds]           pyperf command, whole process, plain vs RUSTIQUE=1, $WHOLE
#   pyperf-venv <b> | pyperformance <b> plain|enabled <cpus> [tag] | pyperformance-ports <b> <cpus>
#   pyperformance-compare <b> [tag]
#   startup <b> <cpu>                   startup.py cells (bare and other .pth) + decompose.py, repacked wheel
#   instr <b> <cpus> | time <b> <cpu,..> | report <b> | isolate-instr <b> <cpus> | isolate-time <b> <cpu,..>
#   sbprobe <b> <cpu,..>                sb_probe.py on Phase 0's ten non-kernel cells (stub/stub/imported/enabled)
#   gcquiet <gil-cpus> <ft-cpus> [stages]   benches/runtime/run_quiet.sh (the gc tier's wall/pause/RSS run)
#   ftquiet <cpus> [step]               benches/ft/quietbox.sh (threads scaling, both builds, services venvs)
#   est <b> <first-core>                benches/dx/est_validate.sh STEPS=est (estimator, 8 families)
#   pack                                gzip result files > 1 MB
#   waveA | waveB | waveC | all         the schedule (stage_wave* below): A = stages 1-2, B = 3, 4, 6, 9 +
#                                       callgrind/estimator, C = 5, 8, 7
#
# Builds: python-build-standalone CPython 3.15.1, 3.15.1t, 3.14.9, 3.14.9t (release 20261021,
# as wave 1 and Phase 0) for builds/tests/timing; status also on 3.15.2(t), 3.14.10(t)
# (20261026), 3.15.0, 3.14.0, 3.14.1, 3.14.5, 3.14.6, 3.14.8 (uv's latest PBS builds) and
# 3.16.0a1(t) (must refuse).

set -uo pipefail

COMMIT=${COMMIT:?set COMMIT (the wave2 commit measured)}
BASE=${BASE:-/work/w2p3}
RES=${RES:-$BASE/results}
BUNDLE=${BUNDLE:-$BASE/rustique.bundle}
CPY=${CPY:-/work/cpython}
BIN=$BASE/bin            # this run's helpers (status_dump.py, tiers_compare.py, ...) and Phase 0's
SRC=$BASE/src            # the reference clone at $COMMIT (wave 1 / Phase 0 helper scripts)
PYPERFORMANCE_VERSION=${PYPERFORMANCE_VERSION:-1.14.0}
DEPS=${DEPS:-"maturin==1.15.0 pytest==9.1.1 hypothesis==6.170.9 pyperf==2.10.0 networkx pyflakes"}
NUMPY=${NUMPY:-numpy==2.5.4}
BUILDS="3.15.1 3.15.1t 3.14.9 3.14.9t"
DOMAINS=(0-7 8-15 16-23 24-31)
TIMING_CPU=(2 10 20 30)   # one CPU per L3 domain, as wave 1 and Phase 0
# Wave 1's 27 modules, then the wave-2 groups' (docs/targets/{datamodel,re,set,copy}.md)
CONF_W1=test.test_list,test.test_tuple,test.test_sort,test.test_builtin,test.test_json,test.test_float,test.test_format,test.test_strtod,test.test_fstring,test.test_copy,test.test_copyreg,test.test_dataclasses,test.test_strptime,test.test_datetime,test.test_time,test.test_posixpath,test.test_genericpath,test.test_fnmatch,test.test_glob,test.test_pathlib,test.test_os,test.test_logging,test.test_random,test.test_textwrap,test.test_urlparse,test.test_urllib,test.test_statistics
CONF_W2=test.test_enum,test.test_collections,test.test_contextlib,test.test_contextlib_async,test.test_functools,test.test_re,test.test_email,test.test_tokenize,test.test_configparser,test.test_argparse,test.test_uuid,test.test_set,test.test_inspect,test.test_weakref,test.test_pickle,test.test_typing,test.test_http_cookies,test.test_difflib,test.test_string,test.test_shlex,test.test_gettext,test.test_csv,test.test_mailbox
CONF_MODULES=${CONF_MODULES:-$CONF_W1,$CONF_W2}
WHOLE=${WHOLE:-"data/state_snapshot data/log_analytics data/report_render web/json_api devtools/pytest_networkx devtools/import_startup frameworks/django_admin custom/rules_engine"}
FAMILIES=${FAMILIES:-"web data devtools frameworks libs custom services threads"}
NODEPS_FAMILIES="frameworks libs services"   # requirements complete: --no-deps (brief)

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
unset RUSTIQUE RUSTIQUE_ONLY RUSTIQUE_EXCLUDE RUSTIQUE_STATS RUSTIQUE_REPORT RUSTIQUE_CHECK RUSTIQUE_ENABLE \
    RUSTIQUE_TIERS RUSTIQUE_VALIDATE RUSTIQUE_UNVALIDATED_PATCH PYTHON_JIT
export RUSTIQUE_INTERP_REQUIRE=1    # a missing clang/llvm-profdata fails the build (as CI)
mkdir -p "$RES"/{logs,machine,tests,status,conformance,workloads,tiers,whole,pyperformance,startup,scoreboard,gcq,ftq,est}

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
whl() { ls "$BASE/dist/$1"/rustique-*.whl 2>/dev/null | head -1; }

in_build() { # in_build <b> <command...>: inside the build's clone with its venv active
    local b=$1; shift
    (cd "$(work "$b")" && VIRTUAL_ENV="$(venv "$b")" PATH="$(venv "$b")/bin:$PATH" \
        CARGO_TARGET_DIR="$(tgt "$b")" "$@")
}

# ---------------------------------------------------------------------------------------
do_setup() {
    [[ -d $SRC/.git ]] || git clone -q "$BUNDLE" "$SRC"
    git -C "$SRC" fetch -q "$BUNDLE" '+refs/heads/*:refs/remotes/bundle/*' 2>/dev/null
    git -C "$SRC" checkout -q "$COMMIT" || return 1
    git -C "$SRC" log -1 --format='%H %s'
    (cd "$SRC" && rustc -Vv && cargo -V && cargo clippy -V && cargo fmt --version)
    uv --version; valgrind --version; perf --version 2>&1 | head -1; hyperfine --version
    clang-20 --version | head -1; llvm-profdata-20 --version | head -2
    : >"$RES/machine/pythons.txt"
    local b p
    for b in $BUILDS 3.15.2 3.15.2t 3.14.10 3.14.10t 3.15.0 3.14.0 3.14.1 3.14.5 3.14.6 3.14.8 3.16.0a1 3.16.0a1t; do
        p=$(pybin "$b") && { echo "== $b: $p"; "$p" -VV; } >>"$RES/machine/pythons.txt"
    done
    cat "$RES/machine/pythons.txt"
    for v in 3.15.1 3.14.9 3.15.2 3.14.10; do git -C "$CPY/v$v" log -1 --format="v$v %H %s"; done
    uname -a; nproc; free -g
}

do_facts() {
    local f=$RES/machine
    uname -a >"$f/uname.txt"
    cat /etc/os-release >"$f/os-release.txt"
    lscpu >"$f/lscpu.txt"
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
    uptime >"$f/uptime.txt"
    head -30 "$f"/*.txt
}

do_quiet() { # nothing on a timer may wake up during the run
    systemctl list-timers --all --no-pager
    sudo systemctl stop apt-daily.timer apt-daily-upgrade.timer unattended-upgrades.service motd-news.timer \
        sysstat-collect.timer sysstat-summary.timer fwupd-refresh.timer man-db.timer dpkg-db-backup.timer \
        logrotate.timer update-notifier-download.timer 2>&1
    systemctl list-timers --all --no-pager
}

do_build() {
    local b=$1
    [[ -d $(work "$b") ]] || git clone -q "$SRC" "$(work "$b")"
    git -C "$(work "$b")" fetch -q "$SRC" '+refs/remotes/bundle/*:refs/remotes/src/*' 2>/dev/null
    git -C "$(work "$b")" fetch -q "$SRC" "$COMMIT" 2>/dev/null
    git -C "$(work "$b")" checkout -q "$COMMIT" || return 1
    git -C "$(work "$b")" log -1 --format='%H'
    [[ -x $(venv "$b")/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$(venv "$b")"
    "$(venv "$b")/bin/python" -VV
    # shellcheck disable=SC2086
    uv pip install -q --python "$(venv "$b")/bin/python" $DEPS || return 1
    uv pip install --python "$(venv "$b")/bin/python" "$NUMPY" || echo "NUMPY-FAILED: $NUMPY did not install on $b"
    in_build "$b" maturin develop --uv --release || return 1
    in_build "$b" python -c 'import rustique, sys; print(rustique.__file__, rustique.__version__); print(sorted(m for m in sys.modules if "rustique" in m)); rustique.enable(); print("enable ok")'
    ls -la "$(work "$b")"/python/rustique/*.so
    uv pip freeze --python "$(venv "$b")/bin/python" >"$RES/tests/freeze-$b.txt"
}
do_wheel() {
    local b=$1
    mkdir -p "$BASE/dist/$b"; rm -f "$BASE/dist/$b"/*.whl
    in_build "$b" maturin build --release -i "$(venv "$b")/bin/python" -o "$BASE/dist/$b" || return 1
    in_build "$b" python tools/repack_wheel.py "$BASE/dist/$b"/*.whl || return 1
    ls -la "$BASE/dist/$b"
    python3 -c 'import sys, zipfile; [print(i.file_size, i.filename) for i in zipfile.ZipFile(sys.argv[1]).infolist() if i.filename.endswith((".pth", ".so", ".py")) and "/" not in i.filename.rstrip("/")]' "$(whl "$b")"
    sha256sum "$(whl "$b")"
}
do_status() { # the build venv (maturin develop), every target module imported, eager validation
    local b=$1
    in_build "$b" env RUSTIQUE_VALIDATE=eager python "$BIN/status_dump.py" "$RES/status/status-$b.json"
    in_build "$b" env RUSTIQUE_VALIDATE=eager RUSTIQUE_TIERS=interp,gc python "$BIN/status_dump.py" "$RES/status/status-tiers-$b.json" >/dev/null
    in_build "$b" python benches/accel/reach.py "$(venv "$b")/bin/python" --json "$RES/status/reach-$b.json"
}
do_statusx() { # statusx <b> <wheel-build>: a fresh venv on <b> with the release wheel of <wheel-build>
    local b=$1 wb=$2 v="$BASE/sv/$1"
    rm -rf "$v"; uv venv -q --python "$(pyreq "$b")" "$v" || return 1
    "$v/bin/python" -VV
    uv pip install --python "$v/bin/python" --no-deps "$(whl "$wb")" || return 1
    (cd "$SRC" && env RUSTIQUE_VALIDATE=eager "$v/bin/python" "$BIN/status_dump.py" "$RES/status/status-$b.json")
    (cd "$SRC" && env RUSTIQUE_VALIDATE=eager RUSTIQUE_TIERS=interp,gc "$v/bin/python" "$BIN/status_dump.py" "$RES/status/status-tiers-$b.json" >/dev/null)
    (cd "$SRC" && "$v/bin/python" benches/accel/reach.py "$v/bin/python" --json "$RES/status/reach-$b.json")
    # RUSTIQUE_UNVALIDATED_PATCH=1 on an unrecorded release (3.14.0): what the opt-in installs
    (cd "$SRC" && env RUSTIQUE_VALIDATE=eager RUSTIQUE_UNVALIDATED_PATCH=1 "$v/bin/python" "$BIN/status_dump.py" "$RES/status/status-unvalidated-$b.json" >/dev/null)
    return 0
}
do_status316() { # 3.16.0a1(t): the cp315 wheel must not install; the source install, then status
    local b=$1 v="$BASE/sv/$1" wb=3.15.1; [[ $b == *t ]] && wb=3.15.1t
    rm -rf "$v"; uv venv -q --python "$(pyreq "$b")" "$v" || return 1
    "$v/bin/python" -VV
    echo "== wheel $(basename "$(whl "$wb")")"
    uv pip install --python "$v/bin/python" --no-deps "$(whl "$wb")" && echo "UNEXPECTED: the cp315 wheel installed"
    echo "== pip from the source tree (what uv does on a 3.16 pre-release: the sdist)"
    (cd "$SRC" && CARGO_TARGET_DIR="$BASE/target/$b" uv pip install --python "$v/bin/python" --no-deps . ) || { echo "source install refused/failed (above)"; return 0; }
    (cd "$SRC" && env RUSTIQUE_VALIDATE=eager "$v/bin/python" "$BIN/status_dump.py" "$RES/status/status-$b.json")
    (cd /tmp && env RUSTIQUE=1 "$v/bin/python" -c 'import sys; print("RUSTIQUE=1 loads:", sorted(m for m in sys.modules if "rustique" in m))')
}
do_pytest() {
    local b=$1 tag=${2:-} extra=()
    if ! in_build "$b" python -c 'import numpy' 2>/dev/null; then
        echo "numpy is not importable on $b: --continue-on-collection-errors"
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
# conformance: every default group at once (tier targets excluded), eager validation
do_conformance() {
    local b=$1 groups names
    groups=$(in_build "$b" env RUSTIQUE_VALIDATE=eager python -c 'import json; d = json.load(open("'"$RES/status/status-$b.json"'"))["status"]
print(",".join(sorted({t["group"] for t in d["targets"] if not t.get("tier")})))') || return 1
    # every default target installed on this build in the eager status dump (the others are listed apart)
    names=$(in_build "$b" python -c 'import json; d = json.load(open("'"$RES/status/status-$b.json"'"))["status"]
print(",".join(sorted(t["name"] for t in d["targets"] if not t.get("tier") and t["state"] == "installed")))') || return 1
    echo "groups: $groups"; echo "expect installed: $names"
    in_build "$b" python -c 'import json; d = json.load(open("'"$RES/status/status-$b.json"'"))["status"]
print("not installed on this build (not expected):", [(t["name"], t["state"], t.get("refusal")) for t in d["targets"] if not t.get("tier") and t["state"] != "installed"])'
    in_build "$b" env RUSTIQUE_VALIDATE=eager CPYTHON_SRC="$(cpysrc "$b")" python tests/conformance/run_target_suites.py \
        --targets "$groups" --modules "$CONF_MODULES" --expect-installed "$names" --timeout 3600 \
        --json "$RES/conformance/all-groups-$b.json"
}

# ---------------------------------------------------------------------------------------
# workloads
do_wlvenv() { # wlvenv <b> [nowheel]: one venv per family; complete pins with --no-deps
    local b=$1 nowheel=${2:-} f v rc=0 nd
    local wroot; wroot=$(work "$b"); [[ -d $wroot ]] || wroot=$SRC
    for f in $FAMILIES; do
        v=$(wl "$b" "$f"); echo "== $f: $v"
        [[ -x $v/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$v"
        nd=(); [[ " $NODEPS_FAMILIES " == *" $f "* ]] && nd=(--no-deps)
        if grep -qv '^\s*\(#\|$\)' "$wroot/benches/workloads/$f/requirements.txt"; then
            uv pip install --python "$v/bin/python" "${nd[@]}" -r "$wroot/benches/workloads/$f/requirements.txt" || { rc=1; continue; }
        fi
        uv pip install -q --python "$v/bin/python" pyperf==2.10.0 || { rc=1; continue; }
        if [[ -z $nowheel ]]; then
            uv pip install --python "$v/bin/python" --reinstall --no-deps "$(whl "$b")" || { rc=1; continue; }
            ls "$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')" | grep -i rustique
            # RUSTIQUE=1 reaches the extension through the shipped .pth; unset loads nothing
            (cd /tmp && env RUSTIQUE=1 "$v/bin/python" -c 'import sys; print("RUSTIQUE=1:", sorted(m for m in sys.modules if "rustique" in m)); import rustique; s=rustique.status(); print(s["process"].get("switch")); print({k: sum(1 for t in s["targets"] if t["state"] == k) for k in ("installed","armed","pending-import","refused","disabled")})')
            (cd /tmp && "$v/bin/python" -c 'import sys; print("unset:", sorted(m for m in sys.modules if "rustique" in m))')
            (cd /tmp && env RUSTIQUE_VALIDATE=eager RUSTIQUE_TIERS=interp,gc "$v/bin/python" -c 'import rustique; rustique.enable(); s = rustique.status()
print("tiers:", {t["name"]: (t["state"], t.get("refusal")) for t in s["targets"] if t.get("tier")})')
        fi
        uv pip check --python "$v/bin/python" || echo "uv pip check: unsatisfied (recorded, --no-deps family: $f)"
        uv pip freeze --python "$v/bin/python" >"$RES/workloads/freeze-$b-$f.txt"
    done
    return $rc
}
do_wlcheck() { # wlcheck <b> [enabled]: each workload once in its family venv (setup, warm-up, 1 run)
    local b=$1 en=${2:-} f w rc=0 out
    out="$RES/workloads/wlcheck-$b${en:+-enabled}.jsonl"   # (set -u: not in the same `local`)
    : >"$out"
    local wroot; wroot=$(work "$b"); [[ -d $wroot ]] || wroot=$SRC
    for f in $FAMILIES; do
        for w in $(cd "$wroot" && "$(wl "$b" "$f")/bin/python" benches/workloads/run.py list | grep "^$f/"); do
            local t0 r; t0=$(date +%s.%N)
            if r=$(cd "$wroot" && "$(wl "$b" "$f")/bin/python" benches/workloads/run.py one "$w" -n 1 ${en:+--enable} 2>"$RES/logs/wlcheck-$b-${w//\//__}.err"); then
                echo "$r" | tail -1 >>"$out"; echo "ok   $w $(python3 -c "print(round($(date +%s.%N) - $t0, 1))")s"
            else
                echo "FAIL $w"; tail -5 "$RES/logs/wlcheck-$b-${w//\//__}.err"; rc=1
            fi
        done
    done
    return $rc
}
do_workloads() { # workloads <b> <cpus> [tag] [families]: run.py compare per family, default tier
    local b=$1 cpus=$2 tag=${3:-} fams=${4:-$FAMILIES} f rc=0
    for f in $fams; do
        (cd "$(work "$b")" && env -u RUSTIQUE_TIERS taskset -c "$cpus" "$(wl "$b" "$f")/bin/python" benches/workloads/run.py compare "$f" \
            -n 5 -p 5 -o "$RES/workloads/compare-$b-$f$tag.json") || rc=1
    done
    return $rc
}
do_t7() { # t7 <b> <cpus>: T7, isort_stdlib and state_snapshot with the copy group off (RUSTIQUE_EXCLUDE=copy)
    local b=$1 cpus=$2 w rc=0
    for w in devtools/isort_stdlib data/state_snapshot; do
        (cd "$(work "$b")" && env -u RUSTIQUE_TIERS RUSTIQUE_EXCLUDE=copy taskset -c "$cpus" "$(wl "$b" "${w%%/*}")/bin/python" \
            benches/workloads/run.py compare "$w" -n 5 -p 5 -o "$RES/workloads/t7-$b-${w%%/*}.json") || rc=1
    done
    (cd /tmp && RUSTIQUE_EXCLUDE=copy "$(wl "$b" data)/bin/python" -c 'import rustique; rustique.enable(); import copy; s = rustique.status()
print("copy group with RUSTIQUE_EXCLUDE=copy:", {t["name"]: t["state"] for t in s["targets"] if t["group"] == "copy"})')
    return $rc
}
do_tiers() { # tiers <b> <cpus> <sides> [families]: plain/default/+tier(s) interleaved per family
    local b=$1 cpus=$2 sides=$3 fams=${4:-$FAMILIES} f rc=0 tag=${3//,/-}
    for f in $fams; do
        (cd "$(work "$b")" && taskset -c "$cpus" "$(wl "$b" "$f")/bin/python" "$BIN/tiers_compare.py" "$f" --sides "$sides" \
            -n 5 -p 5 -o "$RES/tiers/tiers-$tag-$b-$f.json") || rc=1
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

# ---------------------------------------------------------------------------------------
# pyperformance (wave 1's method: plain vs rustique.enable() through sitecustomize)
do_pyperf_venv() {
    local b=$1 d; d=$(ppdir "$b")
    mkdir -p "$d/site"
    cp "$SRC/benches/results/phase0/sitecustomize.py" "$d/site/sitecustomize.py"
    [[ -x $d/driver/bin/python ]] || uv venv -q --python "$(pyreq "$b")" "$d/driver"
    uv pip install -q --python "$d/driver/bin/python" "pyperformance==$PYPERFORMANCE_VERSION"
    uv pip freeze --python "$d/driver/bin/python"
    (cd "$d" && env -u PYTHONPATH -u RUSTIQUE_ENABLE "$d/driver/bin/pyperformance" venv create \
        --python "$(pybin "$b")") || return 1
    local bp; bp=$(ls "$d"/venv/*/bin/python | head -1)
    echo "benchmark venv python: $bp"
    uv pip freeze --python "$bp" >"$RES/pyperformance/bench-venv-freeze-$b.txt" 2>&1 || true
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=1 "$bp" -c \
        'import sys; m = sys.modules.get("rustique"); s = m and m.status(); print("enabled: rustique loaded via sitecustomize:", m is not None, m and m.__file__, {k: sum(t["state"]==k for t in s["targets"]) for k in ("installed","armed","pending-import","refused")} if s else None)'
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=0 "$bp" -c \
        'import sys; print("plain: rustique loaded:", sorted(m for m in sys.modules if "rustique" in m))'
}
do_pyperformance() { # <b> plain|enabled <cpus> [tag]
    local b=$1 mode=$2 cpus=$3 tag=${4:-} d; d=$(ppdir "$b")
    local en=0; [[ $mode == enabled ]] && en=1
    local out="$RES/pyperformance/$mode-$b$tag.json"
    rm -f "$out"
    local bp; bp=$(ls "$d"/venv/*/bin/python | head -1)
    PYTHONPATH="$(work "$b")/python:$d/site" RUSTIQUE_ENABLE=$en "$bp" -c \
        'import sys; m = sys.modules.get("rustique"); print("rustique loaded:", m and m.__file__)'
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
# startup cells: the repacked release wheel in fresh `python -m venv --without-pip` venvs vs
# identical venvs without it (engine's release_cells.sh method), bare and with another .pth
do_startup() { # <b> <cpu> [rounds]
    local b=$1 cpu=$2 rounds=${3:-300} kind base; base=$(pybin "$b")
    for kind in bare pth; do
        local pv="$BASE/su/$b/plain-$kind" rv="$BASE/su/$b/rq-$kind"
        rm -rf "$pv" "$rv"
        "$base" -m venv --without-pip "$pv" && "$base" -m venv --without-pip "$rv" || return 1
        uv pip install -q --python "$rv/bin/python" --no-deps "$(whl "$b")" || return 1
        if [[ $kind == pth ]]; then
            for v in "$pv" "$rv"; do
                echo "# stands for any other .pth" >"$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')/aaa_other.pth"
            done
        fi
        for v in "$pv" "$rv"; do ls -la "$("$v/bin/python" -c 'import sysconfig; print(sysconfig.get_path("purelib"))')"; done
        for i in 1 2 3; do "$pv/bin/python" -c pass; "$rv/bin/python" -c pass; RUSTIQUE=1 "$rv/bin/python" -c pass
            "$rv/bin/python" -c 'import rustique; rustique.enable()'; done
        (cd "$(work "$b")" && taskset -c "$cpu" "$base" benches/dx/startup.py cells "$pv" "$rv" \
            -r "$rounds" --ir-runs 3 -o "$RES/startup/cells-$b-$kind.json") || return 1
    done
    (cd "$(work "$b")" && taskset -c "$cpu" "$base" benches/startup/decompose.py "$BASE/su/$b/plain-bare" "$BASE/su/$b/rq-bare" \
        --perf-runs 300 --wall-rounds "$rounds" -o "$RES/startup/decompose-$b.json") || return 1
    python3 -c 'import sys, zipfile; [print(f"{i.file_size:,}", i.filename) for i in zipfile.ZipFile(sys.argv[1]).infolist() if i.filename.endswith(".so")]' "$(whl "$b")" \
        | tee "$RES/startup/so-size-$b.txt"
}

# ---------------------------------------------------------------------------------------
# scoreboard (as wave 1 and Phase 0)
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
    return 0
}
do_isolate_instr() { # every 0-100 cell > 1.05, alone in its own callgrind worker, 3 runs
    local b=$1 cpus=$2
    in_build "$b" taskset -c "$cpus" python "$SRC/benches/results/w2-phase0/sb_isolate.py" instr --results "$RES/scoreboard/instr-enabled-$b.json" \
        --cpus "$cpus" --reps 3 -o "$RES/scoreboard/isolated-instr-$b.txt"
}
do_isolate_time() { # every 10k/1M cell > 1.05, alone on one CPU, 10 processes x 5 values, 2 runs
    local b=$1 cpus=$2
    in_build "$b" python "$SRC/benches/results/w2-phase0/sb_isolate.py" time --results "$RES/scoreboard/time-enabled-$b.json".shard*.json \
        --cpus "$cpus" --reps 2 -o "$RES/scoreboard/isolated-time-$b.txt"
}

# The ten non-kernel cells Phase 0 found above 1.05 alone (w2-phase0.md §7.2): stub (no
# extension) / stub again (control) / imported / enabled, wall time and perf counters
SB_CELLS=${SB_CELLS:-"list/pickle.dumps/str/1000000;tuple/for v in x/int/10000;frozenset/x == t/str/1000000;list/pickle round trip/str/1000000;set/x == t/object/10000;list/repr(x)/object/1000000;tuple/for v in x/str/10000;dict/x == t/object/10000;dict/tuple(x)/mixed/10000;dict/hit in x/object/1000000"}
do_sbprobe() { # sbprobe <b> <cpu,..>: alone on one CPU per domain, nothing else timing; Phase 0's ten
    local b=$1 cpus=$2 cells=$SB_CELLS extra        # + every 10k/1M cell this run's isolation left > 1.05
    extra=$(sed -nE 's/^#\s+\S+ (\S.*): [0-9.]+ -> .*STILL > 1\.05.*/\1/p' "$RES/scoreboard/isolated-time-$b.txt" 2>/dev/null | tr '\n' ';')
    cells="$cells;$extra"
    echo "cells: $cells"
    in_build "$b" python "$BIN/sb_probe.py" run --cells "$cells" --cpus "$cpus" -o "$RES/scoreboard/probe-$b.json"
}
do_sbheap() { # sbheap <b> <cpus>: callgrind cells still > 1.05 alone, plain/enabled at 4 heap offsets (untimed)
    local b=$1 cpus=$2
    in_build "$b" taskset -c "$cpus" python "$BIN/sb_heap.py" --isolated "$RES/scoreboard/isolated-instr-$b.txt" \
        -j "$(taskset -c "$cpus" nproc)" -o "$RES/scoreboard/heap-instr-$b.txt"
}

# ---------------------------------------------------------------------------------------
# quiet-box addenda of the workstreams
do_gcquiet() { # gcquiet <gil-cpus> <ft-cpus> [stages]: the gc tier's run (docs/targets/gc.md §8)
    local gil=$1 ft=$2; shift 2
    local w; w=$(work 3.15.1)
    (cd "$w" && WL_VENVS="$BASE/wl" OUT="$RES/gcq" EXTRA_ROOT="$w/benches/workloads" CORE_GIL="$gil" CORE_FT="$ft" \
        bash benches/runtime/run_quiet.sh "${@:-set extra adv services e2e}")
}
do_ftquiet() { # ftquiet <cpus> [step]: threads scaling 1/2/4/8, both builds, in the services venvs
    local cpus=$1 st=${2:-all} w; w=$(work 3.15.1t)
    mkdir -p "$w/benches/ft/_ext"
    cp "$BIN/ft_ext/"*.py "$w/benches/ft/_ext/"
    (cd "$w" && PY_GIL="$(wl 3.15.1 services)/bin/python" PY_FT="$(wl 3.15.1t services)/bin/python" OUT="$RES/ftq" CORES="$cpus" \
        bash benches/ft/quietbox.sh "$st")
}
do_est() { # est <b> <first-core>: estimator predictions on the 65 (STEPS=est, one core per family)
    local b=$1 core=$2 f root="$BASE/est"
    # never alongside the 445de19 re-run of stage 1 (textfix.sh, same domain): wait for it
    while pgrep -f "textfix.sh [0-9a-f]+ [0-9]" >/dev/null; do sleep 20; done
    mkdir -p "$root/venvs"
    for f in $FAMILIES; do ln -sfn "$(wl "$b" "$f")" "$root/venvs/$f-$b"; done
    (cd "$(work "$b")" && STEPS=est bash benches/dx/est_validate.sh "$root" "$b" "$RES/est/$b" "$core")
    cat "$RES/est/$b"/est-*.log | grep -E "predicted|FAILED" | sort
}

stage_pack() {
    find "$RES" -type f \( -name '*.json' -o -name '*.jsonl' -o -name '*.log' -o -name '*.xml' -o -name '*.txt' \) -size +1M ! -name '*.gz' -exec gzip -9f {} +
    du -sh "$RES"
}

# ---------------------------------------------------------------------------------------
# all: the schedule. Lanes run concurrently, each pinned to its own L3 domain; inside a lane,
# in order. Every step is a fresh "$0 <stage>" process, so a fix deployed by atomic rename
# reaches the steps not yet started. At most one timing job per domain; callgrind and other
# untimed jobs never share a domain with a timing job; builds and tests before any timing.
lane_build() { # lane_build <domain> <build>: build, wheel, status, [workload venvs], test
    local d=$1 b=$2
    if CARGO_BUILD_JOBS=8 taskset -c "$d" "$0" build "$b"; then
        CARGO_BUILD_JOBS=8 taskset -c "$d" "$0" wheel "$b"
        taskset -c "$d" "$0" status "$b"
        case $b in 3.15.1|3.15.1t) taskset -c "$d" "$0" wlvenv "$b" ;; esac
    fi
    taskset -c "$d" "$0" test "$b"
}
stage_waveA() { # stages 1-2: builds, tests, status (T10), Rust checks, conformance
    local D0=${DOMAINS[0]} D1=${DOMAINS[1]} D2=${DOMAINS[2]} D3=${DOMAINS[3]} x
    "$0" setup; "$0" facts; "$0" quiet
    lane_build "$D0" 3.15.1 & lane_build "$D1" 3.15.1t & lane_build "$D2" 3.14.9 & lane_build "$D3" 3.14.9t &
    wait
    for x in "3.15.2 3.15.1" "3.15.2t 3.15.1t" "3.14.10 3.14.9" "3.14.10t 3.14.9t" "3.15.0 3.15.1" \
             "3.14.0 3.14.9" "3.14.1 3.14.9" "3.14.5 3.14.9" "3.14.6 3.14.9" "3.14.8 3.14.9"; do
        # shellcheck disable=SC2086
        taskset -c "$D0,$D1" "$0" statusx $x &
    done
    CARGO_BUILD_JOBS=8 taskset -c "$D2" "$0" status316 3.16.0a1 & CARGO_BUILD_JOBS=8 taskset -c "$D3" "$0" status316 3.16.0a1t &
    wait
    CARGO_BUILD_JOBS=32 "$0" rust
    taskset -c "$D0" "$0" conformance 3.15.1 & taskset -c "$D1" "$0" conformance 3.15.1t &
    taskset -c "$D2" "$0" conformance 3.14.9 & taskset -c "$D3" "$0" conformance 3.14.9t &
    wait
    taskset -c "$D0" "$0" pyperf-venv 3.15.1 & taskset -c "$D1" "$0" pyperf-venv 3.15.1t &
    wait
    log "WAVE-A done"
}
stage_waveB() { # stages 3, 4, 6, 9 (timing) and the callgrind/estimator lane
    local D0=${DOMAINS[0]} D1=${DOMAINS[1]} D2=${DOMAINS[2]} D3=${DOMAINS[3]}
    ( "$0" workloads 3.15.1 "$D1"
      "$0" tiers 3.15.1 "$D1" default,interp "frameworks custom web data devtools libs services threads"
      "$0" tiers 3.15.1 "$D1" default,gc+interp "frameworks custom devtools" ) &
    ( "$0" workloads 3.15.1t "$D2"
      "$0" tiers 3.15.1t "$D2" default,gc ) &
    ( "$0" tiers 3.15.1 "$D3" default,gc
      "$0" whole 3.15.1 "$D3"; "$0" whole 3.15.1t "$D3"
      "$0" startup 3.15.1 "${TIMING_CPU[3]}"; "$0" startup 3.14.9 "${TIMING_CPU[3]}" ) &
    ( "$0" instr 3.15.1 "$D0"; "$0" instr 3.15.1t "$D0"
      "$0" isolate-instr 3.15.1 "$D0"; "$0" isolate-instr 3.15.1t "$D0"
      "$0" est 3.15.1 -8
      taskset -c "$D0" "$0" wlstats-run 3.15.1 ) &
    wait
    log "WAVE-B done"
}
stage_waveC() { # stages 5, 7, 8: pyperformance, quiet-box addenda, scoreboard wall time
    local D0=${DOMAINS[0]} D1=${DOMAINS[1]} D2=${DOMAINS[2]} D3=${DOMAINS[3]}
    "$0" pyperformance 3.15.1 plain "$D0" & "$0" pyperformance 3.15.1 enabled "$D1" &
    "$0" pyperformance 3.15.1t plain "$D2" & "$0" pyperformance 3.15.1t enabled "$D3" &
    wait
    "$0" pyperformance-ports 3.15.1 "$D1"; "$0" pyperformance-ports 3.15.1t "$D1"
    "$0" pyperformance-compare 3.15.1; "$0" pyperformance-compare 3.15.1t
    "$0" gcquiet "$D0" "$D1" & "$0" ftquiet "$D2" & "$0" est 3.15.1t 16 &   # est: cores 24-31 (untimed)
    wait
    local T="${TIMING_CPU[0]},${TIMING_CPU[1]},${TIMING_CPU[2]},${TIMING_CPU[3]}"
    "$0" time 3.15.1 "$T"; "$0" isolate-time 3.15.1 "$T"; "$0" sbprobe 3.15.1 "$T"
    "$0" report 3.15.1; "$0" report 3.15.1t
    "$0" pack
    log "WAVE-C done"
}

main() {
    local stage=${1:?stage}; shift
    # A step can be dropped from a running schedule: touch $BASE/skip/<stage>-<first arg>
    if [[ -e $BASE/skip/$stage-${1:-} ]]; then log "SKIP  $stage ${1:-} ($BASE/skip/$stage-${1:-})"; exit 0; fi
    # Wave C's timing never starts while the 445de19 startup cells run (textfix.sh <c> x <cpu>)
    case $stage in pyperformance|pyperformance-ports|gcquiet|ftquiet|time|isolate-time|sbprobe)
        while pgrep -f "textfix.sh [0-9a-f]+ x [0-9]" >/dev/null; do sleep 10; done
        # nor while the 9d0abae re-run (re9.sh) works in domain 0 (A, untimed) or times in domain 3 (B)
        while pgrep -f "re9.sh [A]" >/dev/null; do sleep 10; done
        [[ $stage == ftquiet ]] || while pgrep -f "re9.sh [B]" >/dev/null; do sleep 10; done
        while pgrep -f "est_d0.s[h]" >/dev/null; do sleep 10; done   # the estimator in domain 0
        while pgrep -f "p78.sh [AB]" >/dev/null; do sleep 10; done ;; esac   # the 78751e2 re-run (domains 1, 3)
    case $stage in
        setup) step setup do_setup ;;
        facts) step facts do_facts ;;
        quiet) step quiet do_quiet ;;
        build) step "build-$1" do_build "$1" ;;
        wheel) step "wheel-$1" do_wheel "$1" ;;
        status) step "status-$1" do_status "$1" ;;
        statusx) step "statusx-$1" do_statusx "$@" ;;
        status316) step "status316-$1" do_status316 "$1" ;;
        test) step "pytest-$1" do_pytest "$1" ;;
        rust) step rust-fmt do_fmt; step rust-clippy do_clippy; step rust-test do_cargo_test ;;
        conformance) step "conformance-$1" do_conformance "$1" ;;
        wlvenv) step "wlvenv-$1${2:+-$2}" do_wlvenv "$@" ;;
        wlcheck) step "wlcheck-$1${2:+-$2}" do_wlcheck "$@" ;;
        workloads) step "workloads-$1${3:-}" do_workloads "$@"
            # T7 right after the 3.15.1t headline, in the same domain
            if [[ $1 == 3.15.1t && -z ${3:-} ]]; then step "t7-$1" do_t7 "$1" "$2"; fi ;;
        t7) step "t7-$1" do_t7 "$@" ;;
        tiers) step "tiers-${3//,/-}-$1" do_tiers "$@" ;;
        wlstats-run) step "wlstats-run-$1" do_wlstats_run "$1" ;;
        whole) step "whole-$1" do_whole "$@" ;;
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
        sbprobe) step "sbprobe-$1" do_sbprobe "$@" ;;
        sbheap) step "sbheap-$1" do_sbheap "$@" ;;
        gcquiet) step gcquiet do_gcquiet "$@" ;;
        ftquiet) step "ftquiet-${2:-all}" do_ftquiet "$@" ;;
        est) step "est-$1" do_est "$@" ;;
        pack) stage_pack ;;
        waveA) stage_waveA ;;
        waveB) stage_waveB ;;
        waveC) stage_waveC ;;
        all) stage_waveA; stage_waveB; stage_waveC ;;
        *) echo "unknown stage $stage" >&2; exit 2 ;;
    esac
}
main "$@"
