"""Profile one workload module (benches/workloads/<family>/<name>.py) in steady state.

Three separate runs, each in a fresh process, each after ``setup()`` and one warm-up
``run()``:

  perf      ``python -X perf``; ``perf record -e cpu-clock -F 1999 -g -p <pid>`` is attached
            after the warm-up and ``run()`` repeats until >= --user-seconds of user CPU
            (>= 5000 samples at 1999 Hz).  Samples are symbolized here (this box's perf has
            no libelf): the PBS python binary is non-PIE, so raw IPs are vaddrs looked up in
            ``nm -n -S``; shared objects are mapped through their MMAP2 events and LOAD
            segments; Python frames come from /tmp/perf-<pid>.map (``py::qual:file``).
            Outputs the owner x component matrix (innermost Python frame x leaf native
            symbol), top leaf symbols, and the "C entry" of each sample: the first
            non-dispatch native function below the innermost interpreter frame, i.e. the
            builtin method / slot / C-accelerator function the bytecode called into
            (self + native children).
  cprofile  ``cProfile`` around one ``run()``: tottime/cumtime/calls per function, builtin
            (C) entries, owner roll-up; plus plain (unprofiled) run timings.
  argsizes  ``sys.monitoring`` around one ``run()``: for every call from Python into a C
            callable, the length of the receiver (``self``) and of the first explicit
            argument (resolved from the call site's bytecode when it is a plain load);
            for every stdlib Python function, the length of its first non-self argument
            and of its return value.

Usage (run with the family venv's python):
  python _profile.py all WORKLOAD.py --out PROFILE.json [--user-seconds 3.2]
  python _profile.py perf|cprofile|argsizes WORKLOAD.py --out PART.json
  python _profile.py time WORKLOAD.py [-n 5]
  WL_PERF_DATA_DIR=DIR python _profile.py all ...        # also keep DIR/<name>.perf.data
  python _profile.py reanalyze PROFILE.json --keep-perf-data DIR/<name>.perf.data
  python _profile.py repick PROFILE.json                 # recompute arg_sizes from arg_sizes_all
"""

from __future__ import annotations

import argparse
import bisect
import collections
import importlib.util
import json
import os
import re
import signal
import subprocess
import sys
import sysconfig
import tempfile
import time

STDLIB = os.path.realpath(sysconfig.get_paths()["stdlib"]) + os.sep
PURELIB = os.path.realpath(sysconfig.get_paths()["purelib"]) + os.sep
WORKLOAD_DIR = os.path.dirname(os.path.realpath(__file__)) + os.sep
PERF_FREQ = 1999


# --------------------------------------------------------------------------------------
# Workload loading
# --------------------------------------------------------------------------------------


def load_workload(path):
    path = os.path.realpath(path)
    name = "workload_" + os.path.splitext(os.path.basename(path))[0]
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


# --------------------------------------------------------------------------------------
# Owner classification (file of the innermost Python frame)
# --------------------------------------------------------------------------------------

_DIST_CACHE = None


def _top_to_dist():
    global _DIST_CACHE
    if _DIST_CACHE is None:
        import importlib.metadata as md

        _DIST_CACHE = {}
        for top, dists in md.packages_distributions().items():
            _DIST_CACHE[top] = dists[0].lower().replace("_", "-")
    return _DIST_CACHE


def owner_of(filename, workload_file):
    """Map a code object's filename to an owner label."""
    if not filename:
        return "<generated>"
    if filename.startswith("<frozen "):
        return "stdlib:" + filename[8:-1].split(".")[0]
    if filename == "<template>":
        return "workload"  # Jinja2 templates compiled to Python code (the app's own templates)
    if filename.startswith("<"):
        return "<generated>"
    real = os.path.realpath(filename) if os.path.isabs(filename) else filename
    if real.startswith(PURELIB) or "/site-packages/" in real:
        rel = real.split("/site-packages/", 1)[1]
        top = rel.split("/", 1)[0]
        top = top[:-3] if top.endswith(".py") else top
        return "thirdparty:" + _top_to_dist().get(top, top)
    if real.startswith(STDLIB):
        rel = real[len(STDLIB):]
        top = rel.split("/", 1)[0]
        top = top[:-3] if top.endswith(".py") else top
        return "stdlib:" + top
    if real == workload_file or real.startswith(WORKLOAD_DIR):
        return "workload"
    if not os.path.isabs(filename):
        # Templates compiled to Python code (Jinja2 uses the template name as filename).
        return "workload"
    return "other:" + os.path.basename(real)


def short_file(filename):
    if filename.startswith("<"):
        return filename
    real = os.path.realpath(filename) if os.path.isabs(filename) else filename
    if "/site-packages/" in real:
        return real.split("/site-packages/", 1)[1]
    if real.startswith(STDLIB):
        return real[len(STDLIB):]
    return os.path.basename(real)


# --------------------------------------------------------------------------------------
# Native component classification (leaf symbol -> component)
# --------------------------------------------------------------------------------------

