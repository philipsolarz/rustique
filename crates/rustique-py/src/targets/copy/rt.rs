//! Runtime support shared by the copy-group kernels: the objects captured at install, raw
//! C-API helpers, and the emulation of CPython's Python-frame recursion counter.
//!
//! Every helper here performs *the same C-API operation* CPython's bytecode would perform
//! for the corresponding line of `Lib/copy.py` (`getattr(x, name, None)` is
//! `PyObject_GetOptionalAttr`, `y[k] = v` is `PyObject_SetItem`, ...), so that user code
//! reached through them runs with the same arguments in the same order.

use std::ffi::c_int;
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;

pub type O = *mut ffi::PyObject;
/// `Err(())`: a Python exception is set.
pub type R<T> = Result<T, ()>;

python_extern! {
    /// Borrowed MRO lookup through the type cache (cpython/object.h, exported).
    pub fn _PyType_Lookup(t: *mut ffi::PyTypeObject, name: O) -> O;
    /// Strong-reference variant (safe on free-threaded builds).
    pub fn _PyType_LookupRef(t: *mut ffi::PyTypeObject, name: O) -> O;
    /// `types.MethodType` (cpython/classobject.h).
    pub static mut PyMethod_Type: ffi::PyTypeObject;
}

/// `PyMethodObject` (public layout, cpython/classobject.h).
#[repr(C)]
pub struct PyMethodObject {
    pub ob_base: ffi::PyObject,
    pub im_func: O,
    pub im_self: O,
}

#[inline(always)]
pub unsafe fn method_func(m: O) -> O {
    // SAFETY: an exact bound method.
    unsafe { (*m.cast::<PyMethodObject>()).im_func }
}

#[inline(always)]
pub unsafe fn method_self(m: O) -> O {
    // SAFETY: an exact bound method.
    unsafe { (*m.cast::<PyMethodObject>()).im_self }
}

/// `Py_TYPE(o)->tp_hash` as an address (0 when unset).
#[inline(always)]
pub unsafe fn meta_hash(o: O) -> usize {
    // SAFETY: live object.
    unsafe { (*ty(o)).tp_hash.map_or(0, |f| f as usize) }
}

#[inline(always)]
pub fn nn(p: O) -> R<O> {
    if p.is_null() { Err(()) } else { Ok(p) }
}

#[inline(always)]
pub unsafe fn incref(o: O) -> O {
    // SAFETY: live object.
    unsafe { ffi::Py_INCREF(o) };
    o
}

#[inline(always)]
pub unsafe fn decref(o: O) {
    // SAFETY: owned reference.
    unsafe { ffi::Py_DECREF(o) }
}

#[inline(always)]
pub unsafe fn xdecref(o: O) {
    if !o.is_null() {
        // SAFETY: owned reference.
        unsafe { ffi::Py_DECREF(o) }
    }
}

#[inline(always)]
pub unsafe fn ty(o: O) -> *mut ffi::PyTypeObject {
    // SAFETY: live object.
    unsafe { ffi::Py_TYPE(o) }
}

/// An owned reference released on drop (keeps error paths leak-free).
pub struct Own(pub O);

impl Own {
    #[inline(always)]
    pub fn new(p: O) -> R<Own> {
        nn(p).map(Own)
    }
    #[inline(always)]
    pub fn into_raw(self) -> O {
        let p = self.0;
        std::mem::forget(self);
        p
    }
    #[inline(always)]
    pub fn get(&self) -> O {
        self.0
    }
}

impl Drop for Own {
    #[inline(always)]
    fn drop(&mut self) {
        // SAFETY: owned (possibly null) reference.
        unsafe { xdecref(self.0) }
    }
}

// ---------------------------------------------------------------------------------------
// Captured state
// ---------------------------------------------------------------------------------------

