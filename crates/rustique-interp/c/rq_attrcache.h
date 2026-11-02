/* Megamorphic attribute cache for the generic LOAD_ATTR path (the _LOAD_ATTR override,
 * overrides.c).
 *
 * Keyed by (tp_version_tag, name). A hit replays exactly what _PyObject_GenericGetAttrWithDict
 * (attribute loads) and _PyObject_GetMethodStackRef (method loads) do for the cached
 * resolution (Objects/object.c, 3.15), in their order:
 *   1. the MRO lookup of `name` on type(obj): cached, valid while the version tag holds
 *      (CPython's method-cache rule: any change to the type or a base changes the tag, tags
 *      are never reused);
 *   2. data descriptors run the generic code, except an object __slots__ member
 *      (Py_T_OBJECT_EX, no audit, plain offset; descr_check holds because the member's class
 *      is in type(obj)'s MRO), read as PyMember_GetOne does; a NULL slot runs the generic
 *      code, which raises;
 *   3. the instance attribute: inline values (as _PyObject_TryGetInstanceAttribute) or the
 *      instance dict (PyDict_GetItemRef; only exact dicts with unicode keys, so no key __eq__
 *      can run);
 *   4. not in the instance: a method descriptor ((descr, self) for method loads, its
 *      tp_descr_get for attribute loads, as CPython calls it) or a plain class attribute.
 * Everything else returns 0 and the generic code runs unchanged: no version tag, a
 * tp_getattro other than PyObject_GenericGetAttr, names that are not immortal exact str,
 * other descriptors when the instance lacks the name, absent attributes (AttributeError and
 * its suggestions), a dict that must first be materialized, other interpreters.
 * The kind of a class attribute can change without type(obj)'s tag changing when the
 * attribute's own (mutable) type gains __get__/__set__: entries also hold type(descr) and
 * its version tag (regrtest: test_opcache test_descriptor_added_after_optimization).
 *
 * GIL builds: entries hold the descriptor borrowed under the version tag, like CPython's
 * specialized inline caches. Free-threaded builds: entries hold only integers and pointers
 * compared for identity (never dereferenced); they are seqlocked; a hit that needs the class
 * attribute gets a strong reference from _PyType_LookupStackRefAndVersion and requires the
 * same version tag (rq_lookup_ref, rq_common.h); instance values and slots are read with
 * _Py_TryXGetRef / _Py_TryIncrefCompare (CPython's own lock-free reads), and any contention
 * runs the generic code. */
#ifndef RQ_ATTRCACHE_H
#define RQ_ATTRCACHE_H

#define RQ_AC_N 4096
enum { RQ_K_ABSENT = 1, RQ_K_PLAIN, RQ_K_METHOD, RQ_K_NONDATA, RQ_K_SLOT, RQ_K_NONE };
enum { RQ_D_INLINE = 1, RQ_D_MANAGED, RQ_D_OFFSET, RQ_D_NONE };

typedef struct {
#ifdef Py_GIL_DISABLED
    unsigned int seq;            /* seqlock: odd while a writer fills the entry */
#endif
    unsigned int version;        /* type(obj)'s tag; 0 = empty */
    unsigned char kind, dkind;
    PyObject *name;              /* identity only: immortal interned str */
    PyObject *descr;             /* GIL builds: borrowed while the tag holds; t: identity only */
    PyTypeObject *dtype;         /* type(descr) when it is mutable (else NULL) ... */
    unsigned int dversion;       /* ... and its version tag at fill time */
    PyDictKeysObject *keys;      /* inline values: the type's shared keys at fill time */
    Py_ssize_t ix, nentries;     /* index of name in keys (or -1) and keys' size then */
    Py_ssize_t offset;           /* slot member offset, or tp_dictoffset */
} rq_ac_e;

static rq_ac_e rq_ac[RQ_AC_N];

static inline __attribute__((always_inline)) rq_ac_e *
rq_ac_slot(unsigned int v, PyObject *name)
{
    return &rq_ac[((v * 2654435761u) ^ (unsigned int)((uintptr_t)name >> 4)) & (RQ_AC_N - 1)];
}

