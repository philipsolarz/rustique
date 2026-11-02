//! `uuid.UUID` in `deepcopy` (and `dataclasses.asdict` leaves), natively.
//!
//! CPython copies a UUID through `object.__reduce_ex__(4)` -> `UUID.__getstate__` (a Python
//! method: `{'int': self.int}`, plus `'is_safe': self.is_safe.value` unless it is
//! `SafeUUID.unknown`), then `_reconstruct`: `copyreg.__newobj__(UUID)`, `memo[id(x)] = y`,
//! `deepcopy(state)`, `y.__setstate__(state)` (a Python method: `object.__setattr__` of the
//! `int` slot, and of `is_safe` to `SafeUUID(state['is_safe'])` or `SafeUUID.unknown`).
//! These are standard-library functions, not user code: the kernel emulates them (as it
//! emulates `copy.py`'s and `copyreg`'s) once they are verified to be the recorded ones.
//!
//! Exact subset: `type(x)` is the class `sys.modules['uuid'].UUID` (resolved lazily, never
//! imported), whose `__getstate__`/`__setstate__` have the recorded code fingerprints, no
//! other copy hook (`__deepcopy__`, `__reduce_ex__`, `__reduce__`, `__getnewargs[_ex]__`,
//! `__new__` are `object`'s or absent), metatype `type`, MRO `(UUID, object)`, slots `int`
//! and `is_safe` as member descriptors; the class has not changed since (type version tag,
//! the methods' code objects) and `uuid.SafeUUID` is still the validated enum whose
//! `unknown` member is unchanged; `x.int` is an exact `int` and `x.is_safe` is
//! `SafeUUID.unknown` (the only state without an enum call; any other UUID restarts).

use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;

use super::reduce::addr;
use super::rt::*;

/// Recorded fingerprints (format v2) of `uuid.UUID.__getstate__` / `__setstate__`.
const FP_GETSTATE: crate::accel::Fingerprints = &[(3, 14, "4aed2bf10b533995"), (3, 15, "34553904bd4f8992")];
const FP_SETSTATE: crate::accel::Fingerprints = &[(3, 14, "ac3a32a4fe7a35af"), (3, 15, "1f178d7c0bb94b3c")];

/// What was validated (strong references kept for the process, like `known.rs`'s types).
pub struct Uuid {
    pub cls: *mut ffi::PyTypeObject,
    tag: u32,
    getstate: O,
    getstate_code: O,
    setstate: O,
    setstate_code: O,
    /// Byte offsets of the `int` and `is_safe` slots.
    pub off_int: isize,
    pub off_safe: isize,
    uuid_dict: O,
    s_safeuuid: O,
    safeuuid: *mut ffi::PyTypeObject,
    safe_tag: u32,
    /// `SafeUUID.unknown`.
    pub unknown: O,
    /// The interned `'int'` (the key `__getstate__`'s dict display uses).
    pub s_int: O,
}

// SAFETY: immutable after construction; the objects are kept alive for the process.
unsafe impl Send for Uuid {}
// SAFETY: as above: shared reads of immutable fields only.
unsafe impl Sync for Uuid {}

/// The validated class (null until a UUID was met and validated; set once).
static INFO: AtomicPtr<Uuid> = AtomicPtr::new(std::ptr::null_mut());
/// Classes named `UUID` that failed validation (each checked once per process).
static REJECTED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

#[inline(always)]
unsafe fn version_tag(t: *mut ffi::PyTypeObject) -> u32 {
    // SAFETY: a live type; the tag is written atomically on free-threaded builds.
    unsafe { std::ptr::read_volatile(&raw const (*t).tp_version_tag) }
}

/// The validated `uuid.UUID` if `cls` is it and nothing it depends on changed.
///
/// # Safety
/// Attached thread; `cls` a live type.
#[inline(always)]
pub unsafe fn uuid_class(cls: *mut ffi::PyTypeObject) -> Option<&'static Uuid> {
    // SAFETY: `INFO` points to a leaked, immutable `Uuid`.
    unsafe {
        let p = INFO.load(Relaxed);
        let u = if p.is_null() {
            if !(*cls).tp_name.is_null() && std::ffi::CStr::from_ptr((*cls).tp_name).to_bytes() == b"UUID" {
                &*resolve(cls)?
            } else {
                return None;
            }
        } else {
            &*p
        };
        (u.cls == cls && u.intact()).then_some(u)
    }
}

impl Uuid {
    /// The class, its two methods and `uuid.SafeUUID` are what was validated.
    #[inline(always)]
    unsafe fn intact(&self) -> bool {
        // SAFETY: live objects kept by `self`.
        unsafe {
            let tag = version_tag(self.cls);
            if tag == 0 || tag != self.tag {
                return false;
            }
            if (*self.getstate.cast::<ffi::PyFunctionObject>()).func_code != self.getstate_code
                || (*self.setstate.cast::<ffi::PyFunctionObject>()).func_code != self.setstate_code
            {
                return false;
            }
            let st = version_tag(self.safeuuid);
            if st == 0 || st != self.safe_tag {
                return false;
            }
            // `SafeUUID` as `__getstate__`/`__setstate__` read it: the module global.
            let mut g: O = std::ptr::null_mut();
            let r = ffi::PyDict_GetItemRef(self.uuid_dict, self.s_safeuuid, &mut g);
            if r < 0 {
                ffi::PyErr_Clear();
                return false;
            }
            if !g.is_null() {
                ffi::Py_DECREF(g);
            }
            g == self.safeuuid.cast()
        }
    }