/// Objects the kernels rely on, captured (strong references) when the group installs.
/// Lives for the whole process once installed (kernels may still run after `disable()`).
pub struct St {
    pub minor: u8,
    // copy module
    pub copy_dict: O,
    pub deepcopy_fn: O,
    pub deepcopy_code: O,
    pub deepcopy_defaults: O,
    pub copy_code: O,
    pub atomic: O,
    /// 3.14: `_atomic_types` is a mutable set; its table at install.
    pub(crate) atomic_snap: Option<super::SetSnap>,
    pub(crate) copy_atomic_snap: Option<super::SetSnap>,
    pub(crate) copy_containers_snap: Option<super::SetSnap>,
    pub copy_atomic: O,
    pub copy_atomic_types: Vec<*mut ffi::PyTypeObject>,
    pub copy_container_types: Vec<*mut ffi::PyTypeObject>,
    pub copy_containers: O,
    pub dispatch: O,
    pub dispatch_table: O,
    pub h_list: O,
    pub h_tuple: O,
    pub h_dict: O,
    pub h_frozendict: O,
    pub h_method: O,
    /// (function, code, defaults) for each helper the kernel emulates; checked before use.
    pub helpers: [(O, O, O); 9],
    // copyreg
    pub newobj: O,
    /// `object.__new__`, `object.__reduce_ex__`, `object.__reduce__`, `object.__getstate__`.
    pub object_new: O,
    pub obj_reduce_ex: O,
    pub obj_reduce: O,
    pub obj_getstate: O,
    pub get_method: super::reduce::GetMethodFn,
    pub get_method_addr: usize,
    pub s_getstate: O,
    pub s_getnewargs: O,
    pub s_getnewargs_ex: O,
    pub s_slotnames: O,
    pub s_slots: O,
    pub s_getinitargs: O,
    pub s_del: O,
    // types
    pub atomic_types: [*mut ffi::PyTypeObject; 16],
    pub method_type: *mut ffi::PyTypeObject,
    pub frozendict_type: *mut ffi::PyTypeObject,
    // interned names
    pub s_deepcopy: O,
    pub s_copy: O,
    pub s_copy_method: O,
    pub s_memo: O,
    pub s_reduce_ex: O,
    pub s_reduce: O,
    pub s_setstate: O,
    pub s_dict: O,
    pub s_new: O,
    // tstate layout
}

// SAFETY: immutable after construction; the objects it points to are kept alive.
unsafe impl Send for St {}
// SAFETY: as Send: read-only after construction.
unsafe impl Sync for St {}

pub static ST: AtomicPtr<St> = AtomicPtr::new(std::ptr::null_mut());

#[inline(always)]
pub fn st() -> &'static St {
    // SAFETY: set before any kernel is installed, never freed.
    unsafe { &*ST.load(Relaxed) }
}

// Helper indices in `St::helpers`.
pub const H_LIST: usize = 0;
pub const H_TUPLE: usize = 1;
pub const H_DICT: usize = 2;
pub const H_FROZENDICT: usize = 3;
pub const H_METHOD: usize = 4;
pub const H_KEEP_ALIVE: usize = 5;
pub const H_RECONSTRUCT: usize = 6;
pub const H_NEWOBJ: usize = 7;
pub const H_SLOTNAMES: usize = 8;

/// Is helper `i` still the function the kernel emulates (same code and defaults, no tool
/// listening locally on its code)?
#[inline(always)]
pub unsafe fn helper_intact(st: &St, i: usize) -> bool {
    let (f, code, defaults) = st.helpers[i];
    if f.is_null() {
        return false;
    }
    // SAFETY: live function object (strong ref in St).
    unsafe {
        let fo = f.cast::<ffi::PyFunctionObject>();
        (*fo).func_code == code && (*fo).func_defaults == defaults && crate::accel::guard::code_locally_quiet(code)
    }
}

/// `str`, `int`, `NoneType`, `float`, `bool`: the atomic types copied inline by the
/// container loops (when the atomic set is known intact).
#[inline(always)]
pub fn is_quick(t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: static type objects and an immortal singleton.
    unsafe {
        t == &raw mut ffi::PyUnicode_Type
            || t == &raw mut ffi::PyLong_Type
            || t == ty(ffi::Py_None())
            || t == &raw mut ffi::PyFloat_Type
            || t == &raw mut ffi::PyBool_Type
    }
}

/// The type's attribute access is `object`'s generic one (no `__getattr__`/`__getattribute__`).
#[inline(always)]
pub unsafe fn generic_getattr(t: *mut ffi::PyTypeObject) -> bool {
    // SAFETY: live type objects.
    unsafe { (*t).tp_getattro.map(|f| f as usize) == ffi::PyBaseObject_Type.tp_getattro.map(|f| f as usize) }
}

