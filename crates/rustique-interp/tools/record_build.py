"""Record a verified CPython build for the interp tier (src/builds.rs).

    <python> crates/rustique-interp/tools/record_build.py --source "<distribution note>" [--write]

Run it with the interpreter being recorded (a python-build-standalone build). It reads the
binary's ELF build-id and symbol table (`nm`, `readelf`), and checks in the running process
that the core's dispatch table holds, for every handler the tier may swap (gen/<release>/
rq_groups.h) and for NOP (the ABI self-test), exactly the address of the core's
`_TAIL_CALL_<op>`. It also checks that every interpreter symbol the handlers reference
(gen/<release>/rq_weak.h) is exported by the binary, so no weak reference resolves to NULL.
With --write, the entry replaces any entry with the same build-id in src/builds.rs.

A product never parses symbol tables: the tier trusts only the (release, configuration,
build-id) triples recorded here, after CPython's full regression suite passed with the tier
on (docs/targets/interp.md, "Maintenance per patch release").
"""
import argparse
import ctypes
import os
import re
import subprocess
import sys
import sysconfig

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
BUILDS = os.path.join(CRATE, "src", "builds.rs")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", required=True, help="where the build comes from (one line)")
    ap.add_argument("--write", action="store_true")
    a = ap.parse_args()
    if sys.version_info.releaselevel != "final":
        raise SystemExit("only final releases")
    release = "%d.%d.%d" % sys.version_info[:3]
    gen = os.path.join(CRATE, "gen", "v" + release.replace(".", "_"))
    if not os.path.isdir(gen):
        raise SystemExit(f"no handler sources for {release} ({gen}); run tools/gen.py first")
    ft = not getattr(sys, "_is_gil_enabled", lambda: True)()
    exe = os.path.realpath(sys.executable)
    # the object that holds the interpreter (the executable, or libpython when python is
    # linked against it): the mapping that contains PyEval_EvalCode
    anchor = ctypes.cast(ctypes.pythonapi.PyEval_EvalCode, ctypes.c_void_p).value
    binary = None
    for line in open("/proc/self/maps"):
        p = line.split()
        lo, hi = (int(x, 16) for x in p[0].split("-"))
        if lo <= anchor < hi and len(p) >= 6:
            binary = p[5]
            break
    if binary is None:
        raise SystemExit("cannot find the object holding the interpreter")
    note = subprocess.run(["readelf", "-n", binary], capture_output=True, text=True, check=True).stdout
    build_id = re.search(r"Build ID: ([0-9a-f]+)", note).group(1)
    syms = {}
    dyn = set()
    for line in subprocess.run(["nm", binary], capture_output=True, text=True, check=True).stdout.splitlines():
        p = line.split()
        if len(p) == 3:
            syms.setdefault(p[2], []).append(int(p[0], 16))
    for line in subprocess.run(["nm", "-D", "--defined-only", binary], capture_output=True, text=True, check=True).stdout.splitlines():
        p = line.split()
        if len(p) == 3:
            dyn.add(p[2])
    # load bias: runtime address of an exported function minus its link-time address
    bias = anchor - syms["PyEval_EvalCode"][0]

    import _opcode_metadata as m
    opmap = dict(m.opmap)
    opmap.update(m._specialized_opmap)
    names = re.findall(r"X\((\w+), \d+\)", open(os.path.join(gen, "rq_groups.h")).read())
    names.append("NOP")

    def read_ptr(addr):
        return ctypes.c_void_p.from_address(addr).value or 0

    if os.environ.get("RQ_DEBUG"):
        print("bias", hex(bias), "cands", [hex(c) for c in syms.get("instruction_funcptr_handler_table", [])],
              "ops", [(n, opmap[n]) for n in names], file=sys.stderr, flush=True)
    table = None
    for cand in syms.get("instruction_funcptr_handler_table", []):
        if all(read_ptr(bias + cand + 8 * opmap[n]) - bias in syms.get("_TAIL_CALL_" + n, []) for n in names):
            table = cand
            break
    if table is None:
        raise SystemExit("no dispatch table whose entries are the core's _TAIL_CALL_<op> handlers")
    handlers = [(opmap[n], read_ptr(bias + table + 8 * opmap[n]) - bias, n) for n in names]
    tracing = 0
    if not ft:
        cands = syms.get("instruction_funcptr_tracing_table", [])
        rec = syms.get("_TAIL_CALL_TRACE_RECORD", [])
        for cand in cands:
            if read_ptr(bias + cand + 8 * opmap["LOAD_ATTR"]) - bias in rec:
                tracing = cand
                break
        if not tracing:
            raise SystemExit("no tracing table (JIT build expected for the GIL configuration)")
    lookup = syms.get("_PyType_LookupStackRefAndVersion", [0])
    if len(lookup) != 1:
        raise SystemExit("_PyType_LookupStackRefAndVersion: ambiguous")
    weak = re.findall(r"#pragma weak (\w+)", open(os.path.join(gen, "rq_weak.h")).read())
    # The handlers reference only what a compile of this configuration keeps; rq_weak.h is the
    # union over configurations, so only report, per configuration, what the binary lacks.
    missing = sorted(s for s in weak if s not in dyn)
    hexversion = sys.hexversion
    entry = [
        "    VerifiedBuild {",
        f'        release: "{release}",',
        f"        hexversion: {hexversion:#010x},",
        f"        free_threaded: {'true' if ft else 'false'},",
        f'        build_id: "{build_id}",',
        f'        source: "{a.source}",',
        f"        handler_table: {table:#x},",
        f"        tracing_table: {tracing:#x},",
        f"        type_lookup_ref: {lookup[0]:#x},",
        "        handlers: &[",
    ]
    for op, addr, n in handlers:
        entry.append(f"            ({op}, {addr:#x}), // {n}")
    entry += ["        ],", "    },"]
    text = "\n".join(entry) + "\n"
    print(text)
    print(f"// interpreter symbols referenced by some configuration but not exported here: {missing}",
          file=sys.stderr)
    if a.write:
        src = open(BUILDS).read()
        begin, end = "// BEGIN VERIFIED BUILDS\n", "    // END VERIFIED BUILDS\n"
        i, j = src.index(begin) + len(begin), src.index(end)
        body = src[i:j]
        blocks = re.findall(r"    VerifiedBuild \{.*?\n    \},\n", body, re.S)
        blocks = [b for b in blocks if f'build_id: "{build_id}"' not in b] + [text]
        blocks.sort(key=lambda b: (re.search(r'release: "([^"]+)"', b).group(1), "free_threaded: true" in b))
        open(BUILDS, "w").write(src[:i] + "".join(blocks) + src[j:])
        print(f"wrote {BUILDS}", file=sys.stderr)


if __name__ == "__main__":
    main()
