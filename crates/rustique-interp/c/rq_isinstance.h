/* isinstance(inst, cls) fast path for the CALL_ISINSTANCE override (overrides.c).
 *
 * Replays Objects/abstract.c object_recursive_isinstance / object_isinstance (3.15):
 *  - Py_IS_TYPE(inst, cls) -> 1 (CPython's own first test).
 *  - cls a type whose metaclass is `type`, or resolves __instancecheck__ to type's own method
 *    descriptor: CPython binds and calls it, and it computes object_isinstance(inst, cls)
 *    with no Python frame in between; compute that directly.
 *  - object_isinstance: subtype check; for a negative result, CPython looks up
 *    inst.__class__. When type(inst) uses PyObject_GenericGetAttr and its MRO resolves
 *    '__class__' to object's own data descriptor (an instance dict cannot shadow a data
 *    descriptor; its getter returns type(inst) and has no side effect), that lookup returns
 *    type(inst) and the result is 0. Otherwise the lookup runs exactly as object_isinstance
 *    does it.
 *  - an exact tuple: its items in order, stopping at the first non-zero result; a non-type
 *    item goes to PyObject_IsInstance (with its own recursion guard), so the fast path never
 *    recurses (regrtest: test_isinstance deep nesting).
 *  - everything else (unions, tuple subclasses, ABCs, user __instancecheck__): -2, the
 *    caller runs PyObject_IsInstance unchanged.
 * Only the C-stack recursion guard of the original differs: outside the contract
 * (ACCELERATION 5.8).
 *
 * The two per-type facts are cached by tp_version_tag (CPython's method-cache rule: any
 * change to the type or a base changes the tag, and tags are never reused). An entry is one
 * 64-bit word (version << 8 | fact), so concurrent readers never see a torn entry; a fact is
 * stored only when the tag read before and after computing it is the same, non-zero tag. */
#ifndef RQ_ISINSTANCE_H
#define RQ_ISINSTANCE_H

static PyObject *rq_object_class_descr;    /* object.__dict__['__class__'] */
static PyObject *rq_type_instancecheck;    /* type.__dict__['__instancecheck__'] */

#define RQ_FACT_N 1024
static uint64_t rq_class_facts[RQ_FACT_N];
static uint64_t rq_meta_facts[RQ_FACT_N / 8];

static Py_NO_INLINE int
rq_fact_compute(PyTypeObject *tp, PyObject *attr, PyObject *expect, uint64_t *slot)
{
    unsigned int v = FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag);
    /* Identity comparison only: the looked-up object is never dereferenced. */
    int ok = _PyType_Lookup(tp, attr) == expect;
    if (v != 0 && FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag) == v) {
        __atomic_store_n(slot, ((uint64_t)v << 8) | (uint64_t)ok, __ATOMIC_RELAXED);
    }
    return ok;
}

static inline __attribute__((always_inline)) int
rq_fact(PyTypeObject *tp, PyObject *attr, PyObject *expect, uint64_t *table, unsigned int mask)
{
    unsigned int v = FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag);
    uint64_t *slot = &table[(v * 2654435761u) >> 8 & mask];
    uint64_t e = __atomic_load_n(slot, __ATOMIC_RELAXED);
    if (v != 0 && (e >> 8) == v) {
        return (int)(e & 1);
    }
    return rq_fact_compute(tp, attr, expect, slot);
}

/* inst.__class__ is type(inst) for every instance of tp. */
static inline __attribute__((always_inline)) int
rq_plain_class(PyTypeObject *tp)
{
    if (tp->tp_getattro != PyObject_GenericGetAttr) {
        return 0;
    }
    return rq_fact(tp, &_Py_ID(__class__), rq_object_class_descr, rq_class_facts, RQ_FACT_N - 1);
}

/* type(cls) resolves __instancecheck__ to type's own method. */
static inline __attribute__((always_inline)) int
rq_plain_meta(PyTypeObject *meta)
{
    return rq_fact(meta, &_Py_ID(__instancecheck__), rq_type_instancecheck, rq_meta_facts,
                   RQ_FACT_N / 8 - 1);
}

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
    /* object_isinstance's negative path, verbatim. */
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

/* 1 / 0, -1 with an exception set, or -2: run PyObject_IsInstance unchanged. */
static inline __attribute__((always_inline)) int
rq_isinstance_fast(PyThreadState *tstate, PyObject *inst, PyObject *cls)
{
    if (tstate->interp != rq_main_interp) {
        return (PyObject *)Py_TYPE(inst) == cls ? 1 : -2;
    }
    RQ_TICK(RQ_G_ISINSTANCE);
    int r;
    if ((PyObject *)Py_TYPE(inst) == cls) {
        r = 1;
    }
    else if (PyType_Check(cls)) {
        r = rq_isinstance_type(inst, cls);
    }
    else if (PyTuple_CheckExact(cls)) {
        r = rq_isinstance_tuple(inst, cls);
    }
    else {
        return -2;
    }
    if (r != -2) {
        RQ_HIT(RQ_G_ISINSTANCE);
    }
    return r;
}

#endif
