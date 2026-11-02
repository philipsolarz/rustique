//! Machinery shared by the logging kernels: fallback, tool and type guards, attribute
//! helpers, purity checks and the interned names the kernels use.
//!
//! Every helper here is *pure* in the contract's sense: it runs no Python code and has no
//! observable effect, so a kernel may still restart in CPython's original after using it.

use std::ffi::c_int;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering::Relaxed};

use pyo3::prelude::*;

use pyo3::ffi;

pub type O = *mut ffi::PyObject;
pub type Tp = *mut ffi::PyTypeObject;

// ---------------------------------------------------------------------------------------
// CPython symbols pyo3-ffi does not bind
// ---------------------------------------------------------------------------------------

python_extern! {
    /// Exported (cpython/object.h, 3.14 and 3.15): MRO lookup through the type cache.
    pub fn _PyType_LookupRef(t: Tp, name: O) -> O;
    pub fn PyUnstable_Type_AssignVersionTag(t: Tp) -> c_int;
    pub fn PyTime_Time(result: *mut i64) -> c_int;
}

// ---------------------------------------------------------------------------------------
// Fallback
// ---------------------------------------------------------------------------------------

/// Run CPython's original (`_PyFunction_Vectorcall`) with the unchanged arguments.
///
/// # Safety
/// Vectorcall arguments of a function patched through the registry; attached thread.
#[inline(always)]
pub unsafe fn original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: set by the registry before any kernel is reachable.
    unsafe { crate::accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
}

#[inline(always)]
pub fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

// ---------------------------------------------------------------------------------------
// Tools guard (the framework's: every global event, and local events per code object)
// ---------------------------------------------------------------------------------------

pub static TOOLS: crate::accel::Tools = crate::accel::Tools::new();

/// Pins of the group's shared declarations (`SHARED`: module identities, `time.strftime`,
/// builtins); every kernel checks it next to its own.
pub static SHARED_DEPS: crate::accel::DepFlag = crate::accel::DepFlag::new();

/// A Python function a kernel stands in for (the patched one or a helper it inlines):
/// its code and defaults are still the ones validated at install (`__code__`/`__defaults__`
/// assignment changes behaviour), and no tool listens locally on the code. Holds strong
/// references for the process lifetime.
pub struct FnGuard {
    func: AtomicUsize,
    code: AtomicUsize,
    defaults: AtomicUsize,
}

impl FnGuard {
    pub const fn new() -> Self {
        FnGuard { func: AtomicUsize::new(0), code: AtomicUsize::new(0), defaults: AtomicUsize::new(0) }
    }

    /// Record `func` (an exact function). Earlier references are leaked on purpose (a
    /// concurrent kernel may still read them).
    pub fn set(&self, func: &Bound<'_, PyAny>) {
        let f = func.as_ptr().cast::<ffi::PyFunctionObject>();
        // SAFETY: an exact function object (checked by the caller); attached thread.
        unsafe {
            let code = (*f).func_code;
            let defaults = (*f).func_defaults;
            ffi::Py_INCREF(func.as_ptr());
            ffi::Py_INCREF(code);
            ffi::Py_XINCREF(defaults);
            self.code.store(code as usize, Relaxed);
            self.defaults.store(defaults as usize, Relaxed);
            self.func.store(func.as_ptr() as usize, Relaxed);
        }
    }

    pub fn func(&self) -> O {
        self.func.load(Relaxed) as O
    }

    /// # Safety
    /// `set` ran; attached thread; code layout calibrated.
    #[inline(always)]
    pub unsafe fn ok(&self) -> bool {
        // SAFETY: per the caller (the function is kept alive by `set`).
        unsafe {
            let f = self.func.load(Relaxed) as *mut ffi::PyFunctionObject;
            let code = self.code.load(Relaxed);
            (*f).func_code as usize == code
                && (*f).func_defaults as usize == self.defaults.load(Relaxed)
                && crate::accel::code_locally_quiet(code as O)
        }
    }
}

impl Default for FnGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Every function in `g` passes [`FnGuard::ok`].
///
/// # Safety
/// See [`FnGuard::ok`].
#[inline(always)]
pub unsafe fn fns_ok(g: &[FnGuard]) -> bool {
    // SAFETY: per the caller.
    g.iter().all(|f| unsafe { f.ok() })
}

