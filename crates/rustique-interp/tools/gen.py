"""Generate the checked-in handler sources of one CPython patch release.

    python3 crates/rustique-interp/tools/gen.py --src /work/cpython/v3.15.1 \\
        --pbs-gil <PBS 3.15.1 install dir> --pbs-t <PBS 3.15.1t install dir> [--cc clang-20]

Run from the repository root. Writes crates/rustique-interp/gen/v<X>_<Y>_<Z>/:
  rq_cases.c.h, rq_targets.h   CPython's cases generator (Tools/cases_generator of that tag)
                               on that tag's Python/bytecodes.c followed by c/overrides.c
  rq_groups.h                  the swappable handlers and their override groups, from that
                               tag's Lib/_opcode_metadata.py
  rq_weak.h                    `#pragma weak` for every interpreter symbol the handlers can
                               reference, so the extension loads on any CPython
  include/, python/            the CPython headers the handlers need (only the files a compile
                               reads), copied from the tag: no CPython source at wheel build
  config-gil/, config-t/       pyconfig.h of the verified python-build-standalone builds
  defs-gil.txt, defs-t.txt     the core's own -D flags (sysconfig PY_CFLAGS_NODIST)
  SOURCES.md                   the origin of every vendored file
The original body of every overridden op is checked against c/overrides.lock: if CPython
changed it, generation stops (the override must be re-proven; --update-lock records the new
body after review).
"""
import argparse
import hashlib
import os
import re
import runpy
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
CDIR = os.path.join(CRATE, "c")
OVERRIDES_REL = os.path.relpath(os.path.join(CDIR, "overrides.c"))

# Override groups (c/rq_common.h). The LOAD_ATTR family is swapped as a whole: specialized
# sites that deopt jump directly (not through the table) into the generic handler of the
# same file, so only a family swap reaches polymorphic sites.
GROUP_ATTR, GROUP_SETATTR, GROUP_KWINIT, GROUP_ISINSTANCE = 0, 1, 2, 3


def sh(cmd, **kw):
    r = subprocess.run(cmd, text=True, capture_output=True, **kw)
    if r.returncode:
        sys.stderr.write(r.stderr[-6000:])
        raise SystemExit(f"failed: {' '.join(cmd)}")
    return r.stdout


def py_of(pbs):
    for name in ("python3", "python"):
        p = os.path.join(pbs, "bin", name)
        if os.path.exists(p):
            return p
    raise SystemExit(f"no python in {pbs}/bin")


def op_body(src, name):
    """The text of `op(NAME, ...) {...}` / `inst(NAME, ...) {...}` in bytecodes.c."""
    m = re.search(r"\b(?:op|inst)\(" + re.escape(name) + r"\s*,", src)
    if not m:
        raise SystemExit(f"{name}: not found in bytecodes.c")
    i = src.index("{", m.end())
    depth = 0
    for j in range(i, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0:
                return src[m.start():j + 1]
    raise SystemExit(f"{name}: unbalanced")


# Core helpers copied into c/rq_inline.h (the core inlines them into its own handlers): their
# originals must be unchanged for a release to be generated.
COPIED = [("Python/ceval.c", "_PyObjectArray_FromStackRefArray"),
          ("Python/ceval.c", "_PyObjectArray_Free"),
          ("Python/ceval.c", "_Py_VectorCall_StackRefSteal"),
          ("Objects/object.c", "_PyObject_GetAttrStackRef"),
          ("Objects/call.c", "PyObject_Vectorcall")]


def c_function(text, name):
    m = re.search(r"^" + re.escape(name) + r"\(", text, re.M)
    if not m:
        raise SystemExit(f"{name}: definition not found")
    i = text.index("{", m.end())
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[m.start():j + 1]
    raise SystemExit(f"{name}: unbalanced")


def norm_hash(text):
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r"\s+", " ", text).strip()
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def check_lock(src, bytecodes, update):
    ovr = open(os.path.join(CDIR, "overrides.c")).read()
    names = re.findall(r"override\s+(?:op|inst)\((\w+)", ovr)
    lock_path = os.path.join(CDIR, "overrides.lock")
    lock = {}
    if os.path.exists(lock_path):
        for line in open(lock_path):
            if line.strip() and not line.startswith("#"):
                k, v = line.split()
                lock[k] = v
    bad = []
    entries = [(n, op_body(bytecodes, n)) for n in names]
    entries += [(f"copy:{f}:{fn}", c_function(open(os.path.join(src, f)).read(), fn)) for f, fn in COPIED]
    for n, body in entries:
        h = norm_hash(body)
        if lock.get(n) not in (None, h) and not update:
            bad.append(f"{n}: CPython's original changed ({lock[n]} -> {h})")
        lock[n] = h
    if bad:
        raise SystemExit("override(s) must be re-proven:\n  " + "\n  ".join(bad))
    with open(lock_path, "w") as f:
        f.write("# CPython's original body of each overridden op, and of each core helper copied\n"
                "# into c/rq_inline.h (copy:<file>:<function>) (normalized sha256 prefix).\n"
                "# tools/gen.py refuses a release whose original differs; review, re-prove the\n"
                "# override, then regenerate with --update-lock.\n")
        for n in sorted(lock):
            f.write(f"{n} {lock[n]}\n")
    return names


