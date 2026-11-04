#!/usr/bin/env python3
"""Tables for docs/atlas/w2-attribution.md from layer profiles (benches/atlas/layers.py).

    python benches/atlas/report.py PROFILES... --md out.md --json out.json [--title T]
    python benches/atlas/report.py ftdiff --gil P1... --ft P2... --md out.md

Every number is a mean over workloads of per-workload shares of steady-state CPU cycles
(bpf mode unless the steady state runs in child processes), unless a column says otherwise.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _taxonomy import FAMILY_GROUPS, Opcodes  # noqa: E402
from layers import (KEY_FIELDS, LAYERS, fixup_counter, headline, load_profile,  # noqa: E402
                    rows_to_counter, summarize)

F = {f: i for i, f in enumerate(KEY_FIELDS)}
GROUP_ORDER = [g for g, _ in FAMILY_GROUPS] + ["other"]


def pct(x, d=1):
    return f"{100 * x:.{d}f}"


def mean(xs):
    xs = list(xs)
    return sum(xs) / len(xs) if xs else 0.0


class Prof:
    def __init__(self, path):
        self.raw = load_profile(path)
        self.name = self.raw["workload"]
        self.family = self.name.split("/")[0]
        self.mode, sec = headline(self.raw)
        self.cnt = fixup_counter(rows_to_counter(sec["counter"]))
        self.tot = sum(self.cnt.values()) or 1
        self.sec = dict(sec, summary=summarize(self.cnt))
        self.ops = None

    def share(self, pred):
        return sum(v for k, v in self.cnt.items() if pred(k)) / self.tot

    def by(self, keyf, pred=None):
        c = Counter()
        for k, v in self.cnt.items():
            if pred is None or pred(k):
                kk = keyf(k)
                if kk is not None:
                    c[kk] += v / self.tot
        return c


def _opcodes(profs):
    spec = {}
    for p in profs:
        # opcode metadata is not stored in the profile; derive families from names
        pass
    import opcode as _op  # the analysing interpreter's metadata (same 3.15 opcode set)

    return Opcodes(getattr(_op, "_specializations", {}))


def mean_counters(counters, n):
    out = Counter()
    for c in counters:
        for k, v in c.items():
            out[k] += v / n
    return out


def max_of(profs, f):
    best = {}
    for p in profs:
        for k, v in f(p).items():
            if k not in best or v > best[k][0]:
                best[k] = (v, p.name)
    return best


# ---------------------------------------------------------------------------------------


def section_layers(profs, md, js):
    fams = defaultdict(list)
    for p in profs:
        fams[p.family].append(p)
    md.append("| workload | own | gen. | lang. | stdlib | 3rd-party | runtime | other | "
              "largest parts (stdlib / third-party / runtime / generated) |")
    md.append("|---|---:|---:|---:|---:|---:|---:|---:|---|")
    for fam in sorted(fams):
        for p in sorted(fams[fam], key=lambda p: p.name):
            L = p.sec["summary"]["layers"]
            S = p.sec["summary"]["sublayers"]
            parts = []
            for lay, n in (("stdlib", 2), ("thirdparty", 2), ("runtime", 2), ("generated", 1)):
                best = sorted(((v, k) for k, v in S.items() if k.startswith(lay + ":")),
                              reverse=True)[:n]
                parts += [f"{k.split(':', 1)[1] or lay} {pct(v)}" for v, k in best if v >= 0.02]
            tag = " (x)" if p.mode == "xperf" else ""
            md.append(f"| {p.name}{tag} | " + " | ".join(pct(L[l]) for l in LAYERS) + " | "
                      + ", ".join(parts) + " |")
            js.setdefault("workloads", {})[p.name] = {"mode": p.mode, "layers": L,
                                                     "sublayers": dict(list(S.items())[:30])}
    md.append("")
    md.append("| family | n | own | gen. | lang. | stdlib | 3rd-party | runtime | other |")
    md.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for fam, ps in [(f, fams[f]) for f in sorted(fams)] + [("**all**", profs)]:
        m = {l: mean(p.sec["summary"]["layers"][l] for p in ps) for l in LAYERS}
        js.setdefault("families", {})[fam.strip("*")] = m
        md.append(f"| {fam} | {len(ps)} | " + " | ".join(pct(m[l]) for l in LAYERS) + " |")
    md.append("")


def section_sublayers(profs, md, js, layer, title, top=30):
    n = len(profs)
    m = mean_counters([Counter({k.split(":", 1)[1]: v
                                for k, v in p.sec["summary"]["sublayers"].items()
                                if k.startswith(layer + ":")}) for p in profs], n)
    mx = max_of(profs, lambda p: {k.split(":", 1)[1]: v
                                  for k, v in p.sec["summary"]["sublayers"].items()
                                  if k.startswith(layer + ":")})
    used = Counter()
    for p in profs:
        for k, v in p.sec["summary"]["sublayers"].items():
            if k.startswith(layer + ":") and v >= 0.01:
                used[k.split(":", 1)[1]] += 1
    md.append(f"| {title} | mean | max (workload) | workloads ≥ 1% |")
    md.append("|---|---:|---|---:|")
    for k, v in m.most_common(top):
        md.append(f"| {k or layer} | {pct(v, 2)} | {pct(mx[k][0])} ({mx[k][1]}) | {used[k]} |")
    md.append("")
    js.setdefault("sublayers", {})[layer] = {k: [round(v, 5), round(mx[k][0], 4), mx[k][1],
                                                 used[k]] for k, v in m.most_common()}


def section_opcodes(profs, md, js, ops):
    n = len(profs)
    owners = ("own", "thirdparty", "stdlib", "generated")
    # A: family group x owner, inline vs callee (% of all time)
    acc = Counter()
    spec = Counter()
    state = Counter()
    per_op = Counter()
    per_op_inline = Counter()
    op_callee = defaultdict(Counter)
    entries = Counter()
    for p in profs:
        for k, v in p.cnt.items():
            op = k[F["op"]]
            g = k[F["group"]]
            if not op or g not in owners:
                continue
            if k[F["layer"]] == "runtime" and k[F["sub"]] == "gc":
                continue  # GC that ran inside the handler's eval-breaker check
            w = v / p.tot / n
            grp = ops.group(op) if op != "(eval)" else "frame/return"
            inline = k[F["where"]] == "inline"
            acc[(grp, g, inline)] += w
            base, is_spec = ops.family(op)
            if op != "(eval)":
                spec[(grp, "specialized" if is_spec else "generic")] += w
                st = k[F["state"]]
                if not is_spec:
                    state[(grp, st or "?")] += w
            per_op[op] += w
            if inline:
                per_op_inline[op] += w
            else:
                op_callee[op][k[F["layer"]]] += w
                if not is_spec and op != "(eval)":
                    entries[(op, k[F["entry"]] or k[F["leaf"]], k[F["layer"]] + ":" +
                             k[F["sub"]])] += w
    md.append("Interpreter time by opcode family (mean % of all steady-state cycles). "
              "*inline* = the handler itself and the eval loop (frame push/pop); *callee* = "
              "native code the handler called (attribute lookup, calls into C, allocation...). "
              "Cyclic GC run from a handler's eval-breaker check is excluded.")
    md.append("")
    md.append("| family | own inline | own callee | 3rd-party inline | 3rd-party callee | "
              "stdlib inline | stdlib callee | generated | all | specialized | generic |")
    md.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    tot_row = Counter()
    for grp in GROUP_ORDER:
        row = [acc[(grp, "own", True)], acc[(grp, "own", False)], acc[(grp, "thirdparty", True)],
               acc[(grp, "thirdparty", False)], acc[(grp, "stdlib", True)],
               acc[(grp, "stdlib", False)],
               acc[(grp, "generated", True)] + acc[(grp, "generated", False)]]
        allv = sum(row)
        if allv < 0.0005:
            continue
        for i, x in enumerate(row):
            tot_row[i] += x
        tot_row["all"] += allv
        tot_row["spec"] += spec[(grp, "specialized")]
        tot_row["gen"] += spec[(grp, "generic")]
        md.append(f"| {grp} | " + " | ".join(pct(x) for x in row) + f" | {pct(allv)} | "
                  f"{pct(spec[(grp, 'specialized')])} | {pct(spec[(grp, 'generic')])} |")
    md.append("| **total** | " + " | ".join(pct(tot_row[i]) for i in range(7)) +
              f" | {pct(tot_row['all'])} | {pct(tot_row['spec'])} | {pct(tot_row['gen'])} |")
    md.append("")
    js["opcode_groups"] = {f"{a}|{b}|{'inline' if c else 'callee'}": round(v, 5)
                           for (a, b, c), v in acc.items()}
    # B: generic instruction state
    md.append("Generic (unspecialized base) handlers, by the state of the executing "
              "instruction at the end of the run (from its adaptive bytecode): "
              "*unspecialized* = the instruction never specialized or backed off; *deopt* = the "
              "instruction is specialized but this execution took the generic path; "
              "*?* = instruction not identified. Mean % of all cycles.")
    md.append("")
    md.append("| family | unspecialized | deopt | ? |")
    md.append("|---|---:|---:|---:|")
    for grp in GROUP_ORDER:
        r = [state[(grp, s)] for s in ("unspecialized", "deopt", "?")]
        if sum(r) < 0.0005:
            continue
        md.append(f"| {grp} | " + " | ".join(pct(x, 2) for x in r) + " |")
    md.append("")
    js["generic_state"] = {f"{a}|{b}": round(v, 5) for (a, b), v in state.items()}
    # C: top opcodes
    md.append("| opcode | mean % | inline | callee: language / runtime / stdlib / 3rd-party |")
    md.append("|---|---:|---:|---|")
    for op, v in per_op.most_common(30):
        c = op_callee[op]
        md.append(f"| {op} | {pct(v, 2)} | {pct(per_op_inline[op], 2)} | "
                  f"{pct(c['language'], 2)} / {pct(c['runtime'], 2)} / {pct(c['stdlib'], 2)} / "
                  f"{pct(c['thirdparty'], 2)} |")
    md.append("")
    js["top_ops"] = {op: round(v, 5) for op, v in per_op.most_common(80)}
    # D: generic paths -> C entry points (what an entry-point mechanism could reach)
    md.append("Generic handlers' C entry points (the native function the handler called, "
              "callee time only; mean % of all cycles): these are the paths that end in a type "
              "slot or C entry point.")
    md.append("")
    md.append("| opcode | entry point | callee layer | mean % |")
    md.append("|---|---|---|---:|")
    for (op, e, lay), v in entries.most_common(30):
        md.append(f"| {op} | `{e}` | {lay} | {pct(v, 2)} |")
    md.append("")
    js["generic_entries"] = [[op, e, lay, round(v, 5)] for (op, e, lay), v in
                             entries.most_common(100)]


def section_language(profs, md, js):
    n = len(profs)
    cat = Counter()
    cat_owner = Counter()
    ent = Counter()
    for p in profs:
        for k, v in p.cnt.items():
            if k[F["layer"]] != "language":
                continue
            w = v / p.tot / n
            cat[k[F["sub"]]] += w
            cat_owner[(k[F["sub"]], k[F["group"]])] += w
            ent[(k[F["sub"]], k[F["entry"]] or k[F["leaf"]], k[F["op"]])] += w
    md.append("| category | mean % | from own code | from third-party code |")
    md.append("|---|---:|---:|---:|")
    for c, v in cat.most_common(20):
        md.append(f"| {c} | {pct(v, 2)} | {pct(cat_owner[(c, 'own')], 2)} | "
                  f"{pct(cat_owner[(c, 'thirdparty')], 2)} |")
    md.append("")
    md.append("| category | C entry point (called by the handler) | opcode | mean % |")
    md.append("|---|---|---|---:|")
    for (c, e, op), v in ent.most_common(30):
        md.append(f"| {c} | `{e}` | {op} | {pct(v, 2)} |")
    md.append("")
    js["language"] = {c: round(v, 5) for c, v in cat.most_common()}
    js["language_entries"] = [[c, e, op, round(v, 5)] for (c, e, op), v in ent.most_common(80)]


def section_stdlib_functions(profs, md, js):
    n = len(profs)
    code = Counter()
    for p in profs:
        for k, v in p.cnt.items():
            if k[F["layer"]] in ("stdlib", "generated") and k[F["code"]] and \
                    k[F["code"]] != "(imputed)":
                code[(k[F["layer"]] + ":" + k[F["sub"]], k[F["code"]])] += v / p.tot / n
    md.append("| module | function (innermost frame, self + native callees) | mean % |")
    md.append("|---|---|---:|")
    for (m, c), v in code.most_common(30):
        md.append(f"| {m} | `{c}` | {pct(v, 2)} |")
    md.append("")
    js["stdlib_functions"] = [[m, c, round(v, 5)] for (m, c), v in code.most_common(100)]


def section_thirdparty(profs, md, js):
    n = len(profs)
    users = [p for p in profs if any(k[F["group"]] == "thirdparty" for k in p.cnt)]
    excl = Counter()
    incl = Counter()
    incl_layer = defaultdict(Counter)
    for p in profs:
        for k, v in p.cnt.items():
            w = v / p.tot
            if k[F["layer"]] == "thirdparty":
                excl[k[F["sub"]]] += w / n
            if k[F["group"]] == "thirdparty":
                incl[k[F["owner"]]] += w / n
                incl_layer[k[F["owner"]]][k[F["layer"]]] += w / n
    md.append(f"Third-party code (n = {len(users)} workloads use it). *exclusive* = its "
              "bytecode + its native extensions (the thirdparty layer); *inclusive* = all time "
              "with a frame of it innermost, split by where that time goes.")
    md.append("")
    md.append("| distribution | exclusive | inclusive | of which language | stdlib | runtime |")
    md.append("|---|---:|---:|---:|---:|---:|")
    for d, v in incl.most_common(25):
        L = incl_layer[d]
        md.append(f"| {d} | {pct(excl[d], 2)} | {pct(v, 2)} | {pct(L['language'], 2)} | "
                  f"{pct(L['stdlib'], 2)} | {pct(L['runtime'], 2)} |")
    md.append("")
    js["thirdparty"] = {d: {"exclusive": round(excl[d], 5), "inclusive": round(v, 5),
                            "by_layer": {a: round(b, 5) for a, b in incl_layer[d].items()}}
                        for d, v in incl.most_common()}


def section_entries(profs, md, js, caller_kind):
    """CPython-provided entry points that own / third-party code calls (exact Python stacks
    from the bpf mode): callee = the stdlib/generated function entered from the innermost
    own/third-party frame, or the C entry point its bytecode called."""
    n = len(profs)
    agg = Counter()
    kinds = Counter()
    for p in profs:
        ec = p.sec.get("entry_counter")
        if not ec:
            continue
        for caller, callee, v in ec:
            if caller[0] != caller_kind:
                continue
            kind, name, cl, cs, op = callee
            w = v / p.tot / n
            if kind == "bytecode":
                kinds[f"{caller_kind} bytecode (handlers inline)"] += w
                continue
            if cl == "runtime" and cs == "gc":
                kinds["cyclic GC (run from an eval-breaker check)"] += w
                continue
            if kind == "python":
                if cl in ("other", "unresolved") and (not name or name == "__init__:__init__"
                                                      or not name.split(":")[0].isprintable()
                                                      or name.startswith(":")):
                    kinds["Python frame in transition (unresolved)"] += w
                    continue
                if name.startswith("<cattrs generated"):
                    cl, name = "generated", "cattrs:" + name.split(":", 1)[-1]
                lab = f"{cl}:{name}"
                kinds[f"Python function: {cl}"] += w
            else:
                if name in ("?", "") and cl in ("language", "other"):
                    kinds["native, no stack or no symbol"] += w
                    continue
                lab = f"{cl}:{cs} ← {op} `{name}`"
                kinds[f"native: {cl}"] += w
            agg[lab] += w
    who = "own" if caller_kind == "own" else "third-party"
    md.append(f"Time under CPython-provided code entered directly from {who} code "
              "(mean % of all cycles; Python callees inclusive of everything below them).")
    md.append("")
    md.append("| kind | mean % |")
    md.append("|---|---:|")
    for k, v in kinds.most_common():
        md.append(f"| {k} | {pct(v, 2)} |")
    md.append("")
    md.append("| callee | mean % |")
    md.append("|---|---:|")
    for k, v in agg.most_common(40):
        md.append(f"| {k} | {pct(v, 2)} |")
    md.append("")
    js.setdefault("entries", {})[caller_kind] = {"kinds": {k: round(v, 5) for k, v in
                                                           kinds.most_common()},
                                                 "callees": [[k, round(v, 5)] for k, v in
                                                             agg.most_common(150)]}


def section_distortion(profs, md, js):
    md.append("| workload | own/lang/stdlib/3rd/runtime bpf | same, -X perf | interp+calls+attr "
              "bpf | -X perf | CALL_PY_* + inlined frames bpf | wall -X perf / bpf |")
    md.append("|---|---|---|---:|---:|---:|---:|")
    rows = []
    for p in profs:
        if "xperf" not in p.raw or "bpf" not in p.raw or p.mode != "bpf":
            continue
        r = {}
        for m in ("bpf", "xperf"):
            c = rows_to_counter(p.raw[m]["counter"])
            t = sum(c.values()) or 1
            r[m] = {"L": p.raw[m]["summary"]["layers"],
                    "bucket": sum(v for k, v in c.items() if k[F["comp"]].startswith(
                        ("interp", "perf-tramp")) or k[F["comp"]] in ("lang:calls", "lang:attr")) / t,
                    "pycall": sum(v for k, v in c.items() if k[F["op"]].startswith(
                        ("CALL_PY", "CALL_BOUND_METHOD_EXACT", "CALL_ALLOC_AND_ENTER_INIT",
                         "CALL_KW_PY", "CALL_EX_PY"))) / t}
        wr = p.raw["xperf"]["wall_median_s"] / p.raw["bpf"]["wall_median_s"]
        rows.append((p.name, r, wr))
        lb = "/".join(pct(r["bpf"]["L"][l], 0) for l in ("own", "language", "stdlib",
                                                          "thirdparty", "runtime"))
        lx = "/".join(pct(r["xperf"]["L"][l], 0) for l in ("own", "language", "stdlib",
                                                            "thirdparty", "runtime"))
        md.append(f"| {p.name} | {lb} | {lx} | {pct(r['bpf']['bucket'])} | "
                  f"{pct(r['xperf']['bucket'])} | {pct(r['bpf']['pycall'])} | {wr:.2f} |")
    if rows:
        mb = mean(r["bpf"]["bucket"] for _n, r, _w in rows)
        mx = mean(r["xperf"]["bucket"] for _n, r, _w in rows)
        gw = statistics.geometric_mean(w for _n, _r, w in rows)
        md.append(f"| **mean** | | | {pct(mb)} | {pct(mx)} | "
                  f"{pct(mean(r['bpf']['pycall'] for _n, r, _w in rows))} | {gw:.2f} (geo) |")
        js["distortion"] = {"bucket_bpf": mb, "bucket_xperf": mx, "wall_ratio_geo": gw,
                            "layers_bpf": {l: mean(r["bpf"]["L"][l] for _n, r, _w in rows)
                                           for l in LAYERS},
                            "layers_xperf": {l: mean(r["xperf"]["L"][l] for _n, r, _w in rows)
                                             for l in LAYERS}}
    md.append("")


def section_quality(profs, md, js):
    md.append("| workload | mode | samples | sampled GHz | owner exact | by name | imputed |")
    md.append("|---|---|---:|---:|---:|---:|---:|")
    for p in sorted(profs, key=lambda p: p.name):
        st = p.sec.get("stats", {})
        res = st.get("resolution", {})
        t = sum(res.values()) or 1
        md.append(f"| {p.name} | {p.mode} | {st.get('samples', '')} | "
                  f"{st.get('sampled_ghz', '')} | {pct(res.get('code', 0) / t)} | "
                  f"{pct(res.get('code-by-name', 0) / t)} | {pct(st.get('imputed_share', 0))} |")
    md.append("")


def report_main(a):
    profs = [Prof(p) for p in a.profiles]
    profs.sort(key=lambda p: p.name)
    ops = _opcodes(profs)
    md = [f"# {a.title}", ""]
    js = {"n": len(profs), "workloads_list": [p.name for p in profs]}
    md.append("## Layers per workload\n")
    section_layers(profs, md, js)
    md.append("## Parts of each layer\n")
    for lay, title in (("stdlib", "stdlib module"), ("thirdparty", "distribution"),
                       ("runtime", "runtime part"), ("language", "language category"),
                       ("generated", "generator")):
        section_sublayers(profs, md, js, lay, title)
    md.append("## Interpreter time by opcode\n")
    section_opcodes(profs, md, js, ops)
    md.append("## Language layer\n")
    section_language(profs, md, js)
    md.append("## Stdlib and generated functions\n")
    section_stdlib_functions(profs, md, js)
    md.append("## Third-party code\n")
    section_thirdparty(profs, md, js)
    md.append("## Entry points called by third-party code\n")
    section_entries(profs, md, js, "thirdparty")
    md.append("## Entry points called by own code\n")
    section_entries(profs, md, js, "own")
    md.append("## -X perf distortion\n")
    section_distortion(profs, md, js)
    md.append("## Sampling quality\n")
    section_quality(profs, md, js)
    with open(a.md, "w") as f:
        f.write("\n".join(md))
    with open(a.json, "w") as f:
        json.dump(js, f, indent=1)
    print("\n".join(md[:80]))


def _cpu_per_run(p):
    sec = p.sec
    if sec.get("cpu_s_per_run"):
        return sec["cpu_s_per_run"]
    ghz = sec.get("stats", {}).get("sampled_ghz")
    if ghz and sec.get("runs"):
        return p.tot / (ghz * 1e9) / sec["runs"]
    return None


def ftdiff_main(a):
    """Free-threading cost: same workload on the GIL build and the t build. Absolute CPU time
    per run (rusage, from the driver) x layer/component shares -> where the t build spends
    more."""
    def load(paths):
        out = {}
        for p in paths:
            pr = Prof(p)
            out[pr.name] = pr
        return out
    gil, ft = load(a.gil), load(a.ft)
    md = ["| workload | CPU s/run GIL | t | t/GIL | Δ by layer (ms/run): own / lang / stdlib / "
          "3rd / runtime | largest growing components (ms/run) |", "|---|---:|---:|---:|---|---|"]
    js = {}
    comp_delta_all = Counter()
    n = 0
    for name in sorted(set(gil) & set(ft)):
        g, t = gil[name], ft[name]
        cg, ctt = _cpu_per_run(g), _cpu_per_run(t)
        if not cg or not ctt:
            continue
        n += 1
        lg = g.sec["summary"]["layers"]
        lt = t.sec["summary"]["layers"]
        dl = {l: (lt[l] * ctt - lg[l] * cg) * 1e3 for l in LAYERS}
        compg = g.by(lambda k: k[F["comp"]])
        compt = t.by(lambda k: k[F["comp"]])
        dc = Counter({c: (compt.get(c, 0) * ctt - compg.get(c, 0) * cg) * 1e3
                      for c in set(compg) | set(compt)})
        for c, v in dc.items():
            comp_delta_all[c] += v / (cg * 1e3)  # relative to the GIL run time
        top = ", ".join(f"{c} {v:+.0f}" for c, v in dc.most_common(4))
        md.append(f"| {name} | {cg:.3f} | {ctt:.3f} | {ctt / cg:.2f} | " +
                  " / ".join(f"{dl[l]:+.0f}" for l in ("own", "language", "stdlib", "thirdparty",
                                                     "runtime")) + f" | {top} |")
        js[name] = {"cpu_gil": cg, "cpu_t": ctt, "delta_layers_ms": dl,
                    "delta_components_ms": dict(dc.most_common(15))}
    md.append("")
    md.append("Mean growth by component, as a fraction of the GIL build's run time:")
    md.append("")
    md.append("| component | mean Δ (% of GIL time) |")
    md.append("|---|---:|")
    for c, v in comp_delta_all.most_common(20):
        md.append(f"| {c} | {pct(v / max(1, n))} |")
    md.append("")
    with open(a.md, "w") as f:
        f.write("\n".join(md))
    with open(a.json, "w") as f:
        json.dump(js, f, indent=1)
    print("\n".join(md))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "whole":
        return whole_main(sys.argv[1:])
    if len(sys.argv) > 1 and sys.argv[1] == "enabled":
        return enabled_main(sys.argv[1:])
    if len(sys.argv) > 1 and sys.argv[1] == "handlers":
        return handlers_main(sys.argv[1:])
    if len(sys.argv) > 1 and sys.argv[1] == "ftdiff":
        ap = argparse.ArgumentParser()
        ap.add_argument("cmd")
        ap.add_argument("--gil", nargs="+", required=True)
        ap.add_argument("--ft", nargs="+", required=True)
        ap.add_argument("--md", required=True)
        ap.add_argument("--json", required=True)
        ftdiff_main(ap.parse_args())
        return
    ap = argparse.ArgumentParser()
    ap.add_argument("profiles", nargs="+")
    ap.add_argument("--md", required=True)
    ap.add_argument("--json", required=True)
    ap.add_argument("--title", default="Layer attribution")
    report_main(ap.parse_args())



# ---------------------------------------------------------------------------------------
# per-handler tables (for handler swapping in the tail-calling dispatch table)
# ---------------------------------------------------------------------------------------


def handler_tables(profs, ops, top=40, pairs=30):
    """-> (rows, pair_rows, totals). Share of all steady-state cycles per opcode handler
    (`_TAIL_CALL_<OP>`, the innermost handler on the native stack), inline (the handler
    itself) + callees (native code it called), excluding cyclic GC that merely ran inside
    the handler's eval-breaker check. Mean over workloads."""
    n = len(profs)
    h = Counter()
    hin = Counter()
    hgc = Counter()
    hcall = defaultdict(Counter)
    pair = Counter()
    pair_layer = {}
    tot_interp = 0.0
    for p in profs:
        for k, v in p.cnt.items():
            op = k[F["op"]]
            if not op:
                continue
            w = v / p.tot / n
            if k[F["layer"]] == "runtime" and k[F["sub"]] == "gc":
                hgc[op] += w
                continue
            h[op] += w
            tot_interp += w
            if k[F["where"]] == "inline":
                hin[op] += w
            else:
                e = k[F["entry"]] or k[F["leaf"]]
                hcall[op][e] += w
                pair[(op, e)] += w
                pair_layer.setdefault((op, e), Counter())[k[F["layer"]] + ":" + k[F["sub"]]] += w
    rows = []
    for op, v in h.most_common(top):
        base, spec = ops.family(op) if op != "(eval)" else ("(eval)", False)
        kind = "s" if spec else ("g" if base in ops.specializable else "-")
        rows.append({"op": op, "family": base, "specialized": spec, "kind": kind, "share": v,
                     "inline": hin[op], "callee": v - hin[op],
                     "top_callees": hcall[op].most_common(3)})
    prow = []
    for (op, e), v in pair.most_common(pairs):
        lay = pair_layer[(op, e)].most_common(1)[0][0]
        prow.append({"op": op, "callee": e, "share": v, "layer": lay})
    spec_tot = sum(v for op, v in h.items() if op != "(eval)" and ops.family(op)[1])
    gen_tot = sum(v for op, v in h.items() if op != "(eval)" and not ops.family(op)[1])
    inl = sum(hin.values())
    return rows, prow, {"interp_total": tot_interp, "inline": inl,
                        "specialized": spec_tot, "generic": gen_tot,
                        "eval": h.get("(eval)", 0), "gc_inside_handlers": sum(hgc.values())}


