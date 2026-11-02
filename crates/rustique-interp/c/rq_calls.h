/* Call overrides (overrides.c): object.__setattr__ through CALL_NON_PY_GENERAL, and keyword
 * instantiation of plain Python classes through CALL_KW_NON_PY. */
#ifndef RQ_CALLS_H
#define RQ_CALLS_H

/* object.__setattr__(self, name, value) through CALL_NON_PY_GENERAL (frozen dataclass
 * __init__ and __post_init__, attrs): CPython builds a 3-tuple (_PyObject_MakeTpCall), slices
 * it (wrapperdescr_call: descr_check always holds for `object`), unpacks it (wrap_setattr:
 * PyArg_UnpackTuple, 2 items), runs hackcheck (a no-op unless self is a type) and calls the
 * descriptor's d_wrapped, PyObject_GenericSetAttr. The fast path calls d_wrapped directly for
 * exactly that descriptor object, 3 positional arguments and self not a type; the result is
 * None or the exception d_wrapped raised. No cache: nothing to make thread-safe. */
static PyObject *rq_object_setattr;   /* object.__dict__['__setattr__'] (main interpreter) */

static PyObject *
rq_call_non_py(PyThreadState *tstate, _PyStackRef callable, _PyStackRef *arguments,
               int total_args)
{
    PyObject *c = PyStackRef_AsPyObjectBorrow(callable);
    if (tstate->interp == rq_main_interp) {
        RQ_TICK(RQ_G_SETATTR);
        if (c == rq_object_setattr && total_args == 3) {
            PyObject *self = PyStackRef_AsPyObjectBorrow(arguments[0]);
            if (!PyType_Check(self)) {
                setattrofunc func = (setattrofunc)((PyWrapperDescrObject *)c)->d_wrapped;
                RQ_HIT(RQ_G_SETATTR);
                int r = func(self, PyStackRef_AsPyObjectBorrow(arguments[1]),
                             PyStackRef_AsPyObjectBorrow(arguments[2]));
                PyStackRef_CLOSE(arguments[2]);
                PyStackRef_CLOSE(arguments[1]);
                PyStackRef_CLOSE(arguments[0]);
                PyStackRef_CLOSE(callable);
                if (r < 0) {
                    return NULL;
                }
                Py_RETURN_NONE;
            }
        }
    }
    return _Py_VectorCall_StackRefSteal(callable, arguments, total_args, PyStackRef_NULL);
}

/* Keyword instantiation of a plain Python class: CALL_KW -> type_call -> object_new ->
 * slot_tp_init -> a C-level call of __init__ on a new C eval loop. CPython pushes `C(a, b)`
 * inline (CALL_ALLOC_AND_ENTER_INIT), but not `C(a, b=1)`. The fast path does what
 * CALL_ALLOC_AND_ENTER_INIT does, for exactly the classes the specializer accepts for it
 * (metaclass `type`, heap type, not abstract, tp_new = object_new, tp_alloc =
 * PyType_GenericAlloc, __init__ a Python function with the default vectorcall, no PEP 523
 * hook, stack space and recursion headroom), and passes the keywords to
 * _PyEvalFramePushAndInit as CALL_KW_PY does: allocate, push the _Py_InitCleanup shim
 * (EXIT_INIT_CHECK: "__init__() should return None"), push __init__'s frame. Same object,
 * same argument binding and errors, same Python frames; only the C stack differs (outside the
 * contract). object_new's excess-argument error cannot trigger: tp_init is overridden.
 * GIL builds only: the cache holds __init__ borrowed under the class's version tag (as the
 * specializer's _spec_cache does), which needs the GIL; free-threaded builds never swap this
 * group (the Rust side refuses it). */
#ifndef Py_GIL_DISABLED
typedef struct { unsigned int version; PyObject *init; } rq_init_e;
static rq_init_e rq_init_cache[256];
#endif

