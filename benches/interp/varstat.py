#!/usr/bin/env python3
"""Whole-workload instructions and cycles per run() under eval-loop variants.

    python benches/interp/varstat.py <workload.py> --variant V [--variant V ...] [-r 3]
                                     [--seconds 4] [--taskset 12] [-o out.json]

Variants: plain; copy:MOD (PEP 523 copied loop, rqeval install()); swapall:MOD (every core
opcode handler swapped for MOD's); swap:MOD:OP[,OP] (only those handlers); enable (rustique).
Each (variant, repeat) is a fresh process: setup(), one warm-up run(), then run() in a loop
for --seconds under `perf stat -e instructions:u,cycles:u` gated by a control FIFO, so only
steady-state runs count. Reports per-run medians and the ratio to the first variant.
"""
import argparse, json, os, statistics, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
RQDIR = "/work/w2-interp/rqeval"


def apply_variant(v):
    sys.path.insert(0, os.path.join(HERE, "rqeval"))
    if v == "plain":
        return
    if v == "enable":
        import rustique
        rustique.enable()
        return
    kind, _, rest = v.partition(":")
    if kind == "copy":
        sys.path.insert(0, RQDIR)
        __import__(rest).install()
    elif kind == "swapall":
        import rqpatch
        rqpatch.swap_all(RQDIR, rest)
    elif kind == "swap":
        import rqpatch
        mod, ops = rest.split(":")
        names = []
        for op in ops.split(","):
            if op.endswith("_FAMILY"):
                import _opcode_metadata as om
                fam = op[: -len("_FAMILY")]
                names += [fam] + list(om._specializations[fam])
            else:
                names.append(op)
        rqpatch.swap(RQDIR, mod, names)
    else:
        raise SystemExit("unknown variant " + v)


def driver(a):
    sys.path.insert(0, HERE)
    from opprof import load_workload, _ctl
    apply_variant(a.variant)
    m = load_workload(a.workload)
    st = m.setup()
    ref = m.digest(m.run(st))
    n = 0
    _ctl("enable")
    t0 = time.perf_counter()
    while True:
        res = m.run(st)
        n += 1
        if time.perf_counter() - t0 >= a.seconds and n >= 2:
            break
    _ctl("disable")
    json.dump({"runs": n, "wall": time.perf_counter() - t0, "digest_ok": m.digest(res) == ref},
              open(a.out, "w"))


def one(a, variant):
    tmp = tempfile.mkdtemp(prefix="varstat-")
    ctl, ack = os.path.join(tmp, "ctl"), os.path.join(tmp, "ack")
    os.mkfifo(ctl); os.mkfifo(ack)
    out, st = os.path.join(tmp, "drv.json"), os.path.join(tmp, "stat.csv")
    env = dict(os.environ, RQ_PERF_CTL=ctl, RQ_PERF_ACK=ack, PYTHONHASHSEED="0")
    cmd = ["perf", "stat", "-x,", "-o", st, "-e", "instructions:u,cycles:u", "-D", "-1",
           "--control", f"fifo:{ctl},{ack}", "--", sys.executable, os.path.abspath(__file__),
           "_driver", a.workload, "--variant", variant, "--seconds", str(a.seconds), "--out", out]
    if a.taskset:
        cmd = ["taskset", "-c", a.taskset] + cmd
    subprocess.run(cmd, check=True, env=env, stdout=subprocess.DEVNULL)
    d = json.load(open(out))
    vals = {}
    for line in open(st):
        p = line.strip().split(",")
        if len(p) > 2 and p[0].replace(".", "").isdigit():
            vals[p[2].split(":")[0]] = float(p[0])
    return {"ins": vals["instructions"] / d["runs"], "cyc": vals["cycles"] / d["runs"],
            "wall": d["wall"] / d["runs"], "ok": d["digest_ok"]}


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "_driver":
        ap = argparse.ArgumentParser()
        ap.add_argument("x"); ap.add_argument("workload"); ap.add_argument("--variant")
        ap.add_argument("--seconds", type=float); ap.add_argument("--out")
        driver(ap.parse_args())
        return
    ap = argparse.ArgumentParser()
    ap.add_argument("workload")
    ap.add_argument("--variant", action="append", required=True)
    ap.add_argument("-r", type=int, default=3)
    ap.add_argument("--seconds", type=float, default=4)
    ap.add_argument("--taskset")
    ap.add_argument("-o")
    a = ap.parse_args()
    res = {v: [] for v in a.variant}
    for _ in range(a.r):            # interleave variants
        for v in a.variant:
            res[v].append(one(a, v))
    base = a.variant[0]
    med = {v: {k: statistics.median(r[k] for r in rs) for k in ("ins", "cyc", "wall")} for v, rs in res.items()}
    name = os.path.basename(a.workload)[:-3]
    for v in a.variant:
        b, m = med[base], med[v]
        print(f"{name:20s} {v:28s} Gins/run {m['ins']/1e9:7.3f} ({m['ins']/b['ins']:.3f})  "
              f"Gcyc/run {m['cyc']/1e9:7.3f} ({m['cyc']/b['cyc']:.3f})  ok={all(r['ok'] for r in res[v])}",
              flush=True)
    if a.o:
        json.dump({"workload": name, "raw": res, "median": med}, open(a.o, "w"))


if __name__ == "__main__":
    main()