    /// `x.int` and `x.is_safe` (borrowed) when `x` is in the exact subset.
    ///
    /// # Safety
    /// `x` is a live instance of `self.cls` (exact).
    #[inline(always)]
    pub unsafe fn fields(&self, x: O) -> Option<O> {
        // SAFETY: slot offsets validated against the class's member descriptors.
        unsafe {
            let int = std::ptr::read_volatile(x.cast::<u8>().offset(self.off_int).cast::<O>());
            let safe = std::ptr::read_volatile(x.cast::<u8>().offset(self.off_safe).cast::<O>());
            (!int.is_null() && ty(int) == &raw mut ffi::PyLong_Type && safe == self.unknown).then_some(int)
        }
    }
}

/// Validate `cls` as `uuid.UUID` (C-level reads only: no import, no attribute hooks, no
/// Python code), once.
#[cold]
#[inline(never)]
unsafe fn resolve(cls: *mut ffi::PyTypeObject) -> Option<*const Uuid> {
    {
        let rej = REJECTED.lock().ok()?;
        if rej.contains(&(cls as usize)) {
            return None;
        }
    }
    // SAFETY: attached thread; live objects; failures leave no exception set.
    let r = unsafe { validate(cls) };
    // SAFETY: attached thread.
    if unsafe { !ffi::PyErr_Occurred().is_null() } {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
    }
    match r {
        Some(u) => {
            let b = Box::into_raw(Box::new(u));
            match INFO.compare_exchange(std::ptr::null_mut(), b, Relaxed, Relaxed) {
                Ok(_) => Some(b),
                Err(other) => {
                    // Another thread won; keep its (equal) record. Ours stays leaked with
                    // its references (once per process at most).
                    Some(other)
                }
            }
        }
        None => {
            if let Ok(mut rej) = REJECTED.lock()
                && rej.len() < 64
            {
                rej.push(cls as usize);
            }
            None
        }
    }
}