# Ordered: the first matching rule wins.  (kind, pattern, component); kind "p" = prefix,
# "s" = suffix, "e" = exact, "c" = contains.
_RULES = [
    # interpreter
    ("e", "PyUnstable_Code_GetExtra", "perf-trampoline"), ("p", "py_trampoline", "perf-trampoline"),
    ("p", "perf_map_", "perf-trampoline"), ("p", "compile_trampoline", "perf-trampoline"),
    ("e", "call_tzinfo_method", "datetime"), ("p", "_Py_MakeCoro", "gen/coro"),
    ("p", "_TAIL_CALL_", "interpreter"), ("e", "_PyEval_EvalFrameDefault", "interpreter"),
    ("p", "_PyEval_", "interpreter"), ("p", "_PyFrame_", "interpreter"),
    ("p", "_Py_Specialize", "interpreter"), ("p", "_PyPerfTrampoline", "interpreter"),
    ("e", "py_trampoline_evaluator", "interpreter"), ("p", "_PyEvalFrame", "interpreter"),
    ("p", "_PyThreadState_PushFrame", "interpreter"), ("p", "_PyThreadState_PopFrame", "interpreter"),
    ("e", "clear_thread_frame", "interpreter"), ("e", "clear_gen_frame", "interpreter"),
    ("p", "_Py_HandlePending", "interpreter"), ("p", "_PyCode_", "interpreter"),
    ("p", "_PyFunction_SetVersion", "interpreter"), ("p", "_Py_EnterRecursive", "interpreter"),
    ("p", "_PyInterpreterFrame", "interpreter"), ("e", "_Py_LeaveRecursiveCallTstate", "interpreter"),
    ("p", "_Py_call_instrumentation", "instrumentation"), ("p", "_Py_Instrument", "instrumentation"),
    ("p", "_PyMonitoring", "instrumentation"),
    # calls
    ("p", "_PyObject_Vectorcall", "calls"), ("p", "PyObject_Vectorcall", "calls"),
    ("p", "_PyFunction_Vectorcall", "calls"), ("p", "method_vectorcall", "calls"),
    ("p", "cfunction_", "calls"), ("p", "_PyObject_Call", "calls"), ("p", "PyObject_Call", "calls"),
    ("p", "_PyVectorcall", "calls"), ("p", "PyVectorcall", "calls"), ("p", "_PyStack_", "calls"),
    ("p", "vectorcall_", "calls"), ("p", "_PyObject_MakeTpCall", "calls"),
    ("p", "method_call", "calls"), ("p", "func_descr_get", "calls"), ("p", "method_new", "calls"),
    ("p", "_PyCFunction", "calls"), ("p", "PyCFunction", "calls"), ("p", "meth_", "calls"),
    ("p", "bounded_lru_cache_wrapper", "functools-accel"), ("p", "lru_cache_", "functools-accel"),
    ("p", "partial_", "functools-accel"), ("p", "_functools", "functools-accel"),
    ("p", "keyobject_", "functools-accel"), ("p", "infinite_lru_cache", "functools-accel"),
    ("p", "wrapperdescr_", "calls"), ("p", "wrapper_call", "calls"),
    # argument parsing for C functions
    ("p", "_PyArg_", "arg-parsing"), ("p", "PyArg_", "arg-parsing"), ("p", "vgetargs", "arg-parsing"),
    ("p", "convertsimple", "arg-parsing"), ("p", "convertitem", "arg-parsing"),
    ("p", "find_keyword", "arg-parsing"), ("p", "parser_init", "arg-parsing"),
    # third-party symbol names inside the static binary (sqlite, expat, zlib, openssl, ...)
    ("p", "pysqlite_", "sqlite3-module"), ("p", "_pysqlite_", "sqlite3-module"),
    ("p", "_sqlite3_", "sqlite3-module"), ("p", "sqlite3", "sqlite-engine"), ("p", "XML_", "expat"), ("p", "PyExpat", "expat"),
    ("p", "deflate", "zlib"), ("p", "inflate", "zlib"), ("p", "zlib_", "zlib"),
    ("p", "EVP_", "openssl"), ("p", "SSL_", "openssl"), ("p", "ssl3_", "openssl"),
    ("p", "Hacl_", "hashlib"), ("p", "_hashlib", "hashlib"), ("p", "_sha", "hashlib"),
    ("p", "_md5", "hashlib"), ("p", "_blake2", "hashlib"), ("p", "hashlib", "hashlib"),
    ("p", "ZSTD_", "zstd"), ("p", "mpd_", "decimal"), ("p", "_mpd_", "decimal"),
    ("p", "dec_", "decimal"), ("p", "PyDec", "decimal"),
    # re engine
    ("p", "sre_", "re-engine"), ("p", "_sre", "re-engine"), ("p", "pattern_", "re-engine"),
    ("p", "match_", "re-engine"), ("e", "scanner_match", "re-engine"),
    ("e", "scanner_search", "re-engine"), ("e", "state_init", "re-engine"),
    ("e", "state_fini", "re-engine"), ("e", "getstring", "re-engine"),
    ("p", "pattern_subx", "re-engine"), ("e", "getslice", "re-engine"),
    # json accelerator
    ("p", "encoder_", "json-accel"), ("p", "scanstring", "json-accel"), ("p", "scan_once", "json-accel"),
    ("p", "py_encode_basestring", "json-accel"), ("p", "_parse_object", "json-accel"),
    ("p", "_parse_array", "json-accel"), ("p", "_match_number", "json-accel"),
    ("p", "write_escaped", "json-accel"), ("p", "ascii_escape", "json-accel"),
    ("p", "escape_unicode", "json-accel"), ("p", "_json", "json-accel"),
    ("e", "scanner_call", "json-accel"), ("e", "scanner_new", "json-accel"),
    ("e", "_build_rval_index_tuple", "json-accel"), ("p", "raise_errmsg", "json-accel"),
    # pickle, datetime, io, struct, asyncio, select, socket, ...
    ("p", "_pickle", "pickle"), ("p", "Pickler", "pickle"), ("p", "Unpickler", "pickle"),
    ("p", "save_", "pickle"), ("p", "load_", "pickle"),
    ("p", "datetime_", "datetime"), ("p", "date_", "datetime"), ("p", "delta_", "datetime"),
    ("p", "new_datetime", "datetime"), ("p", "new_date", "datetime"), ("p", "new_delta", "datetime"),
    ("p", "tzinfo_", "datetime"), ("p", "timezone_", "datetime"), ("p", "wrap_strftime", "datetime"),
    ("p", "format_utcoffset", "datetime"), ("p", "_datetime", "datetime"), ("p", "time_strftime", "datetime"),
    ("p", "parse_isoformat", "datetime"), ("p", "parse_hh_mm", "datetime"), ("p", "local_timezone", "datetime"),
    ("p", "_io", "io"), ("p", "_bufferedreader", "io"), ("p", "_bufferedwriter", "io"),
    ("p", "buffered_", "io"), ("p", "_buffered_", "io"), ("p", "textiowrapper", "io"),
    ("p", "_textiowrapper", "io"), ("p", "fileio_", "io"), ("p", "_PyIO", "io"), ("p", "bytesio_", "io"),
    ("p", "_bytesio", "io"), ("p", "stringio_", "io"), ("p", "_stringio", "io"), ("p", "write_str", "io"),
    ("p", "_PyIncrementalNewlineDecoder", "io"), ("p", "incrementalnewlinedecoder", "io"),
    ("p", "_Py_write", "io"), ("p", "_Py_read", "io"),
    ("p", "s_pack", "struct"), ("p", "s_unpack", "struct"), ("p", "_struct", "struct"),
    ("p", "task_", "asyncio-accel"), ("p", "future_", "asyncio-accel"), ("p", "FutureObj", "asyncio-accel"),
    ("p", "TaskObj", "asyncio-accel"), ("p", "_asyncio", "asyncio-accel"), ("p", "TaskStepMethWrapper", "asyncio-accel"),
    ("p", "enter_task", "asyncio-accel"), ("p", "leave_task", "asyncio-accel"), ("p", "swap_current_task", "asyncio-accel"),
    ("p", "get_running_loop", "asyncio-accel"), ("p", "FutureIter", "asyncio-accel"),
    ("p", "select_", "select"), ("p", "pyepoll", "select"), ("p", "_select", "select"),
    ("p", "sock_", "socket"), ("p", "socket_", "socket"), ("p", "_socket", "socket"),
    ("p", "internal_select", "socket"), ("p", "getsockaddrarg", "socket"), ("p", "makesockaddr", "socket"),
    ("p", "setipaddr", "socket"), ("p", "makeipaddr", "socket"), ("p", "socket_getaddrinfo", "socket"),
    ("p", "rlock_", "locks"), ("p", "lock_", "locks"), ("p", "PyThread_", "locks"), ("p", "_PyMutex", "locks"),
    ("p", "PyMutex", "locks"), ("p", "_PySemaphore", "locks"), ("p", "_PyParkingLot", "locks"),
    ("p", "_thread", "locks"), ("p", "thread_", "locks"), ("p", "_PyRecursiveMutex", "locks"),
    ("p", "_PyEvent", "locks"), ("p", "take_gil", "gil"), ("p", "drop_gil", "gil"), ("p", "_PyEval_AcquireLock", "gil"),
    ("p", "deque", "collections-accel"), ("p", "defdict", "collections-accel"), ("p", "_collections", "collections-accel"),
    ("p", "tuplegetter", "collections-accel"), ("p", "_count_elements", "collections-accel"),
    ("p", "itemgetter", "operator"), ("p", "attrgetter", "operator"), ("p", "methodcaller", "operator"),
    ("p", "_operator", "operator"), ("p", "dag_", "operator"),
    ("p", "itertools", "itertools"), ("p", "chain_", "itertools"), ("p", "islice_", "itertools"),
    ("p", "product_", "itertools"), ("p", "groupby", "itertools"), ("p", "_grouper", "itertools"),
    ("p", "tee", "itertools"), ("p", "batched", "itertools"), ("p", "pairwise", "itertools"),
    ("p", "zip_longest", "itertools"), ("p", "starmap", "itertools"), ("p", "accumulate", "itertools"),
    ("p", "random_", "random"), ("p", "_random", "random"), ("p", "genrand", "random"),
    ("p", "math_", "math"), ("p", "binascii", "binascii"), ("p", "base64", "binascii"),
    ("p", "contextvar", "contextvars"), ("p", "context_", "contextvars"), ("p", "PyContext", "contextvars"),
    ("p", "hamt", "contextvars"), ("p", "_PyContext", "contextvars"),
    ("p", "weakref", "weakref"), ("p", "PyWeakref", "weakref"), ("p", "_PyWeakref", "weakref"),
    ("p", "PyObject_ClearWeakRefs", "weakref"), ("p", "proxy_", "weakref"), ("p", "_PyStaticType_ClearWeakRefs", "weakref"),
    ("p", "_PyTime", "time"), ("p", "PyTime", "time"), ("p", "py_get_", "time"), ("p", "time_", "time"),
    ("p", "pytime", "time"), ("p", "_PyDeadline", "time"),
    ("p", "_codecs", "codecs"), ("p", "PyCodec", "codecs"), ("p", "_PyCodec", "codecs"),
    ("p", "codec_", "codecs"), ("p", "_Py_normalize_encoding", "codecs"),
    ("p", "posix_", "os"), ("p", "os_", "os"), ("p", "DirEntry", "os"), ("p", "ScandirIterator", "os"),
    ("p", "_Py_stat", "os"), ("p", "_Py_fstat", "os"), ("p", "path_converter", "os"),
    # exceptions / tracebacks
    ("p", "BaseException", "exceptions"), ("p", "_PyErr", "exceptions"), ("p", "PyErr", "exceptions"),
    ("p", "_PyException", "exceptions"), ("p", "PyException", "exceptions"), ("p", "OSError_", "exceptions"),
    ("p", "KeyError_", "exceptions"), ("p", "StopIteration", "exceptions"), ("p", "_PyTraceBack", "exceptions"),
    ("p", "PyTraceBack", "exceptions"), ("p", "tb_", "exceptions"), ("p", "SyntaxError", "exceptions"),
    ("p", "ImportError", "exceptions"), ("p", "AttributeError", "exceptions"), ("p", "NameError", "exceptions"),
    ("p", "_PyErr_Format", "exceptions"), ("p", "frame_", "exceptions"), ("p", "PyFrame_", "exceptions"),
    ("p", "_PyFrame_GetFrameObject", "exceptions"), ("p", "_PyFrame_New", "exceptions"),
    # generators / coroutines
    ("p", "gen_", "gen/coro"), ("p", "_PyGen", "gen/coro"), ("p", "coro_", "gen/coro"),
    ("p", "async_gen", "gen/coro"), ("p", "PyGen", "gen/coro"), ("p", "_PyCoro", "gen/coro"),
    ("p", "PyCoro", "gen/coro"), ("p", "_PyAsyncGen", "gen/coro"), ("p", "compute_cr_origin", "gen/coro"),
    ("p", "make_gen", "gen/coro"),
    # str
    ("p", "unicode", "str"), ("p", "PyUnicode", "str"), ("p", "_PyUnicode", "str"),
    ("c", "UnicodeWriter", "str"), ("p", "ucs1lib_", "str"), ("p", "ucs2lib_", "str"),
    ("p", "ucs4lib_", "str"), ("p", "asciilib_", "str"), ("p", "stringlib_", "str"),
    ("p", "_PyUnicodeWriter", "str"), ("p", "PyUnicodeWriter", "str"), ("p", "ascii_decode", "str"),
    ("p", "do_string_format", "str"), ("p", "MarkupIterator", "str"), ("p", "render_field", "str"),
    ("p", "output_markup", "str"), ("p", "build_string", "str"), ("p", "parse_field", "str"),
    ("p", "get_field_object", "str"), ("p", "FieldNameIterator", "str"), ("p", "formatter_", "str"),
    ("p", "format_obj", "str"), ("p", "_PyStr", "str"), ("p", "str_", "str"), ("p", "fieldnameiter", "str"),
    ("p", "formatteriter", "str"), ("p", "_Py_DecodeUTF8", "str"), ("p", "_Py_EncodeUTF8", "str"),
    ("p", "utf8_", "str"), ("p", "ascii_", "str"), ("p", "latin1_", "str"), ("p", "charmap", "str"),
    ("p", "_PyLong_FormatWriter", "int"), ("p", "format_string_internal", "str"),
    ("p", "format_long_internal", "int"), ("p", "format_float_internal", "float"),
    ("p", "_PyUnicode_FormatAdvanced", "str"), ("p", "fill_number", "str"), ("p", "calc_number_widths", "str"),
    ("p", "do_markup", "str"), ("p", "anylib_", "str"), ("p", "_PyUnicode_Join", "str"),
    ("p", "split_whitespace", "str"), ("p", "replace", "str"), ("p", "fastsearch", "str"),
    ("p", "default_find", "str"), ("p", "adaptive_find", "str"), ("p", "two_way", "str"),
    ("p", "tailmatch", "str"), ("p", "any_find_slice", "str"), ("p", "case_operation", "str"),
    ("p", "do_lower", "str"), ("p", "do_upper", "str"), ("p", "do_title", "str"),
    ("p", "do_capitalize", "str"), ("p", "do_casefold", "str"), ("p", "do_strip", "str"),
    ("p", "_PyUnicode_ToLower", "str"), ("p", "lower_ucs4", "str"), ("p", "resize_compact", "str"),
    ("p", "intern_", "str"), ("p", "_Py_ascii_", "str"), ("p", "find_maxchar", "str"),
    # bytes
    ("p", "bytes", "bytes"), ("p", "PyBytes", "bytes"), ("p", "_PyBytes", "bytes"),
    ("p", "bytearray", "bytes"), ("p", "PyByteArray", "bytes"), ("p", "_PyByteArray", "bytes"),
    ("p", "_Py_bytes_", "bytes"), ("p", "memory_", "bytes"), ("p", "PyMemoryView", "bytes"),
    ("p", "_PyBytesWriter", "bytes"), ("p", "PyBytesWriter", "bytes"), ("p", "_PyBuffer", "bytes"),
    ("p", "PyBuffer", "bytes"), ("p", "PyObject_GetBuffer", "bytes"), ("p", "mbuf_", "bytes"),
    # int / float / bool / complex
    ("p", "long_", "int"), ("p", "PyLong", "int"), ("p", "_PyLong", "int"), ("e", "k_mul", "int"),
    ("e", "x_divrem", "int"), ("p", "x_add", "int"), ("p", "x_sub", "int"), ("p", "l_divmod", "int"),
    ("p", "long_to_decimal", "int"), ("p", "_Py_long", "int"), ("p", "bool_", "int"), ("p", "PyBool", "int"),
    ("p", "float_", "float"), ("p", "PyFloat", "float"), ("p", "_PyFloat", "float"), ("p", "_Py_dg_", "float"),
    ("e", "PyOS_double_to_string", "float"), ("p", "format_float_short", "float"), ("p", "PyOS_string_to_double", "float"),
    ("p", "_Py_parse_inf_or_nan", "float"), ("p", "_Py_string_to_number_with_underscores", "float"),
    ("p", "Balloc", "float"), ("p", "Bfree", "float"), ("p", "multadd", "float"), ("p", "s2b", "float"),
    ("p", "i2b", "float"), ("p", "mult", "float"), ("p", "pow5mult", "float"), ("p", "lshift", "float"),
    ("p", "quorem", "float"), ("p", "complex_", "float"),
    # dict
    ("p", "dict", "dict"), ("p", "PyDict", "dict"), ("p", "_PyDict", "dict"), ("e", "insertdict", "dict"),
    ("p", "insert_", "dict"), ("p", "unicodekeys_lookup", "dict"), ("p", "_Py_dict_lookup", "dict"),
    ("p", "lookdict", "dict"), ("p", "new_keys_object", "dict"), ("p", "free_keys_object", "dict"),
    ("p", "new_dict", "dict"), ("p", "clone_combined_dict", "dict"), ("p", "do_lookup", "dict"),
    ("p", "compare_unicode", "dict"), ("p", "compare_generic", "dict"), ("p", "find_empty_slot", "dict"),
    ("p", "build_indices", "dict"), ("p", "copy_values", "dict"), ("p", "new_values", "dict"),
    ("p", "_PyObject_InitInlineValues", "dict"), ("p", "make_dict_from_instance_attributes", "dict"),
    ("p", "_PyObject_MaterializeManagedDict", "dict"), ("p", "unicode_get_hash", "dict"),
    ("p", "mappingproxy", "dict"), ("p", "odict", "dict"), ("p", "PyODict", "dict"),
    # list / tuple / set
    ("p", "list", "list/tuple/set"), ("p", "PyList", "list/tuple/set"), ("p", "_PyList", "list/tuple/set"),
    ("p", "tuple", "list/tuple/set"), ("p", "PyTuple", "list/tuple/set"), ("p", "_PyTuple", "list/tuple/set"),
    ("p", "set_", "list/tuple/set"), ("p", "PySet", "list/tuple/set"), ("p", "_PySet", "list/tuple/set"),
    ("p", "frozenset", "list/tuple/set"), ("p", "setiter", "list/tuple/set"), ("p", "make_new_set", "list/tuple/set"),
    ("p", "binarysort", "list/tuple/set"), ("p", "merge_", "list/tuple/set"), ("p", "gallop_", "list/tuple/set"),
    ("p", "count_run", "list/tuple/set"), ("p", "unsafe_", "list/tuple/set"), ("p", "safe_object_compare", "list/tuple/set"),
    ("p", "app1", "list/tuple/set"), ("p", "ins1", "list/tuple/set"),
    # builtins module and simple builtin types
    ("p", "builtin_", "builtins"), ("p", "zip_", "builtins"), ("p", "map_", "builtins"), ("p", "filter_", "builtins"),
    ("p", "enum_", "builtins"), ("p", "reversed_", "builtins"), ("p", "range", "builtins"),
    ("p", "longrangeiter", "builtins"), ("p", "slice_", "builtins"), ("p", "PySlice", "builtins"),
    ("p", "_PySlice", "builtins"), ("p", "min_max", "builtins"), ("p", "sorted", "builtins"),
    ("p", "PyIter_", "abstract"), ("p", "calliter", "builtins"), ("p", "seqiter", "builtins"),
    ("p", "PySeqIter", "builtins"), ("p", "anextawaitable", "builtins"),
    # attr / type
    ("p", "_PyType_Lookup", "attr/type"), ("p", "PyObject_GenericGetAttr", "attr/type"),
    ("p", "_PyObject_GenericGetAttr", "attr/type"), ("p", "_PyObject_GetMethod", "attr/type"),
    ("p", "slot_", "attr/type"), ("p", "type_", "attr/type"), ("p", "PyType_", "attr/type"), ("p", "_PyType_", "attr/type"),
    ("p", "PyObject_GetAttr", "attr/type"), ("p", "PyObject_SetAttr", "attr/type"), ("p", "_PyObject_LookupAttr", "attr/type"),
    ("p", "PyObject_GenericSetAttr", "attr/type"), ("p", "_PyObject_GenericSetAttr", "attr/type"),
    ("p", "_PyObject_StoreInstanceAttribute", "attr/type"), ("p", "_PyObject_TryGetInstanceAttribute", "attr/type"),
    ("p", "lookup_maybe_method", "attr/type"), ("p", "lookup_method", "attr/type"), ("p", "call_method", "attr/type"),
    ("p", "_PyObject_LookupSpecial", "attr/type"), ("p", "PyObject_IsInstance", "attr/type"),
    ("p", "object_isinstance", "attr/type"), ("p", "object_recursive_isinstance", "attr/type"),
    ("p", "recursive_isinstance", "attr/type"), ("p", "PyObject_IsSubclass", "attr/type"),
    ("p", "object_issubclass", "attr/type"), ("p", "recursive_issubclass", "attr/type"),
    ("p", "abstract_get_bases", "attr/type"), ("p", "abstract_issubclass", "attr/type"),
    ("p", "_PyObject_RealIsInstance", "attr/type"), ("p", "_PyObject_RealIsSubclass", "attr/type"),
    ("p", "object_", "attr/type"), ("p", "super_", "attr/type"), ("p", "property_", "attr/type"),
    ("p", "member_", "attr/type"), ("p", "PyMember_", "attr/type"), ("p", "getset_", "attr/type"),
    ("p", "method_get", "attr/type"), ("p", "classmethod_", "attr/type"), ("p", "cm_", "attr/type"),
    ("p", "sm_", "attr/type"), ("p", "descr_", "attr/type"), ("p", "PyDescr_", "attr/type"),
    ("p", "module_getattr", "attr/type"), ("p", "_Py_module_getattro", "attr/type"),
    ("p", "_PyModule_", "attr/type"), ("p", "PyModule_", "attr/type"), ("p", "update_slot", "attr/type"),
    ("p", "_PyObject_GetState", "attr/type"), ("p", "_PyObject_ManagedDictPointer", "attr/type"),
    ("p", "_PyObject_GetDictPtr", "attr/type"), ("p", "PyObject_GenericGetDict", "attr/type"),
    ("p", "subtype_", "memory/gc"), ("p", "PyBaseObject", "attr/type"), ("p", "mro_", "attr/type"),
    ("p", "_PyObject_LookupSpecialMethod", "attr/type"), ("p", "PyMethod_", "calls"), ("p", "PyInstanceMethod", "calls"),
    ("p", "find_name_in_mro", "attr/type"), ("p", "_Py_type_getattro", "attr/type"),
    # memory / gc
    ("p", "gc_", "memory/gc"), ("p", "_PyObject_GC", "memory/gc"), ("p", "PyObject_GC", "memory/gc"),
    ("p", "_PyMem", "memory/gc"), ("p", "PyMem_", "memory/gc"), ("p", "mi_", "memory/gc"), ("p", "_mi_", "memory/gc"),
    ("p", "pymalloc", "memory/gc"), ("p", "_Py_Dealloc", "memory/gc"), ("s", "_dealloc", "memory/gc"),
    ("p", "_PyObject_Malloc", "memory/gc"), ("p", "_PyObject_Free", "memory/gc"), ("p", "_PyObject_Calloc", "memory/gc"),
    ("p", "_PyObject_Realloc", "memory/gc"), ("p", "PyObject_Malloc", "memory/gc"), ("p", "PyObject_Free", "memory/gc"),
    ("p", "PyObject_Realloc", "memory/gc"), ("p", "PyObject_Calloc", "memory/gc"),
    ("p", "_PyObject_New", "memory/gc"), ("p", "PyObject_Init", "memory/gc"), ("p", "_PyObject_Init", "memory/gc"),
    ("p", "PyType_GenericAlloc", "memory/gc"), ("p", "_PyType_AllocNoTrack", "memory/gc"),
    ("p", "PyType_GenericNew", "memory/gc"), ("p", "deduce_unreachable", "memory/gc"), ("p", "move_unreachable", "memory/gc"),
    ("p", "update_refs", "memory/gc"), ("p", "subtract_refs", "memory/gc"), ("p", "visit_", "memory/gc"),
    ("s", "_traverse", "memory/gc"), ("p", "_PyGC", "memory/gc"), ("p", "_Py_ScheduleGC", "memory/gc"),
    ("p", "free_", "memory/gc"), ("p", "_PyFreeList", "memory/gc"), ("p", "_Py_NewReference", "memory/gc"),
    ("p", "_Py_Incref", "memory/gc"), ("p", "_Py_Decref", "memory/gc"), ("p", "Py_DecRef", "memory/gc"),
    ("p", "Py_IncRef", "memory/gc"), ("p", "_PyObject_ClearManagedDict", "memory/gc"), ("s", "_clear", "memory/gc"),
    ("p", "PyObject_CallFinalizer", "memory/gc"), ("p", "_PyObject_Visit", "memory/gc"), ("p", "untrack_", "memory/gc"),
    ("p", "gc_collect", "memory/gc"), ("p", "collect", "memory/gc"), ("p", "_Py_RunGC", "memory/gc"),
    ("p", "_PyObject_AssertFailed", "memory/gc"), ("p", "arena_", "memory/gc"), ("p", "pool_", "memory/gc"),
    ("p", "allocate_from_new_pool", "memory/gc"), ("p", "insert_to_usedpool", "memory/gc"),
    ("p", "insert_to_freepool", "memory/gc"), ("p", "new_arena", "memory/gc"),
    # hashing / generic object protocol
    ("p", "_Py_HashBytes", "hash"), ("p", "siphash", "hash"), ("p", "pysiphash", "hash"), ("p", "_Py_Hash", "hash"),
    ("p", "PyObject_Hash", "hash"), ("p", "_Py_HashPointer", "hash"),
    ("p", "PyObject_RichCompare", "abstract"), ("p", "do_richcompare", "abstract"), ("p", "PyObject_IsTrue", "abstract"),
    ("p", "PyObject_Repr", "abstract"), ("p", "PyObject_Str", "abstract"), ("p", "PyObject_Format", "abstract"),
    ("p", "PyNumber_", "abstract"), ("p", "_PyNumber_", "abstract"), ("p", "binary_op", "abstract"),
    ("p", "ternary_op", "abstract"), ("p", "PyObject_GetItem", "abstract"), ("p", "PyObject_SetItem", "abstract"),
    ("p", "PyObject_DelItem", "abstract"), ("p", "PyObject_GetIter", "abstract"), ("p", "PySequence_", "abstract"),
    ("p", "_PySequence_", "abstract"), ("p", "PyMapping_", "abstract"), ("p", "PyObject_Size", "abstract"),
    ("p", "PyObject_Length", "abstract"), ("p", "PyObject_Not", "abstract"), ("p", "PyObject_Bytes", "abstract"),
    ("p", "PyObject_ASCII", "abstract"), ("p", "PyObject_Dir", "abstract"), ("p", "PyObject_SelfIter", "abstract"),
    ("p", "_PyObject_", "abstract"), ("p", "PyObject_", "abstract"), ("p", "PyIndex", "abstract"),
    ("p", "_PyIndex", "abstract"), ("p", "_PyEval_BinaryOps", "abstract"), ("p", "_Py_BinaryOp", "abstract"),
    # compiler / import / marshal
    ("p", "_PyAST", "compiler"), ("p", "compiler_", "compiler"), ("p", "codegen_", "compiler"), ("p", "_PyPegen", "compiler"),
    ("s", "_rule", "compiler"), ("p", "_PyCompile", "compiler"), ("p", "_PyCfg", "compiler"), ("p", "optimize_", "compiler"),
    ("p", "symtable_", "compiler"), ("p", "_PySymtable", "compiler"), ("p", "assemble", "compiler"),
    ("p", "_PyParser", "compiler"), ("p", "_PyTokenizer", "compiler"), ("p", "tok_", "compiler"), ("p", "astfold", "compiler"),
    ("p", "_PyImport", "import"), ("p", "PyImport", "import"), ("p", "import_", "import"), ("p", "r_object", "marshal"),
    ("p", "w_object", "marshal"), ("p", "PyMarshal", "marshal"),
    ("p", "cell_", "interpreter"), ("p", "PyCell", "interpreter"), ("p", "func_", "calls"), ("p", "PyFunction", "calls"),
    ("p", "_Py_", "other-cpython"), ("p", "Py", "other-cpython"), ("p", "_Py", "other-cpython"),
]
_COMP_CACHE = {}


