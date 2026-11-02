/* Megamorphic attribute cache for the generic LOAD_ATTR path (-DRQ_ATTR_CACHE).
 *
 * Keyed by (type version tag, attribute name). A hit replays exactly what
 * _PyObject_GenericGetAttrWithDict (attribute loads) and _PyObject_GetMethodStackRef (method
 * loads) do for the cached resolution (Objects/object.c, 3.15.1):
 *   1. the MRO lookup of `name` on type(obj) -- cached, valid while tp_version_tag is unchanged
 *      (CPython's own method-cache rule; any change to the type or a base changes the tag);
 *   2. data descriptor -> never cached (always the generic path);
 *   3. instance attribute: inline values (as _PyObject_TryGetInstanceAttribute) or the
 *      instance dict (PyDict_GetItemRef; only unicode-keyed dicts, so no key __eq__ can run);
 *   4. not in the instance: a method descriptor (method loads: (descr, self); attribute loads:
 *      its tp_descr_get, as CPython calls it) or a plain class attribute.
 * Everything else -- no version tag, tp_getattro other than PyObject_GenericGetAttr,
 * non-data descriptors other than method descriptors when the instance lacks the name, absent
 * attributes (the AttributeError path), inline values without a dict -- returns 0 and the
 * generic code runs unchanged. GIL build, main interpreter only (version tags of heap types are
 * per interpreter). */
#ifndef RQ_ATTRCACHE_H
#define RQ_ATTRCACHE_H
#ifdef RQ_ATTR_CACHE
#define RQ_AC_N 4096
enum { RQ_K_ABSENT = 1, RQ_K_PLAIN, RQ_K_METHOD, RQ_K_NONDATA, RQ_K_SLOT, RQ_K_NONE /* negative entry */ };
enum { RQ_D_INLINE = 1, RQ_D_MANAGED, RQ_D_OFFSET, RQ_D_NONE };
typedef struct {
    unsigned int version;
    unsigned char kind, dkind;
    PyObject *name;              /* strong reference */
    PyObject *descr;             /* borrowed: alive while the version tag is unchanged */
    PyTypeObject *dtype;         /* type(descr) when it is mutable (else NULL) ... */
    unsigned int dversion;       /* ... and its version tag: the descriptor kind can change
                                    without type(obj)'s tag changing (test_opcache
                                    test_descriptor_added_after_optimization) */
    PyDictKeysObject *keys;      /* inline values: the type's shared keys at fill time */
    Py_ssize_t ix, nentries, dictoffset;
} rq_ac_e;
static rq_ac_e rq_ac[RQ_AC_N];
static unsigned long long rq_ac_hits, rq_ac_miss;

static Py_NO_INLINE rq_ac_e *rq_ac_fill(PyTypeObject *tp, PyObject *name, rq_ac_e *e,
                                          unsigned int v);

static inline __attribute__((always_inline)) rq_ac_e *
rq_ac_get(PyTypeObject *tp, PyObject *name)
{
    unsigned int v = tp->tp_version_tag;
    if (v == 0) {
        return NULL;
    }
    rq_ac_e *e = &rq_ac[((v * 2654435761u) ^ (unsigned int)((uintptr_t)name >> 4)) & (RQ_AC_N - 1)];
    if (e->version == v && e->name == name) {
        return e;
    }
    return rq_ac_fill(tp, name, e, v);
}

static unsigned long long rq_ac_fills;   /* cache fills: group 0's warm-up, not counted */

