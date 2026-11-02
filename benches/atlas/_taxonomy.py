"""Native-symbol and Python-owner taxonomy of benches/atlas/layers.py (wave 2).

component(symbol, dso) -> one of
  interp:op            an opcode handler of the tail-calling interpreter (_TAIL_CALL_<OP>)
  interp:eval          eval-loop entry/exit, frame push/pop, frame objects
  interp:specialize    the specializer (_Py_Specialize_*)
  interp:pending       eval-breaker work (_Py_HandlePending) that is not GC
  interp:instrument    sys.monitoring / instrumentation
  perf-trampoline      -X perf trampolines (py::...)
  rt:alloc / rt:dealloc / rt:gc / rt:import / rt:ft / rt:startup
  lang:<cat>           object protocol and builtin types (attr, calls, str, bytes, int, float,
                       dict, list/tuple, set, iter, generators, exceptions, abstract, compare,
                       hash, format, builtins, other)
  cmod:<module>        a C stdlib module (re, json, datetime, pickle, collections, ...)
  ext:<pkg>            a third-party native extension (DSO under site-packages)
  rustique             Rustique's extension module
  libc / libm / kernel / unknown
"""

from __future__ import annotations

import json
import os
import re

_SUFFIX_RE = re.compile(r"(\.(cold|warm|hot|isra|constprop|part|lto_priv|llvm|org|localalias)"
                        r"(\.\d+)?)+$|\.\d+$")


def base_name(sym):
    s = sym
    for _ in range(4):
        n = _SUFFIX_RE.sub("", s)
        if n == s:
            break
        s = n
    return s


TAIL = "_TAIL_CALL_"

# ---------------------------------------------------------------------------------------
# rules: first match wins (exact names, then prefixes, then suffixes, then source file)
# ---------------------------------------------------------------------------------------

