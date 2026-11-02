/* Helpers the override file may call (included by rqeval_loop.c before the generated cases). */
#ifndef RQ_SUPPORT_H
#define RQ_SUPPORT_H

/* object.__dict__['__class__'] and type.__dict__['__instancecheck__'], looked up once at
 * module init (static types: they live as long as the process). */
static PyObject *rq_object_class_descr;
static PyObject *rq_type_instancecheck;

/* Per-type facts cached by tp_version_tag (CPython's own method-cache rule: the tag changes
 * whenever the type or a base is modified, and tags are never reused). GIL build only. */
typedef struct { unsigned int version; unsigned char ok; } rq_vcache_entry;
static rq_vcache_entry rq_class_cache[1024];
static rq_vcache_entry rq_meta_cache[128];

/* inst.__class__ is type(inst) for every instance of tp: generic getattr, and the MRO resolves
 * '__class__' to object's data descriptor (instance dicts cannot shadow a data descriptor). */
static inline int
rq_plain_class(PyTypeObject *tp)
{
    if (tp->tp_getattro != PyObject_GenericGetAttr) {
        return 0;
    }
    unsigned int v = tp->tp_version_tag;
    rq_vcache_entry *e = &rq_class_cache[v & 1023];
    if (v != 0 && e->version == v) {
        return e->ok;
    }
    int ok = _PyType_Lookup(tp, &_Py_ID(__class__)) == rq_object_class_descr;
    v = tp->tp_version_tag;
    if (v != 0) {
        rq_class_cache[v & 1023] = (rq_vcache_entry){v, (unsigned char)ok};
    }
    return ok;
}

/* type(cls) resolves __instancecheck__ to type's own method. */
static inline int
rq_plain_meta(PyTypeObject *meta)
{
    unsigned int v = meta->tp_version_tag;
    rq_vcache_entry *e = &rq_meta_cache[v & 127];
    if (v != 0 && e->version == v) {
        return e->ok;
    }
    int ok = _PyType_Lookup(meta, &_Py_ID(__instancecheck__)) == rq_type_instancecheck;
    v = meta->tp_version_tag;
    if (v != 0) {
        rq_meta_cache[v & 127] = (rq_vcache_entry){v, (unsigned char)ok};
    }
    return ok;
}

/* Exact fast path for isinstance(inst, cls) (Objects/abstract.c, object_recursive_isinstance
 * and object_isinstance, 3.15.1). Returns 1/0, -1 with an exception set, or -2 to run
 * PyObject_IsInstance unchanged.
 *  - cls is a type whose metaclass resolves __instancecheck__ to type's own: CPython binds and
 *    calls it and it computes object_isinstance(inst, cls); compute that directly (no Python
 *    frame runs in between in either case);
 *  - object_isinstance: subtype check, then for a negative result the inst.__class__ lookup,
 *    skipped when rq_plain_class proves it returns type(inst); otherwise it is done exactly as
 *    object_isinstance does it (no second subtype walk);
 *  - a tuple: its items in order, stopping at the first non-zero result (the C-stack
 *    recursion guard of the original is outside the contract, ACCELERATION 5.8). */
static inline __attribute__((always_inline)) int
rq_isinstance_type(PyObject *inst, PyObject *cls)
{
    PyTypeObject *tp = Py_TYPE(inst);
    PyTypeObject *meta = Py_TYPE(cls);
    if (meta != &PyType_Type && !rq_plain_meta(meta)) {
        return -2;
    }
    if (PyType_IsSubtype(tp, (PyTypeObject *)cls)) {
        return 1;
    }
    if (rq_plain_class(tp)) {
        return 0;
    }
    PyObject *icls;
    int retval = PyObject_GetOptionalAttr(inst, &_Py_ID(__class__), &icls);
    if (icls != NULL) {
        if (icls != (PyObject *)tp && PyType_Check(icls)) {
            retval = PyType_IsSubtype((PyTypeObject *)icls, (PyTypeObject *)cls);
        }
        else {
            retval = 0;
        }
        Py_DECREF(icls);
    }
    return retval;
}