static Py_NO_INLINE rq_ac_e *
rq_ac_fill(PyTypeObject *tp, PyObject *name, rq_ac_e *e, unsigned int v)
{
    rq_ac_fills++;
    if (tp->tp_getattro != PyObject_GenericGetAttr || !PyUnicode_CheckExact(name)
        || !_PyType_IsReady(tp)) {
        return NULL;
    }
    PyObject *descr = _PyType_Lookup(tp, name);
    if (tp->tp_version_tag != v) {
        return NULL;
    }
    unsigned char kind;
    if (descr == NULL) {
        kind = RQ_K_ABSENT;
    }
    else if (PyDescr_IsData(descr) && Py_TYPE(descr)->tp_descr_get != NULL) {
        /* Data descriptors run the generic code, except an object __slots__ member
         * (member_get -> PyMember_GetOne, Py_T_OBJECT_EX, no audit, plain offset, and
         * descr_check guaranteed because the descriptor's class is in type(obj)'s MRO). */
        PyMemberDef *md = Py_IS_TYPE(descr, &PyMemberDescr_Type)
                          ? ((PyMemberDescrObject *)descr)->d_member : NULL;
        if (md != NULL && md->type == Py_T_OBJECT_EX
            && !(md->flags & (Py_AUDIT_READ | Py_RELATIVE_OFFSET | _Py_AFTER_ITEMS))
            && PyType_IsSubtype(tp, PyDescr_TYPE(descr))) {
            kind = RQ_K_SLOT;
        }
        else {
            kind = RQ_K_NONE;
        }
    }
    else if (_PyType_HasFeature(Py_TYPE(descr), Py_TPFLAGS_METHOD_DESCRIPTOR)) {
        kind = Py_TYPE(descr)->tp_descr_set != NULL ? RQ_K_NONE : RQ_K_METHOD;
    }
    else if (Py_TYPE(descr)->tp_descr_get == NULL) {
        kind = RQ_K_PLAIN;
    }
    else {
        kind = RQ_K_NONDATA;
    }
    PyTypeObject *dtype = NULL;
    unsigned int dversion = 0;
    if (descr != NULL && kind != RQ_K_NONE && kind != RQ_K_SLOT
        && !_PyType_HasFeature(Py_TYPE(descr), Py_TPFLAGS_IMMUTABLETYPE)) {
        dtype = Py_TYPE(descr);
        if (dtype->tp_version_tag == 0 && !PyUnstable_Type_AssignVersionTag(dtype)) {
            kind = RQ_K_NONE;
            dtype = NULL;
        }
        else {
            dversion = dtype->tp_version_tag;
        }
    }
    unsigned char dkind;
    Py_ssize_t ix = -1, nentries = 0, off = 0;
    PyDictKeysObject *keys = NULL;
    if (tp->tp_flags & Py_TPFLAGS_INLINE_VALUES) {
        dkind = RQ_D_INLINE;
        keys = ((PyHeapTypeObject *)tp)->ht_cached_keys;
        if (keys == NULL) {
            return NULL;
        }
        ix = _PyDictKeys_StringLookupSplit(keys, name);
        nentries = keys->dk_nentries;
    }
    else if (tp->tp_flags & Py_TPFLAGS_MANAGED_DICT) {
        dkind = RQ_D_MANAGED;
    }
    else if (tp->tp_dictoffset > 0) {
        dkind = RQ_D_OFFSET;
        off = tp->tp_dictoffset;
    }
    else if (tp->tp_dictoffset == 0) {
        dkind = RQ_D_NONE;
    }
    else {
        return NULL;
    }
    PyObject *old = e->name;
    e->version = v; e->kind = kind; e->dkind = dkind; e->name = Py_NewRef(name);
    e->descr = descr; e->keys = keys; e->ix = ix; e->nentries = nentries; e->dictoffset = off;
    e->dtype = dtype; e->dversion = dversion;
    Py_XDECREF(old);
    return e;
}

/* 1: hit (*res is a new reference; *is_meth set for a (descr, self) method load);
 * 0: not handled (run the generic code); -1: error raised exactly where CPython raises it. */
/* Net-benefit suspension (ACCELERATION 5.6, applied to opcode handlers). Each override group
 * counts its executions and its fast-path hits (attribute-cache fills are excluded: they are
 * the warm-up investment). After RQ_DECIDE windows of 65,536 executions (about 1M), a group
 * whose cumulative hits are below 1/ratio of its executions restores its handlers to the
 * core's for the rest of the process; one decision per process, on a horizon long enough to
 * average out start-up phases (a single-window rule suspended flask_marshmallow's attribute
 * cache during its cold start). Only which, equivalent, handler runs changes.
 *   group 0: the LOAD_ATTR family (attribute cache)  ratio 5    (our family copies cost ~0.8%
 *            when nothing hits: task_planner)
 *   group 1: CALL_NON_PY_GENERAL (object.__setattr__)  ratio 64  (a hit saves ~570 Ir, a
 *            non-hit costs ~10)
 *   group 2: CALL_KW_NON_PY (keyword instantiation)    ratio 64  (a hit saves ~1,100 Ir) */
