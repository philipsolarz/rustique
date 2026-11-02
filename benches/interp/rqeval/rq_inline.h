/* Variant B of the copied loop: local copies of the core helpers that the PBS binary inlines
 * into its handlers through LTO+PGO but that an extension can only call out of line. Each is
 * the 3.15.1 source of the helper restricted to its common case; every other case calls the
 * core's own function, so behaviour is unchanged. Enabled with -DRQ_INLINE_HELPERS. */
#ifndef RQ_INLINE_H
#define RQ_INLINE_H
/* The main interpreter, set at module init. Swapped handlers are process-global (the table is
 * a static of the binary), so every override fast path runs only in the main interpreter: its
 * caches are keyed by type version tags, which heap types of different interpreters can share,
 * and its captured descriptors belong to the main interpreter (found by regrtest:
 * test_interpreter_pool / test_datetime crashed in a subinterpreter before this check). */
static PyInterpreterState *rq_main_interp;
#ifdef RQ_INLINE_HELPERS
#include "pycore_freelist.h"

/* Python/ceval.c _PyEval_FrameClearAndPop + clear_thread_frame, Python/frame.c
 * _PyFrame_ClearExceptCode/_PyFrame_ClearLocals, Python/pystate.c _PyThreadState_PopFrame. */
static inline __attribute__((always_inline)) void
rq_FrameClearAndPop(PyThreadState *tstate, _PyInterpreterFrame *frame)
{
    if (frame->owner != FRAME_OWNED_BY_THREAD || frame->frame_obj != NULL
        || tstate->last_profiled_frame == frame) {
        _PyEval_FrameClearAndPop(tstate, frame);
        return;
    }
    _PyStackRef *sp = frame->stackpointer;
    _PyStackRef *locals = frame->localsplus;
    frame->stackpointer = locals;
    while (sp > locals) {
        sp--;
        PyStackRef_XCLOSE(*sp);
    }
    Py_CLEAR(frame->f_locals);
    PyStackRef_CLEAR(frame->f_funcobj);
    PyStackRef_CLEAR(frame->f_executable);
    PyObject **base = (PyObject **)frame;
    if (base == &tstate->datastack_chunk->data[0]) {
        _PyThreadState_PopFrame(tstate, frame);
    }
    else {
        tstate->datastack_top = base;
    }
}
#define _PyEval_FrameClearAndPop rq_FrameClearAndPop

/* Objects/longobject.c medium_from_stwodigits / _PyCompactLong_{Add,Subtract,Multiply}. */
static inline __attribute__((always_inline)) _PyStackRef
rq_medium_from_stwodigits(stwodigits x)
{
    if (_PY_IS_SMALL_INT(x)) {
        return PyStackRef_FromPyObjectBorrow((PyObject *)&_PyLong_SMALL_INTS[_PY_NSMALLNEGINTS + x]);
    }
    twodigits x_plus_mask = ((twodigits)x) + PyLong_MASK;
    if (!(x_plus_mask < ((twodigits)PyLong_MASK) + PyLong_BASE)) {
        return PyStackRef_NULL;
    }
    PyLongObject *v = (PyLongObject *)_Py_FREELIST_POP(PyLongObject, ints);
    if (v == NULL) {
        v = PyObject_Malloc(sizeof(PyLongObject));
        if (v == NULL) {
            return PyStackRef_NULL;
        }
        _PyObject_Init((PyObject *)v, &PyLong_Type);
        _PyLong_InitTag(v);
    }
    digit abs_x = x < 0 ? (digit)(-x) : (digit)x;
    _PyLong_SetSignAndDigitCount(v, x < 0 ? -1 : 1, 1);
    v->long_value.ob_digit[0] = abs_x;
    return PyStackRef_FromPyObjectStealMortal((PyObject *)v);
}
#define _PyCompactLong_Add(a, b) rq_medium_from_stwodigits(_PyLong_CompactValue(a) + _PyLong_CompactValue(b))
#define _PyCompactLong_Subtract(a, b) rq_medium_from_stwodigits(_PyLong_CompactValue(a) - _PyLong_CompactValue(b))
#define _PyCompactLong_Multiply(a, b) rq_medium_from_stwodigits((stwodigits)_PyLong_CompactValue(a) * _PyLong_CompactValue(b))

#endif
#endif