static Py_NO_INLINE int
rq_isinstance_tuple(PyObject *inst, PyObject *cls)
{
    Py_ssize_t n = PyTuple_GET_SIZE(cls);
    int r = 0;
    for (Py_ssize_t i = 0; i < n; i++) {
        PyObject *item = PyTuple_GET_ITEM(cls, i);
        if ((PyObject *)Py_TYPE(inst) == item) {
            return 1;
        }
        r = PyType_Check(item) ? rq_isinstance_type(inst, item) : -2;
        if (r == -2) {
            r = PyObject_IsInstance(inst, item);
        }
        if (r != 0) {
            break;
        }
    }
    return r;
}

static inline __attribute__((always_inline)) int
rq_isinstance_fast(PyThreadState *tstate, PyObject *inst, PyObject *cls)
{
    if ((PyObject *)Py_TYPE(inst) == cls) {
        return 1;
    }
    if (tstate->interp != rq_main_interp) {
        return -2;
    }
    if (PyType_Check(cls)) {
        return rq_isinstance_type(inst, cls);
    }
    if (PyTuple_CheckExact(cls)) {
        return rq_isinstance_tuple(inst, cls);
    }
    return -2;
}

/* object.__setattr__(self, name, value) through CALL_NON_PY_GENERAL (frozen dataclass
 * __init__, __post_init__, attrs): CPython builds a 3-tuple (_PyObject_MakeTpCall), slices it
 * (wrapperdescr_call), unpacks it (wrap_setattr: PyArg_UnpackTuple), runs hackcheck (a no-op
 * unless self is a type) and calls the descriptor's d_wrapped, PyObject_GenericSetAttr.
 * The fast path calls d_wrapped directly for exactly that descriptor object, 3 positional
 * arguments, self not a type; the result is None or the exception d_wrapped raised. */
static PyObject *rq_object_setattr;   /* object.__dict__['__setattr__'] (main interpreter) */

static PyObject *
rq_call_non_py(_PyStackRef callable, _PyStackRef *arguments, int total_args)
{
    PyObject *c = PyStackRef_AsPyObjectBorrow(callable);
#ifdef RQ_ATTR_CACHE
    RQ_TICK(1);
#endif
    if (c == rq_object_setattr && total_args == 3 && _PyInterpreterState_GET() == rq_main_interp) {
        PyObject *self = PyStackRef_AsPyObjectBorrow(arguments[0]);
        if (!PyType_Check(self)) {
            setattrofunc func = (setattrofunc)((PyWrapperDescrObject *)c)->d_wrapped;
#ifdef RQ_ATTR_CACHE
            RQ_HIT(1);
#endif
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
    return _Py_VectorCall_StackRefSteal(callable, arguments, total_args, PyStackRef_NULL);
}


/* Keyword-argument instantiation of a plain Python class, CALL_KW -> type_call (see
 * docs/atlas/w2-interp.md 4.5). CPython inlines `C(a, b)` (CALL_ALLOC_AND_ENTER_INIT) but runs
 * `C(a, b=1)` through type_call -> object_new -> slot_tp_init -> a C-level call into
 * __init__ on a new C eval loop. The fast path does what CALL_ALLOC_AND_ENTER_INIT does for
 * exactly the classes CPython specializes it for (metaclass `type`, heap type, not abstract,
 * tp_new = object_new, tp_alloc = PyType_GenericAlloc, __init__ a Python function with the
 * default vectorcall), passing the keywords to _PyEvalFramePushAndInit like CALL_KW_PY:
 * allocate, push the _Py_InitCleanup shim (EXIT_INIT_CHECK: "__init__() should return None"),
 * push __init__'s frame. Same object, same argument binding and errors, same Python frames;
 * only the C stack differs (outside the contract). */
typedef struct { unsigned int version; PyObject *init; } rq_init_e;
static rq_init_e rq_init_cache[256];

static inline PyObject *
rq_kw_init_for(PyThreadState *tstate, PyObject *callable_o)
{
#ifdef RQ_ATTR_CACHE
    RQ_TICK(2);
#endif
    if (!PyType_Check(callable_o) || Py_TYPE(callable_o) != &PyType_Type
        || tstate->interp != rq_main_interp) {
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
    if (init == NULL) {
        return NULL;
    }
    PyCodeObject *code = (PyCodeObject *)((PyFunctionObject *)init)->func_code;
    if (!_PyThreadState_HasStackSpace(tstate, code->co_framesize + _Py_InitCleanup.co_framesize)) {
        return NULL;
    }
#ifdef RQ_ATTR_CACHE
    RQ_HIT(2);
#endif
    return init;
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