def groups_header(src, out):
    meta = runpy.run_path(os.path.join(src, "Lib", "_opcode_metadata.py"))
    opmap = dict(meta["opmap"])
    opmap.update(meta["_specialized_opmap"])
    spec = meta["_specializations"]
    rows = [("LOAD_ATTR", GROUP_ATTR)] + [(n, GROUP_ATTR) for n in spec["LOAD_ATTR"]]
    rows += [("CALL_NON_PY_GENERAL", GROUP_SETATTR), ("CALL_KW_NON_PY", GROUP_KWINIT),
             ("CALL_ISINSTANCE", GROUP_ISINSTANCE)]
    never = {"ENTER_EXECUTOR", "JUMP_BACKWARD", "JUMP_BACKWARD_JIT", "JUMP_BACKWARD_NO_JIT",
             "RESUME_CHECK_JIT", "TRACE_RECORD"}
    for n, _ in rows:
        assert n in opmap and opmap[n] < 256 and n not in never and not n.startswith("INSTRUMENTED_"), n
    with open(os.path.join(out, "rq_groups.h"), "w") as f:
        f.write("/* Generated by tools/gen.py from Lib/_opcode_metadata.py: every handler the "
                "interp tier\n * may swap, with its override group (c/rq_common.h). */\n"
                "#define RQ_SWAPPABLE(X) \\\n")
        f.write(" \\\n".join(f"    X({n}, {g})" for n, g in rows) + "\n")
    return rows