// ---------------------------------------------------------------------------------------
// Validated types
// ---------------------------------------------------------------------------------------

/// Small cache of `(type, tp_version_tag)` pairs a validator accepted. A type's version tag
/// changes whenever the type or a base is modified (attribute assignment, `__bases__`), and
/// tags are never reused within an interpreter, so a matching pair proves the class-level
/// lookups the validator checked still hold. Torn reads on free-threaded builds can only
/// produce false negatives (each tag belongs to one type).
///
/// Each entry also remembers where the `N` attributes `names` live in the instances'
/// inline values (GIL builds; see [`TypeCache::read`]).
pub struct TypeCache<const N: usize> {
    ty: [AtomicUsize; 4],
    tag: [AtomicU32; 4],
    next: AtomicUsize,
    validate: unsafe fn(Tp) -> bool,
    names: [AtomicUsize; N],
    /// Per entry and name: index in the type's shared keys, or -1.
    ix: [[AtomicI32; N]; 4],
    /// Per entry and name: `dk_nentries` of the shared keys when an index was not found.
    seen: [[AtomicI32; N]; 4],
}

impl<const N: usize> TypeCache<N> {
    pub const fn new(validate: unsafe fn(Tp) -> bool) -> Self {
        TypeCache {
            ty: [const { AtomicUsize::new(0) }; 4],
            tag: [const { AtomicU32::new(0) }; 4],
            next: AtomicUsize::new(0),
            validate,
            names: [const { AtomicUsize::new(0) }; N],
            ix: [const { [const { AtomicI32::new(-1) }; N] }; 4],
            seen: [const { [const { AtomicI32::new(-1) }; N] }; 4],
        }
    }

    /// The attribute names [`Self::read`] serves (interned, kept alive by `Names`).
    pub fn set_names(&self, names: [usize; N]) {
        for (c, n) in self.names.iter().zip(names) {
            c.store(n, Relaxed);
        }
        self.clear();
    }

    /// # Safety
    /// `t` is a live type; attached thread.
    #[inline(always)]
    pub unsafe fn ok(&self, t: Tp) -> bool {
        // SAFETY: per the caller.
        unsafe { self.entry(t).is_some() }
    }

    /// The cache entry of a validated type.
    ///
    /// # Safety
    /// `t` is a live type; attached thread.
    #[inline(always)]
    pub unsafe fn entry(&self, t: Tp) -> Option<usize> {
        // SAFETY: field read of a live type (atomic on free-threaded builds).
        let tag = unsafe { version_tag(t) };
        if tag != 0 {
            for i in 0..4 {
                if self.tag[i].load(Relaxed) == tag && self.ty[i].load(Relaxed) == t as usize {
                    return Some(i);
                }
            }
        }
        // SAFETY: per the caller.
        unsafe { self.slow(t) }
    }

    #[cold]
    unsafe fn slow(&self, t: Tp) -> Option<usize> {
        // SAFETY: attached thread, live type.
        unsafe {
            let assigned = PyUnstable_Type_AssignVersionTag(t);
            let tag0 = version_tag(t);
            if !(self.validate)(t) {
                return None;
            }
            if assigned == 1 && tag0 != 0 && version_tag(t) == tag0 {
                let i = self.next.fetch_add(1, Relaxed) % 4;
                self.tag[i].store(0, Relaxed);
                self.ty[i].store(t as usize, Relaxed);
                for k in 0..N {
                    self.ix[i][k].store(-1, Relaxed);
                    self.seen[i][k].store(-1, Relaxed);
                }
                self.tag[i].store(tag0, Relaxed);
                return Some(i);
            }
            // Validated now but not cacheable: entry 4 = "read through getattr".
            Some(4)
        }
    }

    pub fn clear(&self) {
        for i in 0..4 {
            self.tag[i].store(0, Relaxed);
            self.ty[i].store(0, Relaxed);
        }
    }

    /// `obj.names[k]` (new reference) or NULL (absent: the caller falls back), for `obj` of
    /// the type of entry `e`. GIL builds with a calibrated layout read the instance's
    /// inline values directly, as `_PyObject_TryGetInstanceAttribute` does (index in the
    /// type's shared keys), and the class attribute when the instance has none (the
    /// validator made sure it is a plain value); everything else goes through
    /// `PyObject_GenericGetAttr`.
    ///
    /// # Safety
    /// Attached thread; `obj` of the entry's type; `e` from [`Self::entry`].
    #[inline(always)]
    pub unsafe fn read(&self, e: usize, obj: O, k: usize) -> O {
        // SAFETY: per the caller.
        unsafe {
            let name = self.names[k].load(Relaxed) as O;
            #[cfg(not(Py_GIL_DISABLED))]
            if e < 4
                && INLINE_OK.load(Relaxed)
                && let Some(v) = self.read_inline(e, obj, k, name)
            {
                return v;
            }
            let _ = e;
            get(obj, name)
        }
    }