/* Classify `name` on tp (a version-tagged, generic-getattr type) into *out. */
static Py_NO_INLINE int
rq_ac_classify(PyTypeObject *tp, PyObject *name, rq_ac_e *out)
{
    unsigned int v;
    PyObject *descr;
#ifdef Py_GIL_DISABLED
    if (!rq_lookup_ref(tp, name, &descr, &v)) {
        return 0;
    }
#else
    v = tp->tp_version_tag;
    descr = _PyType_Lookup(tp, name);
    if (tp->tp_version_tag != v) {
        v = 0;
    }
#endif
    int ok = 0;
    if (v == 0) {
        goto done;
    }
    unsigned char kind;
    Py_ssize_t offset = 0;
    if (descr == NULL) {
        kind = RQ_K_ABSENT;
    }
    else if (PyDescr_IsData(descr) && Py_TYPE(descr)->tp_descr_get != NULL) {
        PyMemberDef *md = Py_IS_TYPE(descr, &PyMemberDescr_Type)
                          ? ((PyMemberDescrObject *)descr)->d_member : NULL;
        if (md != NULL && md->type == Py_T_OBJECT_EX
            && !(md->flags & (Py_AUDIT_READ | Py_RELATIVE_OFFSET | _Py_AFTER_ITEMS))
            && PyType_IsSubtype(tp, PyDescr_TYPE(descr))) {
            kind = RQ_K_SLOT;
            offset = md->offset;
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
        dversion = FT_ATOMIC_LOAD_UINT32_RELAXED(dtype->tp_version_tag);
        if (dversion == 0) {
            if (!PyUnstable_Type_AssignVersionTag(dtype)) {
                kind = RQ_K_NONE;
                dtype = NULL;
            }
            else {
                dversion = FT_ATOMIC_LOAD_UINT32_RELAXED(dtype->tp_version_tag);
            }
        }
    }
    unsigned char dkind;
    Py_ssize_t ix = -1, nentries = 0;
    PyDictKeysObject *keys = NULL;
    if (tp->tp_flags & Py_TPFLAGS_INLINE_VALUES) {
        dkind = RQ_D_INLINE;
        keys = ((PyHeapTypeObject *)tp)->ht_cached_keys;
        if (keys == NULL) {
            goto done;
        }
        ix = _PyDictKeys_StringLookupSplit(keys, name);
        nentries = FT_ATOMIC_LOAD_SSIZE_ACQUIRE(keys->dk_nentries);
    }
    else if (tp->tp_flags & Py_TPFLAGS_MANAGED_DICT) {
        dkind = RQ_D_MANAGED;
    }
    else if (tp->tp_dictoffset > 0) {
        dkind = RQ_D_OFFSET;
        if (kind != RQ_K_SLOT) {
            offset = tp->tp_dictoffset;
        }
        else {
            /* a slot is a data descriptor: the dict is never consulted */
        }
    }
    else if (tp->tp_dictoffset == 0) {
        dkind = RQ_D_NONE;
    }
    else {
        goto done;
    }
    *out = (rq_ac_e){
        .version = v, .kind = kind, .dkind = dkind, .name = name, .descr = descr,
        .dtype = dtype, .dversion = dversion, .keys = keys, .ix = ix, .nentries = nentries,
        .offset = offset,
    };
    ok = 1;
done:
#ifdef Py_GIL_DISABLED
    Py_XDECREF(descr);
#endif
    return ok;
}

#ifdef Py_GIL_DISABLED
static void
rq_ac_store(rq_ac_e *e, const rq_ac_e *src)
{
    unsigned int s = __atomic_load_n(&e->seq, __ATOMIC_RELAXED);
    if ((s & 1) || !__atomic_compare_exchange_n(&e->seq, &s, s + 1, 0, __ATOMIC_ACQ_REL,
                                                 __ATOMIC_RELAXED)) {
        return;     /* another writer: skip, the next miss refills */
    }
    __atomic_thread_fence(__ATOMIC_RELEASE);
    RQ_ST(e->version, src->version);
    RQ_ST(e->kind, src->kind);
    RQ_ST(e->dkind, src->dkind);
    RQ_ST(e->name, src->name);
    RQ_ST(e->descr, src->descr);
    RQ_ST(e->dtype, src->dtype);
    RQ_ST(e->dversion, src->dversion);
    RQ_ST(e->keys, src->keys);
    RQ_ST(e->ix, src->ix);
    RQ_ST(e->nentries, src->nentries);
    RQ_ST(e->offset, src->offset);
    __atomic_store_n(&e->seq, s + 2, __ATOMIC_RELEASE);
}

/* Copy entry e for (v, name) into *out if it is consistent and matches. */
static inline __attribute__((always_inline)) int
rq_ac_read(rq_ac_e *e, unsigned int v, PyObject *name, rq_ac_e *out)
{
    unsigned int s = __atomic_load_n(&e->seq, __ATOMIC_ACQUIRE);
    if (s & 1) {
        return 0;
    }
    out->version = RQ_LD(e->version);
    out->name = RQ_LD(e->name);
    if (out->version != v || out->name != name) {
        return 0;
    }
    out->kind = RQ_LD(e->kind);
    out->dkind = RQ_LD(e->dkind);
    out->descr = RQ_LD(e->descr);
    out->dtype = RQ_LD(e->dtype);
    out->dversion = RQ_LD(e->dversion);
    out->keys = RQ_LD(e->keys);
    out->ix = RQ_LD(e->ix);
    out->nentries = RQ_LD(e->nentries);
    out->offset = RQ_LD(e->offset);
    __atomic_thread_fence(__ATOMIC_ACQUIRE);
    return __atomic_load_n(&e->seq, __ATOMIC_RELAXED) == s;
}
#endif

/* The entry for (type(obj), name): a pointer to the cache entry (GIL builds) or to a
 * validated copy in *tmp (free-threaded builds); NULL when the name cannot be cached. */
static Py_NO_INLINE const rq_ac_e *
rq_ac_fill(PyTypeObject *tp, PyObject *name, rq_ac_e *e, rq_ac_e *tmp)
{
    RQ_ST(rq_groups[RQ_G_ATTR].fills, RQ_LD(rq_groups[RQ_G_ATTR].fills) + 1);
    if (tp->tp_getattro != PyObject_GenericGetAttr || !PyUnicode_CheckExact(name)
        || !_Py_IsImmortal(name) || !_PyType_IsReady(tp)) {
        return NULL;
    }
    if (!rq_ac_classify(tp, name, tmp)) {
        return NULL;
    }
    if (tmp->version != FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag)) {
        return NULL;
    }
#ifdef Py_GIL_DISABLED
    rq_ac_store(rq_ac_slot(tmp->version, name), tmp);
    return tmp;
#else
    (void)e;
    rq_ac_e *dst = rq_ac_slot(tmp->version, name);
    *dst = *tmp;
    return dst;
#endif
}

