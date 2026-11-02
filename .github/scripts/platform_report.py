"""Per-platform coverage table from the platform job's coverage files (wheels.yml job
``platform-report``; docs/targets/dx.md §5). Standard library only.

    python .github/scripts/platform_report.py COVERAGE_DIR FLOORS_JSON [-o report.md] [--json out.json]

Prints a Markdown table (one row per platform and build: targets installed of all, refusals
by category, groups only partly installed) and checks each row against the floor of its
platform family. Exit status 1 when a row is below its floor or a coverage file shows an
unexpected refusal category.
"""

import argparse
import glob
import json
import os
import sys

EXPECTED = ("version", "build", "jit", "platform", "unvalidated-patch")


def family(cov: dict) -> str:
    system = (cov.get("system") or "").lower()
    if system == "windows" or cov.get("platform", "").startswith("win"):
        return "windows"
    if system == "darwin" or cov.get("platform", "").startswith("macosx"):
        return "macos"
    if "musl" in (cov.get("libc") or "") or "musl" in (cov.get("name") or ""):
        return "linux-musl"
    return "linux-glibc"


def minor(py: str) -> str:
    t = "t" if py.endswith("t") else ""
    return ".".join(py.rstrip("t").split(".")[:2]) + t


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("coverage_dir")
    ap.add_argument("floors")
    ap.add_argument("-o", "--output")
    ap.add_argument("--json")
    a = ap.parse_args()
    floors = json.load(open(a.floors))["floors"]
    rows = []
    for path in sorted(glob.glob(os.path.join(a.coverage_dir, "**", "coverage-*.json"), recursive=True)):
        cov = json.load(open(path))
        ts = cov["targets"]
        inst = sum(t["state"] == "installed" for t in ts)
        cats: dict[str, int] = {}
        for t in ts:
            if t["state"] == "refused":
                cats[t.get("refusal") or "other"] = cats.get(t.get("refusal") or "other", 0) + 1
        fam = family(cov)
        floor = floors.get(fam, {}).get(minor(cov["python"]))
        bad = [c for c in cats if c not in EXPECTED]
        ok = (floor is None or inst >= floor) and not bad
        rows.append({"name": cov.get("name") or cov.get("platform"), "family": fam, "python": cov["python"],
                     "installed": inst, "total": len(ts), "refused": cats, "floor": floor,
                     "partial": cov.get("groups_partial", []), "ok": ok, "unexpected": bad})
    rows.sort(key=lambda r: (r["family"], r["name"], r["python"]))
    lines = ["| platform | family | CPython | installed | refused (category: n) | groups partly on | floor | ok |",
             "|---|---|---|---:|---|---|---:|---|"]
    for r in rows:
        ref = ", ".join(f"{k}: {v}" for k, v in sorted(r["refused"].items())) or "-"
        lines.append(f"| {r['name']} | {r['family']} | {r['python']} | {r['installed']} / {r['total']} | {ref} | "
                     f"{', '.join(r['partial']) or '-'} | {r['floor'] if r['floor'] is not None else '-'} | "
                     f"{'yes' if r['ok'] else '**NO**'} |")
    text = "## Rustique platform coverage\n\n" + "\n".join(lines) + "\n"
    if not rows:
        text += "\n(no coverage files found)\n"
    print(text)
    if a.output:
        open(a.output, "w").write(text)
    if a.json:
        json.dump(rows, open(a.json, "w"), indent=1)
    failed = [r for r in rows if not r["ok"]]
    for r in failed:
        why = f"{r['installed']} installed < floor {r['floor']}" if r["floor"] and r["installed"] < r["floor"] else \
            f"unexpected refusal categories {r['unexpected']}"
        print(f"::error::{r['name']} {r['python']}: {why}")
    return 1 if failed or not rows else 0


if __name__ == "__main__":
    sys.exit(main())