EXACT = {
    # eval loop / frames
    "_PyEval_EvalFrameDefault": "interp:eval", "_PyEval_Vector": "interp:eval",
    "PyEval_EvalCode": "interp:eval", "_PyEvalFramePushAndInit": "interp:eval",
    "_PyEvalFramePushAndInit_Ex": "interp:eval", "_PyEval_FrameClearAndPop": "interp:eval",
    "clear_thread_frame": "interp:eval", "clear_gen_frame": "interp:eval",
    "_PyThreadState_PushFrame": "interp:eval", "_PyThreadState_PopFrame": "interp:eval",
    "push_chunk": "interp:eval", "_PyFrame_ClearExceptCode": "interp:eval",
    "_PyFrame_ClearLocals": "interp:eval", "_PyEval_EvalFrame": "interp:eval",
    "_Py_HandlePending": "interp:pending", "_PyEval_MakePendingCalls": "interp:pending",
    "_PyEval_GetBuiltin": "interp:eval", "_PyEval_LoadGlobalStackRef": "interp:eval",
    "_PyEval_LoadName": "interp:eval", "_PyEval_UnpackIterableStackRef": "interp:eval",
    "_PyEval_FormatExcCheckArg": "lang:exceptions", "_PyEval_ExceptionGroupMatch": "lang:exceptions",
    "_PyEval_MonitorRaise": "interp:instrument", "_PyEval_SliceIndex": "lang:abstract",
    "_PyEval_BinaryOps": "lang:abstract", "_PyEval_ConversionFuncs": "lang:format",
    "_Py_GetBaseCodeUnit": "interp:eval", "_PyEval_GetFrame": "interp:eval",
    "_PyEval_EnsureBuiltins": "interp:eval", "_PyEval_EnsureBuiltinsWithModule": "interp:eval",
    "_PyEval_CheckExceptStarTypeValid": "lang:exceptions",
    "_PyEval_SpecialMethodCanSuggest": "lang:attr",
    # allocation
    "PyObject_Malloc": "rt:alloc", "_PyObject_Malloc": "rt:alloc", "PyObject_Calloc": "rt:alloc",
    "_PyObject_Calloc": "rt:alloc", "PyObject_Realloc": "rt:alloc", "_PyObject_Realloc": "rt:alloc",
    "PyMem_Malloc": "rt:alloc", "PyMem_Calloc": "rt:alloc", "PyMem_Realloc": "rt:alloc",
    "PyMem_RawMalloc": "rt:alloc", "PyMem_RawCalloc": "rt:alloc", "PyMem_RawRealloc": "rt:alloc",
    "_PyMem_RawMalloc": "rt:alloc", "_PyMem_RawCalloc": "rt:alloc", "_PyMem_RawRealloc": "rt:alloc",
    "_PyMem_MiMalloc": "rt:alloc", "_PyMem_MiCalloc": "rt:alloc", "_PyMem_MiRealloc": "rt:alloc",
    "_PyObject_MiMalloc": "rt:alloc", "_PyObject_MiCalloc": "rt:alloc",
    "_PyObject_MiRealloc": "rt:alloc", "_PyMem_DebugMalloc": "rt:alloc",
    "pymalloc_alloc": "rt:alloc", "pymalloc_realloc": "rt:alloc", "allocate_from_new_pool": "rt:alloc",
    "_PyType_AllocNoTrack": "rt:alloc", "PyType_GenericAlloc": "rt:alloc",
    "_PyObject_GC_New": "rt:alloc", "_PyObject_GC_NewVar": "rt:alloc", "gc_alloc": "rt:alloc",
    "_PyObject_GC_Link": "rt:alloc", "_Py_NewReference": "rt:alloc", "_PyObject_New": "rt:alloc",
    "_PyObject_NewVar": "rt:alloc", "PyObject_Init": "rt:alloc", "PyObject_InitVar": "rt:alloc",
    "_PyObject_GC_Resize": "rt:alloc", "_Py_NewReferenceNoTotal": "rt:alloc",
    "PyType_GenericNew": "rt:alloc", "new_arena": "rt:alloc", "_PyObject_ArenaMmap": "rt:alloc",
    "_PyMem_Strdup": "rt:alloc", "_PyMem_RawStrdup": "rt:alloc",
    "malloc": "rt:alloc", "calloc": "rt:alloc", "realloc": "rt:alloc", "_int_malloc": "rt:alloc",
    "_int_realloc": "rt:alloc", "malloc_consolidate": "rt:alloc", "sysmalloc": "rt:alloc",
    "__libc_malloc": "rt:alloc", "__libc_calloc": "rt:alloc", "__libc_realloc": "rt:alloc",
    "_PyLong_New": "lang:int", "_PyUnicode_New": "lang:str",
    # deallocation
    "_Py_Dealloc": "rt:dealloc", "PyObject_Free": "rt:dealloc", "_PyObject_Free": "rt:dealloc",
    "PyMem_Free": "rt:dealloc", "PyMem_RawFree": "rt:dealloc", "_PyMem_RawFree": "rt:dealloc",
    "_PyMem_MiFree": "rt:dealloc", "_PyObject_MiFree": "rt:dealloc", "pymalloc_free": "rt:dealloc",
    "insert_to_freepool": "rt:dealloc", "insert_to_usedpool": "rt:dealloc",
    "_PyObject_GC_Del": "rt:dealloc", "PyObject_GC_Del": "rt:dealloc",
    "_Py_DecRef": "rt:dealloc", "Py_DecRef": "rt:dealloc", "_Py_IncRef": "rt:dealloc",
    "Py_IncRef": "rt:dealloc", "subtype_dealloc": "rt:dealloc", "clear_slots": "rt:dealloc",
    "_PyObject_ClearManagedDict": "rt:dealloc", "PyObject_ClearWeakRefs": "rt:dealloc",
    "_PyFreeListState_GET": "rt:alloc", "free": "rt:dealloc", "_int_free": "rt:dealloc",
    "__libc_free": "rt:dealloc", "cfree": "rt:dealloc", "_int_free_merge_chunk": "rt:dealloc",
    "_int_free_create_chunk": "rt:dealloc", "_int_free_maybe_consolidate": "rt:dealloc",
    "_PyTrash_thread_deposit_object": "rt:dealloc", "_PyTrash_thread_destroy_chain": "rt:dealloc",
    "_Py_Dealloc_HandleTrashcan": "rt:dealloc", "_PyObject_ClearFreeLists": "rt:dealloc",
    "PyObject_CallFinalizerFromDealloc": "rt:dealloc", "PyObject_CallFinalizer": "rt:dealloc",
    "_PyObject_ResurrectEndSlow": "rt:dealloc", "_Py_ResurrectReference": "rt:dealloc",
    "_PyMem_FreeDelayed": "rt:ft", "_PyObject_FreeDelayed": "rt:ft",
    # free-threading
    "_Py_MergeZeroLocalRefcount": "rt:ft", "_Py_ExplicitMergeRefcount": "rt:ft",
    "_Py_DecRefShared": "rt:ft", "_Py_DecRefSharedDebug": "rt:ft", "_Py_brc_queue_object": "rt:ft",
    "_Py_brc_merge_refcounts": "rt:ft", "_PyObject_SetMaybeWeakref": "rt:ft",
    "_Py_TryIncrefCompare": "rt:ft", "_Py_TryXGetRef": "rt:ft", "_Py_TryIncRefShared": "rt:ft",
    "_PyObject_TryGetInstanceAttribute": "lang:attr",
    # calls
    "_PyObject_MakeTpCall": "lang:calls", "_PyObject_Call": "lang:calls",
    "PyObject_Call": "lang:calls", "PyObject_Vectorcall": "lang:calls",
    "_PyObject_VectorcallTstate": "lang:calls", "_PyFunction_Vectorcall": "lang:calls",
    "PyVectorcall_Call": "lang:calls", "_PyVectorcall_Call": "lang:calls",
    "PyObject_VectorcallMethod": "lang:calls", "PyObject_CallMethod": "lang:calls",
    "PyObject_CallObject": "lang:calls", "PyObject_CallFunction": "lang:calls",
    "PyObject_CallFunctionObjArgs": "lang:calls", "PyObject_CallMethodObjArgs": "lang:calls",
    "PyObject_CallOneArg": "lang:calls", "PyObject_CallNoArgs": "lang:calls",
    "_PyObject_Call_Prepend": "lang:calls", "_PyStack_AsDict": "lang:calls",
    "_PyStack_UnpackDict": "lang:calls", "_PyStack_UnpackDict_Free": "lang:calls",
    "PyObject_VectorcallDict": "lang:calls", "_PyObject_FastCallDictTstate": "lang:calls",
    "_PyObject_CallMethod": "lang:calls", "PyObject_CallMethodNoArgs": "lang:calls",
    "PyObject_CallMethodOneArg": "lang:calls", "_PyObject_VectorcallPrepend": "lang:calls",
    "method_vectorcall": "lang:calls", "slot_tp_call": "lang:calls", "type_call": "lang:calls",
    "type_vectorcall": "lang:calls", "wrapperdescr_call": "lang:calls", "wrapper_call": "lang:calls",
    "_PyEval_CallBuiltinFunction": "lang:calls",
    # attribute / type machinery
    "_PyType_Lookup": "lang:attr", "_PyType_LookupRef": "lang:attr",
    "_PyType_LookupStackRefAndVersion": "lang:attr", "find_name_in_mro": "lang:attr",
    "PyObject_GenericGetAttr": "lang:attr", "_PyObject_GenericGetAttrWithDict": "lang:attr",
    "PyObject_GenericSetAttr": "lang:attr", "_PyObject_GenericSetAttrWithDict": "lang:attr",
    "PyObject_GetAttr": "lang:attr", "PyObject_SetAttr": "lang:attr",
    "PyObject_GetOptionalAttr": "lang:attr", "_PyObject_LookupAttr": "lang:attr",
    "_PyObject_GetMethod": "lang:attr", "_PyObject_GetMethodStackRef": "lang:attr",
    "PyObject_GetAttrString": "lang:attr", "PyObject_HasAttrWithError": "lang:attr",
    "_PyObject_LookupSpecial": "lang:attr", "_PyObject_LookupSpecialMethod": "lang:attr",
    "PyObject_IsInstance": "lang:builtins", "PyObject_IsSubclass": "lang:builtins",
    "object_recursive_isinstance": "lang:builtins", "object_isinstance": "lang:builtins",
    "recursive_isinstance": "lang:builtins", "abstract_issubclass": "lang:builtins",
    "PyType_IsSubtype": "lang:builtins", "_PyObject_RealIsInstance": "lang:builtins",
    "_PyObject_RealIsSubclass": "lang:builtins", "object_issubclass": "lang:builtins",
    "PyObject_RichCompare": "lang:compare", "PyObject_RichCompareBool": "lang:compare",
    "do_richcompare": "lang:compare", "PyObject_Hash": "lang:hash", "_Py_HashBytes": "lang:hash",
    "pysiphash": "lang:hash", "siphash13": "lang:hash", "_Py_HashDouble": "lang:hash",
    "_Py_HashPointer": "lang:hash", "PyObject_IsTrue": "lang:abstract", "PyObject_Not": "lang:abstract",
    "PyObject_Repr": "lang:format", "PyObject_Str": "lang:format", "PyObject_Format": "lang:format",
    "PyObject_ASCII": "lang:format", "PyUnicode_Format": "lang:format",
    "PyObject_GetIter": "lang:iter", "PyIter_Next": "lang:iter", "PyIter_NextItem": "lang:iter",
    "_PyForIter_VirtualIteratorNext": "lang:iter", "PyObject_SelfIter": "lang:iter",
    "PyObject_Size": "lang:abstract", "PyObject_Length": "lang:abstract",
    "PyObject_GetItem": "lang:abstract", "PyObject_SetItem": "lang:abstract",
    "PyObject_DelItem": "lang:abstract",
    # import
    "PyImport_ImportModuleLevelObject": "rt:import", "PyImport_Import": "rt:import",
    "PyImport_ImportModule": "rt:import", "import_find_and_load": "rt:import",
    "PyImport_GetModule": "rt:import", "import_get_module": "rt:import",
    "_PyEval_ImportName": "rt:import", "_PyEval_ImportFrom": "rt:import",
    "import_ensure_initialized": "rt:import", "PyImport_ImportModuleAttr": "rt:import",
    "PyImport_ImportModuleAttrString": "rt:import", "_PyImport_GetModuleAttr": "rt:import",
    "_PyImport_GetModuleAttrString": "rt:import",
    # process entry and the script runner (outer frames)
    "main": "rt:startup", "_start": "libc", "__libc_start_main": "libc",
    "__libc_start_call_main": "libc", "run_mod": "rt:startup", "run_eval_code_obj": "rt:startup",
    "pyrun_file": "rt:startup", "_PyRun_AnyFileObject": "rt:startup",
    "_PyRun_SimpleFileObject": "rt:startup", "_PyRun_SimpleStringFlagsWithName": "rt:startup",
    "_PyRun_SimpleString": "rt:startup", "_PyRun_String": "rt:startup", "_PyRun_File": "rt:startup",
    "PyRun_StringFlags": "rt:startup", "_PyRun_StringFlagsWithName": "rt:startup",
    # libc helpers that just move bytes: attributed to their caller (see TRANSPARENT)
}