def component_of_symbol(sym):
    c = _COMP_CACHE.get(sym)
    if c is not None:
        return c
    s = sym
    for suf in (".cold", ".warm", ".llvm."):
        i = s.find(suf)
        if i > 0:
            s = s[:i]
    for kind, pat, comp in _RULES:
        if ((kind == "p" and s.startswith(pat)) or (kind == "e" and s == pat)
                or (kind == "s" and s.endswith(pat)) or (kind == "c" and pat in s)):
            c = comp
            break
    else:
        c = "other-cpython"
    _COMP_CACHE[sym] = c
    return c


# --------------------------------------------------------------------------------------
# ELF symbolization
# --------------------------------------------------------------------------------------


class ElfSyms:
    def __init__(self, path):
        self.path = path
        self.addrs, self.names, self.sizes = [], [], []
        self.loads = []  # (offset, vaddr, filesz)
        try:
            out = subprocess.run(["readelf", "-lW", path], capture_output=True, text=True).stdout
            for line in out.splitlines():
                p = line.split()
                if p and p[0] == "LOAD":
                    self.loads.append((int(p[1], 16), int(p[2], 16), int(p[4], 16)))
            syms = []
            for flag in ([], ["-D"]):
                out = subprocess.run(["nm", "-n", "-S", *flag, path], capture_output=True, text=True).stdout
                for line in out.splitlines():
                    p = line.split()
                    if len(p) == 4 and p[2] in "tTwWiI":
                        syms.append((int(p[0], 16), int(p[1], 16), p[3]))
                    elif len(p) == 3 and p[1] in "tTwWiI":
                        syms.append((int(p[0], 16), 0, p[2]))
                if syms:
                    break
            syms.sort()
            for a, sz, n in syms:
                self.addrs.append(a)
                self.sizes.append(sz)
                self.names.append(n)
        except FileNotFoundError:
            pass
        self.has_symtab = bool(self.addrs) and not path.endswith("libc.so.6")

    def offset_to_vaddr(self, off):
        for o, v, sz in self.loads:
            if o <= off < o + sz:
                return off - o + v
        return off

    def lookup(self, vaddr):
        """Return (name, inside) for vaddr; inside=False if past the symbol's size."""
        i = bisect.bisect_right(self.addrs, vaddr) - 1
        if i < 0:
            return None, False
        sz = self.sizes[i]
        return self.names[i], (sz == 0 or vaddr < self.addrs[i] + sz)


