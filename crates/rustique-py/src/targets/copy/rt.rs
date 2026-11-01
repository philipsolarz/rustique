//! Runtime support shared by the copy-group kernels: the objects captured at install, raw
//! C-API helpers, and the emulation of CPython's Python-frame recursion counter.
//!
//! Every helper here performs *the same C-API operation* CPython's bytecode would perform
//! for the corresponding line of `Lib/copy.py` (`getattr(x, name, None)` is
//! `PyObject_GetOptionalAttr`, `y[k] = v` is `PyObject_SetItem`, ...), so that user code
//! reached through them runs with the same arguments in the same order.

use std::ffi::{c_char, c_int};
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;

pub type O = *mut ffi::PyObject;
/// `Err(())`: a Python exception is set.
pub type R<T> = Result<T, ()>;

unsafe extern "C" {
    /// Borrowed MRO lookup through the type cache (cpython/object.h, exported).
    pub fn _PyType_Lookup(t: *mut ffi::PyTypeObject, name: O) -> O;
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
    pub copy_fn: O,
    pub copy_code: O,
    pub atomic: O,
    /// 3.14: `_atomic_types` is a mutable set; its table at install.
    pub atomic_snap: Option<super::SetSnap>,
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
    pub keep_alive: O,
    pub reconstruct: O,
    /// (function, code, defaults) for each helper the kernel emulates; checked before use.
    pub helpers: [(O, O, O); 8],
    // copyreg
    pub newobj: O,
    pub newobj_code: O,
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
    // types
    pub atomic_types: [*mut ffi::PyTypeObject; 16],
    pub method_type: *mut ffi::PyTypeObject,
    pub frozendict_type: *mut ffi::PyTypeObject,
    pub default_type_hash: usize,
    // interned names
    pub s_deepcopy: O,
    pub s_copy: O,
    pub s_copy_method: O,
    pub s_memo: O,
    pub s_reduce_ex: O,
    pub s_reduce: O,
    pub s_setstate: O,
    pub s_dict: O,
    pub s_update: O,
    pub s_items: O,
    pub s_append: O,
    pub s_new: O,
    pub s_error: O,
    pub int4: O,
    /// Byte code of `def f(self, memo): return self` / `def f(self): return self` on this
    /// interpreter, with their flags (used to recognise `Enum.__deepcopy__`-style hooks).
    pub ret_self2: (O, c_int),
    pub ret_self1: (O, c_int),
    // tstate layout
    pub rem_off: usize,
}

// SAFETY: immutable after construction; the objects it points to are kept alive.
unsafe impl Send for St {}
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
pub fn is_quick(st: &St, t: *mut ffi::PyTypeObject) -> bool {
    let a = &st.atomic_types;
    t == a[0] || t == a[1] || t == a[2] || t == a[3] || t == a[4]
}

// ---------------------------------------------------------------------------------------
// Python-frame recursion counter (`tstate->py_recursion_remaining`)
// ---------------------------------------------------------------------------------------

/// Emulates the `_Py_EnterRecursivePy`/`_Py_LeaveRecursiveCallPy` pair CPython runs for
/// each Python frame of `copy.py` the kernel does not actually execute, on the real
/// thread-state counter: user code called from inside a kernel sees the depth CPython's
/// would, and `RecursionError` is raised at the same frame.
pub struct Depth {
    pub rem: *mut c_int,
    headroom: *mut c_int,
    pub pushed: i32,
}

impl Depth {
    #[inline(always)]
    pub unsafe fn new(st: &St) -> Depth {
        // SAFETY: attached thread; offsets calibrated at install.
        unsafe {
            let ts = ffi::PyThreadState_Get().cast::<u8>();
            Depth { rem: ts.add(st.rem_off).cast(), headroom: ts.add(st.rem_off + 8).cast(), pushed: 0 }
        }
    }

    #[inline(always)]
    pub fn remaining(&self) -> c_int {
        // SAFETY: this thread's state.
        unsafe { *self.rem }
    }

