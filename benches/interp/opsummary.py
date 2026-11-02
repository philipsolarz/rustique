#!/usr/bin/env python3
"""Categorize per-opcode profiles (opprof.py record) into interpreter paths.

    python benches/interp/opsummary.py <prof.json>... [--rows 50] [--json out.json]

Every sample lands in one bucket:
  handler self   - the opcode handler itself: dispatch plus the work it inlines
                   (grouped by instruction family: frame push/pop, attribute, call, ...)
  native below   - C code a handler called, by the "deep entry" (first function below the
                   handler that is not generic call plumbing) and whether the chain passes
                   through the C-to-Python call machinery (a nested eval-loop entry).
"""

from __future__ import annotations

import argparse
import collections
import json
import statistics

FRAME_HANDLERS = {
    "CALL_PY_EXACT_ARGS", "CALL_PY_GENERAL", "CALL_BOUND_METHOD_EXACT_ARGS",
    "CALL_BOUND_METHOD_GENERAL", "CALL_KW_PY", "CALL_KW_BOUND_METHOD", "CALL_EX_PY",
    "CALL_ALLOC_AND_ENTER_INIT", "RETURN_VALUE", "RETURN_GENERATOR", "YIELD_VALUE", "RESUME",
    "RESUME_CHECK", "RESUME_CHECK_JIT", "LOAD_ATTR_PROPERTY", "BINARY_OP_SUBSCR_GETITEM",
    "FOR_ITER_GEN", "SEND_GEN", "LOAD_ATTR_GETATTRIBUTE_OVERRIDDEN", "EXIT_INIT_CHECK",
    "INTERPRETER_EXIT", "MAKE_CELL", "COPY_FREE_VARS", "(eval frame)",
}
EXC_HANDLERS = {"exception_unwind", "error", "exit_unwind", "pop_1_error", "pop_2_error",
                "pop_3_error", "pop_4_error", "RAISE_VARARGS", "RERAISE", "PUSH_EXC_INFO",
                "POP_EXCEPT", "CHECK_EXC_MATCH", "CLEANUP_THROW", "WITH_EXCEPT_START",
                "CHECK_EG_MATCH"}


def hgroup(h: str) -> str:
    if h in FRAME_HANDLERS:
        return "frame push/pop + inlined calls"
    if h in EXC_HANDLERS:
        return "exceptions"
    if h.startswith(("LOAD_ATTR", "STORE_ATTR", "LOAD_SUPER_ATTR", "DELETE_ATTR", "LOAD_SPECIAL")):
        return "attribute"
    if h.startswith(("LOAD_GLOBAL", "LOAD_NAME", "STORE_GLOBAL", "STORE_NAME", "LOAD_FROM_DICT")):
        return "globals"
    if h.startswith("CALL") or h == "CALL_FUNCTION_EX":
        return "call to C / generic call"
    if h.startswith(("LOAD_FAST", "STORE_FAST", "LOAD_CONST", "LOAD_SMALL_INT", "POP_TOP", "COPY",
                     "SWAP", "PUSH_NULL", "LOAD_DEREF", "STORE_DEREF", "LOAD_COMMON_CONSTANT",
                     "NOP", "NOT_TAKEN", "END_FOR", "POP_ITER", "DELETE_FAST", "LOAD_CLOSURE",
                     "END_SEND", "POP_BLOCK", "CACHE", "EXTENDED_ARG")):
        return "locals/stack/consts"
    if h.startswith(("TO_BOOL", "POP_JUMP", "COMPARE_OP", "CONTAINS_OP", "IS_OP", "UNARY_NOT",
                     "JUMP")):
        return "compare/branch/bool"
    if h.startswith(("BINARY_OP", "STORE_SUBSCR", "UNARY_", "BINARY_SLICE", "STORE_SLICE",
                     "DELETE_SUBSCR")):
        return "arith/subscript"
    if h.startswith(("BUILD_", "LIST_", "MAP_ADD", "SET_ADD", "DICT_", "UNPACK_", "FORMAT_",
                     "CONVERT_VALUE")):
        return "build/unpack/format"
    if h.startswith(("FOR_ITER", "GET_ITER", "GET_AITER", "GET_ANEXT", "GET_AWAITABLE", "SEND",
                     "GET_YIELD_FROM_ITER", "END_ASYNC_FOR")):
        return "iteration/generators"
    if h.startswith(("MAKE_FUNCTION", "SET_FUNCTION_ATTRIBUTE", "LOAD_BUILD_CLASS")):
        return "make function/class"
    if h.startswith("IMPORT"):
        return "import"
    if h == "(no handler)":
        return "outside the eval loop"
    return "other handlers"


