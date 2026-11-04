"""Which functions of the accelerated stdlib modules changed in CPython patch releases?

For every pair of consecutive final tags of one minor version (v3.X.N -> v3.X.N+1), compare
the target modules function by function: the AST of each function with its docstring removed
(a proxy for the code fingerprint of docs/ACCELERATION.md §3.2, which is line- and
path-independent), and separately whether only the docstring changed (the fingerprint hashes
co_consts, which holds the docstring on 3.14+). Also lists C files of the targets that changed.

    python benches/floors/patch_history.py <bare cpython.git> [3.11 3.12 ...]  > out.txt

Needs `git` and a Python that parses every version's syntax (3.15 parses 3.11-3.15).
"""
import ast, subprocess, sys, re

PY_FILES = ["Lib/json/__init__.py", "Lib/json/encoder.py", "Lib/json/decoder.py", "Lib/json/scanner.py",
            "Lib/copy.py", "Lib/copyreg.py", "Lib/dataclasses.py", "Lib/_strptime.py", "Lib/posixpath.py",
            "Lib/genericpath.py", "Lib/fnmatch.py", "Lib/pathlib/__init__.py", "Lib/pathlib/_local.py",
            "Lib/pathlib/_abc.py", "Lib/pathlib/_os.py", "Lib/pathlib.py", "Lib/logging/__init__.py", "Lib/random.py",
            "Lib/textwrap.py", "Lib/urllib/parse.py", "Lib/statistics.py", "Lib/calendar.py", "Lib/locale.py"]
C_FILES = ["Modules/_json.c", "Objects/floatobject.c", "Python/pystrtod.c", "Python/dtoa.c", "Objects/listobject.c",
           "Objects/tupleobject.c", "Python/bltinmodule.c", "Modules/_datetimemodule.c", "Modules/_randommodule.c",
           "Modules/timemodule.c", "Objects/unicodeobject.c", "Modules/posixmodule.c", "Python/compile.c",
           "Python/codegen.c", "Python/flowgraph.c", "Python/assemble.c", "Lib/opcode.py", "Include/opcode_ids.h"]

def git(repo, *a):
    return subprocess.run(["git", "-C", repo, *a], capture_output=True, text=True).stdout

def funcs(src):
    out = {}
    try:
        tree = ast.parse(src)
    except SyntaxError:
        return None
    def walk(node, prefix):
        for n in ast.iter_child_nodes(node):
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                q = prefix + n.name
                if not isinstance(n, ast.ClassDef):
                    body = n.body
                    doc = None
                    if body and isinstance(body[0], ast.Expr) and isinstance(getattr(body[0], "value", None), ast.Constant) and isinstance(body[0].value.value, str):
                        doc = body[0].value.value
                        body = body[1:]
                    n2 = ast.FunctionDef(name=n.name, args=n.args, body=body or [ast.Pass()], decorator_list=n.decorator_list,
                                         returns=n.returns, type_params=getattr(n, "type_params", []))
                    out[q] = (ast.dump(n2, include_attributes=False), doc)
                walk(n, q + ".")
    walk(tree, "")
    return out

def main():
    repo = sys.argv[1]
    minors = sys.argv[2:] or ["3.11", "3.12", "3.13", "3.14", "3.15"]
    tags = [t for t in git(repo, "tag", "-l", "v3.*").split() if re.fullmatch(r"v3\.\d+\.\d+", t)]
    for minor in minors:
        ts = sorted((t for t in tags if t.startswith("v" + minor + ".")), key=lambda t: int(t.rsplit(".", 1)[1]))
        print(f"# {minor}: {len(ts)} final releases {ts[0]}..{ts[-1]}")
        for a, b in zip(ts, ts[1:]):
            changed = set(git(repo, "diff", "--name-only", a, b, "--", *PY_FILES, *C_FILES).split())
            if not changed:
                print(f"{a} -> {b}: no target file changed")
                continue
            lines = []
            for f in sorted(changed):
                if f.endswith(".py") and f in PY_FILES:
                    fa, fb = funcs(git(repo, "show", f"{a}:{f}")), funcs(git(repo, "show", f"{b}:{f}"))
                    if fa is None or fb is None:
                        lines.append(f"  {f}: (unparsable)"); continue
                    code = sorted(q for q in fa.keys() & fb.keys() if fa[q][0] != fb[q][0])
                    doc = sorted(q for q in fa.keys() & fb.keys() if fa[q][0] == fb[q][0] and fa[q][1] != fb[q][1])
                    added = sorted(fb.keys() - fa.keys()); removed = sorted(fa.keys() - fb.keys())
                    lines.append(f"  {f}: code {code} doc-only {doc} added {added} removed {removed}")
                else:
                    n = len([l for l in git(repo, "diff", a, b, "--", f).splitlines() if l[:1] in "+-" and l[:3] not in ("+++", "---")])
                    lines.append(f"  {f}: C/other, {n} changed lines")
            print(f"{a} -> {b}:")
            print("\n".join(lines))

if __name__ == "__main__":
    main()