PREFIX = [
    ("perf-trampoline", ("py_trampoline_evaluator", "_PyPerfTrampoline", "perf_trampoline",
                         "_PyPerf_", "perf_map_", "trampoline_", "_Py_trampoline")),
    ("rt:ft", ("_PyMutex_", "PyMutex_", "_PyCriticalSection", "PyCriticalSection",
               "_PyParkingLot", "_PySemaphore", "_PyRWMutex", "_PySeqLock", "_PyRecursiveMutex",
               "_PyOnceFlag", "_Py_brc_", "_Py_qsbr_", "_PyEval_StopTheWorld",
               "_PyEval_StartTheWorld", "stop_the_world", "start_the_world", "_Py_yield",
               "tstate_wait_attach", "_PyThreadState_Attach", "_PyThreadState_Detach",
               "_PyThreadState_Suspend", "detach_thread", "_PyMem_QSBR", "_PyObject_SetDeferred",
               "_Py_TryIncref", "_Py_TryXGetStackRef", "_PyRWMutex", "_Py_atomic", "_PyEvent_",
               "PyEvent_", "_PyLock", "_PyIndexPool", "_Py_ReserveTLBCIndex",
               "_PyCode_GetTLBC", "get_tlbc", "create_tlbc", "_PyMem_FreeDelayed",
               "_PyObject_FreeDelayed", "free_work_item", "_Py_MergeZero", "_Py_ExplicitMerge",
               "_Py_DecRefShared", "_Py_IncRefShared")),
    ("rt:locks", ("_PyEval_IsGILEnabled", "take_gil", "drop_gil", "_PyEval_ReleaseLock",
                  "_PyEval_AcquireLock", "PyEval_SaveThread", "PyEval_RestoreThread",
                  "PyThread_acquire_lock", "PyThread_release_lock", "_PyThread_", "pthread_mutex",
                  "pthread_cond", "__pthread_mutex", "__lll_lock", "__lll_unlock", "futex",
                  "__futex", "sem_", "__new_sem", "lock_PyThread", "acquire_timed")),
    ("interp:specialize", ("_Py_Specialize", "specialize", "_PyCode_Quicken", "_Py_GatherStats")),
    ("interp:instrument", ("_Py_call_instrumentation", "_Py_Instrument", "_Py_Monitoring",
                           "call_instrumentation_vector", "_PyMonitoring", "sys_monitoring",
                           "instrument", "de_instrument", "call_one_instrument")),
    ("interp:eval", ("_PyEval_", "_PyFrame_", "_PyStackRef", "PyStackRef", "_PyCode_",
                     "_PyJIT", "_PyOptimizer", "_PyUOp", "_Py_ExecutorInit")),
    ("rt:gc", ("gc_", "_PyGC_", "PyGC_", "update_refs", "subtract_refs", "move_unreachable",
               "move_legacy_finalizers", "deduce_unreachable", "delete_garbage",
               "finalize_garbage", "handle_weakrefs", "visit_decref", "visit_reachable",
               "visit_move", "untrack_tuples", "untrack_dicts", "validate_list",
               "invoke_gc_callback", "_Py_RunGC", "_Py_ScheduleGC", "mark_", "scan_heap",
               "gc_visit", "_PyMem_ProcessDelayed", "_Py_qsbr", "move_legacy_finalizer",
               "expand_region", "completed_cycle", "visit_add_to_container",
               "move_to_reachable", "_PyGC_VisitStackRef", "_PyGC_VisitFrameStack",
               "gc_collect_", "gc_mark", "gc_propagate", "mark_stacks", "mark_global_roots",
               "mark_at_start", "assess_work_to_do", "move_all_transitively")),
    ("rt:dealloc", ("mi_free", "_mi_free", "mi_page_free", "_mi_page_free", "_PyFreeList_Free",
                    "_Py_FreeList", "_PyFreeList_Push", "_Py_Dealloc", "_PyObject_GC_UNTRACK",
                    "PyObject_GC_UnTrack", "_PyTrash", "free_keys_object", "free_values",
                    "_Py_DecRef", "_PyMem_Free")),
    ("rt:alloc", ("mi_malloc", "_mi_malloc", "mi_heap_malloc", "mi_heap_alloc", "_mi_heap_malloc",
                  "mi_zalloc", "_mi_page_malloc", "mi_page_", "_mi_page_", "mi_segment_",
                  "_mi_segment_", "mi_calloc", "mi_realloc", "_mi_heap_realloc", "mi_heap_realloc",
                  "mi_find_page", "mi_page_queue", "_mi_os_", "mi_os_", "_mi_arena", "mi_arena",
                  "_mi_heap_", "mi_heap_", "mi_large", "mi_huge", "_mi_bin", "mi_bin",
                  "_PyFreeList_Pop", "_Py_FreeListPop", "_PyObject_GC_New", "pymalloc_",
                  "_PyMem_Arena", "_PyObject_Arena", "_mi_", "mi_")),
    ("rt:import", ("_PyImport_", "PyImport_", "import_", "PyMarshal_", "_PyMarshal_", "r_object",
                   "r_ref", "r_string", "r_long", "r_short", "r_byte", "r_float", "read_object",
                   "marshal_", "_imp_", "imp_", "_PyImportZip", "exec_builtin_or_dynamic",
                   "_PyImport_LoadDynamicModule", "_Py_ext_module_loader", "_PyModule_FromDef",
                   "PyModule_FromDefAndSpec", "PyModule_ExecDef", "fix_up_extension")),
    ("rt:startup", ("Py_Initialize", "pyinit_", "pycore_", "_PyRuntime_Initialize",
                    "Py_InitializeFromConfig", "pymain_", "Py_RunMain", "Py_BytesMain",
                    "Py_Finalize", "_Py_Finalize", "finalize_", "Py_FinalizeEx", "_PyConfig_",
                    "PyConfig_", "_PyPreConfig", "_PyPathConfig", "init_interp_main",
                    "_PyStaticType_", "_PyStructSequence_InitBuiltin", "_PyTypes_Init",
                    "_PyBuiltin_Init", "_PySys_Create", "_PyInterpreterState_New",
                    "_PyAtExit_", "atexit_")),
    ("lang:calls", ("_PyObject_Vectorcall", "PyObject_Vectorcall", "_PyFunction_Vectorcall",
                    "method_vectorcall", "cfunction_", "PyObject_Call", "_PyObject_Call",
                    "_PyObject_MakeTpCall", "_Py_VectorCall", "PyVectorcall", "_PyVectorcall",
                    "_PyCFunction", "PyCFunction", "vgetargs", "_PyArg_", "PyArg_", "do_mkvalue",
                    "Py_BuildValue", "_Py_BuildValue", "va_build_value", "do_mktuple",
                    "do_mkdict", "converttuple", "convertitem", "convertsimple", "seterror",
                    "vgetargskeywords", "methoddescr_", "classmethoddescr_", "vectorcall_",
                    "_PyEval_Call", "wrapperdescr_", "wrapper_", "builtin_vectorcall",
                    "func_descr_get", "method_new", "PyMethod_New", "meth_get", "meth_dealloc",
                    "partial_vectorcall", "partial_call", "_PyArgv_", "find_keyword",
                    "_PyFunction_", "PyFunction_", "_PyStack_")),
    ("lang:attr", ("_PyType_Lookup", "PyObject_GenericGetAttr", "_PyObject_GenericGetAttr",
                   "PyObject_GenericSetAttr", "_PyObject_GenericSetAttr", "_PyObject_GetMethod",
                   "slot_tp_", "slot_", "type_", "PyType_", "_PyType_", "PyObject_GetAttr",
                   "PyObject_SetAttr", "_PyObject_LookupAttr", "PyObject_GetOptionalAttr",
                   "find_name_in_mro", "property_", "member_", "getset_", "descr_",
                   "PyDescr_", "_PyObject_GetInstanceAttribute", "_PyObject_StoreInstanceAttribute",
                   "_PyObject_InitInlineValues", "_PyObject_MaterializeManagedDict",
                   "_PyObject_GetDictPtr", "_PyObject_ComputedDictPointer", "super_",
                   "PyMember_", "module_getattro", "_Py_module_getattro", "module_setattro",
                   "_PyModule_", "PyModule_GetDict", "lookup_maybe_method", "lookup_method",
                   "vectorcall_method", "call_unbound", "maybe_call_special",
                   "_PyObject_LookupSpecial", "_PySuper_", "classmethod_", "sm_", "cm_",
                   "object_", "_PyObject_IsAbstract", "PyObject_SelfIter")),
    ("lang:str", ("unicode_", "PyUnicode", "_PyUnicode", "ucs1lib_", "ucs2lib_", "ucs4lib_",
                  "asciilib_", "_PyUnicodeWriter", "PyUnicodeWriter", "stringlib_", "_Py_str",
                  "_PyStr", "fastsearch", "default_find", "anylib_", "utf8_", "ascii_decode",
                  "_Py_DecodeUTF8", "_Py_EncodeUTF8", "find_maxchar_surrogates",
                  "resize_compact", "_copy_characters", "do_upper_or_lower", "do_casefold",
                  "do_title", "do_capitalize", "do_swapcase", "do_strip", "split_whitespace",
                  "tailmatch", "any_find_slice", "_PyLong_FormatWriter",
                  "unicodeiter_", "striter_", "formatteriter", "fieldnameiter", "MarkupIterator",
                  "parse_field", "field_name_split", "output_markup", "build_string",
                  "render_field", "get_field_object", "do_string_format", "do_markup")),
    ("lang:format", ("format_", "_PyUnicode_FormatAdvancedWriter", "_PyLong_FormatAdvancedWriter",
                     "_PyFloat_FormatAdvancedWriter", "parse_internal_render_format_spec",
                     "calc_number_widths", "fill_number", "get_integer", "format_obj",
                     "format_string_internal", "format_long_internal", "format_float_internal",
                     "PyOS_double_to_string", "format_float_short", "_Py_dg_", "_PyLong_Format",
                     "long_to_decimal_string", "long_format_binary", "unicode_format_arg",
                     "mainformatlong", "formatlong", "formatfloat", "formatchar")),
    ("lang:bytes", ("bytes", "bytearray", "_Py_bytes_", "PyBytes", "_PyBytes", "PyByteArray",
                    "_PyByteArray", "memory_", "PyMemoryView", "PyBuffer", "_PyBuffer",
                    "PyObject_GetBuffer", "buffer_")),
    ("lang:int", ("long_", "PyLong", "_PyLong", "k_mul", "x_divrem", "x_add", "x_sub",
                  "k_lopsided_mul", "bool_", "PyBool", "l_divmod", "l_mod", "fast_mod",
                  "fast_floor_div", "_PyBool")),
    ("lang:float", ("float_", "PyFloat", "_PyFloat", "PyOS_string_to_double", "_Py_parse_inf",
                    "complex_", "PyComplex")),
    ("lang:dict", ("dict_", "PyDict", "_PyDict", "insertdict", "dictkeys", "dictiter", "dictitems",
                   "dictview", "dictvalues", "dictresize", "_Py_dict_lookup", "unicodekeys_lookup",
                   "lookdict", "new_keys_object", "insert_into_", "insert_split", "build_indices",
                   "dictitems_", "dictkeys_", "dictvalues_", "dict_items", "do_lookup",
                   "compare_unicode", "compare_generic", "unicodekeys_", "insert_combined",
                   "insert_to_emptydict", "setitem_take2", "setitem_lock_held", "delitem_common",
                   "_PyObjectDict_", "make_dict_from_instance_attributes", "new_dict",
                   "copy_values", "clone_combined_dict_keys", "dict_merge", "merge_from_seq2",
                   "odict", "PyODict", "_PyODict", "mappingproxy_", "PyDictProxy", "frozendict")),
    ("lang:list/tuple", ("list_", "PyList", "_PyList", "listiter", "listreviter", "tuple",
                         "PyTuple", "_PyTuple", "unsafe_object_compare", "unsafe_latin_compare",
                         "unsafe_long_compare", "unsafe_float_compare", "unsafe_tuple_compare",
                         "safe_object_compare", "binarysort", "count_run", "merge_", "gallop_",
                         "_PyList_", "ins1", "app1", "list_resize", "list_ass_", "listview")),
    ("lang:set", ("set_", "PySet", "_PySet", "frozenset", "setiter", "set_lookkey",
                  "set_add_entry", "set_table_resize", "make_new_set")),
    ("lang:iter", ("iter_", "PyIter", "_PyIter", "PySeqIter", "seqiter_", "calliter_",
                   "enum_", "reversed_", "range_", "rangeiter_", "longrangeiter_", "PyRange",
                   "_PyRange", "slice_", "PySlice", "_PySlice", "zip_", "map_", "filter_",
                   "anextawaitable", "builtin_next", "builtin_iter", "make_range_object",
                   "compute_range_length", "PySeqIter_New")),
    ("lang:generators", ("gen_", "_PyGen_", "PyGen_", "coro_", "_PyCoro_", "PyCoro_",
                         "async_gen", "_PyAsyncGen", "compute_cr_origin", "gen_send_ex",
                         "_PyGen_FetchStopIterationValue", "gen_iternext", "gen_close",
                         "_gen_throw", "make_gen")),
    ("lang:exceptions", ("_PyErr_", "PyErr_", "BaseException", "_PyException", "PyException",
                         "_PyTraceBack", "PyTraceBack", "tb_", "_Py_CheckRecursiveCall",
                         "_Py_EnterRecursive", "_Py_LeaveRecursive", "OSError_", "oserror_",
                         "StopIteration_", "KeyError_", "SystemExit_", "AttributeError_",
                         "NameError_", "ImportError_", "_PyEval_FormatAwaitableError",
                         "format_exc_check_arg", "_Py_Offer_Suggestions", "_PyFrame_GetLine",
                         "PyFrame_GetLineNumber", "_PyCode_CheckLineNumber", "PyCode_Addr2Line",
                         "_PyCode_InitAddressRange", "_PyLineTable_", "exception_group",
                         "BaseExceptionGroup", "_PyExc_", "PyTraceback")),
    ("lang:compare", ("do_richcompare", "PyObject_RichCompare", "_PyObject_RichCompare")),
    ("lang:hash", ("pysiphash", "siphash", "_Py_Hash", "PyObject_Hash", "_Py_HashSecret")),
    ("lang:abstract", ("PyNumber_", "_PyNumber_", "binary_op", "ternary_op", "binary_iop",
                       "PySequence_", "_PySequence_", "PyMapping_", "_PyMapping_",
                       "PyObject_GetItem", "PyObject_SetItem", "PyObject_DelItem", "PyObject_Size",
                       "PyObject_Length", "PyObject_IsTrue", "PyObject_Not", "_PyObject_GetItem",
                       "PyObject_Bytes", "PyIndex_", "_PyIndex", "PyObject_LengthHint",
                       "_PyObject_HasLen", "PyObject_Type", "_PyObject_LookupAttrId",
                       "_PyObject_IsFreed", "PyObject_Dir", "PyObject_Print", "_Py_Check",
                       "_PyEval_SliceIndex", "PyObject_ClearWeakRefs")),
    ("lang:builtins", ("builtin_", "min_max", "_PyBuiltin_", "bltinmodule", "PyEval_GetBuiltins",
                       "_PyIntrinsics_", "intrinsic_", "_Py_Intrinsic", "print_",
                       "_PyObject_IsInstance", "object_isinstance", "object_issubclass",
                       "recursive_isinstance", "_PyContextVar", "PyContextVar")),
]