    #[cfg(not(Py_GIL_DISABLED))]
    #[inline(always)]
    unsafe fn read_inline(&self, e: usize, obj: O, k: usize, name: O) -> Option<O> {
        // SAFETY: GIL held (no concurrent mutation); layout calibrated (`INLINE_OK`).
        unsafe {
            let t = ffi::Py_TYPE(obj);
            let vals = inline_values(obj, t)?;
            let mut ix = self.ix[e][k].load(Relaxed);
            if ix < 0 {
                let keys = cached_keys(t)?;
                let n = keys_nentries(keys) as i32;
                if self.seen[e][k].load(Relaxed) != n {
                    ix = key_index(keys, name);
                    self.ix[e][k].store(ix, Relaxed);
                    self.seen[e][k].store(n, Relaxed);
                }
            }
            if ix >= 0 {
                if ix as usize >= *vals as usize {
                    return None;
                }
                let v = *vals.add(8).cast::<O>().add(ix as usize);
                if !v.is_null() {
                    return Some(ffi::Py_NewRef(v));
                }
            }
            // Not an instance attribute: the class's plain value, if any.
            Some(_PyType_LookupRef(t, name))
        }
    }
}

// ---------------------------------------------------------------------------------------
// Inline values (GIL builds): pycore_dict.h / pycore_object.h layouts of 3.14 and 3.15,
// calibrated at install against `PyObject_GenericGetAttr` on `logging.root`.
// ---------------------------------------------------------------------------------------

#[cfg(not(Py_GIL_DISABLED))]
static INLINE_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(not(Py_GIL_DISABLED))]
const TPFLAGS_INLINE_VALUES: std::ffi::c_ulong = 1 << 2;

/// `_PyObject_InlineValues(obj)` when the type has inline values and they are valid.
#[cfg(not(Py_GIL_DISABLED))]
#[inline(always)]
unsafe fn inline_values(obj: O, t: Tp) -> Option<*const u8> {
    // SAFETY: live object and type; struct _dictvalues {capacity, size, embedded, valid, values[]}.
    unsafe {
        if (*t).tp_flags & TPFLAGS_INLINE_VALUES == 0 {
            return None;
        }
        let vals = obj.cast::<u8>().add((*t).tp_basicsize as usize).cast_const();
        (*vals.add(3) != 0).then_some(vals)
    }
}

/// `((PyHeapTypeObject *)t)->ht_cached_keys`.
#[cfg(not(Py_GIL_DISABLED))]
#[inline(always)]
unsafe fn cached_keys(t: Tp) -> Option<*const u8> {
    // SAFETY: a heap type (inline values imply it).
    unsafe {
        let k = (*t.cast::<ffi::PyHeapTypeObject>()).ht_cached_keys.cast::<u8>().cast_const();
        (!k.is_null()).then_some(k)
    }
}

/// `dk_nentries` (offset 24 on GIL builds: refcnt, log2_size, log2_index_bytes, kind,
/// version, usable).
#[cfg(not(Py_GIL_DISABLED))]
#[inline(always)]
unsafe fn keys_nentries(keys: *const u8) -> isize {
    // SAFETY: a live PyDictKeysObject.
    unsafe { *keys.add(24).cast::<isize>() }
}

/// Index of `name` (by identity: attribute names are interned) among the shared keys'
/// unicode entries (`dk_indices` at 32, then `1 << dk_log2_index_bytes` index bytes, then
/// `{me_key, me_value}` entries), or -1.
#[cfg(not(Py_GIL_DISABLED))]
unsafe fn key_index(keys: *const u8, name: O) -> i32 {
    // SAFETY: a live split PyDictKeysObject.
    unsafe {
        let n = keys_nentries(keys);
        if !(0..=1024).contains(&n) {
            return -1;
        }
        let entries = keys.add(32 + (1usize << *keys.add(9))).cast::<[O; 2]>();
        (0..n as usize).find(|&i| (*entries.add(i))[0] == name).map_or(-1, |i| i as i32)
    }
}