static inline PyObject *
rq_kw_init_for(PyThreadState *tstate, PyObject *callable_o)
{
#ifdef Py_GIL_DISABLED
    (void)tstate; (void)callable_o;
    return NULL;
#else
    if (tstate->interp != rq_main_interp) {
        return NULL;
    }
    RQ_TICK(RQ_G_KWINIT);
    if (!PyType_Check(callable_o) || Py_TYPE(callable_o) != &PyType_Type) {
        return NULL;
    }
    PyTypeObject *tp = (PyTypeObject *)callable_o;
    unsigned long fl = tp->tp_flags;
    if ((fl & (Py_TPFLAGS_IMMUTABLETYPE | Py_TPFLAGS_IS_ABSTRACT)) || !(fl & Py_TPFLAGS_HEAPTYPE)
        || tp->tp_new != PyBaseObject_Type.tp_new || tp->tp_alloc != PyType_GenericAlloc
        || tstate->interp->eval_frame != NULL || tstate->py_recursion_remaining <= 1) {
        return NULL;
    }
    unsigned int v = tp->tp_version_tag;
    if (v == 0) {
        return NULL;
    }
    rq_init_e *e = &rq_init_cache[(v * 2654435761u) >> 24];
    PyObject *init;
    if (e->version == v) {
        init = e->init;
    }
    else {
        init = _PyType_Lookup(tp, &_Py_ID(__init__));
        if (tp->tp_version_tag != v) {
            return NULL;
        }
        if (init == NULL || !PyFunction_Check(init)
            || ((PyFunctionObject *)init)->vectorcall != _PyFunction_Vectorcall) {
            init = NULL;
        }
        e->version = v;
        e->init = init;      /* borrowed: alive while the class's version tag holds */
    }
    /* The vectorcall is re-checked on every hit: PyFunction_SetVectorcall does not change
     * the class's version tag. */
    if (init == NULL || ((PyFunctionObject *)init)->vectorcall != _PyFunction_Vectorcall) {
        return NULL;
    }
    PyCodeObject *code = (PyCodeObject *)((PyFunctionObject *)init)->func_code;
    if (!_PyThreadState_HasStackSpace(tstate, code->co_framesize + _Py_InitCleanup.co_framesize)) {
        return NULL;
    }
    RQ_HIT(RQ_G_KWINIT);
    return init;
#endif
}

/* Consumes callable, the oparg argument values at self_slot[1..] and kwnames (the stack
 * inputs of CALL_KW, self_or_null being NULL). Returns __init__'s new frame (a shim frame
 * under it), or NULL with an exception set and every input released. */
static _PyInterpreterFrame *
rq_push_kw_init(PyThreadState *tstate, _PyInterpreterFrame *frame, _PyStackRef callable,
                _PyStackRef *self_slot, int oparg, _PyStackRef kwnames, PyObject *init)
{
    PyTypeObject *tp = (PyTypeObject *)PyStackRef_AsPyObjectBorrow(callable);
    PyObject *self_o = PyType_GenericAlloc(tp, 0);
    if (self_o == NULL) {
        for (int i = 1; i <= oparg; i++) {
            PyStackRef_CLOSE(self_slot[i]);
        }
        PyStackRef_CLOSE(kwnames);
        PyStackRef_CLOSE(callable);
        return NULL;
    }
    self_slot[0] = PyStackRef_FromPyObjectSteal(self_o);
    _PyInterpreterFrame *shim = _PyFrame_PushTrampolineUnchecked(
        tstate, (PyCodeObject *)&_Py_InitCleanup, 1, frame);
    shim->localsplus[0] = PyStackRef_DUP(self_slot[0]);
    PyObject *kwnames_o = PyStackRef_AsPyObjectBorrow(kwnames);
    int positional = oparg + 1 - (int)PyTuple_GET_SIZE(kwnames_o);
    _PyInterpreterFrame *temp = _PyEvalFramePushAndInit(
        tstate, PyStackRef_FromPyObjectNew(init), NULL, self_slot, positional, kwnames_o, shim);
    PyStackRef_CLOSE(kwnames);
    PyStackRef_CLOSE(callable);
    if (temp == NULL) {
        _PyEval_FrameClearAndPop(tstate, shim);
        return NULL;
    }
    /* Account for the extra (shim) frame, as CALL_ALLOC_AND_ENTER_INIT does. */
    tstate->py_recursion_remaining--;
    return temp;
}

#endif
