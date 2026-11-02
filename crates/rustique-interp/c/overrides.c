// Rustique interp tier: the override file for CPython's cases generator.
//
// tools/gen.py runs Tools/cases_generator/tier1_generator.py of a CPython patch release on
// that release's Python/bytecodes.c followed by this file. Each `override op(...)` below
// replaces one op of bytecodes.c; its body is CPython's original with one helper call
// wrapped (the wrapper's fallback is the original helper, called with the same arguments).
// tools/gen.py refuses to generate when the original body of an overridden op differs from
// the one recorded in overrides.lock: the override must then be re-proven.
//
// Everything above "BEGIN BYTECODES" is the dummy preamble of bytecodes.c (needed by the
// generator's parser). PSF licence (see gen/LICENSE).

#include "Python.h"
#include "pycore_abstract.h"      // _PyIndex_Check()
#include "pycore_audit.h"         // _PySys_Audit()
#include "pycore_backoff.h"
#include "pycore_cell.h"          // PyCell_GetRef()
#include "pycore_code.h"
#include "pycore_emscripten_signal.h"  // _Py_CHECK_EMSCRIPTEN_SIGNALS
#include "pycore_function.h"
#include "pycore_instruments.h"
#include "pycore_interpolation.h" // _PyInterpolation_Build()
#include "pycore_intrinsics.h"
#include "pycore_long.h"          // _PyLong_ExactDealloc(), _PyLong_GetZero()
#include "pycore_moduleobject.h"  // PyModuleObject
#include "pycore_object.h"        // _PyObject_GC_TRACK()
#include "pycore_opcode_metadata.h"  // uop names
#include "pycore_opcode_utils.h"  // MAKE_FUNCTION_*
#include "pycore_pyatomic_ft_wrappers.h" // FT_ATOMIC_*
#include "pycore_pyerrors.h"      // _PyErr_GetRaisedException()
#include "pycore_pystate.h"       // _PyInterpreterState_GET()
#include "pycore_range.h"         // _PyRangeIterObject
#include "pycore_setobject.h"     // _PySet_NextEntry()
#include "pycore_sliceobject.h"   // _PyBuildSlice_ConsumeRefs
#include "pycore_stackref.h"
#include "pycore_template.h"      // _PyTemplate_Build()
#include "pycore_tuple.h"         // _PyTuple_ITEMS()
#include "pycore_typeobject.h"    // _PySuper_Lookup()

#include "pycore_dict.h"
#include "dictobject.h"
#include "pycore_frame.h"
#include "opcode.h"
#include "optimizer.h"
#include "pydtrace.h"
#include "setobject.h"


#define USE_COMPUTED_GOTOS 0
#include "ceval_macros.h"

/* Flow control macros */

#define inst(name, ...) case name:
#define op(name, ...) /* NAME is ignored */
#define macro(name) static int MACRO_##name
#define super(name) static int SUPER_##name
#define family(name, ...) static int family_##name
#define pseudo(name) static int pseudo_##name
#define label(name) name:

/* Annotations */
#define guard
#define override
#define specializing
#define replicate(TIMES)
#define tier1
#define no_save_ip

// Dummy variables for stack effects.
static PyObject *value, *value1, *value2, *left, *right, *res, *sum, *prod, *sub;
static PyObject *container, *start, *stop, *v, *lhs, *rhs, *res2;
static PyObject *list, *tuple, *dict, *owner, *set, *str, *tup, *map, *keys;
static PyObject *exit_func, *lasti, *val, *retval, *obj, *iter, *exhausted;
static PyObject *aiter, *awaitable, *iterable, *w, *exc_value, *bc, *locals;
static PyObject *orig, *excs, *update, *b, *fromlist, *level, *from;
static PyObject **pieces, **values;
static size_t jump;
// Dummy variables for cache effects
static uint16_t invert, counter, index, hint;
#define unused 0  // Used in a macro def, can't be static
static uint32_t type_version;
static _PyExecutorObject *current_executor;

