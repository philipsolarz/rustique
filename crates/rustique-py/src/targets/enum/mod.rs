//! Group `enum`: the hot paths of `enum.py` — member `.value`/`.name`, value lookup
//! `Color(v)`, `hash(member)` and the `Flag` operators — behind entry points CPython reaches
//! from C, so a fallback costs a few instructions:
//!
//! | target | site | reached from |
//! |---|---|---|
//! | `enum.property.__get__` | `tp_descr_get` of the heap type `enum.property` | generic `LOAD_ATTR` (`m.value`) |
//! | `enum.EnumType.__call__` | `tp_call` of `EnumType` + vectorcall of `EnumType.__call__` | `Color(2)`; metaclass subclasses via `slot_tp_call` |
//! | `enum.Enum.__hash__` | vectorcall of `Enum.__hash__` | `slot_tp_hash` (`hash(m)`, `d[m]`) |
//! | `enum.Flag.operators` | vectorcall of `Flag.__or__/__and__/__xor__/__invert__/__contains__` | `slot_nb_*`, `slot_sq_contains` |
//!
//! OWNER: datamodel workstream. See docs/targets/datamodel.md.
//!
//! Exactness. Every kernel reproduces the Python method's path for its exact subset and
//! runs CPython's method for everything else, before anything observable:
//! - slots: the exact type only (`enum.property` instances, classes whose metaclass is
//!   exactly `EnumType`); subclasses get CPython's `slot_*` dispatcher (the original), which
//!   resolves their own methods;
//! - per-class facts (`ClassCache`, keyed by `tp_version_tag`, which changes on any
//!   attribute change of the class or a base; the metaclass's are checked the same way):
//!   the generic attribute protocol, no descriptor named `_value_`/`_name_`/`_inverted_`,
//!   `__class__` and `_get_value` CPython's, `__new__` is `Enum.__new__`, `_member_map_`
//!   a non-empty exact dict, `_value2member_map_` an exact dict whose keys all have
//!   builtin types (a lookup with an `int`/`str`/`bool`/`float` value then compares in C
//!   only); both dicts are watched: any mutation clears the class's facts;
//! - values: lookups only for exact `int`/`str`/`bool`/`float` values or the member itself;
//!   a miss (CPython's `_missing_`, pseudo-member creation) runs CPython's method;
//! - the code objects of the methods reproduced are the fingerprinted ones (checked per
//!   call by identity) and no tool listens (globally, or locally on any of them).

use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering::*};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyType;

use crate::accel::ClassCache;
use crate::accel::{
    self, Counters, DepFlag, GroupSpec, Original, Prepare, Reason, Refusal, SlotName, TargetSpec, Tools,
};
use crate::targets::generated::adapter::watch_dict_invalidates;
use crate::targets::generated::common::*;

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

static C_PROP: Counters = Counters::new();
static C_CALL: Counters = Counters::new();
static C_HASH: Counters = Counters::new();
static C_FLAG: Counters = Counters::new();
static DEPS: DepFlag = DepFlag::new();
static TOOLS: Tools = Tools::new();

/// Not on free-threaded builds this wave: checking `fget` per call takes a reference to a
/// shared function object (CPython's own path does not: deferred reference counting), and
/// eight threads contending on it made `m.value` 1.139 in CPU per call (8-thread rule).
static PROP: TargetSpec = TargetSpec {
    name: "enum.property.__get__",
    group: "enum",
    module: Some("enum"),
    versions: VERSIONS,
    free_threaded: false,
    counters: &C_PROP,
    prepare: prepare_prop,
    shared: Some(&SHARED),
    release: Some(release),
    ..TargetSpec::DEFAULT
};