#define RQ_WINDOW 65536
#define RQ_DECIDE 16
typedef struct {
    void **table;
    int n, suspended, windows;
    unsigned int total, hits, ratio;
    unsigned long long cum_total, cum_hits;
    unsigned char op[64];
    void *old[64];
} rq_group_t;
static rq_group_t rq_groups[3] = {{.ratio = 5}, {.ratio = 64}, {.ratio = 64}};
#define rq_suspended (rq_groups[0].suspended)

static void
rq_record_patch(void **table, int opcode, void *old)
{
    int g = _PyOpcode_Deopt[opcode] == LOAD_ATTR ? 0
            : opcode == CALL_NON_PY_GENERAL ? 1 : opcode == CALL_KW_NON_PY ? 2 : -1;
    if (g >= 0 && rq_groups[g].n < 64) {
        rq_groups[g].table = table;
        rq_groups[g].op[rq_groups[g].n] = (unsigned char)opcode;
        rq_groups[g].old[rq_groups[g].n] = old;
        rq_groups[g].n++;
    }
}

static Py_NO_INLINE void
rq_window_end(rq_group_t *g)
{
    g->cum_total += g->total;
    g->cum_hits += g->hits;
    g->total = g->hits = 0;
    if (++g->windows != RQ_DECIDE || g->table == NULL || g->suspended) {
        return;
    }
    unsigned long long fills = (g == &rq_groups[0]) ? rq_ac_fills : 0;
    unsigned long long steady = g->cum_total > fills ? g->cum_total - fills : 0;
    if (g->cum_hits * g->ratio < steady) {
        for (int i = 0; i < g->n; i++) {
            __atomic_store_n(&g->table[g->op[i]], g->old[i], __ATOMIC_RELEASE);
        }
        g->suspended = 1;
    }
}

#define RQ_TICK(gi) do { if (++rq_groups[gi].total >= RQ_WINDOW) rq_window_end(&rq_groups[gi]); } while (0)
#define RQ_HIT(gi) (rq_groups[gi].hits++)

/* Per-site negative hint: a site that is generic in the bytecode (not a specialized form
 * deopting) and resolved to something uncacheable for type version V records (RQ_MARK, V) in
 * its own unused inline-cache entries 2..4, and skips the cache probe while type(obj) still
 * has version V. A pure performance hint: a stale or colliding hint only means the generic
 * code runs. The entries are internal state (the specializer rewrites them when it
 * specializes the site; co_code reports them as zero). */
#define RQ_MARK 0x5251
static inline __attribute__((always_inline)) int
rq_site_skip(_Py_CODEUNIT *instr, PyTypeObject *tp)
{
    return instr->op.code == LOAD_ATTR && instr[2].cache == RQ_MARK
           && read_u32(&instr[3].cache) == tp->tp_version_tag;
}

static inline void
rq_site_mark(_Py_CODEUNIT *instr, PyTypeObject *tp)
{
    if (instr->op.code == LOAD_ATTR && tp->tp_version_tag != 0) {
        instr[2].cache = RQ_MARK;
        write_u32(&instr[3].cache, tp->tp_version_tag);
    }
}