def handlers_md(profs, ops, title, top=40, pairs=30):
    rows, prow, t = handler_tables(profs, ops, top, pairs)
    md = [f"### {title} (n = {len(profs)})", "",
          f"Handler-attributed time (handler + the native code it called): "
          f"{pct(t['interp_total'])}% of cycles, of which inline in the handlers "
          f"{pct(t['inline'])}%; by handler kind: specialized {pct(t['specialized'])}%, "
          f"not specialized (generic base or no specialization family) {pct(t['generic'])}%, "
          f"eval entry/exit {pct(t['eval'])}%. Cyclic GC that ran inside a handler's "
          f"eval-breaker check is excluded: {pct(t['gc_inside_handlers'])}%. The rest of the "
          f"cycles have no handler on the stack (C code called from C, e.g. callbacks, or no "
          f"Python frame).", "",
          "kind: s = specialized variant, g = generic base of a specializing family, "
          "- = opcode without specializations.", "",
          "| handler | kind | mean % (incl. callees) | inline | callees | top callees (mean %) |",
          "|---|:-:|---:|---:|---:|---|"]
    for r in rows:
        cal = ", ".join(f"`{e}` {pct(v, 2)}" for e, v in r["top_callees"])
        md.append(f"| {r['op']} | {r['kind']} | {pct(r['share'], 2)} | "
                  f"{pct(r['inline'], 2)} | {pct(r['callee'], 2)} | {cal} |")
    md += ["", "| # | handler | callee (C entry point) | callee layer | mean % |",
           "|---:|---|---|---|---:|"]
    for i, r in enumerate(prow, 1):
        md.append(f"| {i} | {r['op']} | `{r['callee']}` | {r['layer']} | {pct(r['share'], 2)} |")
    md.append("")
    return md, {"rows": [{k: (v if k != "top_callees" else [[e, round(x, 5)] for e, x in v])
                          for k, v in r.items()} for r in rows],
                "pairs": prow, "totals": t}