/// Check the inline-values layout on `obj` (an instance whose class has the generic
/// attribute protocol and no descriptors for `names`): every name read through the inline
/// values must give exactly what `PyObject_GenericGetAttr` gives.
pub fn calibrate_inline(py: Python<'_>, obj: &Bound<'_, PyAny>, names: &[&str]) -> bool {
    #[cfg(Py_GIL_DISABLED)]
    {
        let _ = (py, obj, names);
        false
    }
    #[cfg(not(Py_GIL_DISABLED))]
    // SAFETY: attached thread; reads of a live object's layout, cross-checked.
    unsafe {
        let o = obj.as_ptr();
        let t = ffi::Py_TYPE(o);
        let ok = (|| {
            let vals = inline_values(o, t)?;
            let keys = cached_keys(t)?;
            let mut found = 0;
            for name in names {
                let nm = pyo3::types::PyString::intern(py, name);
                let ix = key_index(keys, nm.as_ptr());
                let r = ffi::PyObject_GenericGetAttr(o, nm.as_ptr());
                if r.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                ffi::Py_DECREF(r);
                if ix < 0 || ix as usize >= *vals as usize || *vals.add(8).cast::<O>().add(ix as usize) != r {
                    return None;
                }
                found += 1;
            }
            (found == names.len() && found >= 4).then_some(())
        })()
        .is_some();
        INLINE_OK.store(ok, Relaxed);
        ok
    }
}

/// # Safety
/// Live type.
#[inline(always)]
pub unsafe fn version_tag(t: Tp) -> u32 {
    // SAFETY: aligned u32 field; CPython writes it atomically on free-threaded builds.
    unsafe { (*(&raw const (*t).tp_version_tag).cast::<AtomicU32>()).load(Relaxed) }
}

/// The class attribute `name` resolves (MRO) to exactly `expected`.
///
/// # Safety
/// Attached thread, live type.
pub unsafe fn resolves_to(t: Tp, name: O, expected: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let r = _PyType_LookupRef(t, name);
        if r.is_null() {
            return false;
        }
        ffi::Py_DECREF(r);
        r == expected
    }
}

/// The class attribute `name` is absent or a plain value (no `__get__`/`__set__`), so
/// `obj.name` reads the instance attribute or that value without running code.
///
/// # Safety
/// Attached thread, live type.
pub unsafe fn plain_attr(t: Tp, name: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let r = _PyType_LookupRef(t, name);
        if r.is_null() {
            return true;
        }
        let rt = ffi::Py_TYPE(r);
        let ok = (*rt).tp_descr_get.is_none() && (*rt).tp_descr_set.is_none();
        ffi::Py_DECREF(r);
        ok
    }
}

/// Generic attribute protocol (no `__getattribute__`/`__getattr__`/`__setattr__`
/// overrides) and an instance `__dict__`.
///
/// # Safety
/// Live type.
pub unsafe fn generic_attrs(t: Tp, setattr_too: bool) -> bool {
    // SAFETY: field reads of a live type.
    unsafe {
        let ga = (*t).tp_getattro.map(|f| f as usize);
        let sa = (*t).tp_setattro.map(|f| f as usize);
        let has_dict = (*t).tp_dictoffset != 0 || ffi::PyType_GetFlags(t) & ffi::Py_TPFLAGS_MANAGED_DICT != 0;
        has_dict
            && ga == Some(ffi::PyObject_GenericGetAttr as *const () as usize)
            && (!setattr_too || sa == Some(ffi::PyObject_GenericSetAttr as *const () as usize))
    }
}

// ---------------------------------------------------------------------------------------
// Attribute helpers
// ---------------------------------------------------------------------------------------

/// `obj.name` through the generic attribute protocol, or NULL with no error set. Only for
/// objects of validated types (generic `tp_getattro`, no descriptor for `name`), where
/// this is exactly what `obj.name` does and runs no Python code.
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub unsafe fn get(obj: O, name: O) -> O {
    // SAFETY: per the caller.
    unsafe {
        let r = ffi::PyObject_GenericGetAttr(obj, name);
        if r.is_null() {
            ffi::PyErr_Clear();
        }
        r
    }
}