static inline __attribute__((always_inline)) const rq_ac_e *
rq_ac_get(PyTypeObject *tp, PyObject *name, rq_ac_e *tmp)
{
    unsigned int v = FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag);
    if (v == 0) {
        return NULL;
    }
    rq_ac_e *e = rq_ac_slot(v, name);
#ifdef Py_GIL_DISABLED
    if (rq_ac_read(e, v, name, tmp)) {
        return tmp;
    }
#else
    if (e->version == v && e->name == name) {
        return e;
    }
#endif
    return rq_ac_fill(tp, name, e, tmp);
}

/* Per-site negative hint: a site that is generic in the bytecode (not a specialized form
 * deopting) and resolved to something uncacheable for type version V records (RQ_MARK, V) in
 * its own unused inline-cache entries, and skips the cache probe while type(obj) still has
 * version V. A pure performance hint: a stale or colliding hint only means the generic code
 * runs. The entries are internal state: the specializer rewrites them when it specializes the
 * site, and code.co_code reports caches as zero. */
#define RQ_MARK 0x5251
static inline __attribute__((always_inline)) int
rq_site_is_generic(_Py_CODEUNIT *instr)
{
    _Py_CODEUNIT w = {.cache = FT_ATOMIC_LOAD_UINT16_RELAXED(instr[0].cache)};
    return w.op.code == LOAD_ATTR;
}

static inline __attribute__((always_inline)) int
rq_site_skip(_Py_CODEUNIT *instr, PyTypeObject *tp)
{
    if (!rq_site_is_generic(instr) || FT_ATOMIC_LOAD_UINT16_RELAXED(instr[2].cache) != RQ_MARK) {
        return 0;
    }
    uint32_t v = (uint32_t)FT_ATOMIC_LOAD_UINT16_RELAXED(instr[3].cache)
                 | ((uint32_t)FT_ATOMIC_LOAD_UINT16_RELAXED(instr[4].cache) << 16);
    return v == FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag);
}