def handlers_main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd")
    ap.add_argument("profiles", nargs="+")
    ap.add_argument("--md", required=True)
    ap.add_argument("--json", required=True)
    a = ap.parse_args(argv)
    profs = [Prof(p) for p in a.profiles]
    ops = _opcodes(profs)
    fams = defaultdict(list)
    for p in profs:
        fams[p.family].append(p)
    md = ["# Interpreter time per opcode handler, CPython 3.15.1", ""]
    js = {}
    for fam in sorted(fams):
        m, j = handlers_md(fams[fam], ops, fam, top=25, pairs=15)
        md += m
        js[fam] = j
    m, j = handlers_md(profs, ops, "all workloads", top=50, pairs=30)
    md = md[:2] + m + md[2:]
    js["all"] = j
    with open(a.md, "w") as f:
        f.write("\n".join(md))
    with open(a.json, "w") as f:
        json.dump(js, f, indent=1, default=float)
    print("\n".join(m))



# ---------------------------------------------------------------------------------------
# Rustique enabled vs plain: what remains in the accelerated modules
# ---------------------------------------------------------------------------------------

WAVE1_MODULES = ("json", "copy", "datetime", "_strptime", "posixpath", "genericpath", "pathlib",
                 "fnmatch", "glob", "logging", "random", "textwrap", "urllib.parse",
                 "statistics", "dataclasses", "os")


