"""Fill the `(3, minor, "...")` fingerprints of the datamodel groups from fp_report.py JSON
output: python benches/datamodel/fill_fp.py fp315.json [fp314.json ...]"""
import json, re, sys, os
ROOT = os.path.join(os.path.dirname(__file__), "..", "..", "crates", "rustique-py", "src", "targets")
MAP = {
    "enum.property.__get__": ("enum/mod.rs", "PROP_GET"),
    "enum.EnumType.__call__": ("enum/mod.rs", "CALL"),
    "enum.Enum.__new__": ("enum/mod.rs", "NEW"),
    "enum.Enum.__hash__": ("enum/mod.rs", "HASH"),
    "enum.Enum.name": ("enum/mod.rs", "NAME_FGET"),
    "enum.Enum.value": ("enum/mod.rs", "VALUE_FGET"),
    "enum.Flag._get_value": ("enum/mod.rs", "GET_VALUE"),
    "enum.Flag.__or__": ("enum/mod.rs", "OR"),
    "enum.Flag.__and__": ("enum/mod.rs", "AND"),
    "enum.Flag.__xor__": ("enum/mod.rs", "XOR"),
    "enum.Flag.__invert__": ("enum/mod.rs", "INVERT"),
    "enum.Flag.__contains__": ("enum/mod.rs", "CONTAINS"),
    "collections.namedtuple": ("generated/namedtuple.rs", "NAMEDTUPLE"),
    "dataclasses._FuncBuilder.add_fns_to_class": ("generated/dataclass.rs", "ADD_FNS_TO_CLASS"),
    "dataclasses._set_new_attribute": ("generated/dataclass.rs", "SET_NEW_ATTRIBUTE"),
    "contextlib.contextmanager": ("contextlib/mod.rs", "CONTEXTMANAGER"),
    "contextlib._GeneratorContextManagerBase.__init__": ("contextlib/mod.rs", "INIT"),
    "functools.singledispatch": ("functools/mod.rs", "SINGLEDISPATCH"),
    "functools.singledispatchmethod.__get__": ("functools/mod.rs", "SDM_GET"),
    "functools._singledispatchmethod_get.__init__": ("functools/mod.rs", "SDM_GET_INIT"),
    "weakref.WeakKeyDictionary.__getitem__": ("functools/mod.rs", "WKD_GETITEM"),
}
for path in sys.argv[1:]:
    d = json.load(open(path))
    minor = d["v"][1]
    for _tgt, what, obs, _exp in d["rows"]:
        if what not in MAP or not obs:
            continue
        f, const = MAP[what]
        p = os.path.join(ROOT, f)
        s = open(p).read()
        pat = re.compile(r"(pub const %s: Fingerprints = &\[)([^\]]*)(\];)" % const)
        m = pat.search(s)
        if not m:
            print("no const", const, "in", f); continue
        body = m.group(2)
        new = re.sub(r"\(3, %d, \"[^\"]*\"\)" % minor, '(3, %d, "%s")' % (minor, obs), body)
        s = s[:m.start(2)] + new + s[m.end(2):]
        open(p, "w").write(s)
        print(f, const, minor, obs)