/// Hashing `o` and comparing it with other pure keys runs no Python code: builtin scalars,
/// tuples/frozensets of pure keys, types with a plain metatype, and instances whose
/// `__hash__` and `__eq__` are `object`'s.
pub unsafe fn pure_key(o: O) -> bool {
    // SAFETY: live object.
    unsafe {
        let t = ty(o);
        if is_quick(t)
            || t == &raw mut ffi::PyBytes_Type
            || t == &raw mut ffi::PyComplex_Type
            || t == &raw mut ffi::PyEllipsis_Type
        {
            return true;
        }
        if t == &raw mut ffi::PyTuple_Type {
            return (0..ffi::PyTuple_GET_SIZE(o)).all(|i| pure_key(ffi::PyTuple_GET_ITEM(o, i)));
        }
        if t == &raw mut ffi::PyFrozenSet_Type {
            let it = ffi::PyObject_GetIter(o);
            if it.is_null() {
                ffi::PyErr_Clear();
                return false;
            }
            let it = Own(it);
            loop {
                let v = ffi::PyIter_Next(it.get());
                if v.is_null() {
                    let ok = ffi::PyErr_Occurred().is_null();
                    ffi::PyErr_Clear();
                    return ok;
                }
                let ok = pure_key(v);
                decref(v);
                if !ok {
                    return false;
                }
            }
        }
        let obj = &raw mut ffi::PyBaseObject_Type;
        let same = |a: Option<ffi::hashfunc>, b: Option<ffi::hashfunc>| a.map(|f| f as usize) == b.map(|f| f as usize);
        let same_rc =
            |a: Option<ffi::richcmpfunc>, b: Option<ffi::richcmpfunc>| a.map(|f| f as usize) == b.map(|f| f as usize);
        if ffi::PyType_Check(o) != 0 {
            let m = t; // the metatype
            return same((*m).tp_hash, (*obj).tp_hash) && same_rc((*m).tp_richcompare, (*obj).tp_richcompare);
        }
        same((*t).tp_hash, (*obj).tp_hash) && same_rc((*t).tp_richcompare, (*obj).tp_richcompare)
    }
}

/// `type.__hash__` (what a metatype without its own `__hash__` uses), as an address.
#[inline(always)]
pub fn default_type_hash() -> usize {
    // SAFETY: static type object read.
    unsafe { ffi::PyType_Type.tp_hash.map_or(0, |f| f as usize) }
}

// ---------------------------------------------------------------------------------------
// Python-frame recursion counter (`tstate->py_recursion_remaining`)
// ---------------------------------------------------------------------------------------

/// `tstate->py_recursion_remaining` of the calling thread: how many more Python frames
/// CPython would enter before raising `RecursionError`.
#[inline(always)]
pub unsafe fn recursion_remaining() -> i32 {
    // SAFETY: attached thread; offset calibrated at install.
    unsafe { *ffi::PyThreadState_Get().cast::<u8>().add(REM_OFF.load(Relaxed)).cast::<c_int>() }
}

/// Offset of `py_recursion_remaining` in `PyThreadState` (calibrated at install).
pub static REM_OFF: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ---------------------------------------------------------------------------------------
// Generic operations (exact equivalents of the bytecode in Lib/copy.py)
// ---------------------------------------------------------------------------------------

/// `getattr(o, name, None)`: new reference or null (no error) / Err.
#[inline(always)]
pub unsafe fn getattr_opt(o: O, name: O) -> R<O> {
    let mut out: O = std::ptr::null_mut();
    // SAFETY: live objects.
    if unsafe { ffi::PyObject_GetOptionalAttr(o, name, &mut out) } < 0 { Err(()) } else { Ok(out) }
}

/// `d.get(k)` on an exact dict (new reference or null).
#[inline(always)]
pub unsafe fn dict_get(d: O, k: O) -> R<O> {
    let mut out: O = std::ptr::null_mut();
    // SAFETY: live dict and key.
    if unsafe { ffi::PyDict_GetItemRef(d, k, &mut out) } < 0 { Err(()) } else { Ok(out) }
}

#[inline(always)]
pub unsafe fn call1(f: O, a: O) -> R<O> {
    // SAFETY: live objects.
    nn(unsafe { ffi::PyObject_CallOneArg(f, a) })
}