def _w1(sub):
    return sub.split(".")[0] in WAVE1_MODULES or sub in WAVE1_MODULES


def enabled_main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd")
    ap.add_argument("--plain", nargs="+", required=True)
    ap.add_argument("--enabled", nargs="+", required=True)
    ap.add_argument("--md", required=True)
    ap.add_argument("--json", required=True)
    a = ap.parse_args(argv)
    plain = {p.name: p for p in map(Prof, a.plain)}
    en = {p.name: p for p in map(Prof, a.enabled)}
    md = ["| workload | CPU s/run plain | enabled | ratio | wave-1 modules plain → enabled (% of plain "
          "time) | Rustique kernels | rest of stdlib | largest remaining wave-1 parts (enabled) |",
          "|---|---:|---:|---:|---|---:|---:|---|"]
    js = {}
    agg = Counter()
    n = 0
    for name in sorted(set(plain) & set(en)):
        p, e = plain[name], en[name]
        cp, ce = _cpu_per_run(p), _cpu_per_run(e)
        if not cp or not ce:
            continue
        n += 1
        scale = ce / cp  # enabled shares -> fraction of the plain run's time
        w1p = p.share(lambda k: k[F["layer"]] == "stdlib" and _w1(k[F["sub"]]))
        w1e = e.share(lambda k: k[F["layer"]] == "stdlib" and _w1(k[F["sub"]])) * scale
        rq = e.share(lambda k: k[F["comp"]] == "rustique" or k[F["sub"]].startswith(
            "rustique")) * scale
        restp = p.share(lambda k: k[F["layer"]] == "stdlib" and not _w1(k[F["sub"]]))
        reste = e.share(lambda k: k[F["layer"]] == "stdlib" and not _w1(k[F["sub"]])
                        and not k[F["sub"]].startswith("rustique")) * scale
        rem = e.by(lambda k: k[F["sub"]], lambda k: k[F["layer"]] == "stdlib" and _w1(k[F["sub"]]))
        top = ", ".join(f"{k} {pct(v * scale)}" for k, v in rem.most_common(3) if v * scale >= 0.01)
        md.append(f"| {name} | {cp:.3f} | {ce:.3f} | {ce / cp:.3f} | {pct(w1p)} → {pct(w1e)} | "
                  f"{pct(rq)} | {pct(restp)} → {pct(reste)} | {top} |")
        js[name] = {"cpu_plain": cp, "cpu_enabled": ce, "w1_plain": w1p, "w1_enabled": w1e,
                    "rustique": rq, "rest_plain": restp, "rest_enabled": reste,
                    "remaining": {k: v * scale for k, v in rem.most_common(8)}}
        for k, v in rem.items():
            agg[k] += v * scale
    md.append("")
    md.append(f"Mean remaining share of wave-1 modules with Rustique enabled (n = {n}, % of the "
              "plain run's time):")
    md.append("")
    md.append("| module | mean % |")
    md.append("|---|---:|")
    for k, v in agg.most_common(15):
        md.append(f"| {k} | {pct(v / max(1, n), 2)} |")
    md.append("")
    with open(a.md, "w") as f:
        f.write("\n".join(md))
    with open(a.json, "w") as f:
        json.dump(js, f, indent=1)
    print("\n".join(md))



