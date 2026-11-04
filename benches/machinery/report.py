#!/usr/bin/env python3
"""Share tables of CPython-provided machinery from profile_machinery.py profiles.

    report.py <m.json>... [--md out.md] [--json out.json]

Groups every machinery entry (see profile_machinery.py) into a candidate, then prints per
workload family the mean and max share of each candidate, plus cProfile call counts and the
implied cost per call. Candidates are grouped by layer (generated / stdlib / language /
runtime / interp / thirdparty native) as in the wave-2 brief.
"""

import json
import re
import sys
from collections import defaultdict

W1 = "wave-1 group (w2-floors)"

# (layer, candidate, regex on the entry key) -- first match wins
RULES = [
    # generated code
    ("generated", "dataclass generated methods", r"^gen:dataclasses:"),
    ("generated", "namedtuple __new__ (eval'd lambda)", r"^gen:namedtuple\?"),
    ("generated", "namedtuple methods (_make/_replace/_asdict/__repr__)", r"^std:collections/__init__\.py:namedtuple\.<locals>\."),
    ("generated", "total_ordering methods", r"^std:functools\.py:_(gt|lt|ge|le)_from_"),
    ("generated", "enum: property.__get__ (.value/.name)", r"^std:enum\.py:property\.__get__"),
    ("generated", "enum: EnumType.__call__ / Enum.__new__ (value lookup)", r"^std:enum\.py:(EnumType\.__call__|Enum\.__new__)"),
    ("generated", "enum: Enum.__hash__", r"^std:enum\.py:Enum\.__hash__"),
    ("generated", "enum: Flag operators", r"^std:enum\.py:Flag\."),
    ("generated", "enum: other", r"^std:enum\.py:"),
    ("generated", "other exec'd code (<string>)", r"^gen:exec"),
    # wave-1 groups (owned by w2-floors)
    (W1, "json", r"^std:json/"),
    (W1, "copy", r"^std:copy\.py"),
    (W1, "dataclasses asdict/astuple/replace", r"^std:dataclasses\.py:(asdict|astuple|replace|_asdict_inner|_astuple_inner)"),
    (W1, "logging", r"^std:logging/"),
    (W1, "paths (posixpath/genericpath/pathlib/fnmatch)", r"^std:(posixpath|genericpath|pathlib/|fnmatch)"),
    (W1, "urllib.parse", r"^std:urllib/parse\.py"),
    (W1, "random", r"^std:random\.py"),
    (W1, "textwrap", r"^std:textwrap\.py"),
    (W1, "statistics", r"^std:statistics\.py"),
    (W1, "datetime text (_strptime)", r"^std:_strptime\.py"),
    # stdlib machinery (this slice)
    ("stdlib", "dataclasses: fields()/other runtime", r"^std:dataclasses\.py:"),
    ("stdlib", "typing: _ProtocolMeta.__instancecheck__", r"^std:typing\.py:_ProtocolMeta"),
    ("stdlib", "typing: other run-time", r"^std:typing\.py:"),
    ("stdlib", "abc: ABCMeta.__instancecheck__/__subclasscheck__", r"^std:abc\.py:"),
    ("stdlib", "collections.abc mixins (Mapping.get/...)", r"^std:_collections_abc\.py:"),
    ("stdlib", "collections (Counter/OrderedDict/...)", r"^std:collections/"),
    ("stdlib", "functools: singledispatch", r"^std:functools\.py:_?singledispatch"),
    ("stdlib", "functools: cached_property", r"^std:functools\.py:cached_property"),
    ("stdlib", "functools: other (update_wrapper/wraps/partialmethod)", r"^std:functools\.py:"),
    ("stdlib", "contextlib: contextmanager helper/__init__", r"^std:contextlib\.py:(contextmanager\.<locals>\.helper|_GeneratorContextManagerBase)"),
    ("stdlib", "contextlib: _GeneratorContextManager.__enter__/__exit__", r"^std:contextlib\.py:_(Async)?GeneratorContextManager\.__(a?enter|a?exit)__"),
    ("stdlib", "contextlib: ExitStack/other", r"^std:contextlib\.py:"),
    ("stdlib", "inspect", r"^std:inspect\.py:"),
    ("stdlib", "weakref (WeakValue/KeyDictionary, WeakSet, finalize)", r"^std:(weakref|_weakrefset)\.py:"),
    ("stdlib", "re module functions (_compile cache)", r"^std:re/__init__\.py:"),
    ("stdlib", "re compiler (sre_compile/_parser)", r"^std:re/"),
    ("stdlib", "string.Template/Formatter", r"^std:string/"),
    ("stdlib", "html.escape/unescape", r"^std:html/"),
    ("stdlib", "uuid", r"^std:uuid\.py:"),
    ("stdlib", "ipaddress", r"^std:ipaddress\.py:"),
    ("stdlib", "os.environ / os helpers", r"^std:os\.py:"),
    ("stdlib", "threading", r"^std:threading\.py:"),
    ("stdlib", "queue", r"^std:queue\.py:"),
    ("stdlib", "traceback/linecache", r"^std:(traceback|linecache)\.py:"),
    ("stdlib", "warnings", r"^std:warnings\.py:"),
    ("stdlib", "gettext", r"^std:gettext\.py:"),
    ("stdlib", "http.cookies / http.cookiejar", r"^std:http/cookie"),
    ("stdlib", "http.client", r"^std:http/client\.py:"),
    ("stdlib", "http.server/socketserver", r"^std:(http/server|socketserver)\.py:"),
    ("stdlib", "email (parser/message/headers/utils)", r"^std:email/"),
    ("stdlib", "base64", r"^std:base64\.py:"),
    ("stdlib", "hmac/hashlib wrappers", r"^std:(hmac|hashlib)\.py:"),
    ("stdlib", "shlex", r"^std:shlex\.py:"),
    ("stdlib", "argparse/optparse", r"^std:(argparse|optparse)\.py:"),
    ("stdlib", "xml.etree", r"^std:xml/"),
    ("stdlib", "tokenize", r"^std:tokenize\.py:"),
    ("stdlib", "csv", r"^std:csv\.py:"),
    ("stdlib", "ast/compile", r"^std:ast\.py:"),
    ("stdlib", "asyncio", r"^std:asyncio/"),
    ("stdlib", "zipfile/tarfile/gzip", r"^std:(zipfile|tarfile|gzip)"),
    ("stdlib", "importlib/import (w2-startup)", r"^std:importlib/"),
    ("stdlib", "urllib.request", r"^std:urllib/request\.py:"),
    ("stdlib", "heapq/bisect", r"^std:(heapq|bisect)\.py:"),
    ("stdlib", "decimal/fractions/numbers", r"^std:(_pydecimal|fractions|numbers)\.py:"),
    ("stdlib", "unittest/doctest", r"^std:(unittest|doctest)"),
    ("stdlib", "other stdlib", r"^std:"),
    # language-level operations reached from user bytecode
    ("language", "getattr()/hasattr()/setattr()/delattr()", r"^lang:builtin_(getattr|hasattr|setattr|delattr)$"),
    ("language", "isinstance/issubclass (C paths)", r"^lang:(object_recursive_isinstance|object_isinstance|PyObject_IsInstance|PyObject_IsSubclass|builtin_isinstance|builtin_issubclass|object_issubclass|_abc__abc_(instancecheck|subclasscheck)|recursive_isinstance)"),
    ("language", "str % (PyUnicode_Format)", r"^lang:(PyNumber_Remainder|PyUnicode_Format|unicode_mod)$"),
    ("language", "str.format/format_map/format()", r"^lang:(unicode_format|do_string_format|unicode_format_map|builtin_format|PyObject_Format|_PyUnicode_FormatAdvanced)"),
    ("language", "str()/repr() of objects", r"^lang:(unicode_new|PyObject_Str|PyObject_Repr|builtin_repr|tp_new_wrapper)$"),
    ("language", "int()/float() from text", r"^lang:(long_vectorcall|PyLong_FromUnicodeObject|float_vectorcall|PyFloat_FromString|_PyLong_FromString)"),
    ("language", "str/bytes codecs (encode/decode)", r"^lang:(unicode_encode|bytes_decode|bytearray_decode|PyUnicode_AsEncodedString|PyUnicode_Decode)"),
    ("language", "str methods", r"^lang:(unicode_|ascii_upper_or_lower|split$|rsplit$|PyUnicode_Contains|_PyUnicode_JoinArray|PyUnicode_Append|PyUnicode_Concat|_PyUnicode_)"),
    ("language", "bytes methods", r"^lang:(bytes_|bytearray_|_Py_bytes_)"),
    ("language", "dict methods/ops", r"^lang:(dict_|_PyDict_|PyDict_|dictiter|dictview|dictitems|dictkeys|dictvalues)"),
    ("language", "list/tuple/set ops (incl. sort)", r"^lang:(list_|listiter|_PyList_|tuple|_PyTuple|set_|make_new_set|frozenset|setiter|PySet|PyList|PyTuple|builtin_sorted)"),
    ("language", "other builtins (sum/min/max/any/all/len/dir/vars/iter/next...)", r"^lang:builtin_"),
    ("language", "iteration/generators/coroutines", r"^lang:(gen_|_Py_MakeCoro|coro_|calliter|enum_next|zip_next|map_next|reversed|filter_next|chain_next|range|_PyGen|async_gen)"),
    ("language", "re engine (_sre) from user code", r"^lang:(_sre_|sre_|pattern_|scanner_)"),
    ("language", "exceptions", r"^lang:(BaseException|_PyErr|PyErr|exception)"),
    ("interp (w2-interp)", "attribute/method lookup & object protocol (generic paths, slot_tp_*)", r"^lang:(PyObject_GenericGetAttr|PyObject_GenericSetAttr|_PyObject_GetMethod|_Py_slot_tp_getattr_hook|slot_|_PySuper_Lookup|_PyObject_GenericGetAttrWithDict|PyObject_GetAttr|PyObject_SetAttr|_PyType_Lookup|property_descr|PyObject_GetOptionalAttr|_PyObject_LookupSpecial|do_richcompare|PyObject_RichCompare|PyObject_GetItem|PyObject_Hash|PyObject_Call|_PyObject_Call|type_call|object_new|object_init|tp_)"),
    ("interp (w2-interp)", "call machinery", r"^lang:\(call machinery\)"),
    ("interp (w2-interp)", "bytecode of own code", r"^interp:own"),
    ("interp (w2-interp)", "bytecode of third-party code", r"^interp:(thirdparty|gen3p|other)"),
    ("runtime", "cyclic GC", r"^rt:gc"),
    ("runtime", "allocation/deallocation from bytecode", r"^lang:(_Py_Dealloc|.*_dealloc$|_PyObject_Malloc|_PyObject_Free|_PyObject_Realloc|PyType_GenericAlloc|_PyObject_GC_New|PyObject_GC_Del|_PyMem|subtype_dealloc|PyObject_Malloc|PyObject_Free|_Py_NewReference|mi_|_mi_)"),
    ("runtime", "import system", r"^lang:(PyImport_|_PyImport|import_)"),
    ("thirdparty native", "third-party extensions (C/Cython/mypyc/Rust)", r"^lang:(CPyPy_|CPyDef_|__pyx_|_pysqlite|pysqlite_|_cffi|pydantic_core|_pydantic|ext:|PyInit|cpy_r)"),
    ("stdlib (C)", "stdlib C modules called directly (json/datetime/hashlib/io/socket/...)", r"^lang:"),
    ("other", "stdlib-driven loops (no user frame)", r"^driven:"),
    ("other", "no Python frame", r"^nopy"),
    ("other", "unclassified", r".*"),
]
_RX = [(lay, cand, re.compile(rx)) for lay, cand, rx in RULES]
THIRDPARTY_COMPONENTS = ("ext:", "numpy", "pandas")