#[inline(always)]
pub unsafe fn call2(f: O, a: O, b: O) -> R<O> {
    let args = [std::ptr::null_mut(), a, b];
    // SAFETY: live objects; slot 0 reserved for PY_VECTORCALL_ARGUMENTS_OFFSET.
    nn(unsafe {
        ffi::PyObject_Vectorcall(f, args.as_ptr().add(1), 2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET, std::ptr::null_mut())
    })
}

static REF1: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static REF2: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static REF_FLAGS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// `(co_code, co_flags)` of `def f(self[, memo]): return self` on this interpreter, compiled
/// on first use (compiling only; kept for the process).
unsafe fn ret_self_ref(n: c_int) -> Option<(O, c_int)> {
    let cell = if n == 2 { &REF2 } else { &REF1 };
    let have = cell.load(Relaxed);
    if !have.is_null() {
        return Some((have, REF_FLAGS.load(Relaxed)));
    }
    let src = if n == 2 { c"def f(self, memo):\n    return self\n" } else { c"def f(self):\n    return self\n" };
    // SAFETY: attached thread; the compiler runs no Python code.
    unsafe {
        let m = ffi::Py_CompileString(src.as_ptr(), c"<rustique>".as_ptr(), ffi::Py_file_input);
        if m.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let m = Own(m);
        let consts = Own(ffi::PyObject_GetAttrString(m.get(), c"co_consts".as_ptr()));
        if consts.get().is_null() || ty(consts.get()) != &raw mut ffi::PyTuple_Type {
            ffi::PyErr_Clear();
            return None;
        }
        for i in 0..ffi::PyTuple_GET_SIZE(consts.get()) {
            let c = ffi::PyTuple_GET_ITEM(consts.get(), i);
            if ty(c) == &raw mut ffi::PyCode_Type {
                let cc = ffi::PyObject_GetAttrString(c, c"co_code".as_ptr());
                if cc.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                REF_FLAGS.store((*c.cast::<crate::accel::version::RawCode>()).co_flags, Relaxed);
                REF_UNITS[(n == 2) as usize].store(ffi::Py_SIZE(c), Relaxed);
                if cell.compare_exchange(std::ptr::null_mut(), cc, Relaxed, Relaxed).is_err() {
                    decref(cc);
                }
                return Some((cell.load(Relaxed), REF_FLAGS.load(Relaxed)));
            }
        }
        None
    }
}

/// `Py_SIZE` (code units) of the reference code objects (1 and 2 parameters).
static REF_UNITS: [std::sync::atomic::AtomicIsize; 2] = [const { std::sync::atomic::AtomicIsize::new(-1) }; 2];

static RET_SELF1: [AtomicPtr<ffi::PyObject>; 8] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 8];
static RET_SELF2: [AtomicPtr<ffi::PyObject>; 8] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 8];

/// Code flags without effect on execution: `CO_HAS_DOCSTRING` and `CO_METHOD` (3.14+; the
/// latter is set on functions defined in a class body).
const CO_INERT: c_int = 0x400_0000 | 0x800_0000;

/// Is `f` a Python function whose code is exactly `return self` with `n` positional
/// parameters (`Enum.__deepcopy__`, `Enum.__copy__`, `Pattern`-like no-op hooks written
/// in Python)? Calling it has no effect besides its frame.
#[inline]
pub unsafe fn is_return_self(f: O, n: c_int) -> bool {
    // SAFETY: live object; layouts of function/code objects (public cpython headers).
    unsafe {
        if ty(f) != &raw mut ffi::PyFunction_Type {
            return false;
        }
        let code = (*f.cast::<ffi::PyFunctionObject>()).func_code;
        let rc = code.cast::<crate::accel::version::RawCode>();
        if !crate::accel::guard::code_locally_quiet(code) {
            return false;
        }
        // Code objects are immutable: remember the ones already verified.
        let cache = if n == 2 { &RET_SELF2 } else { &RET_SELF1 };
        if cache.iter().any(|c| c.load(Relaxed) == code) {
            return true;
        }
        let Some((want, flags)) = ret_self_ref(n) else { return false };
        // Code units (`Py_SIZE` of a code object): `return self` is a handful; any other body
        // is told apart without materialising `co_code`.
        if ffi::Py_SIZE(code) != REF_UNITS[(n == 2) as usize].load(Relaxed)
            || (*rc).co_argcount != n
            || (*rc).co_kwonlyargcount != 0
            || (*rc).co_flags & !CO_INERT != flags & !CO_INERT
            || (*rc).co_nlocalsplus != n
        {
            return false;
        }
        // `co_code` is cached on the code object after the first access.
        let cc = ffi::PyObject_GetAttrString(code, c"co_code".as_ptr());
        if cc.is_null() {
            ffi::PyErr_Clear();
            return false;
        }
        let same = ffi::PyObject_RichCompareBool(cc, want, ffi::Py_EQ) == 1;
        decref(cc);
        if same {
            for c in cache {
                // A strong reference, kept for the process (at most a few code objects).
                if c.compare_exchange(std::ptr::null_mut(), code, Relaxed, Relaxed).is_ok() {
                    incref(code);
                    break;
                }
            }
        }
        same
    }
}