SUFFIX = [("rt:dealloc", ("dealloc", "_finalize")), ("rt:gc", ("_traverse",))]

# source file -> component (via an index of CPython's sources, function name -> file)
FILE_RULES = [
    ("Objects/unicodeobject.c", "lang:str"), ("Objects/unicodectype.c", "lang:str"),
    ("Objects/unicode_", "lang:str"), ("Objects/stringlib/", "lang:str"),
    ("Python/formatter_unicode.c", "lang:format"), ("Python/codecs.c", "lang:str"),
    ("Modules/_codecsmodule.c", "lang:str"), ("Objects/bytesobject.c", "lang:bytes"),
    ("Objects/bytearrayobject.c", "lang:bytes"), ("Objects/bytes_methods.c", "lang:bytes"),
    ("Objects/memoryobject.c", "lang:bytes"), ("Objects/longobject.c", "lang:int"),
    ("Python/mystrtoul.c", "lang:int"), ("Objects/boolobject.c", "lang:int"),
    ("Objects/floatobject.c", "lang:float"), ("Python/dtoa.c", "lang:format"),
    ("Python/pystrtod.c", "lang:float"), ("Objects/complexobject.c", "lang:float"),
    ("Objects/dictobject.c", "lang:dict"), ("Objects/odictobject.c", "lang:dict"),
    ("Objects/listobject.c", "lang:list/tuple"), ("Objects/tupleobject.c", "lang:list/tuple"),
    ("Objects/setobject.c", "lang:set"), ("Objects/typeobject.c", "lang:attr"),
    ("Objects/descrobject.c", "lang:attr"), ("Objects/object.c", "lang:attr"),
    ("Objects/moduleobject.c", "lang:attr"), ("Objects/namespaceobject.c", "lang:attr"),
    ("Objects/call.c", "lang:calls"), ("Objects/methodobject.c", "lang:calls"),
    ("Objects/funcobject.c", "lang:calls"), ("Objects/classobject.c", "lang:calls"),
    ("Python/getargs.c", "lang:calls"), ("Python/modsupport.c", "lang:calls"),
    ("Objects/obmalloc.c", "rt:alloc"), ("Objects/mimalloc/", "rt:alloc"),
    ("Python/gc", "rt:gc"), ("Python/qsbr.c", "rt:ft"), ("Python/brc.c", "rt:ft"),
    ("Python/lock.c", "rt:ft"), ("Python/parking_lot.c", "rt:ft"),
    ("Python/critical_section.c", "rt:ft"), ("Python/uniqueid.c", "rt:ft"),
    ("Python/index_pool.c", "rt:ft"), ("Python/thread", "rt:ft"), ("Python/ceval_gil.c", "rt:ft"),
    ("Python/ceval", "interp:eval"), ("Python/generated_cases", "interp:eval"),
    ("Python/specialize.c", "interp:specialize"), ("Python/frame.c", "interp:eval"),
    ("Python/bytecodes.c", "interp:eval"), ("Objects/frameobject.c", "interp:eval"),
    ("Objects/codeobject.c", "interp:eval"), ("Python/instrumentation.c", "interp:instrument"),
    ("Python/legacy_tracing.c", "interp:instrument"), ("Python/optimizer", "interp:eval"),
    ("Python/jit", "interp:eval"),
    ("Python/perf_trampoline.c", "perf-trampoline"), ("Python/perf_jit", "perf-trampoline"),
    ("Objects/abstract.c", "lang:abstract"), ("Objects/genobject.c", "lang:generators"),
    ("Objects/exceptions.c", "lang:exceptions"), ("Python/errors.c", "lang:exceptions"),
    ("Python/traceback.c", "lang:exceptions"), ("Python/suggestions.c", "lang:exceptions"),
    ("Objects/iterobject.c", "lang:iter"), ("Objects/enumobject.c", "lang:iter"),
    ("Objects/rangeobject.c", "lang:iter"), ("Objects/sliceobject.c", "lang:iter"),
    ("Objects/cellobject.c", "interp:eval"), ("Objects/weakrefobject.c", "lang:other"),
    ("Objects/capsule.c", "lang:other"), ("Objects/structseq.c", "lang:list/tuple"),
    ("Objects/unionobject.c", "lang:other"), ("Objects/genericaliasobject.c", "lang:other"),
    ("Objects/templateobject.c", "lang:format"), ("Objects/interpolationobject.c", "lang:format"),
    ("Objects/picklebufobject.c", "lang:bytes"), ("Objects/lazyimportobject.c", "rt:import"),
    ("Python/bltinmodule.c", "lang:builtins"), ("Python/intrinsics.c", "lang:builtins"),
    ("Python/context.c", "cmod:contextvars"), ("Python/hamt.c", "cmod:contextvars"),
    ("Python/import.c", "rt:import"), ("Python/marshal.c", "rt:import"),
    ("Python/importdl.c", "rt:import"), ("Python/pylifecycle.c", "rt:startup"),
    ("Python/pystate.c", "rt:startup"), ("Python/initconfig.c", "rt:startup"),
    ("Python/preconfig.c", "rt:startup"), ("Python/pathconfig.c", "rt:startup"),
    ("Python/sysmodule.c", "cmod:sys"), ("Python/compile.c", "cmod:compile"),
    ("Python/codegen.c", "cmod:compile"), ("Python/flowgraph.c", "cmod:compile"),
    ("Python/assemble.c", "cmod:compile"), ("Python/symtable.c", "cmod:compile"),
    ("Python/ast", "cmod:compile"), ("Python/Python-ast.c", "cmod:ast"),
    ("Python/Python-tokenize.c", "cmod:tokenize"), ("Parser/", "cmod:compile"),
    ("Python/pythonrun.c", "cmod:compile"), ("Python/future.c", "cmod:compile"),
    ("Python/_warnings.c", "cmod:warnings"), ("Python/hashtable.c", "rt:startup"),
    ("Python/fileutils.c", "cmod:os"), ("Python/pytime.c", "cmod:time"),
    ("Python/bootstrap_hash.c", "cmod:os"), ("Python/structmember.c", "lang:attr"),
    ("Python/pyhash.c", "lang:hash"), ("Python/pymath.c", "lang:float"),
    ("Python/pyctype.c", "lang:str"), ("Python/getversion.c", "rt:startup"),
    ("Python/pystrcmp.c", "lang:str"), ("Python/pystrhex.c", "lang:bytes"),
    ("Python/crossinterp", "cmod:interpreters"), ("Python/remote_debug", "rt:startup"),
    ("Python/stackrefs.c", "interp:eval"), ("Python/tracemalloc.c", "cmod:tracemalloc"),
    ("Python/dynamic_annotations.c", "rt:ft"), ("Python/sysmodule", "cmod:sys"),
    ("Modules/_sre/", "cmod:re"), ("Modules/_json.c", "cmod:json"),
    ("Modules/_pickle.c", "cmod:pickle"), ("Modules/_datetimemodule.c", "cmod:datetime"),
    ("Modules/_zoneinfo.c", "cmod:zoneinfo"), ("Modules/_decimal/", "cmod:decimal"),
    ("Modules/zlibmodule.c", "cmod:zlib"), ("Modules/_hashopenssl.c", "cmod:hashlib"),
    ("Modules/_hacl/", "cmod:hashlib"), ("Modules/sha", "cmod:hashlib"),
    ("Modules/md5module.c", "cmod:hashlib"), ("Modules/blake2module.c", "cmod:hashlib"),
    ("Modules/hmacmodule.c", "cmod:hashlib"), ("Modules/expat/", "cmod:expat"),
    ("Modules/pyexpat.c", "cmod:expat"), ("Modules/_elementtree.c", "cmod:xml.etree"),
    ("Modules/_csv.c", "cmod:csv"), ("Modules/_io/", "cmod:io"), ("Modules/posixmodule.c", "cmod:os"),
    ("Modules/_stat.c", "cmod:os"), ("Modules/unicodedata", "cmod:unicodedata"),
    ("Modules/_collectionsmodule.c", "cmod:collections"),
    ("Modules/itertoolsmodule.c", "cmod:itertools"), ("Modules/_functoolsmodule.c", "cmod:functools"),
    ("Modules/_operator.c", "cmod:operator"), ("Modules/_randommodule.c", "cmod:random"),
    ("Modules/mathmodule.c", "cmod:math"), ("Modules/cmathmodule.c", "cmod:math"),
    ("Modules/_struct.c", "cmod:struct"), ("Modules/binascii.c", "cmod:binascii"),
    ("Modules/_bisectmodule.c", "cmod:bisect"), ("Modules/_heapqmodule.c", "cmod:heapq"),
    ("Modules/_statisticsmodule.c", "cmod:statistics"), ("Modules/timemodule.c", "cmod:time"),
    ("Modules/_uuidmodule.c", "cmod:uuid"), ("Modules/_abc.c", "cmod:abc"),
    ("Modules/_bz2module.c", "cmod:bz2"), ("Modules/_lzmamodule.c", "cmod:lzma"),
    ("Modules/_zstd/", "cmod:zstd"), ("Modules/_lsprof.c", "cmod:profile"),
    ("Modules/_tracemalloc.c", "cmod:tracemalloc"), ("Modules/_asynciomodule.c", "cmod:asyncio"),
    ("Modules/_threadmodule.c", "cmod:threading"), ("Modules/socketmodule.c", "cmod:socket"),
    ("Modules/selectmodule.c", "cmod:selectors"), ("Modules/_ssl", "cmod:ssl"),
    ("Modules/signalmodule.c", "cmod:signal"), ("Modules/_sqlite/", "cmod:sqlite3"),
    ("Modules/arraymodule.c", "cmod:array"), ("Modules/_localemodule.c", "cmod:locale"),
    ("Modules/_weakref.c", "lang:other"), ("Modules/gcmodule.c", "rt:gc"),
    ("Modules/_contextvarsmodule.c", "cmod:contextvars"), ("Modules/_queuemodule.c", "cmod:queue"),
    ("Modules/_ctypes/", "cmod:ctypes"), ("Modules/_posixsubprocess.c", "cmod:subprocess"),
    ("Modules/fcntlmodule.c", "cmod:os"), ("Modules/resource.c", "cmod:os"),
    ("Modules/_string", "lang:format"), ("Modules/atexitmodule.c", "rt:startup"),
    ("Modules/main.c", "rt:startup"), ("Modules/getpath.c", "rt:startup"),
    ("Modules/faulthandler.c", "rt:startup"), ("Modules/_typingmodule.c", "cmod:typing"),
    ("Modules/_opcode.c", "cmod:dis"), ("Modules/_interp", "cmod:interpreters"),
    ("Modules/_suggestions.c", "lang:exceptions"), ("Modules/_tokenize", "cmod:tokenize"),
    ("Modules/_remote_debugging", "cmod:profiling"), ("Modules/_testinternalcapi", "cmod:test"),
    ("Modules/_struct", "cmod:struct"), ("Modules/_codecs", "lang:str"),
    ("Modules/_multiprocessing/", "cmod:multiprocessing"), ("Modules/mmapmodule.c", "cmod:mmap"),
    ("Modules/_csv", "cmod:csv"), ("Modules/_pickle", "cmod:pickle"),
    ("Python/", "rt:startup"), ("Objects/", "lang:other"),
]