static CALL: TargetSpec = TargetSpec {
    name: "enum.EnumType.__call__",
    group: "enum",
    module: Some("enum"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_CALL,
    prepare: prepare_call,
    shared: Some(&SHARED),
    release: Some(release),
    ..TargetSpec::DEFAULT
};

static HASH: TargetSpec = TargetSpec {
    name: "enum.Enum.__hash__",
    group: "enum",
    module: Some("enum"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_HASH,
    prepare: prepare_hash,
    shared: Some(&SHARED),
    release: Some(release),
    ..TargetSpec::DEFAULT
};

static FLAG: TargetSpec = TargetSpec {
    name: "enum.Flag.operators",
    group: "enum",
    module: Some("enum"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_FLAG,
    prepare: prepare_flag,
    shared: Some(&SHARED),
    release: Some(release),
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&PROP, &CALL, &HASH, &FLAG];

static SHARED: GroupSpec = GroupSpec { name: "enum", prepare: prepare_shared };

mod fp {
    use crate::accel::Fingerprints;
    pub const PROP_GET: Fingerprints = &[(3, 14, "b8022a6ef56c1c54"), (3, 15, "a56138c619b4395a")];
    pub const CALL: Fingerprints = &[(3, 14, "99e5dbdb50e26c1f"), (3, 15, "bf18ecf593f41ac4")];
    pub const NEW: Fingerprints = &[(3, 14, "247b8a5900c0f274"), (3, 15, "c4c501d955bce208")];
    pub const HASH: Fingerprints = &[(3, 14, "83c3d1bbeb05254d"), (3, 15, "551fd435be3635f1")];
    pub const NAME_FGET: Fingerprints = &[(3, 14, "fd32aad208ad95bf"), (3, 15, "8c6af6e312a3b8a3")];
    pub const VALUE_FGET: Fingerprints = &[(3, 14, "5294087bd3fcc47d"), (3, 15, "7cf6b6942e27f9bf")];
    pub const GET_VALUE: Fingerprints = &[(3, 14, "53b606a7c667d30a"), (3, 15, "ae8103dde2fe78fb")];
    pub const OR: Fingerprints = &[(3, 14, "b4b08247147ecc53"), (3, 15, "8c310e1246fb7939")];
    pub const AND: Fingerprints = &[(3, 14, "6e003f25d26cd45f"), (3, 15, "dd5978d74a6e9283")];
    pub const XOR: Fingerprints = &[(3, 14, "404c23e61bcbaad3"), (3, 15, "8d4da5ba04013e16")];
    pub const INVERT: Fingerprints = &[(3, 14, "f771e6d7837db565"), (3, 15, "6140431e67f9c95c")];
    pub const CONTAINS: Fingerprints = &[(3, 14, "44c9688f0360ce51"), (3, 15, "3714e7461858e130")];
}

// ---------------------------------------------------------------------------------------
// State (filled by the shared prepare; strong references kept until exit)
// ---------------------------------------------------------------------------------------

/// A validated Python function and its code object.
struct Fn {
    func: Slot,
    code: Slot,
}

impl Fn {
    const fn new() -> Self {
        Fn { func: Slot::new(), code: Slot::new() }
    }
    fn keep(&self, py: Python<'_>, f: &Bound<'_, PyAny>) {
        self.func.keep(f);
        // SAFETY: an exact function (validated by the caller).
        self.code.keep(&unsafe { Bound::from_borrowed_ptr(py, func_code(f.as_ptr())) });
    }
    /// The function still runs the validated code and no tool listens on it locally.
    #[inline(always)]
    unsafe fn intact(&self) -> bool {
        let f = self.func.get();
        // SAFETY: kept alive by this struct; layout calibrated at install.
        !f.is_null() && unsafe { func_code(f) == self.code.get() && accel::code_locally_quiet(self.code.get()) }
    }
    fn release(&self) {
        self.func.release();
        self.code.release();
    }
}

static PROP_GET: Fn = Fn::new();
static CALL_FN: Fn = Fn::new();
static NEW_FN: Fn = Fn::new();
static HASH_FN: Fn = Fn::new();
static NAME_FGET: Fn = Fn::new();
static VALUE_FGET: Fn = Fn::new();
static GET_VALUE: Fn = Fn::new();
static OR_FN: Fn = Fn::new();
static AND_FN: Fn = Fn::new();
static XOR_FN: Fn = Fn::new();
static INVERT_FN: Fn = Fn::new();
static CONTAINS_FN: Fn = Fn::new();

/// `enum.property` and `EnumType` (types), `Enum.__new__`'s staticmethod,
/// `object.__dict__['__class__']`.
static PROP_TYPE: Slot = Slot::new();
/// `Enum.__dict__['value']` and `['name']`: the two `enum.property` objects member
/// `.value`/`.name` resolve to (any other `enum.property` falls back at once).
static VALUE_PROP: Slot = Slot::new();
static NAME_PROP: Slot = Slot::new();
static ENUM_TYPE: Slot = Slot::new();
static NEW_SM: Slot = Slot::new();
static OBJ_CLASS: Slot = Slot::new();

static N_FGET: Slot = Slot::new();
static N_VALUE_: Slot = Slot::new();
static N_NAME_: Slot = Slot::new();
static N_INVERTED_: Slot = Slot::new();
static N_MEMBER_MAP: Slot = Slot::new();
static N_V2M: Slot = Slot::new();
static N_NEW: Slot = Slot::new();
static N_CLASS: Slot = Slot::new();
static N_GET_VALUE: Slot = Slot::new();
static N_CALL: Slot = Slot::new();
static N_GET: Slot = Slot::new();

static ORIG_DESCR_GET: Original<ffi::descrgetfunc> = Original::new();
static ORIG_TP_CALL: Original<ffi::ternaryfunc> = Original::new();

fn release(_py: Python<'_>) {
    for f in [
        &PROP_GET,
        &CALL_FN,
        &NEW_FN,
        &HASH_FN,
        &NAME_FGET,
        &VALUE_FGET,
        &GET_VALUE,
        &OR_FN,
        &AND_FN,
        &XOR_FN,
        &INVERT_FN,
        &CONTAINS_FN,
    ] {
        f.release();
    }
    for s in [&PROP_TYPE, &VALUE_PROP, &NAME_PROP, &ENUM_TYPE, &NEW_SM, &OBJ_CLASS] {
        s.release();
    }
    CLASS_FACTS.clear();
    META_FACTS.clear();
    PROP_TAG.store(0, Relaxed);
}

// ---------------------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------------------

/// `type(member)` reads `_value_`/`_name_` through the generic protocol, no descriptors.
const B_ATTRS: u32 = 1;
/// `B_ATTRS` plus `_inverted_` plain, `__class__` and `_get_value` CPython's.
const B_FLAG: u32 = 2;
/// `cls(value)` may be answered from `_value2member_map_` (see module docs).
const B_LOOKUP: u32 = 4;
/// Marks a computed verdict (a payload of 0 would be ambiguous with nothing).
const B_DONE: u32 = 1 << 31;

static CLASS_FACTS: ClassCache<512> = ClassCache::new();
/// Metaclasses other than `EnumType` itself: payload 1 = ok.
static META_FACTS: ClassCache<16> = ClassCache::new();
/// `EnumType`'s tag when last validated.
static ENUM_TYPE_TAG: AtomicU32 = AtomicU32::new(0);
/// `enum.property`'s tag when last validated.
static PROP_TAG: AtomicU32 = AtomicU32::new(0);

#[inline(always)]
unsafe fn class_bits(cls: Tp) -> u32 {
    // SAFETY: per `compute_class`.
    unsafe {
        match CLASS_FACTS.get(cls) {
            Some(b) => b,
            None => compute_class(cls),
        }
    }
}

/// All keys have builtin, immutable types: a lookup with an exact int/str/bool/float
/// compares in C only.
unsafe fn keys_safe(d: P) -> bool {
    // SAFETY: an exact dict, iterated under its critical section (free-threaded builds).
    unsafe {
        let f = || {
            let mut pos: ffi::Py_ssize_t = 0;
            let mut k: P = null_mut();
            let mut v: P = null_mut();
            while ffi::PyDict_Next(d, &mut pos, &mut k, &mut v) != 0 {
                let t = ffi::Py_TYPE(k);
                let ok = t == &raw mut ffi::PyLong_Type
                    || t == &raw mut ffi::PyUnicode_Type
                    || t == &raw mut ffi::PyBool_Type
                    || t == &raw mut ffi::PyFloat_Type
                    || t == &raw mut ffi::PyTuple_Type
                    || t == &raw mut ffi::PyBytes_Type
                    || t == &raw mut ffi::PyFrozenSet_Type
                    || t == &raw mut ffi::PyComplex_Type
                    || k == ffi::Py_None();
                if !ok {
                    return false;
                }
            }
            true
        };
        with_cs(d, f)
    }
}

/// Run `f` holding `o`'s critical section (free-threaded builds; plain call otherwise).
#[inline]
pub(crate) unsafe fn with_cs<R>(o: P, f: impl FnOnce() -> R) -> R {
    #[cfg(Py_GIL_DISABLED)]
    // SAFETY: attached thread; the section is ended before returning.
    unsafe {
        let mut cs = std::mem::MaybeUninit::<ffi::PyCriticalSection>::uninit();
        ffi::PyCriticalSection_Begin(cs.as_mut_ptr(), o);
        let r = f();
        ffi::PyCriticalSection_End(cs.as_mut_ptr());
        r
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = o;
        f()
    }
}

#[cold]
unsafe fn compute_class(cls: Tp) -> u32 {
    // SAFETY: attached thread; MRO lookups and dict reads run no Python code.
    unsafe {
        let (tag, epoch) = CLASS_FACTS.begin(cls);
        let mut bits = B_DONE;
        if generic_getattr(cls) && plain_attr(cls, N_VALUE_.get()) && plain_attr(cls, N_NAME_.get()) {
            bits |= B_ATTRS;
            if plain_attr(cls, N_INVERTED_.get())
                && resolves_to(cls, N_CLASS.get(), OBJ_CLASS.get())
                && resolves_to(cls, N_GET_VALUE.get(), GET_VALUE.func.get())
            {
                bits |= B_FLAG;
            }
        }
        let mm = lookup(cls, N_MEMBER_MAP.get());
        let v2m = lookup(cls, N_V2M.get());
        let nw = lookup(cls, N_NEW.get());
        // Watch the two dicts only for a verdict that relies on them (classes under
        // construction have no members yet: no watch, no cost).
        if !mm.is_null()
            && !v2m.is_null()
            && ffi::PyDict_CheckExact(mm) != 0
            && ffi::PyDict_CheckExact(v2m) != 0
            && (nw == NEW_FN.func.get() || nw == NEW_SM.get())
            && ffi::PyDict_Size(mm) > 0
            && watch_dict_invalidates(mm, &CLASS_FACTS, cls)
            && watch_dict_invalidates(v2m, &CLASS_FACTS, cls)
            && ffi::PyDict_Size(mm) > 0
            && keys_safe(v2m)
        {
            bits |= B_LOOKUP;
        }
        for o in [mm, v2m, nw] {
            if !o.is_null() {
                ffi::Py_DECREF(o);
            }
        }
        CLASS_FACTS.put(cls, tag, epoch, bits);
        bits
    }
}

/// The metaclass's attribute protocol leaves `cls._member_map_`, `cls._value2member_map_`
/// and `cls.__new__` to the class, and its `__call__` is `EnumType.__call__`.
#[inline(always)]
unsafe fn meta_ok(meta: Tp) -> bool {
    // SAFETY: per `compute_meta`.
    unsafe {
        if meta.cast::<ffi::PyObject>() == ENUM_TYPE.get() {
            let t = tag_of(meta);
            return t != 0 && ENUM_TYPE_TAG.load(Relaxed) == t || compute_enum_type(meta);
        }
        match META_FACTS.get(meta) {
            Some(b) => b == 1,
            None => compute_meta(meta),
        }
    }
}

unsafe fn meta_facts(meta: Tp) -> bool {
    // SAFETY: attached thread; live type; MRO lookups only.
    unsafe {
        let tt = &raw mut ffi::PyType_Type;
        if addr!((*meta).tp_getattro) != addr!((*tt).tp_getattro)
            || ffi::PyType_IsSubtype(meta, ENUM_TYPE.get().cast()) == 0
        {
            return false;
        }
        if !resolves_to(meta, N_MEMBER_MAP.get(), null_mut())
            || !resolves_to(meta, N_V2M.get(), null_mut())
            || !resolves_to(meta, N_CALL.get(), CALL_FN.func.get())
        {
            return false;
        }
        // `__new__` on the metaclass is a non-data descriptor (the class's own wins).
        let nw = lookup(meta, N_NEW.get());
        if nw.is_null() {
            return true;
        }
        let ok = (*ffi::Py_TYPE(nw)).tp_descr_set.is_none();
        ffi::Py_DECREF(nw);
        ok
    }
}

#[cold]
unsafe fn compute_enum_type(meta: Tp) -> bool {
    // SAFETY: per `meta_facts`.
    unsafe {
        let tag = ensure_tag(meta);
        let ok = meta_facts(meta);
        if ok && tag != 0 && tag_of(meta) == tag {
            ENUM_TYPE_TAG.store(tag, Relaxed);
        }
        ok
    }
}

#[cold]
unsafe fn compute_meta(meta: Tp) -> bool {
    // SAFETY: per `meta_facts`.
    unsafe {
        let (tag, epoch) = META_FACTS.begin(meta);
        let ok = meta_facts(meta);
        META_FACTS.put(meta, tag, epoch, u32::from(ok));
        ok
    }
}

/// `enum.property` instances: generic attribute protocol, `fget` a plain instance
/// attribute, `__get__` the validated function.
#[inline(always)]
unsafe fn prop_type_ok(t: Tp) -> bool {
    // SAFETY: live type.
    unsafe {
        let tag = tag_of(t);
        tag != 0 && PROP_TAG.load(Relaxed) == tag || compute_prop_type(t)
    }
}

#[cold]
unsafe fn compute_prop_type(t: Tp) -> bool {
    // SAFETY: attached thread; MRO lookups only.
    unsafe {
        let tag = ensure_tag(t);
        let ok = generic_getattr(t) && plain_attr(t, N_FGET.get()) && resolves_to(t, N_GET.get(), PROP_GET.func.get());
        if ok && tag != 0 && tag_of(t) == tag {
            PROP_TAG.store(tag, Relaxed);
        }
        ok
    }
}

// ---------------------------------------------------------------------------------------
// Value lookup: `cls(value)` = EnumType.__call__ + Enum.__new__ (hit path)
// ---------------------------------------------------------------------------------------

/// The member `cls(value)` returns, or NULL (no error set) when the kernel cannot prove the
/// lookup runs no Python code or the value is not a member. The caller checked the
/// metaclass (`meta_ok`) and the tools.
#[inline]
unsafe fn lookup_member(cls: P, value: P) -> P {
    // SAFETY: attached thread; `cls` a class.
    unsafe {
        let ct = cls.cast::<ffi::PyTypeObject>();
        if class_bits(ct) & B_LOOKUP == 0 || !NEW_FN.intact() {
            return null_mut();
        }
        let vt = ffi::Py_TYPE(value);
        if vt == ct {
            return incref(value);
        }
        if !(vt == &raw mut ffi::PyLong_Type
            || vt == &raw mut ffi::PyUnicode_Type
            || vt == &raw mut ffi::PyBool_Type
            || vt == &raw mut ffi::PyFloat_Type)
        {
            return null_mut();
        }
        let v2m = lookup(ct, N_V2M.get());
        if v2m.is_null() {
            return null_mut();
        }
        let mut r: P = null_mut();
        let rc = if ffi::PyDict_CheckExact(v2m) != 0 { ffi::PyDict_GetItemRef(v2m, value, &mut r) } else { 0 };
        ffi::Py_DECREF(v2m);
        if rc < 0 {
            ffi::PyErr_Clear();
            return null_mut();
        }
        r
    }
}

/// Tools quiet for the value-lookup path (`EnumType.__call__` and `Enum.__new__` frames).
#[inline(always)]
unsafe fn lookup_quiet() -> bool {
    // SAFETY: attached thread.
    unsafe { TOOLS.quiet() && CALL_FN.intact() && NEW_FN.intact() }
}

/// `tp_call` of `EnumType` (CPython: `slot_tp_call` → `EnumType.__call__`).
unsafe extern "C" fn k_enumtype_call(cls: P, args: P, kw: P) -> P {
    // SAFETY: CPython's tp_call protocol.
    unsafe {
        let meta = ffi::Py_TYPE(cls);
        if meta.cast::<ffi::PyObject>() != ENUM_TYPE.get()
            || ffi::PyTuple_GET_SIZE(args) != 1
            || (!kw.is_null() && ffi::PyDict_Size(kw) != 0)
        {
            C_CALL.fallback(Reason::Args);
            return ORIG_TP_CALL.get()(cls, args, kw);
        }
        // A class whose facts rule the lookup out (a value map with a key of a user type, no
        // members yet, another `__new__`) takes CPython's path at once: the cached verdict
        // costs a few loads; the dict watcher recomputes it when either map changes.
        if class_bits(cls.cast()) & B_LOOKUP == 0 {
            C_CALL.fallback(Reason::Args);
            return ORIG_TP_CALL.get()(cls, args, kw);
        }
        if !DEPS.ok() || !lookup_quiet() || !meta_ok(meta) {
            C_CALL.fallback(Reason::Tools);
            return ORIG_TP_CALL.get()(cls, args, kw);
        }
        let r = lookup_member(cls, ffi::PyTuple_GET_ITEM(args, 0));
        if r.is_null() {
            C_CALL.fallback(Reason::Args);
            return ORIG_TP_CALL.get()(cls, args, kw);
        }
        C_CALL.hit();
        r
    }
}

/// `EnumType.__call__` as a function (metaclass subclasses reach it from `slot_tp_call`).
unsafe extern "C" fn k_enum_call_vc(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        if nargs(nargsf) != 2 || !kw.is_null() || f != CALL_FN.func.get() {
            C_CALL.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let cls = *args;
        // Classes whose metaclass is exactly `EnumType` reach this function from the
        // `tp_call` kernel's fallback (`slot_tp_call`), which already tried: don't twice.
        if ffi::PyType_Check(cls) == 0 || ffi::Py_TYPE(cls).cast::<ffi::PyObject>() == ENUM_TYPE.get() {
            C_CALL.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DEPS.ok() || !lookup_quiet() || !meta_ok(ffi::Py_TYPE(cls)) {
            C_CALL.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let r = lookup_member(cls, *args.add(1));
        if r.is_null() {
            C_CALL.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        C_CALL.hit();
        r
    }
}

// ---------------------------------------------------------------------------------------
// `.value` / `.name`
// ---------------------------------------------------------------------------------------

/// `tp_descr_get` of `enum.property` (CPython: `slot_tp_descr_get` → `property.__get__`).
unsafe extern "C" fn k_prop_descr_get(this: P, inst: P, owner: P) -> P {
    // SAFETY: CPython's tp_descr_get protocol.
    unsafe {
        if (this != VALUE_PROP.get() && this != NAME_PROP.get()) || inst.is_null() || inst == ffi::Py_None() {
            C_PROP.fallback(Reason::Args);
            return ORIG_DESCR_GET.get()(this, inst, owner);
        }
        if !TOOLS.quiet() || !PROP_GET.intact() || !prop_type_ok(ffi::Py_TYPE(this)) {
            C_PROP.fallback(Reason::Tools);
            return ORIG_DESCR_GET.get()(this, inst, owner);
        }
        let fget = get_plain(this, N_FGET.get());
        if fget.is_null() {
            C_PROP.fallback(Reason::Args);
            return ORIG_DESCR_GET.get()(this, inst, owner);
        }
        let name = if fget == VALUE_FGET.func.get() && VALUE_FGET.intact() {
            N_VALUE_.get()
        } else if fget == NAME_FGET.func.get() && NAME_FGET.intact() {
            N_NAME_.get()
        } else {
            null_mut()
        };
        ffi::Py_DECREF(fget);
        if name.is_null() || class_bits(ffi::Py_TYPE(inst)) & B_ATTRS == 0 {
            C_PROP.fallback(Reason::Args);
            return ORIG_DESCR_GET.get()(this, inst, owner);
        }
        let r = get_plain(inst, name);
        if r.is_null() {
            C_PROP.fallback(Reason::Error);
            return ORIG_DESCR_GET.get()(this, inst, owner);
        }
        C_PROP.hit();
        r
    }
}

// ---------------------------------------------------------------------------------------
// hash(member)
// ---------------------------------------------------------------------------------------

/// `Enum.__hash__(self)` = `hash(self._name_)`.
unsafe extern "C" fn k_hash(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        if nargs(nargsf) != 1 || !kw.is_null() || f != HASH_FN.func.get() {
            C_HASH.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DEPS.ok() || !TOOLS.quiet() || !HASH_FN.intact() {
            C_HASH.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let this = *args;
        if class_bits(ffi::Py_TYPE(this)) & B_ATTRS == 0 {
            C_HASH.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let name = get_plain(this, N_NAME_.get());
        if name.is_null() || !(ffi::PyUnicode_CheckExact(name) != 0 || name == ffi::Py_None()) {
            if !name.is_null() {
                ffi::Py_DECREF(name);
            }
            C_HASH.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        let h = ffi::PyObject_Hash(name);
        ffi::Py_DECREF(name);
        if h == -1 {
            ffi::PyErr_Clear();
            C_HASH.fallback(Reason::Error);
            return orig(f, args, nargsf, kw);
        }
        C_HASH.hit();
        ffi::PyLong_FromSsize_t(h)
    }
}

// ---------------------------------------------------------------------------------------
// Flag operators
// ---------------------------------------------------------------------------------------

/// Both operands' `_value_` (new references) when both are members of exactly the same
/// class with the Flag facts, the values are exact ints, and the tools are quiet on
/// `_get_value` too.
#[inline]
unsafe fn flag_values(a: P, b: P) -> Option<(P, P)> {
    // SAFETY: attached thread; validated types.
    unsafe {
        let t = ffi::Py_TYPE(a);
        if t != ffi::Py_TYPE(b) || class_bits(t) & B_FLAG == 0 || !GET_VALUE.intact() {
            return None;
        }
        let va = get_plain(a, N_VALUE_.get());
        if va.is_null() {
            return None;
        }
        let vb = get_plain(b, N_VALUE_.get());
        if vb.is_null() || ffi::PyLong_CheckExact(va) == 0 || ffi::PyLong_CheckExact(vb) == 0 {
            ffi::Py_DECREF(va);
            if !vb.is_null() {
                ffi::Py_DECREF(vb);
            }
            return None;
        }
        Some((va, vb))
    }
}

/// `self.__class__(v)` for a Flag result: the class's metaclass call is
/// `EnumType.__call__`, answered from the value map (a miss creates a pseudo-member in
/// CPython's `_missing_`: the caller falls back).
#[inline]
unsafe fn flag_result(cls: P, v: P) -> P {
    // SAFETY: per `lookup_member`.
    unsafe {
        if !lookup_quiet() || !meta_ok(ffi::Py_TYPE(cls)) {
            return null_mut();
        }
        lookup_member(cls, v)
    }
}

macro_rules! flag_binop {
    ($name:ident, $fnst:ident, $op:path) => {
        unsafe extern "C" fn $name(f: P, args: *const P, nargsf: usize, kw: P) -> P {
            // SAFETY: CPython's vectorcall protocol.
            unsafe {
                if nargs(nargsf) != 2 || !kw.is_null() || f != $fnst.func.get() {
                    C_FLAG.fallback(Reason::Args);
                    return orig(f, args, nargsf, kw);
                }
                if !DEPS.ok() || !TOOLS.quiet() || !$fnst.intact() {
                    C_FLAG.fallback(Reason::Tools);
                    return orig(f, args, nargsf, kw);
                }
                let (a, b) = (*args, *args.add(1));
                let Some((va, vb)) = flag_values(a, b) else {
                    C_FLAG.fallback(Reason::Args);
                    return orig(f, args, nargsf, kw);
                };
                let v = $op(va, vb);
                ffi::Py_DECREF(va);
                ffi::Py_DECREF(vb);
                if v.is_null() {
                    ffi::PyErr_Clear();
                    C_FLAG.fallback(Reason::Error);
                    return orig(f, args, nargsf, kw);
                }
                let r = flag_result(ffi::Py_TYPE(a).cast(), v);
                ffi::Py_DECREF(v);
                if r.is_null() {
                    C_FLAG.fallback(Reason::Args);
                    return orig(f, args, nargsf, kw);
                }
                C_FLAG.hit();
                r
            }
        }
    };
}
flag_binop!(k_flag_or, OR_FN, ffi::PyNumber_Or);
flag_binop!(k_flag_and, AND_FN, ffi::PyNumber_And);
flag_binop!(k_flag_xor, XOR_FN, ffi::PyNumber_Xor);

/// `Flag.__contains__(self, other)`: `other._value_ & self._value_ == other._value_`.
unsafe extern "C" fn k_flag_contains(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        if nargs(nargsf) != 2 || !kw.is_null() || f != CONTAINS_FN.func.get() {
            C_FLAG.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DEPS.ok() || !TOOLS.quiet() || !CONTAINS_FN.intact() {
            C_FLAG.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let (a, b) = (*args, *args.add(1));
        // `isinstance(other, self.__class__)` with type(other) is type(self).
        let Some((va, vb)) = flag_values(a, b) else {
            C_FLAG.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        };
        let and = ffi::PyNumber_And(vb, va);
        let r = if and.is_null() {
            null_mut()
        } else {
            let r = ffi::PyObject_RichCompare(and, vb, ffi::Py_EQ);
            ffi::Py_DECREF(and);
            r
        };
        ffi::Py_DECREF(va);
        ffi::Py_DECREF(vb);
        if r.is_null() {
            ffi::PyErr_Clear();
            C_FLAG.fallback(Reason::Error);
            return orig(f, args, nargsf, kw);
        }
        C_FLAG.hit();
        r
    }
}

/// `Flag.__invert__(self)`: the cached `self._inverted_` when it is set.
unsafe extern "C" fn k_flag_invert(f: P, args: *const P, nargsf: usize, kw: P) -> P {
    // SAFETY: CPython's vectorcall protocol.
    unsafe {
        if nargs(nargsf) != 1 || !kw.is_null() || f != INVERT_FN.func.get() {
            C_FLAG.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        if !DEPS.ok() || !TOOLS.quiet() || !INVERT_FN.intact() || !GET_VALUE.intact() {
            C_FLAG.fallback(Reason::Tools);
            return orig(f, args, nargsf, kw);
        }
        let a = *args;
        if class_bits(ffi::Py_TYPE(a)) & B_FLAG == 0 {
            C_FLAG.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        // `self._get_value(self) is None` (an error path in CPython) needs `_value_`.
        let va = get_plain(a, N_VALUE_.get());
        if va.is_null() || va == ffi::Py_None() {
            if !va.is_null() {
                ffi::Py_DECREF(va);
            }
            C_FLAG.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        ffi::Py_DECREF(va);
        let inv = get_plain(a, N_INVERTED_.get());
        if inv.is_null() || inv == ffi::Py_None() {
            if !inv.is_null() {
                ffi::Py_DECREF(inv);
            }
            C_FLAG.fallback(Reason::Args);
            return orig(f, args, nargsf, kw);
        }
        C_FLAG.hit();
        inv
    }
}

// ---------------------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------------------

fn function<'py>(cx: &mut Prepare<'py>, owner: &Bound<'py, PyType>, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    let o = cx.type_attr(owner, name)?;
    // A staticmethod/property wrapper is unwrapped by the caller.
    Ok(o)
}

fn exact_fn(o: &Bound<'_, PyAny>, what: &str) -> Result<(), Refusal> {
    if o.is_exact_instance_of::<pyo3::types::PyFunction>() {
        Ok(())
    } else {
        Err(Refusal::new(format!("{what} is not a Python function")))
    }
}

fn module_type<'py>(cx: &mut Prepare<'py>, n: &str) -> Result<Bound<'py, PyType>, Refusal> {
    Ok(cx.module_attr(n)?.cast_into::<PyType>().map_err(PyErr::from)?)
}

fn prepare_shared<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let prop_t = module_type(cx, "property")?;
    let enum_type = module_type(cx, "EnumType")?;
    let enum_cls = module_type(cx, "Enum")?;
    let flag_cls = module_type(cx, "Flag")?;
    let pg = function(cx, &prop_t, "__get__")?;
    exact_fn(&pg, "enum.property.__get__")?;
    cx.fingerprint_code(&pg, fp::PROP_GET)?;
    let call = function(cx, &enum_type, "__call__")?;
    exact_fn(&call, "EnumType.__call__")?;
    cx.fingerprint_code(&call, fp::CALL)?;
    let new_sm = function(cx, &enum_cls, "__new__")?;
    let new = new_sm.getattr("__func__")?;
    exact_fn(&new, "Enum.__new__")?;
    cx.fingerprint_code(&new, fp::NEW)?;
    let hash = function(cx, &enum_cls, "__hash__")?;
    exact_fn(&hash, "Enum.__hash__")?;
    cx.fingerprint_code(&hash, fp::HASH)?;
    let fget_of = |cx: &mut Prepare<'py>, owner: &Bound<'py, PyType>, n: &str| -> Result<Bound<'py, PyAny>, Refusal> {
        let p = cx.type_attr(owner, n)?;
        if !p.get_type().is(&prop_t) {
            return Err(Refusal::new(format!("Enum.{n} is not an enum.property")));
        }
        // An instance attribute of the enum.property (generic protocol: no Python code).
        let fget = crate::accel::prepare::interned(cx.py(), "fget");
        // SAFETY: attached thread; generic attribute read of an exact enum.property.
        let f = unsafe {
            let r = ffi::PyObject_GenericGetAttr(p.as_ptr(), fget.as_ptr());
            Bound::from_owned_ptr_or_err(cx.py(), r)?
        };
        exact_fn(&f, n)?;
        Ok(f)
    };
    let value_prop = cx.type_attr(&enum_cls, "value")?;
    let name_prop = cx.type_attr(&enum_cls, "name")?;
    let name_fget = fget_of(cx, &enum_cls, "name")?;
    cx.fingerprint_code(&name_fget, fp::NAME_FGET)?;
    let value_fget = fget_of(cx, &enum_cls, "value")?;
    cx.fingerprint_code(&value_fget, fp::VALUE_FGET)?;
    let gv = function(cx, &flag_cls, "_get_value")?;
    exact_fn(&gv, "Flag._get_value")?;
    cx.fingerprint_code(&gv, fp::GET_VALUE)?;
    let mut ops = Vec::new();
    for (n, f) in [
        ("__or__", fp::OR),
        ("__and__", fp::AND),
        ("__xor__", fp::XOR),
        ("__invert__", fp::INVERT),
        ("__contains__", fp::CONTAINS),
    ] {
        let o = function(cx, &flag_cls, n)?;
        exact_fn(&o, n)?;
        cx.fingerprint_code(&o, f)?;
        ops.push(o);
    }
    cx.pin_global(&g, "hash", &DEPS)?;
    let object = py.get_type::<pyo3::types::PyAny>();
    let obj_class = crate::accel::prepare::type_dict(&object)?
        .get_item("__class__")?
        .ok_or_else(|| Refusal::new("object.__class__ missing"))?;

    PROP_GET.keep(py, &pg);
    CALL_FN.keep(py, &call);
    NEW_FN.keep(py, &new);
    HASH_FN.keep(py, &hash);
    NAME_FGET.keep(py, &name_fget);
    VALUE_FGET.keep(py, &value_fget);
    GET_VALUE.keep(py, &gv);
    for (st, o) in [&OR_FN, &AND_FN, &XOR_FN, &INVERT_FN, &CONTAINS_FN].into_iter().zip(&ops) {
        st.keep(py, o);
    }
    PROP_TYPE.keep(prop_t.as_any());
    VALUE_PROP.keep(&value_prop);
    NAME_PROP.keep(&name_prop);
    ENUM_TYPE.keep(enum_type.as_any());
    NEW_SM.keep(&new_sm);
    OBJ_CLASS.keep(&obj_class);
    for (s, n) in [
        (&N_FGET, "fget"),
        (&N_VALUE_, "_value_"),
        (&N_NAME_, "_name_"),
        (&N_INVERTED_, "_inverted_"),
        (&N_MEMBER_MAP, "_member_map_"),
        (&N_V2M, "_value2member_map_"),
        (&N_NEW, "__new__"),
        (&N_CLASS, "__class__"),
        (&N_GET_VALUE, "_get_value"),
        (&N_CALL, "__call__"),
        (&N_GET, "__get__"),
    ] {
        s.name(py, n);
    }
    CLASS_FACTS.clear();
    META_FACTS.clear();
    ENUM_TYPE_TAG.store(0, Relaxed);
    PROP_TAG.store(0, Relaxed);
    // The code-object layout the tools guard reads (calibrated by any vectorcall site).
    if !accel::version::calibrate_monitoring(py) {
        return Err(Refusal::with(accel::Category::Layout, "local monitoring layout not calibrated"));
    }
    Ok(())
}

/// The group's state was built from this very module (the registry ran the shared prepare;
/// the fingerprint report runs only target prepares: then run it here).
fn group_state<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    let t = cx.module_attr("property")?;
    if t.as_ptr() != PROP_TYPE.get() || CALL_FN.func.get().is_null() {
        prepare_shared(py, cx)?;
    }
    Ok(())
}

fn prepare_prop<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    group_state(py, cx)?;
    let t = cx.module_attr("property")?.cast_into::<PyType>().map_err(PyErr::from)?;
    // A reference class with a Python `__get__`: its slot is CPython's dispatcher too.
    let types = cx.loaded_module("types")?;
    let dca = cx.get(&types.dict(), "DynamicClassAttribute")?.cast_into::<PyType>().map_err(PyErr::from)?;
    cx.heap_slot(&t, SlotName::TpDescrGet, k_prop_descr_get as ffi::descrgetfunc, &ORIG_DESCR_GET, Some(&dca))
}

fn prepare_call<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    group_state(py, cx)?;
    let t = cx.module_attr("EnumType")?.cast_into::<PyType>().map_err(PyErr::from)?;
    // A reference class with a Python `__call__` (enum.verify): the same dispatcher.
    let verify = cx.module_attr("verify")?.cast_into::<PyType>().map_err(PyErr::from)?;
    cx.heap_slot(&t, SlotName::TpCall, k_enumtype_call as ffi::ternaryfunc, &ORIG_TP_CALL, Some(&verify))?;
    let call = CALL_FN.func.get_bound(cx.py());
    cx.vectorcall(&call, k_enum_call_vc, &accel::FUNCTION_VECTORCALL)
}

fn prepare_hash<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    group_state(py, cx)?;
    let f = HASH_FN.func.get_bound(cx.py());
    cx.vectorcall(&f, k_hash, &accel::FUNCTION_VECTORCALL)
}

fn prepare_flag<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    group_state(py, cx)?;
    let ks: [(&Fn, ffi::vectorcallfunc); 5] = [
        (&OR_FN, k_flag_or),
        (&AND_FN, k_flag_and),
        (&XOR_FN, k_flag_xor),
        (&INVERT_FN, k_flag_invert),
        (&CONTAINS_FN, k_flag_contains),
    ];
    for (st, k) in ks {
        let f = st.func.get_bound(cx.py());
        cx.vectorcall(&f, k, &accel::FUNCTION_VECTORCALL)?;
    }
    Ok(())
}
