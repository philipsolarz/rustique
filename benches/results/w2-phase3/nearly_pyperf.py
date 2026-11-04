"""Nearly sorted lists (n/50 random swaps, core's _wall_big_sort.py shapes) under pyperf: the one
large shape near the bar there, as a second method.
  python nearly_pyperf.py --mode plain|on -p 10 -n 5 --affinity CPU -o out.json"""
import random
import pyperf

def make(kind, n):
    base = list(range(n)) if kind == "int" else [i / 7 for i in range(n)] if kind == "float" else [f"k{i:08d}" for i in range(n)]
    r = random.Random(7)
    for _ in range(max(1, n // 50)):
        i, j = r.randrange(n), r.randrange(n)
        base[i], base[j] = base[j], base[i]
    return base

runner = pyperf.Runner(add_cmdline_args=lambda cmd, args: cmd.extend(["--mode", args.mode]))
runner.argparser.add_argument("--mode", default="plain", choices=("plain", "on"))
args = runner.parse_args()
if args.mode == "on":
    import rustique
    rustique.enable()
for kind, n in (("str", 10**5), ("str", 10**6), ("int", 10**6), ("float", 10**6)):
    x = make(kind, n)
    runner.bench_func(f"sorted(list {kind} nearly {n})", sorted, x)
