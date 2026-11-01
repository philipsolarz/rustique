//! Patch sites: where a kernel goes, what it replaced, how to put it back.
//!
//! Three kinds, each a single aligned pointer store that concurrent readers see either
//! before or after:
//! - [`Kind::MlMeth`]: the `ml_meth` field of a `PyMethodDef`. Static tables (builtin
//!   types, `builtins`, most C accelerator modules, also the method tables of heap types
//!   created with `PyType_FromSpec`, which keep pointing at the static array) are shared by
//!   every interpreter of the process: the swap is process-global.
//! - [`Kind::Slot`]: a type slot (`tp_*`, `nb_*`, `sq_*`, `mp_*`). Static builtin types are
//!   process-global; heap types are per interpreter (each interpreter builds its own).
//! - [`Kind::Vectorcall`]: `PyFunctionObject.vectorcall` of a pure-Python function, set with
//!   the public `PyFunction_SetVectorcall` (stops the world on free-threaded builds, clears
//!   `func_version` so the specializer stops inlining calls to it). Per interpreter.

use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;

use crate::patch::writable;

unsafe extern "C" {
    fn PyFunction_SetVectorcall(func: *mut ffi::PyFunctionObject, vectorcall: ffi::vectorcallfunc);
    /// Exported "for external JIT support" (pycore_function.h); used only to give a function
    /// its specializer version back on uninstall.
    fn _PyFunction_SetVersion(func: *mut ffi::PyFunctionObject, version: u32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    MlMeth,
    Slot,
    Vectorcall,
}

/// One patch site.
#[derive(Debug)]
pub struct Site {
    pub name: String,
    pub kind: Kind,
    /// `MlMeth`/`Slot`: address of the function-pointer field. `Vectorcall`: the function
    /// object (a strong reference is held while the site exists).
    pub addr: usize,
    pub original: usize,
    pub kernel: usize,
    pub installed: bool,
}

// SAFETY: plain addresses; the referenced memory is process-global or kept alive.
unsafe impl Send for Site {}

/// Which type slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotName {
    TpRepr,
    TpStr,
    TpHash,
    TpCall,
    TpRichcompare,
    TpIter,
    TpIternext,
    NbAdd,
    NbRemainder,
    SqConcat,
    SqContains,
    MpSubscript,
}

impl SlotName {
    pub fn parse(s: &str) -> PyResult<Self> {
        Ok(match s {
            "tp_repr" => Self::TpRepr,
            "tp_str" => Self::TpStr,
            "tp_hash" => Self::TpHash,
            "tp_call" => Self::TpCall,
            "tp_richcompare" => Self::TpRichcompare,
            "tp_iter" => Self::TpIter,
            "tp_iternext" => Self::TpIternext,
            "nb_add" => Self::NbAdd,
            "nb_remainder" => Self::NbRemainder,
            "sq_concat" => Self::SqConcat,
            "sq_contains" => Self::SqContains,
            "mp_subscript" => Self::MpSubscript,
            _ => return Err(PyValueError::new_err(format!("unknown slot {s:?}"))),
        })
    }
}

/// Address of a slot field, `None` if the type has no such table or the slot is empty.
///
/// # Safety
/// `t` is a ready type object.
pub unsafe fn slot_addr(t: *mut ffi::PyTypeObject, slot: SlotName) -> Option<usize> {
    // SAFETY: field address computations on a ready type; sub-tables checked for NULL.
    unsafe {
        let addr = match slot {
            SlotName::TpRepr => (&raw mut (*t).tp_repr) as usize,
            SlotName::TpStr => (&raw mut (*t).tp_str) as usize,
            SlotName::TpHash => (&raw mut (*t).tp_hash) as usize,
            SlotName::TpCall => (&raw mut (*t).tp_call) as usize,
            SlotName::TpRichcompare => (&raw mut (*t).tp_richcompare) as usize,
            SlotName::TpIter => (&raw mut (*t).tp_iter) as usize,
            SlotName::TpIternext => (&raw mut (*t).tp_iternext) as usize,
            SlotName::NbAdd | SlotName::NbRemainder => {
                let nb = (*t).tp_as_number;
                if nb.is_null() {
                    return None;
                }
                if slot == SlotName::NbAdd {
                    (&raw mut (*nb).nb_add) as usize
                } else {
                    (&raw mut (*nb).nb_remainder) as usize
                }
            }
            SlotName::SqConcat | SlotName::SqContains => {
                let sq = (*t).tp_as_sequence;
                if sq.is_null() {
                    return None;
                }
                if slot == SlotName::SqConcat {
                    (&raw mut (*sq).sq_concat) as usize
                } else {
                    (&raw mut (*sq).sq_contains) as usize
                }
            }
            SlotName::MpSubscript => {
                let mp = (*t).tp_as_mapping;
                if mp.is_null() {
                    return None;
                }
                (&raw mut (*mp).mp_subscript) as usize
            }
        };
        (*(addr as *const usize) != 0).then_some(addr)
    }
}