static PyObject *
dummy_func(
    PyThreadState *tstate,
    _PyInterpreterFrame *frame,
    unsigned char opcode,
    unsigned int oparg,
    _Py_CODEUNIT *next_instr,
    PyObject **stack_pointer,
    int throwflag,
    PyObject *args[]
)
{
    // Dummy labels.
    pop_1_error:
    // Dummy locals.
    PyObject *dummy;
    _Py_CODEUNIT *this_instr;
    PyObject *attr;
    PyObject *attrs;
    PyObject *bottom;
    PyObject *callable;
    PyObject *callargs;
    PyObject *codeobj;
    PyObject *cond;
    PyObject *descr;
    PyObject *exc;
    PyObject *exit;
    PyObject *fget;
    PyObject *fmt_spec;
    PyObject *func;
    uint32_t func_version;
    PyObject *getattribute;
    PyObject *kwargs;
    PyObject *kwdefaults;
    PyObject *len_o;
    PyObject *match;
    PyObject *match_type;
    PyObject *method;
    PyObject *mgr;
    Py_ssize_t min_args;
    PyObject *names;
    PyObject *new_exc;
    PyObject *next;
    PyObject *none;
    PyObject *null;
    PyObject *prev_exc;
    PyObject *receiver;
    PyObject *rest;
    int result;
    PyObject *self;
    PyObject *seq;
    PyObject *slice;
    PyObject *step;
    PyObject *subject;
    PyObject *top;
    PyObject *type;
    PyObject *typevars;
    PyObject *val0;
    PyObject *val1;
    int values_or_none;

    switch (opcode) {

// BEGIN BYTECODES //

        // isinstance fast path (rq_isinstance.h: rq_isinstance_fast); otherwise identical to
        // Python/bytecodes.c _CALL_ISINSTANCE.
        override op(_CALL_ISINSTANCE, (callable, null, instance, cls -- res)) {
            /* isinstance(o, o2) */
            STAT_INC(CALL, hit);
            PyObject *inst_o = PyStackRef_AsPyObjectBorrow(instance);
            PyObject *cls_o = PyStackRef_AsPyObjectBorrow(cls);
            int retval = rq_isinstance_fast(tstate, inst_o, cls_o);
            if (retval == -2) {
                retval = PyObject_IsInstance(inst_o, cls_o);
            }
            if (retval < 0) {
                ERROR_NO_POP();
            }
            (void)null; // Silence compiler warnings about unused variables
            PyStackRef_CLOSE(cls);
            PyStackRef_CLOSE(instance);
            DEAD(null);
            PyStackRef_CLOSE(callable);
            res = retval ? PyStackRef_True : PyStackRef_False;
            assert((!PyStackRef_IsNull(res)) ^ (_PyErr_Occurred(tstate) != NULL));
        }

        // Megamorphic attribute cache on the generic LOAD_ATTR path (rq_attrcache.h); the
        // body is Python/bytecodes.c _LOAD_ATTR with the two helpers wrapped.
        override op(_LOAD_ATTR, (owner -- attr, self_or_null[oparg&1])) {
            PyObject *name = GETITEM(FRAME_CO_NAMES, oparg >> 1);
            if (oparg & 1) {
                /* Designed to work in tandem with CALL, pushes two values. */
                attr = rq_LoadAttr_StackRefSteal(tstate, owner, name, self_or_null, this_instr);
                DEAD(owner);
                ERROR_IF(PyStackRef_IsNull(attr));
            }
            else {
                /* Classic, pushes one value. */
                attr = rq_GetAttrStackRef(tstate, PyStackRef_AsPyObjectBorrow(owner), name, this_instr);
                PyStackRef_CLOSE(owner);
                ERROR_IF(PyStackRef_IsNull(attr));
            }
        }

        // object.__setattr__ fast path (rq_calls.h: rq_call_non_py); otherwise identical to
        // Python/bytecodes.c _CALL_NON_PY_GENERAL.
        override op(_CALL_NON_PY_GENERAL, (callable, self_or_null, args[oparg] -- res)) {
#if TIER_ONE
            assert(opcode != INSTRUMENTED_CALL);
#endif
            int total_args = oparg;
            _PyStackRef *arguments = args;
            if (!PyStackRef_IsNull(self_or_null)) {
                arguments--;
                total_args++;
            }
            PyObject *res_o = rq_call_non_py(
                tstate,
                callable,
                arguments,
                total_args);
            DEAD(args);
            DEAD(self_or_null);
            DEAD(callable);
            ERROR_IF(res_o == NULL);
            res = PyStackRef_FromPyObjectSteal(res_o);
        }

        // Keyword instantiation of plain Python classes pushed inline (rq_calls.h:
        // rq_kw_init_for / rq_push_kw_init); otherwise identical to Python/bytecodes.c
        // _CALL_KW_NON_PY.
        override op(_CALL_KW_NON_PY, (callable, self_or_null, args[oparg], kwnames -- res)) {
#if TIER_ONE
            assert(opcode != INSTRUMENTED_CALL);
#endif
            PyObject *init = PyStackRef_IsNull(self_or_null)
                ? rq_kw_init_for(tstate, PyStackRef_AsPyObjectBorrow(callable)) : NULL;
            if (init != NULL) {
                _PyInterpreterFrame *new_frame = rq_push_kw_init(
                    tstate, frame, callable, args - 1, oparg, kwnames, init);
                DEAD(kwnames);
                DEAD(args);
                DEAD(self_or_null);
                DEAD(callable);
                SYNC_SP();
                if (new_frame == NULL) {
                    ERROR_NO_POP();
                }
                frame->return_offset = INSTRUCTION_SIZE;
                DISPATCH_INLINED(new_frame);
            }
            int total_args = oparg;
            _PyStackRef *arguments = args;
            if (!PyStackRef_IsNull(self_or_null)) {
                arguments--;
                total_args++;
            }
            PyObject *res_o = _Py_VectorCall_StackRefSteal(
                callable,
                arguments,
                total_args,
                kwnames);
            DEAD(kwnames);
            DEAD(args);
            DEAD(self_or_null);
            DEAD(callable);
            ERROR_IF(res_o == NULL);
            res = PyStackRef_FromPyObjectSteal(res_o);
        }

        // END BYTECODES //

    }
 dispatch_opcode:
 error:
 exception_unwind:
 exit_unwind:
 handle_eval_breaker:
 resume_frame:
 start_frame:
 unbound_local_error:
    ;
}

// Future families go below this point //
