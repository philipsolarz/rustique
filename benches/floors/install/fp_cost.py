"""What do code fingerprints cost at install, and how much of it is docstrings?

For each group: the originals its targets fingerprint (`_accel.fingerprint_report`, run in a
separate process so the measured one has computed nothing yet), then three callgrind
regions in fresh processes:
  v2       `_bench("fingerprint", f, 1)` for every Python original (first time in the
           process: includes CPython's de-optimised bytecode copy, `_PyCode_GetCode`)
  nodoc    the same with each code object's docstring constant replaced by "" (an estimate
           of a v3 format that skips the docstring; same structure otherwise)
  c        `_bench("fingerprint_c", obj, 1)` for every C original

    python benches/floors/install/fp_cost.py [group ...]
"""
import json, os, subprocess, sys, types
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "targets"))
from _callgrind import region  # noqa: E402

IMPORTS = {"json": "json", "copy": "copy", "dataclasses": "copy,dataclasses", "logging": "logging",
           "random": "random", "posixpath": "posixpath", "fnmatch": "fnmatch", "pathlib": "pathlib",
           "datetime": "datetime,_strptime", "textwrap": "textwrap", "urllib": "urllib.parse",
           "statistics": "statistics", "float": "", "containers": "", "builtins": ""}

RESOLVE = r'''
import importlib, sys, types, inspect
def resolve(dotted):
    parts = dotted.split(".")
    for i in range(len(parts), 0, -1):
        try:
            obj = importlib.import_module(".".join(parts[:i]))
        except Exception:
            continue
        for p in parts[i:]:
            d = getattr(obj, "__dict__", {})
            obj = d[p] if isinstance(obj, type) and p in d else getattr(obj, p)
        return obj
    import builtins
    obj = builtins
    for p in parts:
        obj = getattr(obj, p)
    return obj
def unwrap(o):
    if isinstance(o, (staticmethod, classmethod)):
        o = o.__func__
    if isinstance(o, property):
        o = o.fget
    if hasattr(o, "func") and type(o).__name__ == "cached_property":
        o = o.func
    while hasattr(o, "__wrapped__") and not isinstance(o, types.FunctionType):
        o = o.__wrapped__
    return o
def tryres(n):
    try:
        return unwrap(resolve(n))
    except Exception:
        return None
def nodoc(f):
    c = f.__code__.replace()  # always a fresh code object (CPython's co_code cache empty)
    if c.co_flags & 0x4000000 and c.co_consts and isinstance(c.co_consts[0], str):
        c = c.replace(co_consts=("",) + c.co_consts[1:])
    return types.FunctionType(c, f.__globals__, f.__name__, f.__defaults__, f.__closure__)
def fresh(f):
    c = f.__code__.replace()
    c = c.replace(co_consts=tuple(c.co_consts))
    return types.FunctionType(c, f.__globals__, f.__name__, f.__defaults__, f.__closure__)
'''

# Groups that fingerprint through their own helpers (not listed by fingerprint_report).
MANUAL = {
    "copy": ["copy.deepcopy", "copy.copy", "copy._deepcopy_list", "copy._deepcopy_tuple", "copy._deepcopy_dict",
             "copy._deepcopy_method", "copy._keep_alive", "copy._reconstruct", "copyreg.__newobj__",
             "copyreg._slotnames", "copy._deepcopy_frozendict"],
    "dataclasses": ["dataclasses.asdict", "dataclasses._asdict_inner", "dataclasses.astuple",
                    "dataclasses._astuple_inner", "dataclasses.fields", "dataclasses._is_dataclass_instance"],
    "posixpath": ["posixpath.join", "posixpath.split", "posixpath.splitext", "posixpath.basename", "posixpath.dirname",
                  "posixpath.isabs", "posixpath.relpath", "posixpath.abspath", "posixpath._get_sep",
                  "genericpath._splitext", "genericpath.commonprefix", "genericpath._commonprefix", "os.fspath",
                  "os.getcwd", "os.getcwdb", "posixpath.normpath"],
    "fnmatch": ["fnmatch.fnmatch", "fnmatch.fnmatchcase", "fnmatch.filter", "fnmatch._compile_pattern.__wrapped__",
                "fnmatch.translate", "fnmatch._translate", "fnmatch._join_translated_parts", "re.escape", "re.compile",
                "re._compile", "re._parser._parse", "re._parser._class_escape", "posixpath.normcase"],
    "pathlib": ["pathlib." + n for n in [
        "PurePath.__new__", "PurePath.__init__", "PurePath.with_segments", "PurePath.__truediv__", "PurePath.__str__",
        "PurePath._format_parsed_parts", "PurePath._from_parsed_parts", "PurePath._from_parsed_string",
        "PurePath._parse_path", "PurePath._raw_path", "PurePath.drive", "PurePath.root", "PurePath._tail",
        "PurePath.parent", "PurePath.name", "PurePath.suffix", "PurePath.stem", "PurePath.parts", "PurePath.parents",
        "PurePath.relative_to", "PurePath._str_normcase", "PurePath.__eq__", "PurePath.__hash__",
        "_PathParents.__init__", "_PathParents.__len__", "_PathParents.__getitem__", "Path.__new__"]]
               + ["posixpath.splitdrive", "posixpath.splitroot"],
}