unsafe fn validate(cls: *mut ffi::PyTypeObject) -> Option<Uuid> {
    let st = st();
    // SAFETY: attached thread; C-level reads of live objects.
    unsafe {
        let py = Python::assume_attached();
        if ty(cls.cast()) != &raw mut ffi::PyType_Type || ffi::PyType_GetFlags(cls) & ffi::Py_TPFLAGS_HEAPTYPE == 0 {
            return None;
        }
        // `sys.modules['uuid'].UUID is cls`.
        let modules = ffi::PyImport_GetModuleDict();
        if modules.is_null() || ty(modules) != &raw mut ffi::PyDict_Type {
            return None;
        }
        let m = Own(dict_get(modules, intern(c"uuid")?).ok()?);
        if m.get().is_null() || ty(m.get()) != &raw mut ffi::PyModule_Type {
            return None;
        }
        let md = ffi::PyModule_GetDict(m.get());
        let a = Own(dict_get(md, intern(c"UUID")?).ok()?);
        if a.get() != cls.cast() {
            return None;
        }
        // MRO (UUID, object); the copy protocol is object's except the two methods.
        let mro = (*cls).tp_mro;
        if mro.is_null()
            || ffi::PyTuple_GET_SIZE(mro) != 2
            || ffi::PyTuple_GET_ITEM(mro, 1) != (&raw mut ffi::PyBaseObject_Type).cast()
        {
            return None;
        }
        if !generic_getattr(cls)
            || (*cls).tp_finalize.is_some()
            || (*cls).tp_del.is_some()
            || !_PyType_Lookup(cls, st.s_del).is_null()
            || _PyType_Lookup(cls, st.s_reduce_ex) != st.obj_reduce_ex
            || _PyType_Lookup(cls, st.s_reduce) != st.obj_reduce
            || _PyType_Lookup(cls, st.s_new) != st.object_new
            || !_PyType_Lookup(cls, st.s_getnewargs_ex).is_null()
            || !_PyType_Lookup(cls, st.s_getnewargs).is_null()
            || !_PyType_Lookup(cls, st.s_deepcopy).is_null()
            || (*cls).tp_dictoffset != 0
            || ffi::PyType_GetFlags(cls) & ffi::Py_TPFLAGS_MANAGED_DICT != 0
        {
            return None;
        }
        let getstate = _PyType_Lookup(cls, st.s_getstate);
        let setstate = _PyType_Lookup(cls, st.s_setstate);
        for (f, fp) in [(getstate, FP_GETSTATE), (setstate, FP_SETSTATE)] {
            if f.is_null() || ty(f) != &raw mut ffi::PyFunction_Type {
                return None;
            }
            let fb = Bound::from_borrowed_ptr(py, f);
            let got = crate::accel::fingerprint::function(&fb).ok()?;
            let want = fp.iter().find(|(ma, mi, _)| (*ma, *mi) == (3, st.minor))?.2;
            if got != want {
                return None;
            }
        }
        let off = |name: &std::ffi::CStr| -> Option<isize> {
            let d = _PyType_Lookup(cls, intern(name)?);
            if d.is_null() || ty(d) != &raw mut ffi::PyMemberDescr_Type {
                return None;
            }
            let def = (*d.cast::<ffi::PyMemberDescrObject>()).d_member;
            ((*def).type_code == ffi::Py_T_OBJECT_EX && (*def).flags & ffi::Py_READONLY == 0).then(|| (*def).offset)
        };
        let (off_int, off_safe) = (off(c"int")?, off(c"is_safe")?);
        // `SafeUUID.unknown`: a member stored in the enum class's dict, reached by plain
        // type attribute lookup (no metaclass hook, no metaclass descriptor of that name).
        let s_safeuuid = intern(c"SafeUUID")?;
        let su = Own(dict_get(md, s_safeuuid).ok()?);
        let su_t = su.get().cast::<ffi::PyTypeObject>();
        if su.get().is_null() || ffi::PyType_Check(su.get()) == 0 {
            return None;
        }
        let meta = ty(su.get());
        if addr((*meta).tp_getattro) != addr(ffi::PyType_Type.tp_getattro) {
            return None;
        }
        let s_unknown = intern(c"unknown")?;
        if !_PyType_Lookup(meta, s_unknown).is_null() {
            return None;
        }
        let unknown = _PyType_Lookup(su_t, s_unknown);
        if unknown.is_null() || ty(unknown) != su_t || (*su_t).tp_descr_get.is_some() {
            return None;
        }
        // `self.is_safe != SafeUUID.unknown` runs no Python code: object's comparison.
        if addr((*su_t).tp_richcompare) != addr(ffi::PyBaseObject_Type.tp_richcompare) {
            return None;
        }
        let tag = version_tag(cls);
        let safe_tag = version_tag(su_t);
        if tag == 0 || safe_tag == 0 {
            return None;
        }
        Some(Uuid {
            cls: incref(cls.cast()).cast(),
            tag,
            getstate: incref(getstate),
            getstate_code: incref((*getstate.cast::<ffi::PyFunctionObject>()).func_code),
            setstate: incref(setstate),
            setstate_code: incref((*setstate.cast::<ffi::PyFunctionObject>()).func_code),
            off_int,
            off_safe,
            uuid_dict: incref(md),
            s_safeuuid,
            safeuuid: incref(su.get()).cast(),
            safe_tag,
            unknown: incref(unknown),
            s_int: intern(c"int")?,
        })
    }
}

/// An interned string kept for the process.
unsafe fn intern(s: &std::ffi::CStr) -> Option<O> {
    // SAFETY: attached thread.
    let o = unsafe { ffi::PyUnicode_InternFromString(s.as_ptr()) };
    if o.is_null() {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return None;
    }
    Some(o)
}

/// `UUID.__getstate__(x)` for a UUID in the exact subset: `{'int': x.int}` (new reference).
///
/// # Safety
/// Attached thread; `int` the exact int read by [`Uuid::fields`].
pub unsafe fn getstate(u: &Uuid, int: O) -> R<O> {
    // SAFETY: attached thread.
    unsafe {
        let d = Own::new(ffi::PyDict_New())?;
        if ffi::PyDict_SetItem(d.get(), u.s_int, int) < 0 {
            return Err(());
        }
        Ok(d.into_raw())
    }
}

/// `UUID.__setstate__(y, state)` for a state `getstate` produced (copied): `int` set to
/// `state['int']`, `is_safe` to `SafeUUID.unknown`, as `object.__setattr__` sets the slots.
///
/// # Safety
/// Attached thread; `y` a fresh instance of `u.cls`, `state` a dict.
pub unsafe fn setstate(u: &Uuid, y: O, state: O) -> R<()> {
    // SAFETY: slot offsets validated; `y` is new (no other reference), its slots NULL.
    unsafe {
        if ty(state) != &raw mut ffi::PyDict_Type || ffi::PyDict_Size(state) != 1 {
            return Err(());
        }
        let mut int: O = std::ptr::null_mut();
        if ffi::PyDict_GetItemRef(state, u.s_int, &mut int) <= 0 {
            ffi::PyErr_Clear();
            return Err(());
        }
        let slot_int = y.cast::<u8>().offset(u.off_int).cast::<O>();
        let slot_safe = y.cast::<u8>().offset(u.off_safe).cast::<O>();
        let old_i = std::ptr::replace(slot_int, int);
        let old_s = std::ptr::replace(slot_safe, incref(u.unknown));
        xdecref(old_i);
        xdecref(old_s);
        Ok(())
    }
}