static inline __attribute__((always_inline)) int
rq_attr_fast(PyThreadState *tstate, PyObject *obj, PyObject *name, int meth, PyObject **res,
             int *is_meth, _Py_CODEUNIT *instr)
{
    PyTypeObject *tp = Py_TYPE(obj);
    RQ_TICK(0);
    if (tstate->interp != rq_main_interp || rq_site_skip(instr, tp)) {
        return 0;
    }
    rq_ac_e *e = rq_ac_get(tp, name);
    if (e == NULL || e->kind == RQ_K_NONE) {
        rq_ac_miss++;
        rq_site_mark(instr, tp);
        return 0;
    }
    if (e->dtype != NULL
        && (Py_TYPE(e->descr) != e->dtype || e->dtype->tp_version_tag != e->dversion)) {
        e->version = 0;    /* the class attribute's kind may have changed: refill next time */
        return 0;
    }
    if (e->kind == RQ_K_SLOT) {
        PyObject *v = *(PyObject **)((char *)obj + ((PyMemberDescrObject *)e->descr)->d_member->offset);
        if (v == NULL) {
            return 0;    /* AttributeError: raised by the generic code */
        }
        *res = Py_NewRef(v);
        *is_meth = 0;
        rq_ac_hits++; RQ_HIT(0);
        return 1;
    }
    PyObject *dict = NULL;
    PyObject *inst = NULL;
    switch (e->dkind) {
    case RQ_D_INLINE: {
        PyDictValues *values = _PyObject_InlineValues(obj);
        if (values->valid) {
            Py_ssize_t ix = e->ix;
            PyDictKeysObject *keys = ((PyHeapTypeObject *)tp)->ht_cached_keys;
            if (ix < 0 && (keys != e->keys || keys->dk_nentries != e->nentries)) {
                ix = _PyDictKeys_StringLookupSplit(keys, name);
            }
            if (ix >= 0) {
                inst = values->values[ix];
            }
            if (inst != NULL) {
                *res = Py_NewRef(inst);
                *is_meth = 0;
                rq_ac_hits++; RQ_HIT(0);
                return 1;
            }
            break;
        }
        dict = (PyObject *)_PyObject_GetManagedDict(obj);
        if (dict == NULL) {
            return 0;    /* the generic code materializes the dict */
        }
        break;
    }
    case RQ_D_MANAGED:
        dict = (PyObject *)_PyObject_GetManagedDict(obj);
        break;
    case RQ_D_OFFSET:
        dict = *(PyObject **)((char *)obj + e->dictoffset);
        break;
    default:
        break;
    }
    if (dict != NULL) {
        if (!PyDict_CheckExact(dict)
            || ((PyDictObject *)dict)->ma_keys->dk_kind == DICT_KEYS_GENERAL) {
            return 0;
        }
        Py_INCREF(dict);
        int rc = PyDict_GetItemRef(dict, name, &inst);
        Py_DECREF(dict);
        if (rc < 0) {
            return -1;
        }
        if (inst != NULL) {
            *res = inst;
            *is_meth = 0;
            rq_ac_hits++; RQ_HIT(0);
            return 1;
        }
    }
    switch (e->kind) {
    case RQ_K_PLAIN:
        *res = Py_NewRef(e->descr);
        *is_meth = 0;
        rq_ac_hits++; RQ_HIT(0);
        return 1;
    case RQ_K_METHOD:
        if (meth) {
            *res = Py_NewRef(e->descr);
            *is_meth = 1;
            rq_ac_hits++; RQ_HIT(0);
            return 1;
        }
        else {
            descrgetfunc f = Py_TYPE(e->descr)->tp_descr_get;
            if (f == NULL) {
                return 0;
            }
            rq_ac_hits++; RQ_HIT(0);
            *is_meth = 0;
            *res = f(e->descr, obj, (PyObject *)tp);
            return *res != NULL ? 1 : -1;
        }
    default:
        return 0;
    }
}

static _PyStackRef
rq_LoadAttr_StackRefSteal(PyThreadState *tstate, _PyStackRef owner, PyObject *name,
                          _PyStackRef *self_or_null, _Py_CODEUNIT *instr)
{
    PyObject *res;
    int is_meth = 0;
    int r = rq_attr_fast(tstate, PyStackRef_AsPyObjectBorrow(owner), name, 1, &res, &is_meth,
                         instr);
    if (r == 0) {
        return _Py_LoadAttr_StackRefSteal(tstate, owner, name, self_or_null);
    }
    if (r < 0) {
        PyStackRef_CLOSE(owner);
        *self_or_null = PyStackRef_NULL;
        return PyStackRef_NULL;
    }
    if (is_meth) {
        *self_or_null = owner;
    }
    else {
        PyStackRef_CLOSE(owner);
        *self_or_null = PyStackRef_NULL;
    }
    return PyStackRef_FromPyObjectSteal(res);
}

static _PyStackRef
rq_GetAttrStackRef(PyThreadState *tstate, PyObject *v, PyObject *name, _Py_CODEUNIT *instr)
{
    PyObject *res;
    int is_meth = 0;
    int r = rq_attr_fast(tstate, v, name, 0, &res, &is_meth, instr);
    if (r == 0) {
        return _PyObject_GetAttrStackRef(v, name);
    }
    if (r < 0) {
        return PyStackRef_NULL;
    }
    return PyStackRef_FromPyObjectSteal(res);
}
#endif
#endif
