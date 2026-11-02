"""Real open-source test suites under Rustique, the way a user would run them (docs/atlas/w2-dx.md §4).

    python benches/dx/real_projects.py setup  --py 3.15.1 --wheels DIR --work DIR [project...]
    python benches/dx/real_projects.py run    --py 3.15.1 --work DIR [--repeat N] [project...]
    python benches/dx/real_projects.py table  --work DIR [--py B ...]

``setup``: for each project, a shallow clone at the pinned tag, a fresh venv of the
python-build-standalone interpreter (``uv venv``), the project (editable) and its test
dependencies, then ``pip install`` of the rustique *wheel* (so the startup ``.pth`` is there,
as for users) and ``python -m rustique status``. ``run``: one warm-up run (compiles the
``.pyc`` files), then ``python -m rustique diff --keep <dir> -- <test command>`` (plain with
``RUSTIQUE=0`` and enabled with ``RUSTIQUE=1``; exit code, stdout/stderr and every test's
outcome compared; the per-target report of the enabled run kept). Results go to
``WORK/results/<py>/<project>/`` and a JSON summary per project; ``table`` prints the markdown
table of the doc. Standard library only (needs ``git`` and ``uv`` on PATH).
"""

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import time

# name: (GitHub repo, pinned tag, test dependencies, test command after `python`)
PROJECTS = {
    "requests": ("psf/requests", "v2.34.2",
                 "pytest pytest-httpbin==2.1.0 httpbin~=0.10.0 pytest-mock trustme requests[socks]",
                 "-m pytest -q tests"),
    "flask": ("pallets/flask", "3.1.3", "pytest<9 asgiref greenlet python-dotenv", "-m pytest -q tests"),
    "attrs": ("python-attrs/attrs", "26.1.0", "pytest hypothesis pympler cloudpickle",
              "-m pytest -q tests -p no:mypy-plugins --ignore=tests/test_mypy.yml"),
    "rich": ("Textualize/rich", "v15.0.0", "pytest attrs", "-m pytest -q tests"),
    "httpx": ("encode/httpx", "0.28.1", "pytest trio trustme uvicorn chardet anyio httpx[brotli,zstd,cli,http2,socks]",
              "-m pytest -q tests"),
    "jinja": ("pallets/jinja", "3.1.6", "pytest trio", "-m pytest -q tests"),
    "click": ("pallets/click", "8.5.0", "pytest", "-m pytest -q tests"),
    "marshmallow": ("marshmallow-code/marshmallow", "4.3.1", "pytest simplejson", "-m pytest -q tests"),
    "sqlglot": ("tobymao/sqlglot", "v30.28.0", "pytest pytz duckdb pandas",
                "-m pytest -q tests --ignore=tests/test_sqlglotc.py"),
    "black": ("psf/black", "26.10.1", "pytest<9", "-m pytest -q tests"),
    "pydantic": ("pydantic/pydantic", "v2.14.2",
                 "pytest pytz dirty-equals pytest-mock faker jsonschema time-machine packaging pytest-benchmark "
                 "cloudpickle pytest-run-parallel", "-m pytest -q tests --ignore=tests/benchmarks --ignore=tests/test_docs.py"),
    "django": ("django/django", "6.1.2", "asgiref sqlparse",
               "tests/runtests.py --parallel 1 -v 1 basic utils_tests template_tests forms_tests serializers "
               "model_fields queries i18n timezones humanize_tests logging_tests ordering view_tests"),
    "arrow": ("arrow-py/arrow", "1.4.0", "pytest pytest-cov pytest-mock pytz simplejson dateparser python-dateutil",
              "-m pytest -q tests"),
    "dateutil": ("dateutil/dateutil", "4.0.0", "pytest pytest-cov freezegun hypothesis attrs", "-m pytest -q tests"),
}