def cflags(out, cfg, defs):
    return (["-std=gnu11", "-O2", "-w", "-fPIC", "-DNDEBUG", "-DPy_BUILD_CORE_MODULE",
             f"-DRQ_SET=scan_{cfg}", '-DRQ_RELEASE_TAG="scan"'] + defs
            + ["-I", os.path.join(out, f"config-{cfg}"), "-I", out, "-I", CDIR])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True, help="CPython source tree of the release (its tag)")
    ap.add_argument("--pbs-gil", required=True)
    ap.add_argument("--pbs-t", required=True)
    ap.add_argument("--cc", default="clang-20")
    ap.add_argument("--update-lock", action="store_true")
    ap.add_argument("--base", help="an older release dir under gen/ (v3_15_1): headers identical "
                    "to its copies are not vendored again (build.rs searches it second)")
    a = ap.parse_args()
    src = os.path.abspath(a.src)
    ver = re.search(r'#define PY_VERSION\s+"([^"]+)"', open(os.path.join(src, "Include", "patchlevel.h")).read()).group(1)
    if not re.fullmatch(r"\d+\.\d+\.\d+", ver):
        raise SystemExit(f"{ver}: only final releases")
    out = os.path.join(CRATE, "gen", "v" + ver.replace(".", "_"))
    if os.path.exists(out):
        shutil.rmtree(out)
    os.makedirs(out)
    bytecodes = open(os.path.join(src, "Python", "bytecodes.c")).read()
    overridden = check_lock(src, bytecodes, a.update_lock)

    # 1. the cases generator of this release, on its bytecodes.c + our overrides
    gens = os.path.join(src, "Tools", "cases_generator")
    for tool, name in (("tier1_generator.py", "rq_cases.c.h"), ("target_generator.py", "rq_targets.h")):
        sh([sys.executable, os.path.join(gens, tool), "-o", os.path.join(out, name),
            os.path.join(src, "Python", "bytecodes.c"), OVERRIDES_REL])
        p = os.path.join(out, name)
        s = open(p).read().replace(src + "/", "")
        open(p, "w").write(s)
    rows = groups_header(src, out)

    # 2. configuration of the verified builds
    cfgs = {}
    for cfg, pbs in (("gil", a.pbs_gil), ("t", a.pbs_t)):
        py = py_of(pbs)
        info = sh([py, "-c", "import sys, sysconfig, json; print(json.dumps([sys.version, "
                   "sysconfig.get_paths()['include'], sysconfig.get_config_var('PY_CFLAGS_NODIST')]))"])
        import json
        version, inc, nodist = json.loads(info)
        if not version.startswith(ver + " "):
            raise SystemExit(f"{pbs}: {version} is not {ver}")
        os.makedirs(os.path.join(out, f"config-{cfg}"))
        shutil.copy(os.path.join(inc, "pyconfig.h"), os.path.join(out, f"config-{cfg}", "pyconfig.h"))
        defs = [f for f in nodist.split() if f.startswith("-D")]
        open(os.path.join(out, f"defs-{cfg}.txt"), "w").write(" ".join(defs) + "\n")
        cfgs[cfg] = defs

    # 3. the headers a compile reads (both configurations), copied from the source tree
    open(os.path.join(out, "rq_weak.h"), "w").write("/* placeholder */\n")
    src_inc = ["-I", os.path.join(src, "Python"), "-I", os.path.join(src, "Include"),
               "-I", os.path.join(src, "Include", "internal"),
               "-I", os.path.join(src, "Include", "internal", "mimalloc")]
    deps = set()
    for cfg, defs in cfgs.items():
        for scan in ([], ["-DRQ_WEAK_SCAN"]):
            mk = sh([a.cc] + cflags(out, cfg, defs) + scan + src_inc
                    + ["-M", os.path.join(CDIR, "rq_handlers.c")])
            for tok in mk.replace("\\\n", " ").split():
                p = os.path.realpath(tok)
                if p.startswith(src + os.sep):
                    deps.add(os.path.relpath(p, src))
    vendored = []
    shared = 0
    base = os.path.join(CRATE, "gen", a.base) if a.base else None
    if base:
        open(os.path.join(out, "base.txt"), "w").write(a.base + "\n")
    for rel in sorted(deps):
        top, rest = rel.split(os.sep, 1)
        if top == "Include":
            dst = os.path.join(out, "include", rest)
        elif top == "Python":
            dst = os.path.join(out, "python", rest)
        else:
            raise SystemExit(f"unexpected header {rel}")
        if base:
            twin = os.path.join(base, os.path.relpath(dst, out))
            if os.path.exists(twin) and open(twin, "rb").read() == open(os.path.join(src, rel), "rb").read():
                shared += 1
                continue
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy(os.path.join(src, rel), dst)
        vendored.append((os.path.relpath(dst, out), rel))
    # Headers as a compile sees them: the base release's tree with this release's files over
    # it (one directory tree, so quoted includes resolve next to the including file).
    hdr = tempfile.mkdtemp(prefix="rqhdr-")
    for r in ([base] if base else []) + [out]:
        for top in ("python", "include"):
            if os.path.isdir(os.path.join(r, top)):
                shutil.copytree(os.path.join(r, top), os.path.join(hdr, top), dirs_exist_ok=True)
    inc = ["-I", os.path.join(hdr, "python"), "-I", os.path.join(hdr, "include"),
           "-I", os.path.join(hdr, "include", "internal"),
           "-I", os.path.join(hdr, "include", "internal", "mimalloc")]

    # 4. weak references: every Py*/_Py* symbol any handler of the file references
    weak = set()
    with tempfile.TemporaryDirectory() as td:
        for cfg, defs in cfgs.items():
            obj = os.path.join(td, f"{cfg}.o")
            sh([a.cc] + cflags(out, cfg, defs) + ["-DRQ_WEAK_SCAN"] + inc
               + ["-c", os.path.join(CDIR, "rq_handlers.c"), "-o", obj])
            for line in sh(["nm", "-u", obj]).splitlines():
                s = line.split()[-1]
                if s.startswith(("Py", "_Py")):
                    weak.add(s)
    with open(os.path.join(out, "rq_weak.h"), "w") as f:
        f.write("/* Generated by tools/gen.py: every interpreter symbol the handlers of this "
                "release can reference\n * is a weak reference, so the extension loads on any "
                "CPython (the tier refuses where\n * the binary is not a verified build). */\n")
        for s in sorted(weak):
            f.write(f"#pragma weak {s}\n")

    # 5. the origin of every vendored file
    gitrev = ""
    try:
        gitrev = sh(["git", "-C", src, "describe", "--tags", "--always"]).strip()
    except Exception:
        pass
    with open(os.path.join(out, "SOURCES.md"), "w") as f:
        f.write(f"# Handler sources for CPython {ver}\n\n")
        f.write(f"Generated by `crates/rustique-interp/tools/gen.py` from the CPython source tree "
                f"of tag v{ver}{(' (' + gitrev + ')') if gitrev else ''}; PSF licence "
                f"(`../LICENSE`).\n\n")
        f.write(f"- `rq_cases.c.h`, `rq_targets.h`: `Tools/cases_generator/{{tier1,target}}_generator.py` "
                f"of v{ver} on `Python/bytecodes.c` of v{ver} + `c/overrides.c` "
                f"(overridden: {', '.join(overridden)}).\n")
        f.write(f"- `rq_groups.h`: {len(rows)} swappable handlers from `Lib/_opcode_metadata.py` of v{ver}.\n")
        f.write(f"- `rq_weak.h`: {len(weak)} weak references.\n")
        for cfg, pbs in (("gil", a.pbs_gil), ("t", a.pbs_t)):
            f.write(f"- `config-{cfg}/pyconfig.h`, `defs-{cfg}.txt`: from the python-build-standalone "
                    f"build `{os.path.basename(os.path.normpath(pbs))}` (`{' '.join(cfgs[cfg])}`).\n")
        if base:
            f.write(f"\n{shared} more headers are byte-identical to `../{a.base}/` (`base.txt`: "
                    f"build.rs lays this directory's files over that tree).\n")
        f.write(f"\nVendored from CPython v{ver} ({len(vendored)} files, unmodified):\n\n")
        for dst, rel in vendored:
            f.write(f"- `{dst}` ← `{rel}`\n")
    shutil.rmtree(hdr, ignore_errors=True)
    print(f"generated {out}: {len(vendored)} headers, {len(weak)} weak symbols, {len(rows)} swappable handlers")


if __name__ == "__main__":
    main()
