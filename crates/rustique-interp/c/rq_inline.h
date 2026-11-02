/* Local copies of core helpers that the core's own handlers have inlined (LTO + PGO + BOLT)
 * but an extension can only call out of line, where the PGO-laid-out exported copy may even
 * sit in cold code (measured: +29 Ir per _PyObject_GetAttrStackRef call, +60-180 Ir per
 * _Py_VectorCall_StackRefSteal + PyObject_Vectorcall). Each is the source of the release
 * (identical in 3.15.1 and 3.15.2; tools/gen.py checks the originals against
 * c/overrides.lock), restricted to its common case where noted; everything else calls the
 * core's own function, so behaviour is unchanged. PSF licence (gen/LICENSE). */
#ifndef RQ_INLINE_H
#define RQ_INLINE_H

/* Python/ceval.c _PyObjectArray_FromStackRefArray / _PyObjectArray_Free, verbatim. */
static inline __attribute__((always_inline)) PyObject **
rq_ObjectArray_FromStackRefArray(_PyStackRef *input, Py_ssize_t nargs, PyObject **scratch)
{
    PyObject **result;
    if (nargs > MAX_STACKREF_SCRATCH) {
        // +1 in case PY_VECTORCALL_ARGUMENTS_OFFSET is set.
        result = PyMem_Malloc((nargs + 1) * sizeof(PyObject *));
        if (result == NULL) {
            PyErr_NoMemory();
            return NULL;
        }
    }
    else {
        result = scratch;
    }
    result++;
    result[0] = NULL; /* Keep GCC happy */
    for (int i = 0; i < nargs; i++) {
        result[i] = PyStackRef_AsPyObjectBorrow(input[i]);
    }
    return result;
}

static inline __attribute__((always_inline)) void
rq_ObjectArray_Free(PyObject **array, PyObject **scratch)
{
    if (array != scratch) {
        PyMem_Free(array);
    }
}

/* Python/ceval.c _Py_VectorCall_StackRefSteal, verbatim except that PyObject_Vectorcall is
 * written as its definition (Objects/call.c: _PyObject_VectorcallTstate with the current
 * thread state, here the handler's tstate). */
static inline __attribute__((always_inline)) PyObject *
rq_VectorCall_StackRefSteal(
    PyThreadState *tstate,
    _PyStackRef callable,
    _PyStackRef *arguments,
    int total_args,
    _PyStackRef kwnames)
{
    PyObject *res;
    PyObject *args_o_temp[MAX_STACKREF_SCRATCH+1];
    PyObject **args_o = rq_ObjectArray_FromStackRefArray(arguments, total_args, args_o_temp);
    if (args_o == NULL) {
        res = NULL;
        goto cleanup;
    }
    PyObject *callable_o = PyStackRef_AsPyObjectBorrow(callable);
    PyObject *kwnames_o = PyStackRef_AsPyObjectBorrow(kwnames);
    int positional_args = total_args;
    if (kwnames_o != NULL) {
        positional_args -= (int)PyTuple_GET_SIZE(kwnames_o);
    }
    res = _PyObject_VectorcallTstate(
        tstate, callable_o, args_o,
        positional_args | PY_VECTORCALL_ARGUMENTS_OFFSET,
        kwnames_o);
    rq_ObjectArray_Free(args_o - 1, args_o_temp);
    assert((res != NULL) ^ (PyErr_Occurred() != NULL));
cleanup:
    PyStackRef_XCLOSE(kwnames);
    // arguments is a pointer into the GC visible stack,
    // so we must NULL out values as we clear them.
    for (int i = total_args-1; i >= 0; i--) {
        _PyStackRef tmp = arguments[i];
        arguments[i] = PyStackRef_NULL;
        PyStackRef_CLOSE(tmp);
    }
    PyStackRef_CLOSE(callable);
    return res;
}

/* Objects/object.c _PyObject_GetAttrStackRef, its common path: types (whose tp_getattro is
 * type's own and may return deferred references), objects without tp_getattro and non-str
 * names call the core's function. */
static inline __attribute__((always_inline)) _PyStackRef
rq_GetAttrStackRef_core(PyObject *v, PyObject *name)
{
    getattrofunc f = Py_TYPE(v)->tp_getattro;
    if (!PyUnicode_Check(name) || f == NULL || f == PyType_Type.tp_getattro) {
        return _PyObject_GetAttrStackRef(v, name);
    }
    PyObject *result = f(v, name);
    if (result == NULL) {
        _PyObject_SetAttributeErrorContext(v, name);
        return PyStackRef_NULL;
    }
    return PyStackRef_FromPyObjectSteal(result);
}

#endif