def sh(cmd, log=None, cwd=None, env=None, check=False):
    t0 = time.perf_counter()
    with open(log, "a") if log else open(os.devnull, "w") as f:
        if log:
            f.write(f"$ {cmd if isinstance(cmd, str) else shlex.join(cmd)}\n")
            f.flush()
        p = subprocess.run(cmd, shell=isinstance(cmd, str), cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT)
    if check and p.returncode:
        raise SystemExit(f"failed ({p.returncode}): {cmd}")
    return p.returncode, time.perf_counter() - t0


def interp(py: str) -> str:
    req = f"cpython-{py[:-1]}+freethreaded-linux-x86_64-gnu" if py.endswith("t") else f"cpython-{py}-linux-x86_64-gnu"
    return subprocess.check_output(["uv", "python", "find", req], text=True).strip()


def paths(work, py, name):
    res = os.path.join(work, "results", py, name)
    return os.path.join(work, "src", name), os.path.join(work, f"venv-{py}", name), res


def setup(args):
    exe = interp(args.py)
    for name in args.projects or PROJECTS:
        repo, tag, deps, _ = PROJECTS[name]
        src, venv, res = paths(args.work, args.py, name)
        os.makedirs(res, exist_ok=True)
        log = os.path.join(res, "install.log")
        if os.path.exists(log):
            os.remove(log)
        if not os.path.isdir(src):
            sh(["git", "clone", "-q", "--depth", "1", "--branch", tag, f"https://github.com/{repo}", src], log, check=True)
        commit = subprocess.check_output(["git", "-C", src, "log", "-1", "--format=%H %cs"], text=True).strip()
        sh(["uv", "venv", "-q", "--clear", "--python", exe, venv], log, check=True)
        vpy = os.path.join(venv, "bin", "python")
        rc, _ = sh(["uv", "pip", "install", "-q", "--python", vpy, "-e", src, *shlex.split(deps)], log)
        if rc:  # one failing optional dependency should not hide the rest: install them one by one
            for d in shlex.split(deps):
                sh(["uv", "pip", "install", "-q", "--python", vpy, d], log)
            sh(["uv", "pip", "install", "-q", "--python", vpy, "-e", src], log)
        wheel = [w for w in os.listdir(args.wheels) if w.endswith(".whl") and f"-cp{args.py[0]}{args.py[2:4]}-cp" in w
                 and (w.split("-")[3].endswith("t") == args.py.endswith("t"))]
        sh(["uv", "pip", "install", "-q", "--python", vpy, "--reinstall", "--no-deps",
            os.path.join(args.wheels, wheel[0])], log, check=True)
        sh([vpy, "-m", "rustique", "status"], os.path.join(res, "status.txt"))
        freeze = subprocess.run(["uv", "pip", "freeze", "--python", vpy], capture_output=True, text=True).stdout
        with open(os.path.join(res, "freeze.txt"), "w") as f:
            f.write(freeze)
        json.dump({"project": name, "tag": tag, "commit": commit, "wheel": wheel[0], "python": args.py},
                  open(os.path.join(res, "meta.json"), "w"))
        print(f"{name}: installed ({commit})", flush=True)


def summarize(res: str, diff_out: str, rc: int, seconds: float) -> dict:
    s = {"diff_rc": rc, "diff_seconds": round(seconds, 1)}
    m = re.search(r"^  (identical|DIFFERENT)[: ].*?$", diff_out, re.M)
    s["verdict"] = m.group(0).strip() if m else diff_out.strip().splitlines()[-1:]
    m = re.search(r"time: plain ([\d.]+) s, rustique ([\d.]+) s, ratio ([\d.]+)", diff_out)
    if m:
        s["plain_s"], s["rustique_s"], s["ratio"] = map(float, m.groups())
    m = re.search(r"(\d+) test outcomes", diff_out)
    s["tests"] = int(m.group(1)) if m else None
    rep = os.path.join(res, "diff-runs", "report.json")
    if os.path.exists(rep):
        recs = [json.loads(ln) for ln in open(rep) if ln.strip()]
        s["processes"] = len(recs)
        hits, fb = {}, {}
        for r in recs:
            for k, c in (r.get("stats") or {}).items():
                hits[k] = hits.get(k, 0) + c.get("hits", 0)
                for reason, n in (c.get("fallback") or {}).items():
                    fb.setdefault(k, {}).setdefault(reason, 0)
                    fb[k][reason] += n
        s["hits_total"] = sum(hits.values())
        s["fallbacks_total"] = sum(sum(v.values()) for v in fb.values())
        reasons = {}
        for v in fb.values():
            for reason, n in v.items():
                reasons[reason] = reasons.get(reason, 0) + n
        s["fallback_reasons"] = reasons
        s["top"] = sorted(((k, hits.get(k, 0), fb.get(k, {})) for k in set(hits) | set(fb)),
                          key=lambda x: -(x[1] + sum(x[2].values())))[:8]
        st = recs[0]["status"]["targets"] if recs else []
        s["states"] = {state: sum(t["state"] == state for t in st) for state in ("installed", "pending-import", "refused")}
    return s