static Py_NO_INLINE void
rq_site_mark(_Py_CODEUNIT *instr, PyTypeObject *tp)
{
    unsigned int v = FT_ATOMIC_LOAD_UINT32_RELAXED(tp->tp_version_tag);
    if (v != 0 && rq_site_is_generic(instr)) {
        FT_ATOMIC_STORE_UINT16_RELAXED(instr[2].cache, RQ_MARK);
        FT_ATOMIC_STORE_UINT16_RELAXED(instr[3].cache, (uint16_t)(v & 0xffff));
        FT_ATOMIC_STORE_UINT16_RELAXED(instr[4].cache, (uint16_t)(v >> 16));
    }
}

/* Free-threaded builds: a strong reference to the cached class attribute, valid for entry e
 * (same version tag, same object, same descriptor type and type version), or 0. */
#ifdef Py_GIL_DISABLED
static Py_NO_INLINE int
rq_ac_descr_ref(PyTypeObject *tp, PyObject *name, const rq_ac_e *e, PyObject **ref)
{
    PyObject *descr;
    unsigned int v;
    if (!rq_lookup_ref(tp, name, &descr, &v)) {
        return 0;
    }
    if (v != e->version || descr != e->descr || descr == NULL
        || (e->dtype != NULL
            && (Py_TYPE(descr) != e->dtype
                || FT_ATOMIC_LOAD_UINT32_RELAXED(e->dtype->tp_version_tag) != e->dversion))) {
        Py_XDECREF(descr);
        return 0;
    }
    *ref = descr;
    return 1;
}
#  define RQ_REF_CLOSE() Py_XDECREF(ref)
#else
#  define RQ_REF_CLOSE() ((void)0)
#endif

/* 1: hit (*res is a new reference; *is_meth set for a (descr, self) method load);
 * 0: not handled (run the generic code; nothing observable happened);
 * -1: error raised exactly where CPython raises it. */
