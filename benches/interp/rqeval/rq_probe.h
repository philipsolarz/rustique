/* Analysis-only override support (-DRQ_ATTR_PROBE): counts the executions of the generic
 * LOAD_ATTR path by (type, name, method-load) with per-execution instance facts. Not a
 * candidate kernel: it only observes (no Python code runs, no dict is materialized). */
#ifndef RQ_PROBE_H
#define RQ_PROBE_H
#ifdef RQ_ATTR_PROBE
#define RQ_PROBE_N 65536
typedef struct {
    PyTypeObject *tp;
    PyObject *name;
    unsigned char meth;
    unsigned long long n, n_dict, n_dict_has, n_inline, n_inline_has;
} rq_probe_e;
static rq_probe_e rq_probe_tab[RQ_PROBE_N];
static unsigned long long rq_probe_lost;

static void
rq_attr_probe(PyObject *owner, PyObject *name, int meth)
{
    PyTypeObject *tp = Py_TYPE(owner);
    size_t h = ((((uintptr_t)tp) >> 4) * 31 + (((uintptr_t)name) >> 4) * 7 + (size_t)meth) & (RQ_PROBE_N - 1);
    rq_probe_e *e = NULL;
    for (int i = 0; i < 32; i++) {
        rq_probe_e *c = &rq_probe_tab[(h + i) & (RQ_PROBE_N - 1)];
        if (c->tp == tp && c->name == name && c->meth == meth) { e = c; break; }
        if (c->tp == NULL) {
            c->tp = (PyTypeObject *)Py_NewRef(tp); c->name = Py_NewRef(name); c->meth = (unsigned char)meth;
            e = c; break;
        }
    }
    if (e == NULL) { rq_probe_lost++; return; }
    e->n++;
    if (tp->tp_flags & Py_TPFLAGS_MANAGED_DICT) {
        PyDictObject *d = _PyObject_GetManagedDict(owner);
        if (d != NULL) {
            e->n_dict++;
            if (PyUnicode_CheckExact(name) && _PyDict_GetItem_KnownHash((PyObject *)d, name, PyObject_Hash(name)) != NULL) e->n_dict_has++;
        }
        else if ((tp->tp_flags & Py_TPFLAGS_INLINE_VALUES) && _PyObject_InlineValues(owner)->valid) {
            e->n_inline++;
            PyDictKeysObject *keys = ((PyHeapTypeObject *)tp)->ht_cached_keys;
            Py_ssize_t ix = keys ? _PyDictKeys_StringLookup(keys, name) : -1;
            if (ix >= 0 && _PyObject_InlineValues(owner)->values[ix] != NULL) e->n_inline_has++;
        }
    }
    else if (tp->tp_dictoffset > 0) {
        PyObject *d = *(PyObject **)((char *)owner + tp->tp_dictoffset);
        if (d != NULL && PyDict_Check(d)) {
            e->n_dict++;
            if (PyUnicode_CheckExact(name) && _PyDict_GetItem_KnownHash(d, name, PyObject_Hash(name)) != NULL) e->n_dict_has++;
        }
    }
}

static PyObject *
rq_probe_dump(PyObject *self, PyObject *Py_UNUSED(args))
{
    PyObject *out = PyList_New(0);
    for (size_t i = 0; i < RQ_PROBE_N; i++) {
        rq_probe_e *e = &rq_probe_tab[i];
        if (e->tp == NULL) continue;
        PyObject *t = Py_BuildValue("(OOiKKKKK)", e->tp, e->name, (int)e->meth, e->n, e->n_dict,
                                    e->n_dict_has, e->n_inline, e->n_inline_has);
        if (t == NULL || PyList_Append(out, t) < 0) { Py_XDECREF(t); Py_DECREF(out); return NULL; }
        Py_DECREF(t);
    }
    return Py_BuildValue("(NK)", out, rq_probe_lost);
}
#endif
#endif