DEALLOC = ("_Py_Dealloc", "subtype_dealloc", "_PyObject_Free", "PyObject_GC_Del",
           "_Py_DecRef", "free_", "_PyMem_", "PyMem_Free", "_PyObject_GC_UNTRACK", "PyObject_Free")


def egroup(e: str, reentry: bool) -> str:
    if reentry:
        return "C->Python call machinery"
    if e.startswith(("_Py_HandlePending", "gc_", "_Py_RunGC", "_PyGC_", "visit_", "deduce_")):
        return "runtime: cyclic GC"
    if e.endswith("_dealloc") or e.startswith(DEALLOC) or e.endswith("_clear") and "dict" not in e:
        return "runtime: dealloc"
    if e.startswith(("_PyObject_Malloc", "PyObject_Malloc", "_PyObject_GC_New", "PyType_GenericAlloc",
                     "_PyType_AllocNoTrack", "_PyObject_New", "gc_alloc", "_PyObject_MallocWithType",
                     "PyObject_GC_", "_PyObject_GC_", "_PyLong_New", "PyTuple_New",
                     "_PyTuple_FromArray", "_PyTuple_FromStackRefStealOnSuccess", "_PyObject_Realloc")):
        return "runtime: alloc"
    if e.startswith(("_PyFrame_ClearExceptCode", "_PyEval_FrameClearAndPop", "clear_thread_frame",
                     "_PyFrame_ClearLocals", "_PyThreadState_PopFrame", "_PyEvalFramePushAndInit",
                     "_PyThreadState_PushFrame", "_PyFrame_Initialize")):
        return "frame push/pop (out of line)"
    if e.startswith("slot_"):
        return "user dunder via type slot (slot_*)"
    if e in ("type_call", "object_new", "object_init", "tp_new_wrapper", "object_vectorcall",
             "type_new", "type_vectorcall") or e.endswith("_vectorcall") and e.startswith(
             ("tuple", "list", "dict", "set", "frozenset", "str", "int", "float", "range", "bool",
              "bytes", "enum", "type")):
        return "instantiation (type_call, builtin type calls)"
    if e.startswith(("method_vectorcall", "partial_", "wrapperdescr_call", "wrapper_call",
                     "PyArg_UnpackTuple", "cfunction_vectorcall", "methoddescr_", "classmethod",
                     "_PyMethod", "PyMethod_New", "meth_", "func_descr_get", "method_get",
                     "_PyStack_UnpackDict", "_PyObject_VectorcallPrepend")):
        return "bound methods / partial / wrappers"
    if e.startswith(("PyObject_GenericGetAttr", "_PyObject_GenericGetAttrWithDict",
                     "_PyObject_GetMethod", "PyObject_GetAttr", "PyObject_GetOptionalAttr",
                     "PyObject_GenericSetAttr", "PyObject_SetAttr", "_PyObject_StoreInstanceAttribute",
                     "_PyType_Lookup", "find_name_in_mro", "assign_version_tag", "super_",
                     "_PySuper_Lookup", "do_super_lookup", "type_getattro", "_Py_type_getattro",
                     "module_getattro", "_Py_module_getattro", "_PyObject_LookupAttr",
                     "_PyObject_SetAttributeErrorContext", "PyObject_GenericGetDict",
                     "_PyObject_LookupSpecial", "property_descr_get", "member_get", "getset_get",
                     "_PyObject_GetInstanceDict", "_PyObject_MaterializeManagedDict",
                     "_PyEval_LoadName", "PyDict_GetItemRef", "_PyEval_LoadGlobal")):
        return "attribute/name lookup (generic)"
    if e.startswith(("object_recursive_isinstance", "object_isinstance", "PyObject_IsInstance",
                     "_PyObject_RealIsInstance", "PyType_IsSubtype", "object_issubclass",
                     "PyObject_IsSubclass", "_abc_", "recursive_isinstance")):
        return "isinstance/issubclass (generic)"
    if e.startswith("builtin_"):
        return "builtin functions"
    if e.startswith(("_PyErr_", "PyErr_", "get_exception_handler", "PyTraceBack_", "_PyTraceBack_",
                     "BaseException_", "PyException", "_PyEval_ExceptionGroup", "tb_",
                     "_PyEval_MatchClass", "_PyEval_FormatExc", "AttributeError_", "KeyError_",
                     "_PyErr_SetKeyError", "_PyEval_MonitorRaise", "PyObject_GetOptionalAttrString")):
        return "exceptions"
    if e.startswith(("do_richcompare", "PyObject_RichCompare", "PySequence_Contains", "PyObject_Hash",
                     "_PySequence_IterSearch", "PyObject_IsTrue")):
        return "compare/contains/truth (generic)"
    if e.startswith(("PyNumber_", "binary_op", "long_", "float_", "_PyLong_", "dec_", "complex_")):
        return "number protocol"
    if e.startswith(("PyObject_Format", "PyUnicode_Format", "PyObject_Repr", "PyObject_Str",
                     "unicode_mod", "_PyUnicode_JoinArray", "_PyUnicodeWriter", "PyUnicodeWriter",
                     "_PyEval_FormatKwargs", "object_format", "format_")):
        return "formatting/str()/repr()"
    if e.startswith(("_PyDict_", "dict_", "PyDict_", "_Py_dict_lookup", "dictiter", "dictview",
                     "_PyDictView")):
        return "dict"
    if e.startswith(("unicode_", "PyUnicode_", "_PyUnicode_", "str_")):
        return "str"
    if e.startswith(("list_", "tuple_", "_PyList_", "PyList_", "_PyTuple_", "PySequence_",
                     "set_", "PySet_", "_PySet_", "listiter", "tupleiter", "frozenset_")):
        return "list/tuple/set"
    if e.startswith(("gen_", "_PyGen_", "chain_", "_PyCoro", "coro_", "async_gen", "zip_",
                     "enum_next", "map_next", "filter_next", "reversed_", "islice", "PyIter_",
                     "calliter", "iter_")):
        return "iteration/generators (C)"
    if e.startswith(("_PyEval_ImportName", "import_", "PyImport_", "_PyImport_")):
        return "import"
    return "other C (extensions, stdlib C modules, ...)"