def names_of(group):
    if group in MANUAL:
        return MANUAL[group]
    imp = IMPORTS[group]
    code = ("import importlib, json; from rustique import _rustique; A = _rustique._accel\n"
            f"[importlib.import_module(m) for m in {imp!r}.split(',') if m]\n"
            f"print(json.dumps(sorted({{w for t, w, o, e in A.fingerprint_report([{group!r}]) if o}})))")
    r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    if r.returncode:
        print(json.dumps({"group": group, "error": r.stderr[-600:]}), flush=True)
        return None
    return json.loads(r.stdout.strip().splitlines()[-1])

def main():
    groups = sys.argv[1:] or list(IMPORTS)
    for g in groups:
        names = names_of(g)
        if names is None:
            continue
        imp = IMPORTS[g]
        setup = ("from rustique import _rustique; A = _rustique._accel; import importlib\n"
                 f"[importlib.import_module(m) for m in {imp!r}.split(',') if m]\n" + RESOLVE +
                 f"objs = [o for o in (tryres(n) for n in {names!r}) if o is not None]\n"
                 "py = [o for o in objs if isinstance(o, types.FunctionType)]\n"
                 "cf = [o for o in objs if not isinstance(o, types.FunctionType) and not isinstance(o, types.CodeType)]\n"
                 "nd = [nodoc(f) for f in py]\n"
                 "fr = [fresh(f) for f in py]\n"
                 "A._bench('fingerprint', (lambda: 0), 1); A._bench('fingerprint_c', len, 1)\n")
        v2 = region("for f in py: A._bench('fingerprint', f, 1)", setup=setup)
        nd = region("for f in nd: A._bench('fingerprint', f, 1)", setup=setup)
        fr = region("for f in fr: A._bench('fingerprint', f, 1)", setup=setup)
        warm = region("for f in py: A._bench('fingerprint', f, 1)", setup=setup + "for f in py: f.__code__.co_code\n")
        c = region("for o in cf: A._bench('fingerprint_c', o, 1)", setup=setup)
        base = region("for f in py: pass", setup=setup)
        info = subprocess.run([sys.executable, "-c", setup +
                               "doc = sum(len((f.__code__.co_consts[0] if f.__code__.co_flags & 0x4000000 else '') or '') for f in py)\n"
                               "codeb = sum(len(f.__code__.co_code) for f in py)\n"
                               "print(len(py), len(cf), doc, codeb)"],
                              capture_output=True, text=True, check=True).stdout.split()
        npy, nc, doc, codeb = map(int, info)
        print(json.dumps({"group": g, "py_originals": npy, "c_originals": nc, "doc_chars": doc, "co_code_bytes": codeb,
                          "fp_code_v2": v2 - base, "fp_code_v2_freshcopy": fr - base, "fp_code_nodoc": nd - base, "fp_code_v2_getcode_cached": warm - base,
                          "note": "v2 and getcode_cached are both with CPython's co_code cache filled (code.replace() in setup); fresh = first fingerprint of a fresh code object (true install cost); fresh - nodoc = docstring share",
                          "fp_c": c - base, "names": names}), flush=True)

if __name__ == "__main__":
    main()