LIB_PREFIX = [
    ("cmod:expat", ("normal_", "little2_", "big2_", "XmlInitEncoding", "XmlUtf8", "XmlParse",
                    "XmlPrologState", "Xml", "XML_", "initScan", "unknown_isName",
                    "unknown_toUtf8", "latin1_toUtf8", "ascii_toUtf8", "utf8_toUtf8",
                    "checkCharRefNumber", "doContent", "doProlog", "storeAtts", "lookup",
                    "poolGrow", "poolAppend", "poolStoreString", "contentProcessor",
                    "prologProcessor", "epilogProcessor", "reportDefault", "addBinding",
                    "hashTableIterNext", "keyeq", "hash", "storeRawNames", "processXmlDecl",
                    "getAttributeId", "normalizePublicId", "callStoreEntityValue")),
    ("cmod:zlib", ("deflate", "inflate", "crc32", "adler32", "longest_match", "fill_window",
                   "compress_block", "_tr_", "zng_", "slide_hash", "zlib_", "updatewindow",
                   "chunkmemset", "chunkcopy", "build_tree", "pqdownheap", "gen_bitlen",
                   "send_tree", "scan_tree", "zcalloc", "zcfree", "compare256", "functable",
                   "emit_", "insert_string", "quick_insert", "update_hash", "flush_pending",
                   "read_buf", "bi_", "send_bits", "detect_data_type", "chunksize",
                   "inflate_table", "fold_", "crc_", "adler_", "PREFIX(")),
    ("cmod:hashlib", ("EVP_", "evp_", "ossl_", "OPENSSL_", "CRYPTO_", "SHA1", "SHA256", "SHA512",
                      "sha1_", "sha256_", "sha512_", "MD5", "md5_", "Hacl_", "python_hashlib_",
                      "KECCAK", "keccak", "SHA3", "ERR_", "OBJ_", "BLAKE2", "blake2", "EVPXOF",
                      "md_", "digest_", "ossl", "openssl", "HMAC", "hmac")),
    ("cmod:decimal", ("mpd_", "_mpd_", "libmpdec")),
    ("cmod:re", ("sre_", "_sre_", "SRE_", "sre_ucs", "state_init", "state_fini", "getstring")),
    ("cmod:bz2", ("BZ2_",)), ("cmod:lzma", ("lzma_", "lz_")), ("cmod:zstd", ("ZSTD_", "HUF_", "FSE_")),
    ("cmod:ctypes", ("ffi_",)), ("cmod:sqlite3", ("sqlite3",)),
    ("libm", ("__ieee754_", "__exp", "__log", "__pow", "__sin", "__cos", "__fma")),
]