    /// Enter one emulated Python frame.
    #[inline(always)]
    pub unsafe fn push(&mut self) -> R<()> {
        // SAFETY: this thread's state.
        unsafe {
            let r = *self.rem;
            *self.rem = r - 1;
            if r <= 0 && self.too_deep() {
                *self.rem = r;
                return Err(());
            }
        }
        self.pushed += 1;
        Ok(())
    }

    #[cold]
    unsafe fn too_deep(&mut self) -> bool {
        // SAFETY: this thread's state; mirrors `_Py_CheckRecursiveCallPy`.
        unsafe {
            if *self.headroom != 0 {
                if *self.rem < -50 {
                    ffi::Py_FatalError(c"Cannot recover from Python stack overflow.".as_ptr());
                }
                false
            } else {
                *self.headroom += 1;
                ffi::PyErr_SetString(ffi::PyExc_RecursionError, c"maximum recursion depth exceeded".as_ptr());
                *self.headroom -= 1;
                true
            }
        }
    }

    /// Leave one emulated Python frame.
    #[inline(always)]
    pub fn pop(&mut self) {
        // SAFETY: this thread's state.
        unsafe { *self.rem += 1 };
        self.pushed -= 1;
    }

    /// A Python frame entered and left with nothing observable in between (a call the
    /// kernel performs natively): only its depth check remains.
    #[inline(always)]
    pub unsafe fn touch(&mut self) -> R<()> {
        // SAFETY: as push.
        unsafe { self.push()? };
        self.pop();
        Ok(())
    }

    /// Unwind every frame still entered (error paths).
    pub fn unwind(&mut self) {
        while self.pushed > 0 {
            self.pop();
        }
    }
}

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
pub unsafe fn truth(o: O) -> R<bool> {
    // SAFETY: live object.
    match unsafe { ffi::PyObject_IsTrue(o) } {
        r if r < 0 => Err(()),
        r => Ok(r != 0),
    }
}

#[inline(always)]
pub unsafe fn call1(f: O, a: O) -> R<O> {
    // SAFETY: live objects.
    nn(unsafe { ffi::PyObject_CallOneArg(f, a) })
}

#[inline(always)]
pub unsafe fn call0(f: O) -> R<O> {
    // SAFETY: live object.
    nn(unsafe { ffi::PyObject_CallNoArgs(f) })
}

#[inline(always)]
pub unsafe fn call2(f: O, a: O, b: O) -> R<O> {
    let args = [std::ptr::null_mut(), a, b];
    // SAFETY: live objects; slot 0 reserved for PY_VECTORCALL_ARGUMENTS_OFFSET.
    nn(unsafe {
        ffi::PyObject_Vectorcall(f, args.as_ptr().add(1), 2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET, std::ptr::null_mut())
    })
}

/// `o.name(a)` (method load + call).
#[inline(always)]
pub unsafe fn call_method1(o: O, name: O, a: O) -> R<O> {
    let args = [o, a];
    // SAFETY: live objects.
    nn(unsafe {
        ffi::PyObject_VectorcallMethod(
            name,
            args.as_ptr(),
            2 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET,
            std::ptr::null_mut(),
        )
    })
}

/// `o.name()`.
#[inline(always)]
pub unsafe fn call_method0(o: O, name: O) -> R<O> {
    let args = [o];
    // SAFETY: live objects.
    nn(unsafe {
        ffi::PyObject_VectorcallMethod(
            name,
            args.as_ptr(),
            1 | ffi::PY_VECTORCALL_ARGUMENTS_OFFSET,
            std::ptr::null_mut(),
        )
    })
}