/// The `PyMethodDef` behind a method descriptor, class-method descriptor or builtin
/// function / bound builtin method.
///
/// # Safety
/// Attached thread.
pub unsafe fn method_def(obj: *mut ffi::PyObject) -> Option<*mut ffi::PyMethodDef> {
    // SAFETY: type-checked layout reads.
    unsafe {
        let t = ffi::Py_TYPE(obj);
        let def = if t == &raw mut ffi::PyMethodDescr_Type || t == &raw mut ffi::PyClassMethodDescr_Type {
            (*obj.cast::<ffi::PyMethodDescrObject>()).d_method
        } else if ffi::PyCFunction_Check(obj) != 0 {
            (*obj.cast::<ffi::PyCFunctionObject>()).m_ml
        } else {
            return None;
        };
        (!def.is_null() && !(*def).ml_name.is_null()).then_some(def)
    }
}

/// Calling-convention bits of `ml_flags` (without METH_CLASS/STATIC/COEXIST).
pub const CONV_MASK: c_int =
    ffi::METH_VARARGS | ffi::METH_KEYWORDS | ffi::METH_NOARGS | ffi::METH_O | ffi::METH_FASTCALL | ffi::METH_METHOD;

impl Site {
    /// A site on the `ml_meth` of the method table entry behind `obj`.
    pub fn ml_meth(obj: &Bound<'_, PyAny>, kernel: usize) -> PyResult<(Self, c_int)> {
        // SAFETY: attached thread.
        let def = unsafe { method_def(obj.as_ptr()) }
            .ok_or_else(|| PyTypeError::new_err("not a builtin method or function"))?;
        // SAFETY: valid method def.
        let (name, flags, slot) = unsafe {
            (
                CStr::from_ptr((*def).ml_name).to_string_lossy().into_owned(),
                (*def).ml_flags,
                (&raw mut (*def).ml_meth) as usize,
            )
        };
        if !writable(slot, size_of::<usize>()) {
            return Err(PyRuntimeError::new_err(format!("{name}: method table is read-only")));
        }
        // SAFETY: aligned pointer-sized field.
        let original = unsafe { (*(slot as *const AtomicUsize)).load(Ordering::Acquire) };
        Ok((Site { name, kind: Kind::MlMeth, addr: slot, original, kernel, installed: false }, flags & CONV_MASK))
    }

    /// A site on a type slot.
    pub fn slot(t: &Bound<'_, pyo3::types::PyType>, slot: SlotName, kernel: usize) -> PyResult<Self> {
        let tp = t.as_type_ptr();
        // SAFETY: ready type.
        let addr = unsafe { slot_addr(tp, slot) }
            .ok_or_else(|| PyValueError::new_err(format!("{slot:?} is empty on this type")))?;
        if !writable(addr, size_of::<usize>()) {
            return Err(PyRuntimeError::new_err(format!("{slot:?}: type object is read-only")));
        }
        // SAFETY: aligned pointer-sized field.
        let original = unsafe { (*(addr as *const AtomicUsize)).load(Ordering::Acquire) };
        Ok(Site {
            name: format!("{}.{slot:?}", t.qualname()?),
            kind: Kind::Slot,
            addr,
            original,
            kernel,
            installed: false,
        })
    }

    /// A site on the vectorcall entry of an exact pure-Python function.
    pub fn vectorcall(func: &Bound<'_, PyAny>, kernel: usize) -> PyResult<Self> {
        let p = func.as_ptr();
        // SAFETY: type check, then layout read of a function object.
        unsafe {
            if ffi::Py_TYPE(p) != &raw mut ffi::PyFunction_Type {
                return Err(PyTypeError::new_err("not a pure-Python function"));
            }
            let original = (*p.cast::<ffi::PyFunctionObject>()).vectorcall.map_or(0, |f| f as usize);
            ffi::Py_INCREF(p); // released by `release`
            Ok(Site {
                name: func.getattr("__qualname__")?.to_string(),
                kind: Kind::Vectorcall,
                addr: p as usize,
                original,
                kernel,
                installed: false,
            })
        }
    }