def dso_component(dso):
    base = os.path.basename(dso)
    if base.startswith("libc.so"):
        return "libc"
    if base.startswith("libm.so"):
        return "libm"
    if base.startswith("ld-linux"):
        return "ld.so"
    if base in ("[vdso]", "[vsyscall]"):
        return "vdso"
    if "[kernel" in dso:
        return "kernel"
    if "/site-packages/" in dso:
        rel = dso.split("/site-packages/", 1)[1]
        top = rel.split("/", 1)[0]
        return "ext:" + _top_to_dist().get(top, top)
    if "lib-dynload" in dso:
        return "c-module:" + base.split(".")[0]
    return "so:" + base.split(".so")[0]


# --------------------------------------------------------------------------------------
# perf mode
# --------------------------------------------------------------------------------------


def _rusage():
    import resource

    r = resource.getrusage(resource.RUSAGE_SELF)
    return r.ru_utime, r.ru_stime


def child_perf(workload_path, user_seconds):
    mod = load_workload(workload_path)
    state = mod.setup()
    d0 = mod.digest(mod.run(state))
    sys.stdout.write("READY\n")
    sys.stdout.flush()
    sys.stdin.readline()
    u0, s0 = _rusage()
    w0 = time.perf_counter()
    times = []
    while True:
        t = time.perf_counter()
        r = mod.run(state)
        times.append(time.perf_counter() - t)
        u, s = _rusage()
        if u - u0 >= user_seconds and len(times) >= 2:
            break
        r = None
    w = time.perf_counter() - w0
    u, s = _rusage()
    sys.stdout.write("DONE " + json.dumps({
        "runs": len(times), "run_times": times, "wall": w, "utime": u - u0, "stime": s - s0}) + "\n")
    sys.stdout.flush()
    sys.stdin.readline()  # perf has been stopped: the digest is not in the profile
    d = mod.digest(r)
    sys.stdout.write(json.dumps({"digest_stable": d == d0, "digest": d0}) + "\n")
    sys.stdout.flush()
    sys.stdin.readline()


