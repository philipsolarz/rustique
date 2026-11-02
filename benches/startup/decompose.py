"""Decompose the whole-process cost of `RUSTIQUE=1 python -c pass` (docs/atlas/w2-startup.md §1).

    python benches/startup/decompose.py PLAIN_VENV RUSTIQUE_VENV [--perf-runs 300] [-o out.json]

PLAIN_VENV is a fresh venv without Rustique, RUSTIQUE_VENV the same with the wheel
installed (`pip install` of a release wheel: the `.pth` startup line is only in wheels).
Variant venvs are made next to RUSTIQUE_VENV (`<venv>.var-<name>`) that link its `rustique`
package and carry a modified startup line:

    unset   shipped line, RUSTIQUE unset             (installed but off)
    off     shipped line, RUSTIQUE=off               (gate + the exec'd loader string compiled)
    load    line without `.startup()`, RUSTIQUE=1    (extension loaded, nothing enabled)
    on      shipped line, RUSTIQUE=1

Measured per variant: callgrind total Ir of the whole process (PYTHONHASHSEED=0), and with
`perf stat -r N`: user/kernel instructions and cycles, page faults, task-clock and elapsed
time. On `on`, callgrind `--toggle-collect` gives the inclusive Ir of: `_imp_create_dynamic`
(dlopen, relocations, module object), `_imp_exec_dynamic` (module exec slot), `startup()`
(the quiet enable) and the exit hook. The table attributes the RUSTIQUE=1 − unset delta:

    loader string     off − unset
    locate + _load    load − off − create − exec   (importlib's spec/_load, os.path.isfile)
    create / exec     toggles
    enable            toggle on startup()
    exit hook         toggle
    rest              on − load − enable − exit hook (GC traversal and teardown of what the
                      extension and the targets keep alive, sys.modules watcher events)
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import subprocess

TOGGLES = {
    "create (dlopen + relocations + module object)": "_imp_create_dynamic",
    "exec (module exec slot)": "_imp_exec_dynamic",
    "enable (_accel.startup())": "*__pyfunction_startup",
    "exit hook": "*__pyfunction_exit_hook",
}


def site_packages(py: str) -> str:
    return subprocess.check_output([py, "-c", "import sysconfig; print(sysconfig.get_path('purelib'))"],
                                   text=True).strip()


def env_for(extra: dict[str, str]) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.startswith(("RUSTIQUE", "PYTHON", "VIRTUAL_ENV"))}
    env["PYTHONHASHSEED"] = "0"
    env.update(extra)
    return env


def variant(rq_venv: str, name: str, line: str) -> str:
    """A venv linking RUSTIQUE_VENV's rustique package, with `line` as the startup .pth."""
    path = f"{rq_venv.rstrip('/')}.var-{name}"
    base = os.path.realpath(os.path.join(rq_venv, "bin", "python"))
    py = os.path.join(path, "bin", "python")
    if not os.path.exists(py):
        home = open(os.path.join(rq_venv, "pyvenv.cfg")).read()
        exe = re.search(r"^executable\s*=\s*(.+)$", home, re.M)
        subprocess.check_call([exe.group(1).strip() if exe else base, "-m", "venv", "--without-pip", path])
    src, dst = site_packages(os.path.join(rq_venv, "bin", "python")), site_packages(py)
    for entry in os.listdir(src):
        if entry.startswith("rustique") and not os.path.exists(os.path.join(dst, entry)):
            os.symlink(os.path.join(src, entry), os.path.join(dst, entry))
    with open(os.path.join(dst, "_rustique_startup.pth"), "w") as f:
        f.write(line)
    subprocess.check_call([py, "-c", "import rustique"], env=env_for({}))  # warm .pyc
    return py


def callgrind(py: str, env: dict[str, str], toggle: str | None = None, code: str = "pass") -> int:
    cmd = ["valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null"]
    if toggle:
        cmd.append(f"--toggle-collect={toggle}")
    out = subprocess.run(cmd + [py, "-c", code], env=env, capture_output=True, text=True)
    m = re.search(r"Collected : (\d+)", out.stderr)
    if out.returncode or not m:
        raise SystemExit(f"{py}: callgrind failed:\n{out.stderr[-2000:]}")
    return int(m.group(1))


PERF_EVENTS = "instructions:u,instructions:k,cycles:u,cycles:k,page-faults,task-clock"


def perf(py: str, env: dict[str, str], runs: int, code: str = "pass") -> dict[str, float]:
    out = subprocess.run(["perf", "stat", "-x", ";", "-r", str(runs), "-e", PERF_EVENTS, py, "-c", code],
                         env=env, capture_output=True, text=True)
    res: dict[str, float] = {}
    for line in out.stderr.splitlines():
        parts = line.split(";")
        if len(parts) > 3 and parts[2] in PERF_EVENTS.split(","):
            try:
                res[parts[2]] = float(parts[0])
            except ValueError:
                pass
    return res