// ---------------------------------------------------------------------------------------
// Free-threading helpers
// ---------------------------------------------------------------------------------------

/// Run `f` holding `o`'s per-object lock on free-threaded builds (plain call otherwise).
#[inline(always)]
pub unsafe fn with_cs<T>(o: O, f: impl FnOnce() -> T) -> T {
    #[cfg(Py_GIL_DISABLED)]
    {
        let mut cs = std::mem::MaybeUninit::<ffi::PyCriticalSection>::uninit();
        // SAFETY: live object; begin/end paired.
        unsafe { ffi::PyCriticalSection_Begin(cs.as_mut_ptr(), o) };
        let r = f();
        // SAFETY: paired with begin.
        unsafe { ffi::PyCriticalSection_End(cs.as_mut_ptr()) };
        r
    }
    #[cfg(not(Py_GIL_DISABLED))]
    {
        let _ = o;
        f()
    }
}

/// `x[i]` for the list iterator protocol (`listiter_next`): new reference, or None past
/// the end (the size is re-read every step).
#[inline(always)]
pub unsafe fn list_item(x: O, i: ffi::Py_ssize_t) -> Option<O> {
    // SAFETY: live list.
    unsafe {
        #[cfg(Py_GIL_DISABLED)]
        {
            let it = ffi::PyList_GetItemRef(x, i);
            if it.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            Some(it)
        }
        #[cfg(not(Py_GIL_DISABLED))]
        {
            if i < ffi::PyList_GET_SIZE(x) { Some(incref(ffi::PyList_GET_ITEM(x, i))) } else { None }
        }
    }
}

/// The items iterator of an exact dict or frozendict (`dictiter_iternextitem`): same
/// order, same "changed size" / "keys changed" errors, strong references to key and value.
pub struct DictIt {
    pub d: O,
    pos: ffi::Py_ssize_t,
    used: ffi::Py_ssize_t,
    len: ffi::Py_ssize_t,
    done: bool,
}

#[inline(always)]
unsafe fn ma_used(d: O) -> ffi::Py_ssize_t {
    // SAFETY: an exact dict or frozendict (PyDictObject layout).
    unsafe { std::ptr::read_volatile(&raw const (*d.cast::<ffi::PyDictObject>()).ma_used) }
}

impl DictIt {
    /// Borrowed dict; the caller keeps it alive.
    #[inline(always)]
    pub unsafe fn new(d: O) -> DictIt {
        // SAFETY: as ma_used.
        let used = unsafe { ma_used(d) };
        DictIt { d, pos: 0, used, len: used, done: false }
    }

