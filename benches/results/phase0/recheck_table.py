"""Table of the 10-process re-measurements (recheck-*.json) next to the 3-process scoreboard
run and the committed matrix. Usage (needs benches/scoreboard.py of the measured commit and
any importable `rustique`, a stub is enough):
    python recheck_table.py --repo <checkout> --results <phase0 results dir> --build 3.15.1
"""
import argparse, glob, importlib.util, os, sys

ap = argparse.ArgumentParser()
ap.add_argument("--repo", required=True)
ap.add_argument("--results", required=True)
ap.add_argument("--build", required=True)
args = ap.parse_args()
spec = importlib.util.spec_from_file_location("scoreboard", os.path.join(args.repo, "benches", "scoreboard.py"))
sb = importlib.util.module_from_spec(spec); sys.modules["scoreboard"] = sb; spec.loader.exec_module(sb)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from analyze_phase0 import parse_matrix, kernel_path  # noqa: E402

R = os.path.join(args.results, "scoreboard")
b = args.build
re_cells, _ = sb.load_results(sorted(glob.glob(f"{R}/recheck-{b}-*.json.shard0.json")))
main_cells, _ = sb.load_results(sorted(glob.glob(f"{R}/time-enabled-{b}.json.shard*.json")))
pub = {k[1:]: v for k, v in parse_matrix(os.path.join(args.repo, "docs", "scoreboard", f"enabled-{b}.md")).items()}
print(f"| cell ({b}) | kernel | published | phase 0, 3 proc (rel. std b/e) | recheck, 10 proc (rel. std b/e) |")
print("|---|---|---:|---:|---:|")
for (build, key), e in sorted(re_cells.items(), key=lambda kv: kv[0][1]):
    if "time" not in e or "enabled" not in e["time"]:
        continue
    t, op, s, n = key.split("/")[0], "/".join(key.split("/")[1:-2]), key.split("/")[-2], int(key.split("/")[-1])
    r = e["time"]["enabled"] / e["time"]["builtin"]
    m = main_cells.get((build, key), {})
    mr = m["time"]["enabled"] / m["time"]["builtin"] if m.get("time") else float("nan")
    p = pub.get((t, op, s, n), (None, "?"))[1]
    nz = e["noise"]; mn = m.get("noise", {})
    label = {10_000: "10k", 1_000_000: "1M"}[n]
    print(f"| `{t}/{op}/{s}/{label}` | {'K' if kernel_path(t, op) else ''} | {p} | {mr:.3f} ({mn.get('builtin', 0):.1%}/{mn.get('enabled', 0):.1%}) | "
          f"{r:.3f} ({nz['builtin']:.1%}/{nz['enabled']:.1%}) |")