def wall(variants: dict[str, tuple[str, dict[str, str]]], rounds: int, code: str = "pass") -> dict[str, float]:
    import time
    times: dict[str, list[float]] = {k: [] for k in variants}
    for _ in range(rounds):
        for k, (py, env) in variants.items():
            t0 = time.perf_counter_ns()
            pid = os.posix_spawn(py, [py, "-c", code], env)
            os.waitpid(pid, 0)
            times[k].append((time.perf_counter_ns() - t0) / 1e6)
    return {k: statistics.median(v) for k, v in times.items()}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("plain")
    ap.add_argument("rustique")
    ap.add_argument("--perf-runs", type=int, default=300)
    ap.add_argument("--wall-rounds", type=int, default=300)
    ap.add_argument("-o", "--output")
    args = ap.parse_args()
    plain = os.path.join(args.plain, "bin", "python")
    rq = os.path.join(args.rustique, "bin", "python")
    shipped = open(os.path.join(site_packages(rq), "_rustique_startup.pth")).read()
    assert shipped.rstrip().endswith('.startup()")'), shipped
    # the same line without the final `.startup()` call (wave 1: `._accel.startup()`)
    load_line = re.sub(r'(\._accel)?\.startup\(\)"\)\s*$', '")\n', shipped)
    load_py = variant(args.rustique, "load", load_line)
    e = env_for({})
    cells = {
        "plain": (plain, e),
        "unset": (rq, e),
        "off": (rq, env_for({"RUSTIQUE": "off"})),
        "load": (load_py, env_for({"RUSTIQUE": "1"})),
        "on": (rq, env_for({"RUSTIQUE": "1"})),
    }
    ir = {k: callgrind(py, env) for k, (py, env) in cells.items()}
    tog = {k: callgrind(rq, cells["on"][1], t) for k, t in TOGGLES.items()}
    pf = {k: perf(py, env, args.perf_runs) for k, (py, env) in cells.items()}
    wl = wall(cells, args.wall_rounds)
    create, execm = tog["create (dlopen + relocations + module object)"], tog["exec (module exec slot)"]
    enable, exit_hook = tog["enable (_accel.startup())"], tog["exit hook"]
    parts = [
        ("`.pth` with RUSTIQUE unset (vs plain venv)", ir["unset"] - ir["plain"]),
        ("loader string compile + env test (off − unset)", ir["off"] - ir["unset"]),
        ("locate + importlib `_load` (load − off − create − exec)", ir["load"] - ir["off"] - create - execm),
        ("create: dlopen + relocations + module object", create),
        ("exec: module exec slot (`_accel` registration)", execm),
        ("enable: `_accel.startup()`", enable),
        ("exit hook", exit_hook),
        ("rest: GC/teardown/watcher (on − load − enable − exit)", ir["on"] - ir["load"] - enable - exit_hook),
    ]
    delta = ir["on"] - ir["unset"]
    print(f"{'part':<58} {'Ir':>11} {'% of on−unset':>14}")
    for name, v in parts:
        share = f"{100 * v / delta:.1f}%" if not name.startswith("`.pth`") else ""
        print(f"{name:<58} {v:>11,} {share:>14}")
    print(f"{'RUSTIQUE=1 − unset':<58} {delta:>11,}   ({ir['on'] / ir['unset']:.4f}x)")
    print(f"\n{'cell':<8} {'Ir':>12} {'instr:u':>12} {'instr:k':>11} {'cyc:u':>12} {'cyc:k':>11} "
          f"{'faults':>7} {'task ms':>8} {'wall ms':>8}")
    for k in cells:
        p = pf[k]
        print(f"{k:<8} {ir[k]:>12,} {p.get('instructions:u', 0):>12,.0f} {p.get('instructions:k', 0):>11,.0f} "
              f"{p.get('cycles:u', 0):>12,.0f} {p.get('cycles:k', 0):>11,.0f} {p.get('page-faults', 0):>7.0f} "
              f"{p.get('task-clock', 0):>8.3f} {wl[k]:>8.3f}")
    for a, b in (("on", "unset"), ("unset", "plain"), ("on", "plain")):
        print(f"{a} / {b}: Ir {ir[a] / ir[b]:.4f}, wall {wl[a] / wl[b]:.4f} (+{wl[a] - wl[b]:.3f} ms), "
              f"cycles {(pf[a]['cycles:u'] + pf[a]['cycles:k']) / (pf[b]['cycles:u'] + pf[b]['cycles:k']):.4f}")
    if args.output:
        with open(args.output, "w") as f:
            json.dump({"ir": ir, "toggles": tog, "perf": pf, "wall_ms": wl, "parts": parts}, f, indent=1)


if __name__ == "__main__":
    main()