    fn current(&self) -> usize {
        // SAFETY: field addresses validated at construction; function kept alive.
        unsafe {
            match self.kind {
                Kind::MlMeth | Kind::Slot => (*(self.addr as *const AtomicUsize)).load(Ordering::Acquire),
                Kind::Vectorcall => (*(self.addr as *mut ffi::PyFunctionObject)).vectorcall.map_or(0, |f| f as usize),
            }
        }
    }

    /// Swap the kernel in; fails (changing nothing) if someone else changed the entry point.
    pub fn install(&mut self) -> PyResult<()> {
        if self.installed {
            return Ok(());
        }
        match self.kind {
            Kind::MlMeth | Kind::Slot => {
                // SAFETY: validated writable aligned field.
                let cell = unsafe { &*(self.addr as *const AtomicUsize) };
                cell.compare_exchange(self.original, self.kernel, Ordering::AcqRel, Ordering::Acquire).map_err(
                    |_| PyRuntimeError::new_err(format!("{}: entry point changed by someone else", self.name)),
                )?;
            }
            Kind::Vectorcall => {
                if self.current() != self.original {
                    return Err(PyRuntimeError::new_err(format!("{}: vectorcall changed by someone else", self.name)));
                }
                // SAFETY: live function; kernel has the vectorcall signature.
                unsafe {
                    PyFunction_SetVectorcall(
                        self.addr as *mut ffi::PyFunctionObject,
                        std::mem::transmute::<usize, ffi::vectorcallfunc>(self.kernel),
                    )
                };
            }
        }
        self.installed = true;
        Ok(())
    }

    /// Put the original back (if the kernel is still there). With `restore_version`, a
    /// function also gets its specializer version back (see [`restore_func_version`]).
    pub fn uninstall(&mut self, restore_version: bool) {
        if !self.installed {
            return;
        }
        match self.kind {
            Kind::MlMeth | Kind::Slot => {
                // SAFETY: validated writable aligned field.
                let cell = unsafe { &*(self.addr as *const AtomicUsize) };
                let _ = cell.compare_exchange(self.kernel, self.original, Ordering::AcqRel, Ordering::Acquire);
            }
            Kind::Vectorcall => {
                if self.current() == self.kernel {
                    // SAFETY: live function; original is its former vectorcall.
                    unsafe {
                        PyFunction_SetVectorcall(
                            self.addr as *mut ffi::PyFunctionObject,
                            std::mem::transmute::<usize, ffi::vectorcallfunc>(self.original),
                        );
                        if restore_version {
                            restore_func_version(self.addr as *mut ffi::PyFunctionObject);
                        }
                    }
                }
            }
        }
        self.installed = false;
    }

    /// Drop the strong reference of a vectorcall site.
    pub fn release(self) {
        if self.kind == Kind::Vectorcall {
            // SAFETY: the reference taken in `vectorcall`.
            unsafe { ffi::Py_DECREF(self.addr as *mut ffi::PyObject) };
        }
    }
}

/// Give a function whose version `PyFunction_SetVectorcall` cleared its code object's
/// version back, so call sites specialize (`CALL_PY_EXACT_ARGS`) again. Uses the field
/// layout of `PyFunctionObject`/`PyCodeObject` (public cpython/ headers) and
/// `_PyFunction_SetVersion`; only valid while the vectorcall is `_PyFunction_Vectorcall`.
///
/// # Safety
/// Live function object; attached thread; no other thread runs Python code (GIL builds) —
/// on free-threaded builds callers must accept a racy version write (prototype only).
pub unsafe fn restore_func_version(func: *mut ffi::PyFunctionObject) {
    // SAFETY: per the caller.
    unsafe {
        let code = (*func).func_code.cast::<crate::accel::version::RawCode>();
        let version = (*code).co_version;
        if version >= 2 && (*func).func_version == 1 {
            (*func).func_version = 0;
            _PyFunction_SetVersion(func, version);
        }
    }
}

/// A group of sites installed all-or-nothing.
#[derive(Debug, Default)]
pub struct Patch {
    pub sites: Vec<Site>,
}

impl Patch {
    pub fn install(&mut self) -> PyResult<()> {
        for i in 0..self.sites.len() {
            if let Err(e) = self.sites[i].install() {
                for s in &mut self.sites[..i] {
                    s.uninstall(false);
                }
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn uninstall(&mut self, restore_version: bool) {
        for s in self.sites.iter_mut().rev() {
            s.uninstall(restore_version);
        }
    }
}