static inline __attribute__((always_inline)) int
rq_attr_fast(PyThreadState *tstate, PyObject *obj, PyObject *name, int meth, PyObject **res,
             int *is_meth, _Py_CODEUNIT *instr)
{
    if (tstate->interp != rq_main_interp) {
        return 0;
    }
    PyTypeObject *tp = Py_TYPE(obj);
    RQ_TICK(RQ_G_ATTR);
    if (rq_site_skip(instr, tp)) {
        return 0;
    }
    rq_ac_e tmp;
    const rq_ac_e *e = rq_ac_get(tp, name, &tmp);
    if (e == NULL || e->kind == RQ_K_NONE) {
        rq_site_mark(instr, tp);
        return 0;
    }
#ifdef Py_GIL_DISABLED
    PyObject *ref = NULL;
    if (e->dtype != NULL && !rq_ac_descr_ref(tp, name, e, &ref)) {
        return 0;   /* the class attribute changed (or its kind may have): generic code */
    }
#else
    if (e->dtype != NULL
        && (Py_TYPE(e->descr) != e->dtype || e->dtype->tp_version_tag != e->dversion)) {
        /* e->descr is alive while type(obj)'s tag holds (checked by rq_ac_get). */
        ((rq_ac_e *)e)->version = 0;   /* the class attribute's kind may have changed */
        return 0;
    }
#endif
    if (e->kind == RQ_K_SLOT) {
        PyObject **addr = (PyObject **)((char *)obj + e->offset);
#ifdef Py_GIL_DISABLED
        PyObject *v = _Py_TryXGetRef(addr);
#else
        PyObject *v = Py_XNewRef(*addr);
#endif
        if (v == NULL) {
            return 0;    /* AttributeError (or contention): the generic code decides */
        }
        *res = v;
        *is_meth = 0;
        RQ_HIT(RQ_G_ATTR);
        return 1;
    }
    PyObject *dict = NULL;
    switch (e->dkind) {
    case RQ_D_INLINE: {
        PyDictValues *values = _PyObject_InlineValues(obj);
        if (FT_ATOMIC_LOAD_UINT8(values->valid)) {
            PyDictKeysObject *keys = ((PyHeapTypeObject *)tp)->ht_cached_keys;
            Py_ssize_t ix = e->ix;
            if (keys != e->keys
                || (ix < 0 && FT_ATOMIC_LOAD_SSIZE_ACQUIRE(keys->dk_nentries) != e->nentries)) {
                ix = _PyDictKeys_StringLookupSplit(keys, name);
            }
            if (ix >= 0) {
#ifdef Py_GIL_DISABLED
                PyObject *value = _Py_atomic_load_ptr_acquire(&values->values[ix]);
                if (value != NULL) {
                    if (!_Py_TryIncrefCompare(&values->values[ix], value)) {
                        RQ_REF_CLOSE();
                        return 0;
                    }
                    RQ_REF_CLOSE();
                    *res = value;
                    *is_meth = 0;
                    RQ_HIT(RQ_G_ATTR);
                    return 1;
                }
                if (!FT_ATOMIC_LOAD_UINT8(values->valid)) {
                    RQ_REF_CLOSE();
                    return 0;
                }
#else
                PyObject *value = values->values[ix];
                if (value != NULL) {
                    *res = Py_NewRef(value);
                    *is_meth = 0;
                    RQ_HIT(RQ_G_ATTR);
                    return 1;
                }
#endif
            }
            break;   /* not in the instance */
        }
        dict = (PyObject *)_PyObject_GetManagedDict(obj);
        if (dict == NULL) {
            RQ_REF_CLOSE();
            return 0;    /* the generic code materializes the dict */
        }
        break;
    }
    case RQ_D_MANAGED:
        dict = (PyObject *)_PyObject_GetManagedDict(obj);
        break;
    case RQ_D_OFFSET:
        dict = FT_ATOMIC_LOAD_PTR_ACQUIRE(*(PyObject **)((char *)obj + e->offset));
        break;
    default:
        break;
    }
    if (dict != NULL) {
        if (!PyDict_CheckExact(dict)
            || ((PyDictKeysObject *)FT_ATOMIC_LOAD_PTR_ACQUIRE(((PyDictObject *)dict)->ma_keys))
                   ->dk_kind == DICT_KEYS_GENERAL) {
            RQ_REF_CLOSE();
            return 0;
        }
        PyObject *inst = NULL;
        Py_INCREF(dict);
        int rc = PyDict_GetItemRef(dict, name, &inst);
        Py_DECREF(dict);
        if (rc < 0) {
            RQ_REF_CLOSE();
            return -1;
        }
        if (inst != NULL) {
            RQ_REF_CLOSE();
            *res = inst;
            *is_meth = 0;
            RQ_HIT(RQ_G_ATTR);
            return 1;
        }
    }
    if (e->kind != RQ_K_PLAIN && e->kind != RQ_K_METHOD) {
        RQ_REF_CLOSE();
        return 0;
    }
    /* Not in the instance: the class attribute itself. */
#ifdef Py_GIL_DISABLED
    if (ref == NULL && !rq_ac_descr_ref(tp, name, e, &ref)) {
        return 0;
    }
    PyObject *descr = ref;
#else
    PyObject *descr = e->descr;
#endif
    if (e->kind == RQ_K_PLAIN || meth) {
#ifdef Py_GIL_DISABLED
        *res = ref;     /* the strong reference from the lookup */
#else
        *res = Py_NewRef(descr);
#endif
        *is_meth = e->kind == RQ_K_METHOD;
        RQ_HIT(RQ_G_ATTR);
        return 1;
    }
    /* RQ_K_METHOD, attribute load: bind it as CPython does (f(descr, obj, type(obj))). */
    descrgetfunc f = Py_TYPE(descr)->tp_descr_get;
    if (f == NULL) {
        RQ_REF_CLOSE();
        return 0;
    }
    RQ_HIT(RQ_G_ATTR);
    *is_meth = 0;
    *res = f(descr, obj, (PyObject *)tp);
    RQ_REF_CLOSE();
    return *res != NULL ? 1 : -1;
}

/* _Py_LoadAttr_StackRefSteal with the cache in front (method loads, oparg & 1). */
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

/* _PyObject_GetAttrStackRef with the cache in front (attribute loads). */
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