/// `_PyObject_GetMethod` (exported by 3.14 and 3.15): the method lookup `obj.name(...)`
/// call sites use, without creating a bound method.
pub static GET_METHOD: crate::accel::Original<unsafe extern "C" fn(O, O, *mut O) -> c_int> =
    crate::accel::Original::new();

/// `obj.name` resolves to the class's function `func` (the instance does not shadow it):
/// what `self.name(...)` calls is `func`.
///
/// # Safety
/// Attached thread; `obj` of a validated type; `GET_METHOD` resolved.
#[inline(always)]
pub unsafe fn bound_to(obj: O, name: O, func: O) -> bool {
    // SAFETY: per the caller.
    unsafe {
        let mut m: O = std::ptr::null_mut();
        let unbound = GET_METHOD.get()(obj, name, &mut m);
        if m.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let ok = unbound == 1 && m == func;
        ffi::Py_DECREF(m);
        ok
    }
}

/// Values whose `str`, `repr`, `%`-conversions and `__format__` are CPython C code with no
/// hooks: exact `str`, `int`, `bool`, `float`, `None`.
///
/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn pure_scalar(o: O) -> bool {
    // SAFETY: header reads.
    unsafe {
        let t = ffi::Py_TYPE(o);
        t == &raw mut ffi::PyUnicode_Type
            || t == &raw mut ffi::PyLong_Type
            || t == &raw mut ffi::PyFloat_Type
            || o == ffi::Py_None()
            || t == &raw mut ffi::PyBool_Type
    }
}

/// # Safety
/// A live object.
#[inline(always)]
pub unsafe fn is_type(o: O, t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: header read of a live object.
    unsafe { ffi::Py_TYPE(o) == t }
}

/// # Safety
/// A live object.
#[inline(always)]
pub unsafe fn exact_str(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyUnicode_Type) }
}

/// # Safety
/// A live object.
#[inline(always)]
pub unsafe fn exact_int(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyLong_Type) }
}

/// # Safety
/// A live object.
#[inline(always)]
pub unsafe fn exact_dict(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyDict_Type) }
}

/// # Safety
/// A live object.
#[inline(always)]
pub unsafe fn exact_tuple(o: O) -> bool {
    // SAFETY: header read.
    unsafe { is_type(o, &raw mut ffi::PyTuple_Type) }
}

/// Truthiness of an object whose `bool()` runs no Python code: `Some(b)` for exact
/// `bool`/`None`/`int`/`str`/`tuple`/`dict`, `None` otherwise.
///
/// # Safety
/// Live object.
#[inline(always)]
pub unsafe fn pure_truth(o: O) -> Option<bool> {
    // SAFETY: header reads; the size reads are lock-free in CPython too.
    unsafe {
        if o == ffi::Py_None() || o == ffi::Py_False() {
            Some(false)
        } else if o == ffi::Py_True() {
            Some(true)
        } else if exact_str(o) {
            Some(ffi::PyUnicode_GET_LENGTH(o) != 0)
        } else if exact_tuple(o) {
            Some(ffi::PyTuple_GET_SIZE(o) != 0)
        } else if exact_dict(o) {
            Some(ffi::PyDict_Size(o) != 0)
        } else if exact_int(o) {
            let r = ffi::PyObject_IsTrue(o);
            Some(r == 1)
        } else {
            None
        }
    }
}

/// Visit every `(key, value)` of an exact dict under its critical section
/// (free-threaded builds), stopping when `f` returns false. Returns false if stopped.
///
/// # Safety
/// Attached thread; `d` an exact dict; `f` must not run Python code.
pub unsafe fn dict_all(d: O, mut f: impl FnMut(O, O) -> bool) -> bool {
    // SAFETY: per the caller; borrowed key/value references are used under the section.
    unsafe {
        #[cfg(Py_GIL_DISABLED)]
        let mut cs: ffi::PyCriticalSection = std::mem::zeroed();
        #[cfg(Py_GIL_DISABLED)]
        ffi::PyCriticalSection_Begin(&mut cs, d);
        let mut pos: ffi::Py_ssize_t = 0;
        let mut k: O = std::ptr::null_mut();
        let mut v: O = std::ptr::null_mut();
        let mut all = true;
        while ffi::PyDict_Next(d, &mut pos, &mut k, &mut v) != 0 {
            if !f(k, v) {
                all = false;
                break;
            }
        }
        #[cfg(Py_GIL_DISABLED)]
        ffi::PyCriticalSection_End(&mut cs);
        all
    }
}