def _parse_perf(data_path, workload_file, python_bin):
    out = subprocess.run(
        ["perf", "script", "-i", data_path, "-F", "tid,ip,sym,dso", "--show-mmap-events"],
        capture_output=True, text=True, errors="replace").stdout
    mmaps = []  # (start, end, pgoff, dso)
    mm_re = re.compile(r"PERF_RECORD_MMAP2? \S+: \[(0x[0-9a-f]+)\((0x[0-9a-f]+)\) @ (0x[0-9a-f]+|0) .*?\]: (\S+) (.*)$")
    elfs = {}

    def elf(path):
        e = elfs.get(path)
        if e is None:
            e = elfs[path] = ElfSyms(path)
        return e

    pyelf = elf(python_bin)
    samples = []
    cur = None
    for line in out.splitlines():
        if not line.strip():
            if cur:
                samples.append(cur)
            cur = None
            continue
        if "PERF_RECORD_MMAP" in line:
            m = mm_re.search(line)
            if m and "x" in m.group(4):
                start, ln, pg = int(m.group(1), 16), int(m.group(2), 16), int(m.group(3), 16)
                mmaps.append((start, start + ln, pg, m.group(5).strip()))
            continue
        if line[0] != "\t" and not line.startswith(" " * 6):
            if cur:
                samples.append(cur)
            cur = []
            continue
        if cur is None:
            continue
        p = line.strip().split(None, 1)
        ip = int(p[0], 16)
        rest = p[1] if len(p) > 1 else ""
        j = rest.rfind(" (")
        sym, dso = (rest[:j], rest[j + 2:-1]) if j >= 0 else (rest, "")
        cur.append((ip, sym, dso))
    if cur:
        samples.append(cur)

    mmaps.sort()
    mstarts = [m[0] for m in mmaps]
    res_cache = {}
    stats = {"py_bin_frames": 0, "py_bin_outside": 0}

    def resolve(ip, sym, dso, leaf):
        """-> (symbol, component, is_python_frame)"""
        if sym.startswith("py::"):
            return sym, "interpreter", True
        key = (ip if leaf else ip - 1, dso)
        r = res_cache.get(key)
        if r is not None:
            return r
        a = key[0]
        if dso == python_bin or os.path.basename(dso).startswith("python3"):
            name, inside = pyelf.lookup(a)
            stats["py_bin_frames"] += 1
            if not inside:
                stats["py_bin_outside"] += 1
            name = name or "?"
            for suf in (".cold", ".warm"):
                if name.endswith(suf):
                    name = name[: -len(suf)]
            r = (name, component_of_symbol(name), False)
        elif dso.startswith("/tmp/perf-") or dso in ("//anon", "[unknown]", ""):
            r = ("[jit/trampoline]", "interpreter", False) if dso.startswith("/tmp/perf-") or dso == "//anon" \
                else ("[unknown]", "unknown", False)
        else:
            comp = dso_component(dso)
            i = bisect.bisect_right(mstarts, ip) - 1
            name = None
            if i >= 0 and mmaps[i][0] <= ip < mmaps[i][1] and os.path.exists(mmaps[i][3]):
                e = elf(mmaps[i][3])
                off = a - mmaps[i][0] + mmaps[i][2]
                nm_, _ = e.lookup(e.offset_to_vaddr(off))
                if nm_ and e.has_symtab:
                    name = nm_
            label = os.path.basename(dso).split(".")[0]
            r = (f"{label}:{name or '?'}", comp, False)
        res_cache[key] = r
        return r

    return samples, resolve, stats


DISPATCH = {"calls", "abstract", "arg-parsing"}

# Reachability of a sample, decided from its C entry (the native function the bytecode
# called into) and its innermost Python frame:
#   kernel       entry is a builtin method / type slot / C-accelerator function that does
#                real work (str, bytes, re, json, datetime, ...): reachable behind that entry
#   thin         entry is a builtin/slot whose cost is per-call overhead or generic object
#                protocol (getattr, isinstance, len, type(), dict/list subscripts, ...)
#   stdlib-py    interpreter/object-model work inside a stdlib pure-Python function:
#                reachable only by replacing that function behind its Python entry point
#   ext-native   third-party or bundled native libraries (pydantic-core, markupsafe,
#                sqlite engine, aiohttp's llhttp, ...): not a target
#   os-runtime   sockets, select/epoll, locks/GIL, os, time, libc entries
#   interp       interpreter, calls, attribute lookup, allocation/GC, exceptions,
#                generators in third-party/workload code: plainly unreachable
KERNEL_COMPONENTS = {"str", "bytes", "int", "float", "re-engine", "json-accel", "datetime", "io", "struct",
                     "hashlib", "binascii", "codecs", "decimal", "pickle", "zlib", "random", "math",
                     "collections-accel", "operator", "itertools", "zstd", "functools-accel"}
NATIVE_LIB_COMPONENTS = {"sqlite-engine", "expat", "openssl"}
OS_COMPONENTS = {"socket", "select", "os", "locks", "gil", "time", "asyncio-accel", "libc", "libm", "vdso",
                 "ld.so", "kernel"}
THIN_ENTRIES = {"builtin_getattr", "builtin_hasattr", "builtin_setattr", "builtin_delattr", "builtin_isinstance",
                "builtin_issubclass", "builtin_len", "builtin_callable", "builtin_iter", "builtin_next",
                "builtin_id", "builtin_hash", "builtin_globals", "builtin_locals", "builtin_vars", "builtin_dir",
                "builtin___build_class__", "builtin___import__"}
KERNEL_CONTAINER_ENTRIES = {"list_sort", "list_sort_impl", "list_index", "list_count", "list_contains",
                            "tuple_index", "tuple_count", "tuplecontains", "list___contains__",
                            "builtin_sorted", "builtin_min", "builtin_max", "builtin_sum", "builtin_any",
                            "builtin_all", "builtin_format", "builtin_repr", "builtin_ascii", "builtin_ord",
                            "builtin_chr", "builtin_print", "builtin_round", "builtin_abs", "builtin_divmod",
                            "builtin_pow", "builtin_bin", "builtin_hex", "builtin_oct", "min_max"}


def reach_bucket(owner, entry_sym, entry_comp, leaf_comp=None):
    if entry_comp == "sqlite3-module":
        # stdlib _sqlite3 glue (row tuples, str decoding, converters) is reachable behind
        # Cursor.execute/fetch*; time inside the bundled sqlite engine is not.
        return "ext-native" if leaf_comp in ("sqlite-engine", "libc") else "kernel"
    if entry_comp in KERNEL_COMPONENTS or entry_sym in KERNEL_CONTAINER_ENTRIES:
        return "kernel"
    if entry_comp in NATIVE_LIB_COMPONENTS or entry_comp.startswith(("ext:", "so:", "c-module:")):
        return "ext-native"
    if entry_comp in OS_COMPONENTS:
        return "os-runtime"
    if entry_sym in THIN_ENTRIES or entry_comp in ("dict", "list/tuple/set", "builtins", "contextvars", "weakref"):
        return "thin"
    if owner.startswith("stdlib:"):
        return "stdlib-py"
    return "interp"


