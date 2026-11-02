"""Attribute the extension's code size to components by symbol name (docs/atlas/w2-startup.md §4).

    python benches/startup/so_size.py path/to/_rustique.so [--top 30]

`nm -S -C` sizes of every defined text/data symbol, bucketed by the first matching rule
(a symbol mentioning a native type anywhere -- including PyO3 generics instantiated for it
-- counts as native). Also prints ELF facts that drive load cost: file size, `.text`,
`.data.rel.ro`, relocation counts by type, imported symbols.
"""
import argparse
import collections
import re
import subprocess

NATIVE = re.compile(r"rustique_py::(list|dict|set|tuple|frozenset|expr|capi|scalars|typing|storage|convert|schema)\b"
                    r"|rustique_core::|rustique_api::|compact_str::|hashbrown::raw::RawTable<\(rustique_core")
RULES = [
    ("native types (+ rustique-core)", NATIVE),
    ("builtins group (sum/min/max/sorted/any/all)", re.compile(r"rustique_py::(builtins|objsort)\b")),
    ("containers group (takeover)", re.compile(r"rustique_py::takeover\b")),
    ("accel framework", re.compile(r"rustique_py::(accel|patch|sync)\b")),
    ("targets (wave-1 kernels)", re.compile(r"rustique_py::targets::")),
    ("pyo3 runtime", re.compile(r"\bpyo3(_ffi)?::")),
    ("Rust std/core/alloc", re.compile(r"\b(std|core|alloc|gimli|addr2line|miniz_oxide|object|rustc_demangle|hashbrown|foldhash|parking_lot)::|^_?Unwind|__rust|rust_")),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("so")
    ap.add_argument("--top", type=int, default=0)
    a = ap.parse_args()
    out = subprocess.run(["nm", "-S", "-C", "--defined-only", a.so], capture_output=True, text=True, check=True).stdout
    buckets = collections.Counter()
    tops = []
    for line in out.splitlines():
        parts = line.split(" ", 3)
        if len(parts) < 4:
            continue
        size, kind, name = int(parts[1], 16), parts[2], parts[3]
        if kind.lower() not in "tdrb":
            continue
        cat = next((n for n, r in RULES if r.search(name)), "other")
        buckets[(cat, "text" if kind.lower() == "t" else "data")] += size
        tops.append((size, cat, name))
    total = sum(buckets.values())
    print(f"{'component':<46} {'text KB':>9} {'data KB':>9} {'share':>7}")
    for cat in [n for n, _ in RULES] + ["other"]:
        t, d = buckets[(cat, 'text')], buckets[(cat, 'data')]
        print(f"{cat:<46} {t / 1024:>9.1f} {d / 1024:>9.1f} {100 * (t + d) / total:>6.1f}%")
    print(f"{'total (symbol sizes)':<46} {sum(v for (c, k), v in buckets.items() if k == 'text') / 1024:>9.1f} "
          f"{sum(v for (c, k), v in buckets.items() if k == 'data') / 1024:>9.1f}")
    rel = subprocess.run(["readelf", "-rW", a.so], capture_output=True, text=True).stdout
    kinds = collections.Counter(m.group(1) for m in re.finditer(r"\s(R_X86_64_\w+)", rel))
    dyn = subprocess.run(["readelf", "-W", "--dyn-syms", a.so], capture_output=True, text=True).stdout
    und = [l.split()[-1] for l in dyn.splitlines() if " UND " in l and l.split()[-1] != "UND"]
    secs = subprocess.run(["size", "-A", a.so], capture_output=True, text=True).stdout
    import os
    print(f"\nfile {os.path.getsize(a.so):,} B; relocations {dict(kinds)}; imported symbols {len(und)} "
          f"({sum(1 for u in und if re.match(r'_?Py', u))} Python C-API)")
    for s in (".text", ".rodata", ".data.rel.ro", ".eh_frame", ".gcc_except_table", ".rela.dyn"):
        m = re.search(rf"^{re.escape(s)}\s+(\d+)", secs, re.M)
        if m:
            print(f"  {s:<20} {int(m.group(1)):>10,} B")
    if a.top:
        for size, cat, name in sorted(tops, reverse=True)[: a.top]:
            print(f"{size:>8} {cat[:20]:<20} {name[:150]}")


if __name__ == "__main__":
    main()