# leaf functions that do not decide the component: the caller does (memcpy under list_resize
# is list work). malloc/free are matched first (EXACT) and are not transparent.
TRANSPARENT_PREFIX = ("__memcpy", "__memmove", "__memset", "memcpy", "memmove", "memset",
                      "__strlen", "strlen", "__memcmp", "memcmp", "__strcmp", "strcmp",
                      "__strchr", "strchr", "__memchr", "memchr", "__wmemset", "__nss",
                      "__mempcpy", "mempcpy", "__stpcpy", "__strcpy", "strcpy", "__strncmp",
                      "__memrchr", "__rawmemchr", "__strnlen", "__wcslen", "_dl_", "__tls_get_addr",
                      "__stack_chk_fail", "_Py_bswap", "__x86_", "__GI_", "_IO_")
TRANSPARENT_COMPONENTS = {"libc", "kernel", "unknown", "libm-transparent"}

STARTUP_FRAMES = ("Py_InitializeFromConfig", "pyinit_main", "pyinit_core", "pymain_init",
                  "_PyRuntime_Initialize", "Py_Initialize", "init_interp_main", "pycore_interp_init")
FINALIZE_FRAMES = ("Py_FinalizeEx", "_Py_Finalize", "Py_Finalize", "finalize_modules",
                   "finalize_interp_clear", "_PyGC_CollectNoFail", "Py_ExitStatusException",
                   "pymain_free", "_PyImport_Fini", "finalize_interp_delete", "wait_for_thread_shutdown",
                   "_PyAtExit_Call", "atexit_callfuncs")