def analyze_perf(data_path, workload_file):
    python_bin = os.path.realpath(sys.executable)
    samples, resolve, stats = _parse_perf(data_path, workload_file, python_bin)
    n = len(samples)
    owner_c, comp_c, cell_c = collections.Counter(), collections.Counter(), collections.Counter()
    pyself_c, leaf_c, entry_c = collections.Counter(), collections.Counter(), collections.Counter()
    entry_owner_c, reach_c, reach_owner_c = collections.Counter(), collections.Counter(), collections.Counter()
    owner_entry_py = collections.Counter()
    owner_cache = {}
    for chain in samples:
        if not chain:
            continue
        frames = [resolve(ip, sym, dso, i == 0) for i, (ip, sym, dso) in enumerate(chain)]
        leaf_sym, leaf_comp, leaf_is_py = frames[0]
        if leaf_is_py:
            leaf_sym, leaf_comp = "[py trampoline]", "interpreter"
        elif leaf_comp == "other-cpython":
            # static helpers of bundled libraries (sqlite's vdbe*/btree*/pcache*, expat, ...)
            # carry no prefix: inherit the library from the nearest classified caller.
            for sym2, comp2, is_py2 in frames[1:]:
                if is_py2 or comp2 == "interpreter":
                    break
                if comp2 in NATIVE_LIB_COMPONENTS or comp2 in ("zlib", "decimal", "hashlib"):
                    leaf_comp = comp2
                    break
        elif leaf_comp == "libc" and len(frames) > 1:
            # libc has no symtab here (internal memcpy/strlen variants are not in .dynsym):
            # name the libc sample by its caller instead.
            leaf_sym = "libc<-" + frames[1][0]
        # innermost Python frame
        owner, pyfunc = "<no-python-frame>", "<no-python-frame>"
        for sym, comp, is_py in frames:
            if is_py:
                o = owner_cache.get(sym)
                if o is None:
                    qual, _, fn = sym[4:].partition(":")
                    o = owner_cache[sym] = (owner_of(fn, workload_file), f"{short_file(fn)}:{qual}")
                owner, pyfunc = o
                break
        # C entry: first non-dispatch native frame below the innermost interpreter frame
        k = None
        for i, (sym, comp, is_py) in enumerate(frames):
            if comp == "interpreter" or is_py:
                k = i
                break
        if k == 0:
            entry = ("<interpreter>", "interpreter")
        else:
            top = (k if k is not None else len(frames)) - 1
            j = top
            while j > 0 and frames[j][1] in DISPATCH:
                j -= 1
            entry = (frames[j][0], frames[j][1])
        owner_c[owner] += 1
        comp_c[leaf_comp] += 1
        cell_c[(owner, leaf_comp)] += 1
        pyself_c[pyfunc] += 1
        leaf_c[(leaf_sym, leaf_comp)] += 1
        entry_c[entry] += 1
        entry_owner_c[(owner, entry[0], entry[1])] += 1
        b = reach_bucket(owner, entry[0], entry[1], leaf_comp)
        reach_c[b] += 1
        okind = owner.split(":")[0] if owner.startswith(("stdlib:", "thirdparty:")) else owner
        reach_owner_c[(b, okind)] += 1
        if owner.startswith("stdlib:"):
            owner_entry_py[(pyfunc, b)] += 1

    def share(c, total=n):
        return round(c / total, 5) if total else 0.0

    return {
        "samples": n,
        "python_owner": {k: share(v) for k, v in owner_c.most_common()},
        "native_component": {k: share(v) for k, v in comp_c.most_common()},
        "matrix": [{"python_owner": o, "native_component": c, "share": share(v)}
                   for (o, c), v in cell_c.most_common(60)],
        "top_python_self": [{"func": f, "share": share(v)} for f, v in pyself_c.most_common(60)],
        "top_native": [{"symbol": s, "component": c, "share": share(v)} for (s, c), v in leaf_c.most_common(60)],
        "top_native_entry": [{"symbol": s, "component": c, "share": share(v)} for (s, c), v in entry_c.most_common(60)],
        "entry_matrix": [{"python_owner": o, "symbol": sy, "component": c, "share": share(v)}
                         for (o, sy, c), v in entry_owner_c.most_common(100)],
        "reachability": {k: share(v) for k, v in reach_c.most_common()},
        "reachability_by_owner_kind": [{"bucket": b, "owner_kind": o, "share": share(v)}
                                       for (b, o), v in reach_owner_c.most_common()],
        "stdlib_functions": [{"func": f, "bucket": b, "share": share(v)}
                             for (f, b), v in owner_entry_py.most_common(60)],
        "symbolization": {"python_binary_frames": stats["py_bin_frames"],
                          "outside_symbol_size": stats["py_bin_outside"]},
    }


