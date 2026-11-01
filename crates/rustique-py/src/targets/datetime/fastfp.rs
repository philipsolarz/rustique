//! A cheap canonical fingerprint of a pure-Python function (install-cost budget: the
//! registry's `fingerprint_code` costs ~0.25M Ir per `_strptime` function, this ~5–20k):
//! the same fields (bytecode, names, local names and kinds, flags, argument counts,
//! exception table, constants recursively, defaults), read from the code object's fields
//! (layout calibrated by `accel::version::code_layout_ok_for`) and hashed 8 bytes at a time.
//! Path- and line-independent.

use pyo3::ffi;

use crate::accel::version::RawCode;

unsafe extern "C" {
    fn PyCode_GetCode(code: *mut ffi::PyObject) -> *mut ffi::PyObject;
}

struct H(u64);

impl H {
    #[inline(always)]
    fn word(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.word(b.len() as u64);
        let (chunks, rest) = b.as_chunks::<8>();
        for c in chunks {
            self.word(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut a = [0u8; 8];
            a[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(a));
        }
    }
}

unsafe fn bytes_of<'a>(b: *mut ffi::PyObject) -> &'a [u8] {
    // SAFETY: an exact bytes object kept alive by the caller.
    unsafe { std::slice::from_raw_parts(ffi::PyBytes_AsString(b).cast::<u8>(), ffi::PyBytes_Size(b) as usize) }
}

unsafe fn str_obj(h: &mut H, s: *mut ffi::PyObject) {
    // SAFETY: a live str.
    unsafe {
        let kind = ffi::PyUnicode_KIND(s) as usize;
        let n = ffi::PyUnicode_GET_LENGTH(s) as usize;
        h.word(kind as u64);
        h.bytes(std::slice::from_raw_parts(ffi::PyUnicode_DATA(s).cast::<u8>(), n * kind));
    }
}

unsafe fn repr(h: &mut H, o: *mut ffi::PyObject) -> Option<()> {
    // SAFETY: a live object of a builtin constant type (no user code).
    unsafe {
        let r = ffi::PyObject_Repr(o);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        str_obj(h, r);
        ffi::Py_DECREF(r);
        Some(())
    }
}

unsafe fn obj(h: &mut H, o: *mut ffi::PyObject, depth: u32) -> Option<()> {
    if depth > 16 {
        return None;
    }
    // SAFETY: live constants of a code object.
    unsafe {
        if ffi::PyUnicode_CheckExact(o) != 0 {
            h.word(1);
            str_obj(h, o);
        } else if ffi::PyBytes_CheckExact(o) != 0 {
            h.word(2);
            h.bytes(bytes_of(o));
        } else if ffi::PyLong_CheckExact(o) != 0 {
            h.word(3);
            let mut overflow = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(o, &mut overflow);
            if overflow != 0 {
                repr(h, o)?;
            } else {
                h.word(v as u64);
            }
        } else if ffi::PyFloat_CheckExact(o) != 0 {
            h.word(4);
            h.word(ffi::PyFloat_AS_DOUBLE(o).to_bits());
        } else if o == ffi::Py_None() {
            h.word(5);
        } else if o == ffi::Py_True() {
            h.word(6);
        } else if o == ffi::Py_False() {
            h.word(7);
        } else if o == ffi::Py_Ellipsis() {
            h.word(8);
        } else if ffi::PyTuple_CheckExact(o) != 0 {
            h.word(9);
            let n = ffi::PyTuple_GET_SIZE(o);
            h.word(n as u64);
            for i in 0..n {
                obj(h, ffi::PyTuple_GET_ITEM(o, i), depth + 1)?;
            }
        } else if ffi::PyFrozenSet_CheckExact(o) != 0 {
            // Order-independent: combine the elements' own fingerprints.
            h.word(10);
            let mut acc = 0u64;
            let it = ffi::PyObject_GetIter(o);
            if it.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            loop {
                let x = ffi::PyIter_Next(it);
                if x.is_null() {
                    break;
                }
                let mut e = H(0);
                let ok = obj(&mut e, x, depth + 1);
                ffi::Py_DECREF(x);
                ok?;
                acc = acc.wrapping_add(e.0);
            }
            ffi::Py_DECREF(it);
            if !ffi::PyErr_Occurred().is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            h.word(acc);
        } else if ffi::PySlice_Check(o) != 0 && ffi::Py_TYPE(o) == &raw mut ffi::PySlice_Type {
            // 3.14+ folds constant slices (`z[:3]`).
            h.word(13);
            let sl = o.cast::<ffi::PySliceObject>();
            for x in [(*sl).start, (*sl).stop, (*sl).step] {
                obj(h, x, depth + 1)?;
            }
        } else if ffi::PyCode_Check(o) != 0 {
            h.word(11);
            code(h, o)?;
        } else {
            h.word(12);
            repr(h, o)?;
        }
        Some(())
    }
}

unsafe fn strs(h: &mut H, t: *mut ffi::PyObject) -> Option<()> {
    // SAFETY: a tuple of str from a code object.
    unsafe {
        if ffi::PyTuple_CheckExact(t) == 0 {
            return None;
        }
        let n = ffi::PyTuple_GET_SIZE(t);
        h.word(n as u64);
        for i in 0..n {
            str_obj(h, ffi::PyTuple_GET_ITEM(t, i));
        }
        Some(())
    }
}

unsafe fn code(h: &mut H, c: *mut ffi::PyObject) -> Option<()> {
    // SAFETY: a live code object whose layout the caller calibrated.
    unsafe {
        let raw = &*c.cast::<RawCode>();
        for v in [raw.co_flags, raw.co_argcount, raw.co_posonlyargcount, raw.co_kwonlyargcount] {
            h.word(v as u64);
        }
        let co = PyCode_GetCode(c);
        if co.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        h.bytes(bytes_of(co));
        ffi::Py_DECREF(co);
        strs(h, raw.co_names)?;
        strs(h, raw.co_localsplusnames)?;
        h.bytes(bytes_of(raw.co_localspluskinds));
        h.bytes(bytes_of(raw.co_exceptiontable));
        let consts = raw.co_consts;
        let n = ffi::PyTuple_GET_SIZE(consts);
        h.word(n as u64);
        for i in 0..n {
            obj(h, ffi::PyTuple_GET_ITEM(consts, i), 0)?;
        }
        Some(())
    }
}

/// Fingerprint of an exact pure-Python function (code, defaults, keyword defaults), as 16
/// hex digits; `None` if it cannot be computed.
///
/// # Safety
/// Attached thread; `f` a live exact function whose code layout was calibrated.
pub unsafe fn function(f: *mut ffi::PyObject) -> Option<String> {
    // SAFETY: per the caller.
    unsafe {
        let func = f.cast::<ffi::PyFunctionObject>();
        let mut h = H(0x6461_7465_7469_6d65);
        code(&mut h, (*func).func_code)?;
        for d in [(*func).func_defaults, (*func).func_kwdefaults] {
            if d.is_null() {
                h.word(0);
            } else {
                obj(&mut h, d, 0)?;
            }
        }
        Some(format!("{:016x}", h.0))
    }
}

/// Fingerprint of a C method definition (`ml_name`, `ml_flags`, `ml_doc`), as 16 hex digits.
///
/// # Safety
/// `def` is a live `PyMethodDef`.
pub unsafe fn method_def(def: *const ffi::PyMethodDef) -> String {
    // SAFETY: per the caller; NUL-terminated strings or NULL.
    unsafe {
        let mut h = H(0x6366_756e_6364_6566);
        for p in [(*def).ml_name, (*def).ml_doc] {
            if p.is_null() {
                h.word(0);
            } else {
                h.bytes(std::ffi::CStr::from_ptr(p).to_bytes());
            }
        }
        h.word((*def).ml_flags as u64);
        format!("{:016x}", h.0)
    }
}