GC_FRAMES = ("gc_collect_main", "_PyGC_Collect", "_Py_RunGC", "gc_collect_increment",
             "gc_collect_full", "gc_collect_young", "gc_collect_region", "gc_collect_internal",
             "gc_collect", "collect_with_callback", "PyGC_Collect", "gc_collect_impl",
             "_PyGC_CollectNoFail", "gc_collect_gen", "deduce_unreachable_heap")
IMPORT_FRAMES = ("import_find_and_load", "PyImport_ImportModuleLevelObject",
                 "_PyEval_ImportName", "PyImport_Import", "PyImport_ImportModule",
                 "_PyImport_LoadDynamicModuleWithSpec", "PyMarshal_ReadObjectFromString",
                 "marshal_loads", "_imp_exec_builtin", "_imp_create_builtin",
                 "_imp_exec_dynamic", "_imp_create_dynamic")
MODULE_BODY_FRAMES = ("builtin_exec", "PyEval_EvalCode", "exec_code_in_module")
DEALLOC_CHAIN = ("_Py_Dealloc", "subtype_dealloc", "_PyObject_GC_Del", "PyObject_Free",
                 "_PyTrash_thread_destroy_chain")
CALL_WRAPPERS = ("type_call", "_PyObject_MakeTpCall", "PyObject_Call", "_PyObject_Call",
                 "PyObject_Vectorcall", "_PyObject_Vectorcall", "vectorcall_", "slot_tp_call",
                 "wrapperdescr_call", "wrapper_call", "method_vectorcall", "cfunction_",
                 "_PyFunction_Vectorcall", "PyVectorcall", "_Py_VectorCall", "methoddescr_",
                 "classmethoddescr_", "builtin_vectorcall", "_PyEval_Call", "_PyVectorcall",
                 "PyObject_CallOneArg", "PyObject_CallNoArgs", "_PyObject_VectorcallTstate",
                 "PyObject_VectorcallMethod", "_PyObject_Call_Prepend", "_PyEval_Vector",
                 "PyObject_CallMethod", "_PyObject_CallMethod", "PyObject_CallFunction",
                 "PyObject_CallObject", "_PyObject_FastCallDictTstate", "PyObject_VectorcallDict",
                 "_PyEval_CallBuiltinFunction")


def src_index(cpython_src, cache_path):
    if cache_path and os.path.exists(cache_path):
        with open(cache_path) as f:
            return json.load(f)
    defre = re.compile(r"^(?:[A-Za-z_][\w \*]*\s\**)?([A-Za-z_]\w*)\s*\(")
    index = {}
    for top in ("Objects", "Python", "Modules", "Parser"):
        for dp, _dn, fns in os.walk(os.path.join(cpython_src, top)):
            for fn in fns:
                if not fn.endswith((".c", ".h")):
                    continue
                full = os.path.join(dp, fn)
                rel = os.path.relpath(full, cpython_src)
                if "/clinic/" in rel:
                    rel = rel.replace("/clinic/", "/").removesuffix(".h")
                if "Modules/_test" in rel:
                    continue
                try:
                    lines = open(full, encoding="utf-8", errors="replace").read().splitlines()
                except OSError:
                    continue
                for ln in lines:
                    if not ln or ln[0] in " \t#/{}*" or ln.startswith(("return", "if", "else")):
                        continue
                    m = defre.match(ln)
                    if m and not ln.rstrip().endswith(";"):
                        index.setdefault(m.group(1), rel)
    if cache_path:
        os.makedirs(os.path.dirname(cache_path), exist_ok=True)
        with open(cache_path, "w") as f:
            json.dump(index, f)
    return index


class Components:
    def __init__(self, cpython_src, cache_path):
        self.index = src_index(cpython_src, cache_path)
        self.cache = {}

    def of(self, sym, dso):
        key = (sym, dso)
        c = self.cache.get(key)
        if c is None:
            c = self.cache[key] = self._of(sym, dso)
        return c

    def _of(self, sym, dso):
        dbase = os.path.basename(dso or "")
        if dso == "[kernel]":
            return "kernel"
        if dso == "[perf-map]" or (sym or "").startswith("py::"):
            return "perf-trampoline"
        fam = self._dso_family(dbase, dso)
        if sym is None:
            if fam:
                return fam
            if dbase.startswith(("libc.", "ld-linux", "libpthread")):
                return "libc"
            return "unknown"
        b = base_name(sym)
        if b.startswith(TAIL):
            return "interp:op"
        if fam and fam.startswith(("ext:", "rustique")):
            return fam
        c = EXACT.get(b)
        if c:
            return c
        if b.startswith(TRANSPARENT_PREFIX):
            return "libc"
        if dbase.startswith(("libc.", "ld-linux", "libpthread")):
            for comp, prefixes in PREFIX:
                if comp == "rt:ft" and b.startswith(prefixes):
                    return comp
            return "libc"
        if dbase.startswith("libm."):
            return "libm"
        if fam and not fam.startswith("lib:"):
            return fam
        for comp, sufs in SUFFIX:
            if b.endswith(sufs) and not b.startswith(("_Py_", "Py")):
                return comp
        for comp, prefixes in PREFIX:
            if b.startswith(prefixes):
                return comp
        rel = self.index.get(b)
        if rel:
            for prefix, comp in FILE_RULES:
                if rel.startswith(prefix):
                    return comp
            if rel.startswith("Modules/"):
                name = os.path.basename(rel).removesuffix(".c").removesuffix("module")
                return "cmod:" + (name.lstrip("_") or "other")
            return "lang:other"
        for comp, prefixes in LIB_PREFIX:
            if b.startswith(prefixes):
                return comp
        if fam:
            return fam
        return "unknown"

    @staticmethod
    def _dso_family(dbase, dso):
        if not dbase or dbase.startswith(("python3", "libpython", "libc.", "ld-linux",
                                          "libpthread", "libm.", "libdl.", "librt.")):
            return None
        d = dso or ""
        if "rustique" in dbase or "/rustique/" in d:
            return "rustique"
        if "/site-packages/" in d:
            top = d.split("/site-packages/", 1)[1].split("/")[0]
            top = top.removesuffix(".libs")
            return "ext:" + top.split(".")[0]
        if "/lib-dynload/" in d:
            name = dbase.split(".")[0].lstrip("_").removesuffix("module")
            return "cmod:" + name
        if dbase.startswith(("libz.", "libz-ng")):
            return "cmod:zlib"
        if dbase.startswith(("libssl", "libcrypto")):
            return "cmod:hashlib"
        if dbase.startswith("libexpat"):
            return "cmod:expat"
        if dbase.startswith("libffi"):
            return "cmod:ctypes"
        if dbase.startswith("libsqlite"):
            return "cmod:sqlite3"
        if dbase.startswith("libmpdec"):
            return "cmod:decimal"
        if dbase.startswith(("libstdc++", "libgcc")):
            return "lib:cxx"
        return "lib:" + dbase.split(".so")[0]