def classify(key, comp=None):
    if key.startswith("lang:") and comp and comp.startswith(THIRDPARTY_COMPONENTS):
        return "thirdparty native", "third-party extensions (C/Cython/mypyc/Rust)"
    for lay, cand, rx in _RX:
        if rx.search(key):
            return lay, cand
    return "other", "unclassified"


def load(paths):
    return [json.load(open(p)) for p in paths]


def table(profs):
    fams = sorted({p["family"] for p in profs})
    per = defaultdict(lambda: defaultdict(float))  # (layer, cand) -> workload -> share
    entries = defaultdict(lambda: defaultdict(float))
    for p in profs:
        lc = p.get("lang_component", {})
        for k, v in p["entries"].items():
            lay, cand = classify(k, lc.get(k))
            per[(lay, cand)][p["workload"]] += v
            entries[(lay, cand)][k] += v / len(profs)
    rows = []
    for (lay, cand), d in per.items():
        row = {"layer": lay, "candidate": cand, "all_mean": sum(d.values()) / len(profs)}
        for f in fams:
            ws = [p["workload"] for p in profs if p["family"] == f]
            vals = [(d.get(w, 0.0), w) for w in ws]
            row[f] = (sum(v for v, _ in vals) / len(ws), max(vals))
        row["top_entries"] = sorted(entries[(lay, cand)].items(), key=lambda kv: -kv[1])[:4]
        rows.append(row)
    rows.sort(key=lambda r: (-r["all_mean"]))
    return fams, rows