def mode_perf(workload_path, user_seconds, keep_data=None):
    tmpdir = tempfile.mkdtemp(prefix="wlperf-")
    data = keep_data or os.path.join(tmpdir, "perf.data")
    child = subprocess.Popen([sys.executable, "-X", "perf", os.path.abspath(__file__), "_child-perf",
                              workload_path, str(user_seconds)],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    line = child.stdout.readline()
    if line.strip() != "READY":
        raise RuntimeError("child failed: " + line)
    perf = subprocess.Popen(["perf", "record", "-q", "-e", "cpu-clock", "-F", str(PERF_FREQ), "-g",
                             "-p", str(child.pid), "-o", data],
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    time.sleep(0.6)
    child.stdin.write("GO\n")
    child.stdin.flush()
    line = child.stdout.readline()
    perf.send_signal(signal.SIGINT)
    perf.wait()
    info = json.loads(line[5:])
    child.stdin.write("DIGEST\n")
    child.stdin.flush()
    info.update(json.loads(child.stdout.readline()))
    info["pid"] = child.pid
    child.stdin.write("EXIT\n")
    child.stdin.flush()
    child.wait()
    res = analyze_perf(data, os.path.realpath(workload_path))
    res["perf_run"] = info
    if not keep_data:
        try:
            os.remove(data)
            os.rmdir(tmpdir)
        except OSError:
            pass
    return res


# --------------------------------------------------------------------------------------
# cProfile mode
# --------------------------------------------------------------------------------------

_BUILTIN_NAME_RES = [
    (re.compile(r"^<method '([^']+)' of '([^']+)' objects>$"), lambda m: f"{m.group(2)}.{m.group(1)}"),
    (re.compile(r"^<built-in method builtins\.(\w+)>$"), lambda m: f"builtins.{m.group(1)}"),
    (re.compile(r"^<built-in method ([\w.]+)>$"), lambda m: m.group(1)),
    (re.compile(r"^<built-in method (\w+) of ([\w.]+) object at 0x[0-9a-f]+>$"),
     lambda m: f"{m.group(2)}.{m.group(1)}"),
    (re.compile(r"^<function (\S+) at 0x[0-9a-f]+>$"), lambda m: m.group(1)),
]


def norm_builtin(name):
    for rx, f in _BUILTIN_NAME_RES:
        m = rx.match(name)
        if m:
            return f(m)
    return name


def mode_cprofile(workload_path):
    import cProfile
    import pstats

    mod = load_workload(workload_path)
    state = mod.setup()
    mod.run(state)
    plain = []
    for _ in range(3):
        t = time.perf_counter()
        mod.run(state)
        plain.append(time.perf_counter() - t)
    prof = cProfile.Profile()
    t = time.perf_counter()
    prof.enable()
    mod.run(state)
    prof.disable()
    t_prof = time.perf_counter() - t
    st = pstats.Stats(prof)
    wf = os.path.realpath(workload_path)
    total_tt = sum(v[2] for v in st.stats.values()) or 1e-12
    builtin_rows, py_rows = [], []
    owner_tt = collections.Counter()
    total_calls = builtin_calls = 0
    for (fn, line, name), (cc, nc, tt, ct, _callers) in st.stats.items():
        total_calls += nc
        if fn == "~":
            builtin_calls += nc
            builtin_rows.append({"name": norm_builtin(name), "raw": name, "calls": nc,
                                 "tottime_s": round(tt, 6), "tottime_share": round(tt / total_tt, 5)})
            owner_tt["<builtin C calls>"] += tt
        else:
            o = owner_of(fn, wf)
            owner_tt[o] += tt
            py_rows.append({"func": f"{short_file(fn)}:{line}:{name}", "owner": o, "calls": nc,
                            "tottime_s": round(tt, 6), "cumtime_s": round(ct, 6),
                            "tottime_share": round(tt / total_tt, 5)})
    builtin_rows.sort(key=lambda r: -r["tottime_s"])
    py_rows.sort(key=lambda r: -r["tottime_s"])
    by_calls = sorted(builtin_rows, key=lambda r: -r["calls"])[:30]
    plain.sort()
    return {
        "wall_s": round(plain[1], 4), "plain_run_times": [round(x, 4) for x in plain],
        "cprofile_run_s": round(t_prof, 4), "cprofile_total_tottime_s": round(total_tt, 4),
        "cprofile_total_calls": total_calls, "cprofile_builtin_call_total": builtin_calls,
        "cprofile_builtin_calls": builtin_rows[:60],
        "cprofile_builtin_by_calls": [{"name": r["name"], "calls": r["calls"],
                                       "tottime_s": r["tottime_s"]} for r in by_calls],
        "cprofile_top": py_rows[:60],
        "cprofile_owner_tottime": {k: round(v / total_tt, 5) for k, v in owner_tt.most_common()},
    }


# --------------------------------------------------------------------------------------
# argsizes mode (sys.monitoring)
# --------------------------------------------------------------------------------------

_SIMPLE_LOADS = {"LOAD_FAST", "LOAD_FAST_BORROW", "LOAD_FAST_CHECK", "LOAD_DEREF", "LOAD_GLOBAL",
                 "LOAD_NAME", "LOAD_CONST", "LOAD_SMALL_INT", "LOAD_COMMON_CONSTANT"}
_PAIR_LOADS = {"LOAD_FAST_LOAD_FAST", "LOAD_FAST_BORROW_LOAD_FAST_BORROW"}


def _size(x):
    t = type(x)
    if t is str or t is bytes or t is bytearray or t is list or t is tuple or t is dict \
            or t is set or t is frozenset or t is memoryview:
        return len(x)
    if t is int:
        return x.bit_length()
    try:
        if isinstance(x, (str, bytes, list, tuple, dict, set)):
            return len(x)
    except Exception:
        pass
    return None


def _hist(values):
    h = collections.Counter()
    for v in values:
        b = "0" if v == 0 else (f"{1 << (v.bit_length() - 1)}-{(1 << v.bit_length()) - 1}" if v > 1 else "1")
        h[b] += 1
    return dict(sorted(h.items(), key=lambda kv: int(kv[0].split("-")[0])))


def summarize(values):
    if not values:
        return None
    v = sorted(values)
    n = len(v)

    def q(p):
        return v[min(n - 1, int(p * n))]

    return {"n": n, "p50": q(0.5), "p90": q(0.9), "p99": q(0.99), "max": v[-1], "histogram": _hist(v)}


_PREFERRED_PARAMS = ("string", "s", "text", "data", "value", "obj", "source", "line", "url", "msg",
                     "header", "name", "fmt", "b", "payload", "body", "content")


def _pick_param(code):
    """Index of the parameter whose size we record for a stdlib Python function."""
    n = code.co_argcount + code.co_kwonlyargcount
    names = code.co_varnames[:n]
    cand = [i for i, nm in enumerate(names) if nm not in ("self", "cls")]
    if not cand:
        return -1
    for i in cand:
        if names[i] in _PREFERRED_PARAMS:
            return i
    return cand[0]


def callable_key(c):
    t = type(c)
    tn = t.__name__
    try:
        if tn == "method_descriptor" or tn == "classmethod_descriptor" or tn == "wrapper_descriptor":
            oc = c.__objclass__
            mod = oc.__module__
            return (f"{oc.__qualname__}.{c.__name__}" if mod == "builtins"
                    else f"{mod}.{oc.__qualname__}.{c.__name__}"), "descr"
        if tn == "builtin_function_or_method":
            s = c.__self__
            if s is None or type(s).__name__ == "module":
                return f"{getattr(c, '__module__', None) or getattr(s, '__name__', '?')}.{c.__name__}", "func"
            st = type(s)
            if st is type:
                return f"{s.__qualname__}.{c.__name__}", "func"
            mod = st.__module__
            return (f"{st.__qualname__}.{c.__name__}" if mod == "builtins"
                    else f"{mod}.{st.__qualname__}.{c.__name__}"), "bound"
        if tn == "method-wrapper":
            st = type(c.__self__)
            return f"{st.__qualname__}.{c.__name__}", "bound"
        if t is type:
            return f"type:{c.__module__}.{c.__qualname__}()" if c.__module__ != "builtins" \
                else f"type:{c.__qualname__}()", "type"
        if tn in ("function", "method", "cell"):
            return None, None
        mod = t.__module__
        if hasattr(t, "__call__") and not hasattr(c, "__code__"):
            return (f"{t.__qualname__}()" if mod == "builtins" else f"{mod}.{t.__qualname__}()"), "obj"
    except Exception:
        return None, None
    return None, None


def mode_argsizes(workload_path, max_runs=1):
    import dis

    mod = load_workload(workload_path)
    state = mod.setup()
    mod.run(state)
    wf = os.path.realpath(workload_path)
    M = sys.monitoring
    TOOL = 4
    M.use_tool_id(TOOL, "wl-argsizes")
    DISABLE = M.DISABLE
    instr_cache = {}
    calls = collections.Counter()
    self_sz = collections.defaultdict(list)
    arg1_sz = collections.defaultdict(list)
    join_total = collections.defaultdict(list)
    resolved = collections.Counter()
    callers = collections.defaultdict(collections.Counter)
    py_arg = collections.defaultdict(list)
    py_ret = collections.defaultdict(list)
    py_calls = collections.Counter()
    owner_cache = {}
    getframe = sys._getframe

    def resolver_for(code, off):
        key = (code, off)
        r = instr_cache.get(key)
        if r is not None or key in instr_cache:
            return r
        r = None
        try:
            ins = [i for i in dis.get_instructions(code)]
            idx = next(i for i, x in enumerate(ins) if x.offset == off)
            call = ins[idx]
            if call.opname in ("CALL", "CALL_KW"):
                nargs = call.arg
                pushes = []
                j = idx - 1
                if call.opname == "CALL_KW":
                    j -= 1  # kwnames tuple
                while j >= 0 and len(pushes) < nargs:
                    x = ins[j]
                    if x.opname in _SIMPLE_LOADS and not (x.opname == "LOAD_GLOBAL" and (x.arg & 1)):
                        pushes.append((x.opname, x.argval))
                    elif x.opname in _PAIR_LOADS:
                        a, b = x.argval
                        pushes.append(("LOAD_FAST", b))
                        pushes.append(("LOAD_FAST", a))
                    elif (x.opname == "LOAD_ATTR" and not (x.arg & 1) and j > 0
                          and ins[j - 1].opname in ("LOAD_FAST", "LOAD_FAST_BORROW")):
                        pushes.append(("ATTR", (ins[j - 1].argval, x.argval)))
                        j -= 1
                    else:
                        break
                    j -= 1
                if nargs >= 1 and len(pushes) >= nargs:
                    r = pushes[nargs - 1]
        except Exception:
            r = None
        instr_cache[key] = r
        return r

    def fetch(frame, r):
        op, v = r
        if op in ("LOAD_CONST", "LOAD_SMALL_INT", "LOAD_COMMON_CONSTANT"):
            return v
        if op == "LOAD_GLOBAL" or op == "LOAD_NAME":
            g = frame.f_globals
            return g[v] if v in g else frame.f_builtins.get(v)
        if op == "ATTR":
            base, attr = v
            obj = frame.f_locals[base]
            d = getattr(obj, "__dict__", None)
            if d is not None and attr in d:
                return d[attr]
            raise KeyError(attr)
        return frame.f_locals[v]

    def on_call(code, off, c, arg0):
        key, kind = callable_key(c)
        if key is None:
            return
        calls[key] += 1
        if kind in ("descr",):
            s = _size(arg0)
            if s is not None:
                self_sz[key].append(s)
            r = resolver_for(code, off)
            if r is not None:
                try:
                    a1 = fetch(getframe(1), r)
                    resolved[key] += 1
                    s1 = _size(a1)
                    if s1 is not None:
                        arg1_sz[key].append(s1)
                        if key.endswith(".join") and type(a1) in (list, tuple):
                            join_total[key].append(sum(len(x) for x in a1 if type(x) in (str, bytes)))
                except Exception:
                    pass
        elif kind == "bound":
            try:
                s = _size(c.__self__)
                if s is not None:
                    self_sz[key].append(s)
            except Exception:
                pass
            s1 = _size(arg0)
            if s1 is not None:
                resolved[key] += 1
                arg1_sz[key].append(s1)
                if key.endswith(".join") and type(arg0) in (list, tuple):
                    join_total[key].append(sum(len(x) for x in arg0 if type(x) in (str, bytes)))
        else:
            s1 = _size(arg0)
            if s1 is not None:
                resolved[key] += 1
                arg1_sz[key].append(s1)
        o = owner_cache.get(code)
        if o is None:
            o = owner_cache[code] = owner_of(code.co_filename, wf)
        callers[key][o] += 1

    stdlib_codes = {}
    py_param = {}

    def py_key(code):
        k = stdlib_codes.get(code)
        if k is None:
            fn = code.co_filename
            o = owner_of(fn, wf)
            k = stdlib_codes[code] = (f"{short_file(fn)}:{code.co_qualname}" if o.startswith("stdlib:") else "")
        return k

    def on_start(code, off):
        k = py_key(code)
        if not k:
            return DISABLE
        py_calls[k] += 1
        idx = py_param.get(code)
        if idx is None:
            idx = py_param[code] = _pick_param(code)
        if idx >= 0:
            try:
                v = getframe(1).f_locals[code.co_varnames[idx]]
                s = _size(v)
                if s is not None:
                    py_arg[k].append(s)
            except Exception:
                pass

    def on_return(code, off, val):
        k = py_key(code)
        if not k:
            return DISABLE
        s = _size(val)
        if s is not None and type(val) in (str, bytes):
            py_ret[k].append(s)

    E = M.events
    M.register_callback(TOOL, E.CALL, on_call)
    M.register_callback(TOOL, E.PY_START, on_start)
    M.register_callback(TOOL, E.PY_RETURN, on_return)
    t = time.perf_counter()
    M.set_events(TOOL, E.CALL | E.PY_START | E.PY_RETURN)
    try:
        for _ in range(max_runs):
            mod.run(state)
    finally:
        M.set_events(TOOL, 0)
        M.free_tool_id(TOOL)
    elapsed = time.perf_counter() - t

    builtin = {}
    for key, n in calls.most_common(150):
        builtin[key] = {
            "calls": n,
            "callers": dict(callers[key].most_common(4)),
            "self_len": summarize(self_sz.get(key, [])),
            "arg1_len": summarize(arg1_sz.get(key, [])),
            "arg1_resolved_frac": round(resolved[key] / n, 3),
        }
        if key in join_total:
            builtin[key]["join_total_chars"] = summarize(join_total[key])
    stdlib = {}
    for key, n in py_calls.most_common(120):
        stdlib[key] = {"calls": n, "arg1_len": summarize(py_arg.get(key, [])),
                       "ret_len": summarize(py_ret.get(key, []))}
    return {"argsizes_run_s": round(elapsed, 3), "c_calls": builtin, "stdlib_py_calls": stdlib}


# --------------------------------------------------------------------------------------
# Orchestration
# --------------------------------------------------------------------------------------


def _sub(mode, workload, extra=()):
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        tmp = f.name
    subprocess.run([sys.executable, os.path.abspath(__file__), mode, workload, "--out", tmp, *extra],
                   check=True)
    with open(tmp) as f:
        res = json.load(f)
    os.remove(tmp)
    return res


# Builtins whose cost does not depend on an argument's size (per-call overhead only).
SIZE_INSENSITIVE = {
    "builtins.isinstance", "builtins.issubclass", "builtins.getattr", "builtins.setattr",
    "builtins.hasattr", "builtins.callable", "builtins.len", "builtins.iter", "builtins.next",
    "builtins.id", "builtins.hash", "builtins.globals", "builtins.locals", "builtins.vars",
    "builtins.delattr", "list.append", "list.pop", "dict.get", "dict.pop", "dict.setdefault",
    "dict.items", "dict.keys", "dict.values", "dict.__contains__", "set.add", "set.discard",
    "list.extend", "dict.update", "dict.copy", "deque.append", "deque.popleft", "collections.deque.append",
    "collections.deque.popleft", "object.__new__", "object.__init__", "type.__new__",
}


SIZE_INSENSITIVE_PREFIXES = ("_thread.", "time.", "_contextvars.", "select.", "_socket.", "posix.",
                             "_asyncio.", "collections.deque.", "_abc.", "_weakref", "type:")


def pick_arg_hotspots(prof, n=10):
    """Top builtin hotspots (cProfile tottime) + top stdlib Python functions (perf)."""
    keys = []
    cs = prof["argsizes"]["c_calls"]
    for r in prof["cprofile_builtin_calls"]:
        if r["name"] in SIZE_INSENSITIVE or r["name"].startswith(SIZE_INSENSITIVE_PREFIXES):
            continue
        if r["name"] in cs and r["name"] not in keys:
            keys.append(r["name"])
        if len(keys) >= 6:
            break
    ss = prof["argsizes"]["stdlib_py_calls"]
    for r in prof["top_python_self"]:
        f = r["func"]
        if f in ss and f not in keys:
            keys.append(f)
        if len(keys) >= n:
            break
    out = {}
    for k in keys:
        if k in cs:
            d = cs[k]
            measure = "arg1_len" if (k.endswith(".join") or k.startswith("re.Pattern.")
                                     or k.startswith("builtins.") or d["self_len"] is None) else "self_len"
            s = d.get(measure) or d.get("self_len") or d.get("arg1_len")
            if s:
                out[k] = {**s, "measure": measure, "calls": d["calls"]}
        else:
            d = ss[k]
            s = d.get("arg1_len") or d.get("ret_len")
            if s:
                out[k] = {**s, "measure": "arg1_len" if d.get("arg1_len") else "ret_len", "calls": d["calls"]}
    return out


def mode_all(workload, out, user_seconds):
    name = os.path.splitext(os.path.basename(workload))[0]
    t0 = time.time()
    keep = os.environ.get("WL_PERF_DATA_DIR")
    extra = ["--user-seconds", str(user_seconds)]
    if keep:
        os.makedirs(keep, exist_ok=True)
        extra += ["--keep-perf-data", os.path.join(keep, name + ".perf.data")]
    perf = _sub("perf", workload, extra)
    cp = _sub("cprofile", workload)
    arg = _sub("argsizes", workload)
    pr = perf["perf_run"]
    cpu = pr["utime"] + pr["stime"]
    prof = {
        "workload": name,
        "family": os.path.basename(os.path.dirname(os.path.realpath(workload))),
        "python": sys.version.split()[0] + (" (free-threaded)" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""),
        "samples": perf["samples"],
        "wall_s": cp["wall_s"],
        "python_owner": perf["python_owner"],
        "native_component": perf["native_component"],
        "matrix": perf["matrix"],
        "top_python_self": perf["top_python_self"],
        "top_native": perf["top_native"],
        "cprofile_builtin_calls": cp["cprofile_builtin_calls"],
        "arg_sizes": {},
        # extensions to the shared schema (documented in docs/atlas/workloads-web.md)
        "top_native_entry": perf["top_native_entry"],
        "entry_matrix": perf["entry_matrix"],
        "reachability": perf["reachability"],
        "reachability_by_owner_kind": perf["reachability_by_owner_kind"],
        "stdlib_functions": perf["stdlib_functions"],
        "cpu_user_s": round(pr["utime"], 3), "cpu_sys_s": round(pr["stime"], 3),
        "kernel_share_rusage": round(pr["stime"] / cpu, 4) if cpu else 0.0,
        "perf_runs": pr["runs"], "perf_run_s_median": round(sorted(pr["run_times"])[len(pr["run_times"]) // 2], 4),
        "perf_overhead": round(sorted(pr["run_times"])[len(pr["run_times"]) // 2] / cp["wall_s"], 3),
        "digest": pr["digest"], "digest_stable": pr["digest_stable"],
        "symbolization": perf["symbolization"],
        "plain_run_times": cp["plain_run_times"],
        "cprofile_run_s": cp["cprofile_run_s"], "cprofile_total_calls": cp["cprofile_total_calls"],
        "cprofile_builtin_call_total": cp["cprofile_builtin_call_total"],
        "cprofile_builtin_by_calls": cp["cprofile_builtin_by_calls"],
        "cprofile_top": cp["cprofile_top"], "cprofile_owner_tottime": cp["cprofile_owner_tottime"],
        "argsizes": arg,
        "profiled_at": time.strftime("%Y-%m-%dT%H:%M:%S"), "profile_wall_s": round(time.time() - t0, 1),
    }
    prof["arg_sizes"] = pick_arg_hotspots(prof)
    prof["argsizes_run_s"] = arg["argsizes_run_s"]
    prof["arg_sizes_all"] = {"c_calls": arg["c_calls"], "stdlib_py_calls": arg["stdlib_py_calls"]}
    del prof["argsizes"]
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w") as f:
        json.dump(prof, f, indent=1)
    print(f"{name}: {prof['samples']} samples, wall {prof['wall_s']}s, -X perf x{prof['perf_overhead']}, "
          f"kernel {prof['kernel_share_rusage']:.1%} -> {out}")


def mode_time(workload, n):
    mod = load_workload(workload)
    t = time.perf_counter()
    state = mod.setup()
    print(f"setup {time.perf_counter() - t:.3f}s")
    ds = set()
    for i in range(n + 1):
        t = time.perf_counter()
        r = mod.run(state)
        el = time.perf_counter() - t
        d = mod.digest(r)
        ds.add(d)
        print(f"run{i} {el:.3f}s {d[:16]}")
    print("digest stable" if len(ds) == 1 else "DIGEST UNSTABLE")


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "_child-perf":
        child_perf(sys.argv[2], float(sys.argv[3]))
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["all", "perf", "cprofile", "argsizes", "time", "analyze", "repick", "reanalyze"])
    ap.add_argument("workload")
    ap.add_argument("--out")
    ap.add_argument("--user-seconds", type=float, default=3.2)
    ap.add_argument("-n", type=int, default=3)
    ap.add_argument("--keep-perf-data")
    a = ap.parse_args()
    if a.mode == "time":
        mode_time(a.workload, a.n)
        return
    if a.mode == "all":
        mode_all(a.workload, a.out, a.user_seconds)
        return
    if a.mode == "reanalyze":  # workload arg = profile JSON; --keep-perf-data = its perf.data
        with open(a.workload) as f:
            prof = json.load(f)
        wl = os.path.join(WORKLOAD_DIR, prof["workload"] + ".py")
        res = analyze_perf(a.keep_perf_data, os.path.realpath(wl))
        for k in ("samples", "python_owner", "native_component", "matrix", "top_python_self", "top_native",
                  "top_native_entry", "entry_matrix", "reachability", "reachability_by_owner_kind",
                  "stdlib_functions", "symbolization"):
            prof[k] = res[k]
        with open(a.workload, "w") as f:
            json.dump(prof, f, indent=1)
        return
    if a.mode == "repick":  # recompute arg_sizes of an existing profile JSON (workload arg = JSON path)
        with open(a.workload) as f:
            prof = json.load(f)
        prof["argsizes"] = prof["arg_sizes_all"]
        prof["arg_sizes"] = pick_arg_hotspots(prof)
        del prof["argsizes"]
        with open(a.workload, "w") as f:
            json.dump(prof, f, indent=1)
        return
    if a.mode == "perf":
        res = mode_perf(a.workload, a.user_seconds, a.keep_perf_data)
    elif a.mode == "analyze":  # re-analyze a kept perf.data: workload arg = perf.data path
        res = analyze_perf(a.workload, "")
    elif a.mode == "cprofile":
        res = mode_cprofile(a.workload)
    else:
        res = mode_argsizes(a.workload)
    with open(a.out, "w") as f:
        json.dump(res, f, indent=1)


if __name__ == "__main__":
    main()