    /// Like [`Self::next`] but borrowed references on GIL builds (valid until the next
    /// Python code or C-API call that can run Python code; strong on free-threaded builds,
    /// where `owned` says to release them).
    #[inline(always)]
    pub unsafe fn next_borrowed(&mut self) -> R<Option<(O, O)>> {
        #[cfg(Py_GIL_DISABLED)]
        {
            // SAFETY: as next.
            unsafe { self.next() }
        }
        #[cfg(not(Py_GIL_DISABLED))]
        {
            if self.done {
                return Ok(None);
            }
            // SAFETY: live dict; no other thread runs while we hold the GIL.
            unsafe {
                let d = self.d;
                if ma_used(d) != self.used {
                    self.done = true;
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"dictionary changed size during iteration".as_ptr());
                    return Err(());
                }
                let mut k: O = std::ptr::null_mut();
                let mut v: O = std::ptr::null_mut();
                if ffi::PyDict_Next(d, &mut self.pos, &mut k, &mut v) == 0 {
                    self.done = true;
                    return Ok(None);
                }
                if self.len == 0 {
                    self.done = true;
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"dictionary keys changed during iteration".as_ptr());
                    return Err(());
                }
                self.len -= 1;
                Ok(Some((k, v)))
            }
        }
    }

    /// Whether [`Self::next_borrowed`] returns strong references.
    pub const OWNED: bool = cfg!(Py_GIL_DISABLED);

    #[inline(always)]
    pub unsafe fn next(&mut self) -> R<Option<(O, O)>> {
        if self.done {
            return Ok(None);
        }
        // SAFETY: live dict; the per-object lock on free-threaded builds, as dictiter.
        unsafe {
            let d = self.d;
            let r = with_cs(d, || {
                if ma_used(d) != self.used {
                    return Err(0);
                }
                let mut k: O = std::ptr::null_mut();
                let mut v: O = std::ptr::null_mut();
                if ffi::PyDict_Next(d, &mut self.pos, &mut k, &mut v) == 0 {
                    return Ok(None);
                }
                if self.len == 0 {
                    return Err(1);
                }
                self.len -= 1;
                Ok(Some((incref(k), incref(v))))
            });
            match r {
                Ok(Some(kv)) => Ok(Some(kv)),
                Ok(None) => {
                    self.done = true;
                    Ok(None)
                }
                Err(e) => {
                    self.done = true;
                    let msg = if e == 0 {
                        c"dictionary changed size during iteration"
                    } else {
                        c"dictionary keys changed during iteration"
                    };
                    ffi::PyErr_SetString(ffi::PyExc_RuntimeError, msg.as_ptr());
                    Err(())
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Small vectors (no allocation for small call trees)
// ---------------------------------------------------------------------------------------

/// A vector whose first `N` elements live inline; beyond that everything moves to the heap.
pub struct SVec<T, const N: usize> {
    len: usize,
    inline: [std::mem::MaybeUninit<T>; N],
    heap: Vec<T>,
    spilled: bool,
}

impl<T, const N: usize> SVec<T, N> {
    #[inline(always)]
    pub fn new() -> Self {
        SVec { len: 0, inline: [const { std::mem::MaybeUninit::uninit() }; N], heap: Vec::new(), spilled: false }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        if self.spilled { self.heap.len() } else { self.len }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    pub fn push(&mut self, v: T) {
        if self.spilled {
            self.heap.push(v);
        } else if self.len < N {
            self.inline[self.len].write(v);
            self.len += 1;
        } else {
            self.spill();
            self.heap.push(v);
        }
    }

    #[cold]
    fn spill(&mut self) {
        let mut h = Vec::with_capacity(N * 4);
        for i in 0..self.len {
            // SAFETY: initialised slots, moved out exactly once.
            h.push(unsafe { self.inline[i].assume_init_read() });
        }
        self.len = 0;
        self.heap = h;
        self.spilled = true;
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        if self.spilled {
            self.heap.pop()
        } else if self.len == 0 {
            None
        } else {
            self.len -= 1;
            // SAFETY: initialised slot, moved out once.
            Some(unsafe { self.inline[self.len].assume_init_read() })
        }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        if self.spilled {
            &self.heap
        } else {
            // SAFETY: the first `len` slots are initialised.
            unsafe { std::slice::from_raw_parts(self.inline.as_ptr().cast(), self.len) }
        }
    }

    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.spilled {
            &mut self.heap
        } else {
            // SAFETY: the first `len` slots are initialised.
            unsafe { std::slice::from_raw_parts_mut(self.inline.as_mut_ptr().cast(), self.len) }
        }
    }

    /// Drop every element (in order) and empty the vector, keeping its storage.
    pub fn clear(&mut self) {
        if self.spilled {
            self.heap.clear();
        } else {
            let n = self.len;
            self.len = 0;
            for i in 0..n {
                // SAFETY: initialised slots, dropped once.
                unsafe { self.inline[i].assume_init_drop() };
            }
        }
    }
}

impl<T, const N: usize> Drop for SVec<T, N> {
    fn drop(&mut self) {
        if !self.spilled {
            for i in (0..self.len).rev() {
                // SAFETY: initialised slots, dropped once.
                unsafe { self.inline[i].assume_init_drop() };
            }
        }
    }
}

impl<T, const N: usize> Default for SVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