# ---------------------------------------------------------------------------------------
# opcode families (for the interpreter split)
# ---------------------------------------------------------------------------------------

FAMILY_GROUPS = [
    ("attribute", ("LOAD_ATTR", "STORE_ATTR", "DELETE_ATTR", "LOAD_SUPER_ATTR",
                   "INSTRUMENTED_LOAD_SUPER_ATTR")),
    ("call", ("CALL", "CALL_KW", "CALL_FUNCTION_EX", "CALL_INTRINSIC_1", "CALL_INTRINSIC_2",
              "INSTRUMENTED_CALL", "INSTRUMENTED_CALL_KW", "INSTRUMENTED_CALL_FUNCTION_EX",
              "EXIT_INIT_CHECK")),
    ("frame/return", ("RESUME", "RETURN_VALUE", "RETURN_GENERATOR", "YIELD_VALUE",
                      "INTERPRETER_EXIT", "start_frame", "exit_unwind", "exit_unwind_notrace",
                      "INSTRUMENTED_RESUME", "INSTRUMENTED_RETURN_VALUE",
                      "INSTRUMENTED_YIELD_VALUE", "(eval)")),
    ("binary/compare", ("BINARY_OP", "COMPARE_OP", "CONTAINS_OP", "IS_OP", "UNARY_NEGATIVE",
                        "UNARY_NOT", "UNARY_INVERT", "TO_BOOL", "BINARY_SLICE")),
    ("subscript-store", ("STORE_SUBSCR", "DELETE_SUBSCR", "STORE_SLICE")),
    ("globals/closures", ("LOAD_GLOBAL", "LOAD_NAME", "STORE_NAME", "STORE_GLOBAL",
                          "DELETE_GLOBAL", "DELETE_NAME", "LOAD_DEREF", "STORE_DEREF",
                          "DELETE_DEREF", "LOAD_FROM_DICT_OR_DEREF", "LOAD_FROM_DICT_OR_GLOBALS",
                          "MAKE_CELL", "COPY_FREE_VARS", "LOAD_LOCALS")),
    ("locals/stack", ("LOAD_FAST", "LOAD_FAST_AND_CLEAR", "LOAD_FAST_BORROW",
                      "LOAD_FAST_BORROW_LOAD_FAST_BORROW", "LOAD_FAST_CHECK", "LOAD_FAST_LOAD_FAST",
                      "STORE_FAST", "STORE_FAST_LOAD_FAST", "STORE_FAST_STORE_FAST", "DELETE_FAST",
                      "LOAD_CONST", "LOAD_SMALL_INT", "LOAD_COMMON_CONSTANT", "POP_TOP",
                      "PUSH_NULL", "COPY", "SWAP", "NOP", "END_FOR", "POP_ITER", "NOT_TAKEN",
                      "EXTENDED_ARG", "CACHE", "RESERVED", "INSTRUMENTED_POP_ITER",
                      "INSTRUMENTED_END_FOR", "INSTRUMENTED_NOT_TAKEN")),
    ("control", ("JUMP_BACKWARD", "JUMP_BACKWARD_NO_INTERRUPT", "JUMP_FORWARD",
                 "POP_JUMP_IF_FALSE", "POP_JUMP_IF_TRUE", "POP_JUMP_IF_NONE",
                 "POP_JUMP_IF_NOT_NONE", "INSTRUMENTED_JUMP_BACKWARD",
                 "INSTRUMENTED_JUMP_FORWARD", "INSTRUMENTED_POP_JUMP_IF_FALSE",
                 "INSTRUMENTED_POP_JUMP_IF_TRUE", "INSTRUMENTED_POP_JUMP_IF_NONE",
                 "INSTRUMENTED_POP_JUMP_IF_NOT_NONE", "ENTER_EXECUTOR")),
    ("iteration", ("FOR_ITER", "GET_ITER", "GET_LEN", "SEND", "END_SEND", "GET_AWAITABLE",
                   "GET_AITER", "GET_ANEXT", "END_ASYNC_FOR", "CLEANUP_THROW",
                   "INSTRUMENTED_FOR_ITER", "INSTRUMENTED_END_SEND",
                   "INSTRUMENTED_END_ASYNC_FOR")),
    ("build/unpack", ("BUILD_LIST", "BUILD_MAP", "BUILD_SET", "BUILD_SLICE", "BUILD_TUPLE",
                      "LIST_APPEND", "LIST_EXTEND", "SET_ADD", "SET_UPDATE", "MAP_ADD",
                      "DICT_MERGE", "DICT_UPDATE", "UNPACK_SEQUENCE", "UNPACK_EX")),
    ("format", ("FORMAT_SIMPLE", "FORMAT_WITH_SPEC", "CONVERT_VALUE", "BUILD_STRING",
                "BUILD_TEMPLATE", "BUILD_INTERPOLATION")),
    ("function/class", ("MAKE_FUNCTION", "SET_FUNCTION_ATTRIBUTE", "LOAD_BUILD_CLASS",
                        "SETUP_ANNOTATIONS", "MATCH_CLASS", "MATCH_KEYS", "MATCH_MAPPING",
                        "MATCH_SEQUENCE")),
    ("exception/with", ("PUSH_EXC_INFO", "POP_EXCEPT", "CHECK_EXC_MATCH", "CHECK_EG_MATCH",
                        "RERAISE", "RAISE_VARARGS", "WITH_EXCEPT_START", "LOAD_SPECIAL",
                        "error", "exception_unwind", "pop_1_error", "pop_2_error")),
    ("import", ("IMPORT_NAME", "IMPORT_FROM")),
    ("instrumentation", ("INSTRUMENTED_INSTRUCTION", "INSTRUMENTED_LINE", "stop_tracing",
                         "TRACE_RECORD")),
]
_GROUP_OF = {op: g for g, ops in FAMILY_GROUPS for op in ops}


class Opcodes:
    """Specialized name -> (base family, specialized?)."""

    def __init__(self, specializations):
        self.specializable = set(specializations)
        self.base = {}
        for base, specs in specializations.items():
            self.base[base] = base
            for s in specs:
                self.base[s] = base

    def family(self, op):
        b = self.base.get(op, op)
        return b, b != op

    def group(self, op):
        b, _ = self.family(op)
        return _GROUP_OF.get(b, "other")