def run(args):
    for name in args.projects or PROJECTS:
        _, _, _, cmd = PROJECTS[name]
        src, venv, res = paths(args.work, args.py, name)
        vpy = os.path.join(venv, "bin", "python")
        if not os.path.exists(vpy):
            print(f"{name}: not set up", flush=True)
            continue
        test = [vpy, *shlex.split(cmd)]
        env = {k: v for k, v in os.environ.items() if not k.startswith("RUSTIQUE") and k != "VIRTUAL_ENV"}
        env["PATH"] = os.path.join(venv, "bin") + os.pathsep + env["PATH"]
        rc_w, t_w = sh(test, os.path.join(res, "warmup.log"), cwd=src, env=env)
        keep = os.path.join(res, "diff-runs")
        diff = [vpy, "-m", "rustique", "diff", "--keep", keep, "--repeat", str(args.repeat), "--", *test]
        t0 = time.perf_counter()
        p = subprocess.run(diff, cwd=src, env=env, capture_output=True, text=True)
        seconds = time.perf_counter() - t0
        with open(os.path.join(res, "diff.txt"), "w") as f:
            f.write(p.stdout + p.stderr)
        s = summarize(res, p.stdout, p.returncode, seconds)
        s.update(json.load(open(os.path.join(res, "meta.json"))), warmup_rc=rc_w, warmup_s=round(t_w, 1))
        json.dump(s, open(os.path.join(res, "summary.json"), "w"), indent=1)
        print(f"{name}: diff rc {p.returncode} {s.get('verdict')} ratio {s.get('ratio')} ({seconds:.0f} s)", flush=True)


def table(args):
    rows = []
    for py in args.py_list or sorted(os.listdir(os.path.join(args.work, "results"))):
        for name in PROJECTS:
            f = os.path.join(args.work, "results", py, name, "summary.json")
            if not os.path.exists(f):
                continue
            s = json.load(open(f))
            top = ", ".join(f"{k} {h:,}" + (f" (fb {sum(fb.values()):,})" if fb else "") for k, h, fb in s.get("top", [])[:3])
            rows.append(f"| {name} {s['tag']} | {py} | {s.get('tests') or '-'} | {s.get('plain_s', '-')} | "
                        f"{s.get('rustique_s', '-')} | {s.get('ratio', '-')} | {s.get('verdict')} | "
                        f"{s.get('processes', '-')} | {s.get('hits_total', 0):,} / {s.get('fallbacks_total', 0):,} "
                        f"{s.get('fallback_reasons', {})} | {top} |")
    print("| project | py | tests | plain s | rustique s | ratio | diff | procs | hits / fallbacks (reasons) | top targets |")
    print("|---|---|---:|---:|---:|---:|---|---:|---|---|")
    print("\n".join(rows))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("cmd", choices=("setup", "run", "table"))
    p.add_argument("--py", default="3.15.1")
    p.add_argument("--py-list", nargs="*")
    p.add_argument("--wheels")
    p.add_argument("--work", required=True)
    p.add_argument("--repeat", type=int, default=1)
    p.add_argument("projects", nargs="*")
    args = p.parse_intermixed_args()
    {"setup": setup, "run": run, "table": table}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
