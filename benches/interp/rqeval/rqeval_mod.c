/* _rqeval: a PEP 523 eval-frame function that is a generated copy of CPython's own
 * interpreter loop (Tools/cases_generator applied to the running release's
 * Python/bytecodes.c plus an override file), installed with
 * _PyInterpreterState_SetEvalFrameAllowSpecialization(interp, 1) so the specializer and
 * inlined Python-to-Python calls keep working. Prototype for docs/atlas/w2-interp.md. */
#define PY_SSIZE_T_CLEAN
#ifndef Py_BUILD_CORE_MODULE
#  define Py_BUILD_CORE_MODULE
#endif
#include "rqeval_loop.c"


static PyObject *
rq_install(PyObject *self, PyObject *Py_UNUSED(args))
{
    PyInterpreterState *interp = _PyInterpreterState_GET();
    _PyFrameEvalFunction cur = _PyInterpreterState_GetEvalFrameFunc(interp);
    if (interp->jit) {
        PyErr_SetString(PyExc_RuntimeError, "the JIT is enabled (PYTHON_JIT=1): not installing");
        return NULL;
    }
    if (cur != _PyEval_EvalFrameDefault && cur != Rq_EvalFrame) {
        PyErr_SetString(PyExc_RuntimeError, "another PEP 523 eval-frame function is installed");
        return NULL;
    }
    _PyInterpreterState_SetEvalFrameFunc(interp, Rq_EvalFrame);
    _PyInterpreterState_SetEvalFrameAllowSpecialization(interp, 1);
    Py_RETURN_NONE;
}

static PyObject *
rq_uninstall(PyObject *self, PyObject *Py_UNUSED(args))
{
    PyInterpreterState *interp = _PyInterpreterState_GET();
    if (_PyInterpreterState_GetEvalFrameFunc(interp) == Rq_EvalFrame) {
        _PyInterpreterState_SetEvalFrameFunc(interp, _PyEval_EvalFrameDefault);
        _PyInterpreterState_SetEvalFrameAllowSpecialization(interp, 0);
    }
    Py_RETURN_NONE;
}

static PyObject *
rq_installed(PyObject *self, PyObject *Py_UNUSED(args))
{
    return PyBool_FromLong(_PyInterpreterState_GetEvalFrameFunc(_PyInterpreterState_GET()) == Rq_EvalFrame);
}

/* Handler swap: write this copy's handler for `opcode` into the core interpreter's own
 * tail-call dispatch table (instruction_funcptr_handler_table, a writable static array in
 * the PBS binary's .data). Every other opcode keeps the core's handler; the swapped handler
 * dispatches onward through the table it is given, i.e. back into the core.
 * patch(core_table_addr, opcode, expected_core_handler_addr) -> previous entry. */
static PyObject *
rq_patch(PyObject *self, PyObject *args)
{
    unsigned long long table, expect;
    int opcode;
    if (!PyArg_ParseTuple(args, "KiK", &table, &opcode, &expect)) {
        return NULL;
    }
    if (opcode < 0 || opcode > 255) {
        PyErr_SetString(PyExc_ValueError, "opcode");
        return NULL;
    }
    void **t = (void **)(uintptr_t)table;
    void *old = __atomic_load_n(&t[opcode], __ATOMIC_RELAXED);
    if (old != (void *)(uintptr_t)expect) {
        PyErr_Format(PyExc_RuntimeError, "core table entry %d is %p, expected %p", opcode, old,
                     (void *)(uintptr_t)expect);
        return NULL;
    }
    void *mine = (void *)instruction_funcptr_handler_table[opcode];
#ifdef RQ_ATTR_CACHE
    rq_record_patch(t, opcode, old);
#endif
    __atomic_store_n(&t[opcode], mine, __ATOMIC_RELEASE);
    return PyLong_FromUnsignedLongLong((uintptr_t)old);
}

static PyObject *
rq_unpatch(PyObject *self, PyObject *args)
{
    unsigned long long table, old;
    int opcode;
    if (!PyArg_ParseTuple(args, "KiK", &table, &opcode, &old)) {
        return NULL;
    }
    void **t = (void **)(uintptr_t)table;
    __atomic_store_n(&t[opcode], (void *)(uintptr_t)old, __ATOMIC_RELEASE);
    Py_RETURN_NONE;
}

static PyObject *
rq_handler(PyObject *self, PyObject *arg)
{
    int opcode = PyLong_AsInt(arg);
    if (opcode < 0 || opcode > 255) {
        if (!PyErr_Occurred()) PyErr_SetString(PyExc_ValueError, "opcode");
        return NULL;
    }
    return PyLong_FromUnsignedLongLong((uintptr_t)instruction_funcptr_handler_table[opcode]);
}

#ifdef RQ_ATTR_CACHE
static PyObject *
rq_ac_stats(PyObject *self, PyObject *Py_UNUSED(args))
{
    return Py_BuildValue("(KKi(iii))", rq_ac_hits, rq_ac_miss, rq_suspended,
                         rq_groups[0].suspended, rq_groups[1].suspended, rq_groups[2].suspended);
}
#endif

static PyMethodDef rq_methods[] = {
    {"patch", rq_patch, METH_VARARGS, NULL},
    {"unpatch", rq_unpatch, METH_VARARGS, NULL},
    {"handler", rq_handler, METH_O, NULL},
#ifdef RQ_ATTR_PROBE
    {"probe_dump", rq_probe_dump, METH_NOARGS, NULL},
#endif
#ifdef RQ_ATTR_CACHE
    {"ac_stats", rq_ac_stats, METH_NOARGS, NULL},
#endif
    {"install", rq_install, METH_NOARGS, NULL},
    {"uninstall", rq_uninstall, METH_NOARGS, NULL},
    {"installed", rq_installed, METH_NOARGS, NULL},
    {NULL, NULL, 0, NULL},
};

static PyModuleDef_Slot rq_slots[] = {
#ifdef Py_GIL_DISABLED
    {Py_mod_gil, Py_MOD_GIL_NOT_USED},
#endif
    {Py_mod_multiple_interpreters, Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED},
    {0, NULL},
};

static struct PyModuleDef rq_def = {PyModuleDef_HEAD_INIT, RQ_MODNAME_STR, NULL, 0, rq_methods, rq_slots};

#define RQ_CAT2(a, b) a##b
#define RQ_CAT(a, b) RQ_CAT2(a, b)
PyMODINIT_FUNC RQ_CAT(PyInit_, RQ_MODNAME)(void)
{
    rq_main_interp = PyInterpreterState_Main();
    rq_object_class_descr = _PyType_Lookup(&PyBaseObject_Type, &_Py_ID(__class__));  /* static types keep tp_dict per interpreter */
    rq_type_instancecheck = _PyType_Lookup(&PyType_Type, &_Py_ID(__instancecheck__));
    rq_object_setattr = _PyType_Lookup(&PyBaseObject_Type, &_Py_ID(__setattr__));
    if (rq_object_class_descr == NULL || rq_type_instancecheck == NULL || rq_object_setattr == NULL
        || !PyObject_TypeCheck(rq_object_setattr, &PyWrapperDescr_Type)) {
        if (!PyErr_Occurred()) PyErr_SetString(PyExc_RuntimeError, "rqeval: descriptors not found");
        return NULL;
    }
    return PyModuleDef_Init(&rq_def);
}