def main():
    argv = sys.argv[1:]
    md = js = None
    if "--md" in argv:
        i = argv.index("--md"); md = argv[i + 1]; del argv[i:i + 2]
    if "--json" in argv:
        i = argv.index("--json"); js = argv[i + 1]; del argv[i:i + 2]
    profs = load(argv)
    fams, rows = table(profs)
    counts = {f: sum(p["family"] == f for p in profs) for f in fams}
    hdr = "| layer | candidate | all (n=%d) mean | " % len(profs) + " | ".join(
        f"{f} (n={counts[f]}) mean / max" for f in fams) + " | top entries (mean share) |"
    out = [hdr, "|" + "---|" * (3 + len(fams) + 1)]
    for r in rows:
        if r["all_mean"] < 0.0005 and all(r[f][1][0] < 0.005 for f in fams):
            continue
        cells = []
        for f in fams:
            m, (mx, w) = r[f]
            cells.append(f"{100 * m:.2f} / {100 * mx:.1f} ({w})" if mx > 0 else "–")
        top = "; ".join(f"`{k.split(':', 1)[-1][:60]}` {100 * v:.2f}" for k, v in r["top_entries"][:3])
        out.append(f"| {r['layer']} | {r['candidate']} | {100 * r['all_mean']:.2f} | "
                   + " | ".join(cells) + f" | {top} |")
    text = "\n".join(out)
    print(text)
    if md:
        open(md, "w").write(text + "\n")
    if js:
        json.dump([{**{k: v for k, v in r.items() if k not in fams}, **{f: r[f] for f in fams}}
                   for r in rows], open(js, "w"), indent=1)


if __name__ == "__main__":
    main()