/// `isinstance(o, str)`.
#[inline(always)]
pub unsafe fn is_str_instance(o: O) -> R<bool> {
    // SAFETY: live object.
    unsafe {
        if ty(o) == &raw mut ffi::PyUnicode_Type {
            return Ok(true);
        }
        if ty(o) == &raw mut ffi::PyTuple_Type {
            // `PyObject_IsInstance` -> `object_isinstance`: a tuple's `__class__` is `tuple`.
            return Ok(false);
        }
        match ffi::PyObject_IsInstance(o, (&raw mut ffi::PyUnicode_Type).cast()) {
            r if r < 0 => Err(()),
            r => Ok(r != 0),
        }
    }
}

/// `next(it)` as `FOR_ITER` does it: Ok(None) when exhausted (StopIteration swallowed).
#[inline(always)]
pub unsafe fn iter_next(it: O) -> R<Option<O>> {
    let mut item: O = std::ptr::null_mut();
    // SAFETY: live iterator.
    match unsafe { ffi::PyIter_NextItem(it, &mut item) } {
        1 => Ok(Some(item)),
        0 => Ok(None),
        _ => Err(()),
    }
}

/// `UNPACK_SEQUENCE 2` (`a, b = v`): exact tuples and lists directly, anything else through
/// `_PyEval_UnpackIterableStackRef`'s algorithm and messages.
pub unsafe fn unpack2(v: O) -> R<(Own, Own)> {
    // SAFETY: live object.
    unsafe {
        let t = ty(v);
        if t == &raw mut ffi::PyTuple_Type && ffi::PyTuple_GET_SIZE(v) == 2 {
            return Ok((Own(incref(ffi::PyTuple_GET_ITEM(v, 0))), Own(incref(ffi::PyTuple_GET_ITEM(v, 1)))));
        }
        let it = ffi::PyObject_GetIter(v);
        if it.is_null() {
            if ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) != 0
                && (*t).tp_iter.is_none()
                && ffi::PySequence_Check(v) == 0
            {
                ffi::PyErr_Format(
                    ffi::PyExc_TypeError,
                    c"cannot unpack non-iterable %.200s object".as_ptr(),
                    (*t).tp_name,
                );
            }
            return Err(());
        }
        let it = Own(it);
        let mut got: [Own; 2] = [Own(std::ptr::null_mut()), Own(std::ptr::null_mut())];
        for (i, slot) in got.iter_mut().enumerate() {
            match iter_next(it.get())? {
                Some(w) => slot.0 = w,
                None => {
                    ffi::PyErr_Format(
                        ffi::PyExc_ValueError,
                        c"not enough values to unpack (expected %d, got %d)".as_ptr(),
                        2 as c_int,
                        i as c_int,
                    );
                    return Err(());
                }
            }
        }
        match iter_next(it.get())? {
            None => {
                let [a, b] = got;
                Ok((a, b))
            }
            Some(w) => {
                decref(w);
                if t == &raw mut ffi::PyList_Type || t == &raw mut ffi::PyTuple_Type || t == &raw mut ffi::PyDict_Type {
                    let ll = if t == &raw mut ffi::PyDict_Type { ffi::PyDict_Size(v) } else { ffi::Py_SIZE(v) };
                    if ll > 2 {
                        ffi::PyErr_Format(
                            ffi::PyExc_ValueError,
                            c"too many values to unpack (expected %d, got %zd)".as_ptr(),
                            2 as c_int,
                            ll,
                        );
                        return Err(());
                    }
                }
                ffi::PyErr_Format(
                    ffi::PyExc_ValueError,
                    c"too many values to unpack (expected %d)".as_ptr(),
                    2 as c_int,
                );
                Err(())
            }
        }
    }
}

/// `CALL_FUNCTION_EX` with a positional-only star argument: `f(*args)`.
pub unsafe fn call_star(f: O, args: O) -> R<O> {
    // SAFETY: live objects.
    unsafe {
        if ty(args) == &raw mut ffi::PyTuple_Type {
            return nn(ffi::PyObject_Call(f, args, std::ptr::null_mut()));
        }
        let t = ty(args);
        if (*t).tp_iter.is_none() && ffi::PySequence_Check(args) == 0 {
            ffi::PyErr_Format(
                ffi::PyExc_TypeError,
                c"Value after * must be an iterable, not %.200s".as_ptr(),
                (*t).tp_name,
            );
            return Err(());
        }
        let tup = Own::new(ffi::PySequence_Tuple(args))?;
        nn(ffi::PyObject_Call(f, tup.get(), std::ptr::null_mut()))
    }
}