def summarize(r):
    T = r["total"]
    cats = collections.Counter()
    for h, v in r["handlers"].items():
        cats["self: " + hgroup(h)] += v
    for h, d, reent, v in r["deep"]:
        cats["below: " + egroup(d, reent)] += v
    return {k: 100.0 * v / T for k, v in cats.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--rows", type=int, default=50)
    ap.add_argument("--json")
    a = ap.parse_args()
    profs = [json.load(open(p)) for p in a.files]
    names = [p["workload"] for p in profs]
    sums = [summarize(p) for p in profs]
    keys = sorted({k for s in sums for k in s}, key=lambda k: -statistics.mean(s.get(k, 0) for s in sums))
    print(f"{'bucket':58s} {'mean':>6s} " + " ".join(f"{n[:9]:>9s}" for n in names))
    for k in keys:
        vals = [s.get(k, 0) for s in sums]
        print(f"{k:58s} {statistics.mean(vals):6.2f} " + " ".join(f"{v:9.2f}" for v in vals))
    # top (handler, deep entry) rows, mean over workloads
    rows = collections.defaultdict(lambda: [0.0] * len(profs))
    for i, p in enumerate(profs):
        for h, d, reent, v in p["deep"]:
            rows[(h, d, reent)][i] += 100.0 * v / p["total"]
    print()
    print(f"{'handler -> deep entry [reentry]':70s} {'mean':>6s} {'max':>6s}")
    for k, vals in sorted(rows.items(), key=lambda kv: -statistics.mean(kv[1]))[: a.rows]:
        h, d, reent = k
        print(f"{h + ' -> ' + d + (' [C->Py]' if reent else ''):70s} {statistics.mean(vals):6.2f} {max(vals):6.2f}")
    if a.json:
        json.dump({"workloads": names, "buckets": {k: [s.get(k, 0) for s in sums] for k in keys},
                   "rows": [[*k, v] for k, v in rows.items()]}, open(a.json, "w"))


if __name__ == "__main__":
    main()