# ---------------------------------------------------------------------------------------
# whole process: startup / import / setup / first run / finalization
# ---------------------------------------------------------------------------------------


def whole_main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd")
    ap.add_argument("profiles", nargs="+")
    ap.add_argument("--md", required=True)
    ap.add_argument("--json", required=True)
    a = ap.parse_args(argv)
    md = ["| workload | process Gcycles | startup | import wl | setup | first run | finalize | "
          "runtime in startup+import+setup+finalize: import / gc / alloc+dealloc / startup |",
          "|---|---:|---:|---:|---:|---:|---:|---|"]
    js = {}
    agg = Counter()
    n = 0
    for path in sorted(a.profiles):
        raw = load_profile(path)
        pl, xp = raw.get("wholeplain"), raw.get("whole")
        if not pl or not xp:
            continue
        n += 1
        ph = pl["phases"]
        cnt = Counter({tuple(k): v for k, v in xp["counter"]})
        tot = sum(cnt.values()) or 1
        once = Counter()
        for k, v in cnt.items():
            if k[0] == "run":
                continue
            once[(k[1], k[2])] += v / tot
        rt = {s: once[("runtime", s)] for s in ("import", "gc", "alloc", "dealloc", "startup")}
        md.append(f"| {raw['workload']} | {pl['cycles'] / 1e9:.2f} | " + " | ".join(
            pct(ph.get(p, 0)) for p in ("startup", "import", "setup", "run", "finalize")) +
            f" | {pct(rt['import'])} / {pct(rt['gc'])} / {pct(rt['alloc'] + rt['dealloc'])} / "
            f"{pct(rt['startup'])} |")
        js[raw["workload"]] = {"cycles": pl["cycles"], "phases": ph, "runtime_once": rt}
        for p in ("startup", "import", "setup", "run", "finalize"):
            agg[p] += ph.get(p, 0)
        for s, v in rt.items():
            agg["rt:" + s] += v
    md.append("")
    md.append(f"Means over {n} workloads (% of whole-process cycles): " + ", ".join(
        f"{p} {pct(agg[p] / n)}" for p in ("startup", "import", "setup", "run", "finalize")) +
        "; runtime outside the first run: " + ", ".join(
        f"{s} {pct(agg['rt:' + s] / n)}" for s in ("import", "gc", "alloc", "dealloc", "startup")))
    md.append("")
    with open(a.md, "w") as f:
        f.write("\n".join(md))
    with open(a.json, "w") as f:
        json.dump(js, f, indent=1)
    print("\n".join(md))


if __name__ == "__main__":
    main()