/// `raise Error(fmt % cls)` with `Error` looked up like `LOAD_GLOBAL` in copy's globals.
#[cold]
pub unsafe fn raise_copy_error(st: &St, fmt: *const c_char, cls: *mut ffi::PyTypeObject) -> R<O> {
    // SAFETY: attached thread, live objects.
    unsafe {
        let msg = Own::new(ffi::PyUnicode_FromFormat(fmt, cls))?;
        let mut err = dict_get(st.copy_dict, st.s_error)?;
        if err.is_null() {
            let builtins = ffi::PyEval_GetBuiltins();
            err = dict_get(builtins, st.s_error)?;
            if err.is_null() {
                ffi::PyErr_Format(ffi::PyExc_NameError, c"name '%U' is not defined".as_ptr(), st.s_error);
                return Err(());
            }
        }
        let err = Own(err);
        let exc = Own::new(ffi::PyObject_CallOneArg(err.get(), msg.get()))?;
        raise_obj(exc.get())
    }
}

/// `raise exc` (RAISE_VARARGS 1).
pub unsafe fn raise_obj(exc: O) -> R<O> {
    // SAFETY: live object.
    unsafe {
        if ffi::PyExceptionInstance_Check(exc) != 0 {
            ffi::PyErr_SetObject(ty(exc).cast(), exc);
        } else if ffi::PyType_Check(exc) != 0 && ffi::PyExceptionClass_Check(exc) != 0 {
            let inst = Own::new(ffi::PyObject_CallNoArgs(exc))?;
            if ffi::PyExceptionInstance_Check(inst.get()) == 0 {
                ffi::PyErr_Format(
                    ffi::PyExc_TypeError,
                    c"calling %R should have returned an instance of BaseException, not %R".as_ptr(),
                    exc,
                    ty(inst.get()),
                );
                return Err(());
            }
            ffi::PyErr_SetObject(exc, inst.get());
        } else {
            ffi::PyErr_SetString(ffi::PyExc_TypeError, c"exceptions must derive from BaseException".as_ptr());
        }
        Err(())
    }
}

/// Is `f` a Python function whose code is exactly `return self` with `n` positional
/// parameters (`Enum.__deepcopy__`, `Enum.__copy__`, `Pattern`-like no-op hooks written
/// in Python)? Calling it has no effect besides its frame.
#[inline]
pub unsafe fn is_return_self(st: &St, f: O, n: c_int) -> bool {
    if st.ret_self2.0.is_null() {
        return false;
    }
    // SAFETY: live object; layouts of function/code objects (public cpython headers).
    unsafe {
        if ty(f) != &raw mut ffi::PyFunction_Type {
            return false;
        }
        let code = (*f.cast::<ffi::PyFunctionObject>()).func_code;
        let rc = code.cast::<crate::accel::version::RawCode>();
        let (want, flags) = if n == 2 { st.ret_self2 } else { st.ret_self1 };
        if (*rc).co_argcount != n
            || (*rc).co_kwonlyargcount != 0
            || (*rc).co_flags != flags
            || (*rc).co_nlocalsplus != n
            || !crate::accel::guard::code_locally_quiet(code)
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

    /// Move every element out, in order.
    pub fn take_all(&mut self) -> Vec<T> {
        if self.spilled {
            self.spilled = false;
            std::mem::take(&mut self.heap)
        } else {
            let mut v = Vec::with_capacity(self.len);
            for i in 0..self.len {
                // SAFETY: initialised slots, moved out exactly once.
                v.push(unsafe { self.inline[i].assume_init_read() });
            }
            self.len = 0;
            v
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