/// `d.get(key)` for an exact int `key` on an exact dict whose keys are all exact ints:
/// `Ok(Some(v))` (new reference) / `Ok(None)` (absent), `Err(())` if some key is not an
/// exact int (the lookup could run a user `__eq__`) or the dict is large.
///
/// # Safety
/// Attached thread; `d` exact dict; `key` exact int.
pub(super) unsafe fn int_keyed_get(d: O, key: O) -> Result<Option<O>, ()> {
    // SAFETY: per the caller.
    unsafe {
        if ffi::PyDict_Size(d) > 32 {
            return Err(());
        }
        // The key's value, computed only if some stored key is a different int object.
        let mut kv: Option<i64> = None;
        let mut found: O = std::ptr::null_mut();
        let mut bad = false;
        dict_all(d, |k, v| {
            if !exact_int(k) {
                bad = true;
                return false;
            }
            if found.is_null() {
                let eq = k == key || {
                    let want = *kv.get_or_insert_with(|| {
                        let mut ov: c_int = 0;
                        let x = ffi::PyLong_AsLongLongAndOverflow(key, &mut ov);
                        if ov != 0 { i64::MIN } else { x }
                    });
                    let mut o2: c_int = 0;
                    let x = ffi::PyLong_AsLongLongAndOverflow(k, &mut o2);
                    if o2 != 0 || want == i64::MIN {
                        // big ints: compare exactly (int == int runs no Python code)
                        ffi::PyObject_RichCompareBool(k, key, ffi::Py_EQ) == 1
                    } else {
                        x == want
                    }
                };
                if eq {
                    found = ffi::Py_NewRef(v);
                }
            }
            true
        });
        if bad {
            if !found.is_null() {
                ffi::Py_DECREF(found);
            }
            return Err(());
        }
        Ok((!found.is_null()).then_some(found))
    }
}

/// `PyDictObject` (cpython/dictobject.h).
#[repr(C)]
struct RawDict {
    ob_base: ffi::PyObject,
    ma_used: ffi::Py_ssize_t,
    _ma_watcher_tag: u64,
    ma_keys: *const u8,
}

/// `dk_kind` of `PyDictKeysObject` (pycore_dict.h, 3.14 and 3.15: after `dk_refcnt`,
/// `dk_log2_size`, `dk_log2_index_bytes`): `DICT_KEYS_GENERAL` 0, `UNICODE` 1, `SPLIT` 2.
/// A non-general dict holds only exact `str` keys (inserting anything else converts it).
const DK_KIND_OFFSET: usize = size_of::<ffi::Py_ssize_t>() + 2;
static DK_KIND_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// # Safety
/// Exact dict.
#[inline(always)]
unsafe fn dk_kind(d: O) -> u8 {
    // SAFETY: per the caller; the keys object outlives this read (QSBR on free-threaded
    // builds, the GIL otherwise).
    unsafe { *(*d.cast::<RawDict>()).ma_keys.add(DK_KIND_OFFSET) }
}

/// Verify the `dk_kind` layout on probe dicts (install time).
pub fn calibrate_dict_kind(py: Python<'_>) -> bool {
    let probe = || -> PyResult<bool> {
        let s = pyo3::types::PyDict::new(py);
        s.set_item("a", 1)?;
        s.set_item("b", 2)?;
        let g = pyo3::types::PyDict::new(py);
        g.set_item(1, 1)?;
        let mixed = pyo3::types::PyDict::new(py);
        mixed.set_item("a", 1)?;
        mixed.set_item(2, 1)?;
        // SAFETY: exact dicts.
        Ok(unsafe { dk_kind(s.as_ptr()) == 1 && dk_kind(g.as_ptr()) == 0 && dk_kind(mixed.as_ptr()) == 0 })
    };
    let ok = probe().unwrap_or(false);
    DK_KIND_OK.store(ok, Relaxed);
    ok
}

/// Every key of the exact dict `d` is an exact `str` (so lookups with `str` keys run no
/// Python code): one byte read when the layout is calibrated, else a bounded scan.
///
/// # Safety
/// Attached thread; exact dict.
#[inline(always)]
pub unsafe fn str_keyed(d: O, max: isize) -> bool {
    // SAFETY: per the caller.
    unsafe {
        if DK_KIND_OK.load(Relaxed) {
            return dk_kind(d) != 0;
        }
        ffi::PyDict_Size(d) <= max && dict_all(d, |k, _| exact_str(k))
    }
}

// ---------------------------------------------------------------------------------------
// Interned names
// ---------------------------------------------------------------------------------------

macro_rules! names {
    ($($field:ident = $s:literal),* $(,)?) => {
        /// Interned attribute names and constants (main interpreter; held forever).
        pub struct Names { $(pub $field: usize,)* }
        // SAFETY: plain addresses of objects kept alive for the process lifetime.
        unsafe impl Sync for Names {}
        impl Names {
            pub fn new() -> Self {
                // SAFETY: attached thread (called from install); NUL-terminated literals.
                unsafe {
                    Names { $($field: ffi::PyUnicode_InternFromString(concat!($s, "\0").as_ptr().cast()) as usize,)* }
                }
            }
        }
    };
}

names! {
    is_enabled_for = "isEnabledFor", disabled = "disabled", cache = "_cache", msg = "msg", self_ = "self",
    level = "level", name = "name", args = "args", levelname = "levelname", levelno = "levelno",
    pathname = "pathname", filename = "filename", module = "module", exc_info = "exc_info",
    exc_text = "exc_text", stack_info = "stack_info", lineno = "lineno", func_name = "funcName",
    created = "created", msecs = "msecs", relative_created = "relativeCreated", thread = "thread",
    thread_name = "threadName", process_name = "processName", process = "process", task_name = "taskName",
    message = "message", asctime = "asctime", style = "_style", datefmt = "datefmt", converter = "converter",
    default_time_format = "default_time_format", default_msec_format = "default_msec_format",
    uses_time = "usesTime", format_time = "formatTime", format_message = "formatMessage",
    format_exception = "formatException", format_stack = "formatStack", format = "format", fmt = "_fmt",
    defaults = "_defaults", asctime_search = "asctime_search", format_ = "_format", get_message = "getMessage",
    t_name = "_name", t_initialized = "_initialized", current_process = "current_process",
    current_task = "current_task", get_name = "get_name", multiprocessing = "multiprocessing",
    asyncio = "asyncio", main_process = "MainProcess", level_fmt = "Level %s", modules = "modules",
    dunder_init = "__init__", find_caller = "findCaller", msec_default = "%s,%03d",
    get_running_loop = "_get_running_loop", current_process_g = "_current_process",
}

impl Default for Names {
    fn default() -> Self {
        Self::new()
    }
}

static NAMES: OnceLock<Names> = OnceLock::new();

pub fn init_names() {
    NAMES.get_or_init(Names::new);
}

/// # Safety
/// `init_names` ran (every kernel is installed after it).
#[inline(always)]
pub unsafe fn n() -> &'static Names {
    // SAFETY: per the caller.
    unsafe { NAMES.get().unwrap_unchecked() }
}

/// The names (after `init_names`), for install-time code.
pub fn names() -> &'static Names {
    NAMES.get_or_init(Names::new)
}

/// A name as an object pointer.
#[inline(always)]
pub fn o(addr: usize) -> O {
    addr as O
}

/// An object pointer captured at install (strong reference held by the target).
#[derive(Debug)]
pub struct Cap(AtomicUsize);

impl Cap {
    pub const fn new() -> Self {
        Cap(AtomicUsize::new(0))
    }
    #[inline(always)]
    pub fn get(&self) -> O {
        self.0.load(Relaxed) as O
    }
    /// Store a strong reference (the previous one, if any, is leaked on purpose: a kernel
    /// running concurrently may still be using it).
    ///
    /// # Safety
    /// Attached thread; `obj` is a live object or NULL.
    pub unsafe fn set(&self, obj: O) {
        // SAFETY: per the caller.
        unsafe { ffi::Py_XINCREF(obj) };
        self.0.store(obj as usize, Relaxed);
    }

    /// [`Self::set`] from a bound object.
    pub fn set_obj(&self, obj: &Bound<'_, PyAny>) {
        // SAFETY: a live object; attached thread (the `Bound` proves it).
        unsafe { self.set(obj.as_ptr()) }
    }

    /// Forget the object (NULL).
    pub fn clear_ptr(&self) {
        self.0.store(0, Relaxed);
    }
}

impl Default for Cap {
    fn default() -> Self {
        Self::new()
    }
}
