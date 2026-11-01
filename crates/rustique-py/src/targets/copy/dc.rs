//! Group `dataclasses`: `asdict` and `astuple` (default factories) as vectorcall kernels,
//! for the calls in which no user code runs.
//!
//! `_asdict_inner` / `_astuple_inner` run natively for atomic values, dataclass instances
//! (`fields(obj)` over `Field` objects, `getattr(obj, f.name)` on plain attributes, slots
//! or class defaults), exact `list`/`dict`/`tuple`, and leaves
//! through the copy group's `deepcopy` engine. Anything that would run user code — a
//! property, a subclass constructor (namedtuple, defaultdict), a leaf whose copy needs a
//! hook, a lazily imported `copy` still to be imported, a key with a user `__hash__` — or
//! a depth past [`MAX_NATIVE_DEPTH`] or the recursion limit restarts the whole call in
//! CPython's original before anything observable happened.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use super::rt::*;
use super::{deep, original};
use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

static C_ASDICT: Counters = Counters::new();
static C_ASTUPLE: Counters = Counters::new();
static DEPS_DC: DepFlag = DepFlag::new();
static TOOLS_DC: Tools = Tools::new();

static ASDICT: TargetSpec = TargetSpec {
    name: "dataclasses.asdict",
    group: "dataclasses",
    module: Some("dataclasses"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_ASDICT,
    prepare: prepare_asdict,
    ..TargetSpec::DEFAULT
};

static ASTUPLE: TargetSpec = TargetSpec {
    name: "dataclasses.astuple",
    group: "dataclasses",
    module: Some("dataclasses"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_ASTUPLE,
    prepare: prepare_astuple,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&ASDICT, &ASTUPLE];

/// Native levels of `_asdict_inner` before the rest of the subtree is left to CPython.
const MAX_NATIVE_DEPTH: u32 = 200;

mod fp {
    use crate::accel::Fingerprints;
    pub const ASDICT: Fingerprints = &[(3, 14, "ba467474bda5ee1e"), (3, 15, "f14fb71ab58f21c4")];
    pub const ASDICT_INNER: Fingerprints = &[(3, 14, "61e431a05f60ff1c"), (3, 15, "0e68478cdd43db85")];
    pub const ASTUPLE: Fingerprints = &[(3, 14, "a4ee7e390673e4ed"), (3, 15, "c775d72bd46f4d7a")];
    pub const ASTUPLE_INNER: Fingerprints = &[(3, 14, "0bd168e290e31931"), (3, 15, "8634bef1cee8bebf")];
    pub const FIELDS: Fingerprints = &[(3, 14, "aa9aafdd64096d40"), (3, 15, "95ffcf6afc1b9828")];
    pub const IS_DC: Fingerprints = &[(3, 14, "44ffbabb4cab69c3"), (3, 15, "8023babe8e03bd3f")];
}

struct DcSt {
    // (function, code, defaults-or-kwdefaults)
    asdict: (O, O, O),
    asdict_inner: (O, O, O),
    astuple: (O, O, O),
    astuple_inner: (O, O, O),
    fields: (O, O, O),
    is_dc: (O, O, O),
    atomic: [*mut ffi::PyTypeObject; 15],
    globals: O,
    s_copy_name: O,
    s_fields: O,
    field: O,
    s_field_type: O,
    /// `dataclasses.Field`.
    field_cls: O,
    s_name: O,
    s_deepcopy_attr: O,
    s_dict_factory: O,
    s_tuple_factory: O,
}

// SAFETY: immutable after install; referenced objects kept alive.
unsafe impl Send for DcSt {}
// SAFETY: as Send: read-only after install.
unsafe impl Sync for DcSt {}

static DST: AtomicPtr<DcSt> = AtomicPtr::new(null_mut());

#[inline(always)]
fn dst() -> &'static DcSt {
    // SAFETY: set before the kernels are installed; never freed.
    unsafe { &*DST.load(Relaxed) }
}

/// The function still has the code and defaults validated at install.
#[inline(always)]
unsafe fn same_code(f: (O, O, O), kw: bool) -> bool {
    // SAFETY: live function (strong ref in DcSt).
    unsafe {
        let fo = f.0.cast::<ffi::PyFunctionObject>();
        (*fo).func_code == f.1 && (if kw { (*fo).func_kwdefaults == f.2 } else { (*fo).func_defaults == f.2 })
    }
}

/// [`same_code`], and no tool listens locally on that code: safe to emulate.
#[inline(always)]
unsafe fn intact(f: (O, O, O), kw: bool) -> bool {
    // SAFETY: live function (strong ref in DcSt).
    unsafe { same_code(f, kw) && accel::code_locally_quiet(f.1) }
}

/// (function, code, defaults or kwdefaults) of a pure-Python function, from the struct.
fn func3(o: &Bound<'_, PyAny>, kw: bool) -> PyResult<(O, O, O)> {
    if o.cast::<pyo3::types::PyFunction>().is_err() {
        return Err(pyo3::exceptions::PyTypeError::new_err("not a function"));
    }
    // SAFETY: an exact function object (strong references taken).
    unsafe {
        let f = o.as_ptr().cast::<ffi::PyFunctionObject>();
        let d = if kw { (*f).func_kwdefaults } else { (*f).func_defaults };
        Ok((incref(o.as_ptr()), incref((*f).func_code), if d.is_null() { d } else { incref(d) }))
    }
}

/// `dataclasses._ATOMIC_TYPES`.
fn dc_atomic_types() -> [*mut ffi::PyTypeObject; 15] {
    // SAFETY: static type objects and singletons.
    unsafe {
        [
            &raw mut ffi::PyUnicode_Type,
            &raw mut ffi::PyLong_Type,
            ty(ffi::Py_None()),
            &raw mut ffi::PyFloat_Type,
            &raw mut ffi::PyBool_Type,
            &raw mut ffi::PyComplex_Type,
            &raw mut ffi::PyBytes_Type,
            &raw mut ffi::PyEllipsis_Type,
            ty(ffi::Py_NotImplemented()),
            &raw mut ffi::PyCode_Type,
            &raw mut ffi::PyCFunction_Type,
            &raw mut ffi::PyFunction_Type,
            &raw mut ffi::PyType_Type,
            &raw mut ffi::PyRange_Type,
            &raw mut ffi::PyProperty_Type,
        ]
    }
}

/// Validate the dataclasses module and build [`DcSt`]; pin what the helpers read.
fn ensure_dc(py: Python<'_>, cx: &mut Prepare<'_>, which: Which) -> Result<(), Refusal> {
    let m = cx.module()?.clone();
    let (_, minor) = cx.version();
    super::tstate_offset(cx, minor)?;
    let g = m.dict();
    // Built by another target of the group from this very module, with the same (already
    // fingerprinted) functions: share it, its pins are declared below anyway.
    let shared = !cx.record && {
        let p = DST.load(Relaxed);
        // SAFETY: never freed.
        !p.is_null() && unsafe { (*p).globals } == g.as_ptr() && unsafe { same_functions(&*p, &g) }?
    };

    let checks: [(&str, crate::accel::Fingerprints); 6] = [
        ("asdict", fp::ASDICT),
        ("_asdict_inner", fp::ASDICT_INNER),
        ("astuple", fp::ASTUPLE),
        ("_astuple_inner", fp::ASTUPLE_INNER),
        ("fields", fp::FIELDS),
        ("_is_dataclass_instance", fp::IS_DC),
    ];
    if !shared {
        for (k, f) in checks {
            let o = super::dget(&g, k)?;
            super::check_fp(cx, &o, f)?;
        }
    }
    if !shared {
        build_dc(py, &g)?;
    }
    pin_dc(cx, &g, minor, which)
}

/// Validate `dataclasses`' globals and store [`DcSt`].
fn build_dc(py: Python<'_>, g: &Bound<'_, PyDict>) -> Result<(), Refusal> {
    let g = g.clone();
    let atomic = dc_atomic_types();
    let at = super::dget(&g, "_ATOMIC_TYPES")?;
    // SAFETY: a static type object.
    // SAFETY: live object.
    if unsafe { ty(at.as_ptr()) } != &raw mut ffi::PyFrozenSet_Type || !super::same_type_set(&at, &atomic)? {
        return Err("dataclasses._ATOMIC_TYPES differs".into());
    }
    let asdict = super::dget(&g, "asdict")?;
    let astuple = super::dget(&g, "astuple")?;
    let kwd_ok = |f: &Bound<'_, PyAny>, k: &str, want: *mut ffi::PyTypeObject| -> PyResult<bool> {
        let d = f.getattr("__kwdefaults__")?;
        let Ok(d) = d.cast_into::<PyDict>() else { return Ok(false) };
        Ok(d.len() == 1 && d.get_item(k)?.is_some_and(|v| v.as_ptr() == want.cast()))
    };
    if !kwd_ok(&asdict, "dict_factory", &raw mut ffi::PyDict_Type)?
        || !kwd_ok(&astuple, "tuple_factory", &raw mut ffi::PyTuple_Type)?
    {
        return Err("dataclasses factories' defaults differ".into());
    }
    let s = DcSt {
        asdict: func3(&asdict, true)?,
        asdict_inner: func3(&super::dget(&g, "_asdict_inner")?, false)?,
        astuple: func3(&astuple, true)?,
        astuple_inner: func3(&super::dget(&g, "_astuple_inner")?, false)?,
        fields: func3(&super::dget(&g, "fields")?, false)?,
        is_dc: func3(&super::dget(&g, "_is_dataclass_instance")?, false)?,
        atomic,
        globals: g.clone().into_ptr(),
        s_copy_name: PyString::intern(py, "copy").into_ptr(),
        s_fields: super::dget(&g, "_FIELDS")?.into_ptr(),
        field: super::dget(&g, "_FIELD")?.into_ptr(),
        s_field_type: PyString::intern(py, "_field_type").into_ptr(),
        field_cls: super::dget(&g, "Field")?.into_ptr(),
        s_name: PyString::intern(py, "name").into_ptr(),
        s_deepcopy_attr: PyString::intern(py, "deepcopy").into_ptr(),
        s_dict_factory: PyString::intern(py, "dict_factory").into_ptr(),
        s_tuple_factory: PyString::intern(py, "tuple_factory").into_ptr(),
    };
    // `Field.name` / `Field._field_type` are slots (member descriptors): reading them is pure.
    for n in ["name", "_field_type"] {
        let k = PyString::intern(py, n);
        // SAFETY: a live type object (checked) and an interned name.
        let ok = unsafe {
            ffi::PyType_Check(s.field_cls) != 0 && {
                let d = _PyType_Lookup(s.field_cls.cast(), k.as_ptr());
                !d.is_null() && ty(d) == &raw mut ffi::PyMemberDescr_Type
            }
        };
        if !ok {
            return Err("dataclasses.Field layout differs".into());
        }
    }
    if PyString::intern(py, "__dataclass_fields__").as_ptr() != s.s_fields {
        return Err("dataclasses._FIELDS differs".into());
    }
    DST.store(Box::leak(Box::new(s)), Relaxed);
    Ok(())
}

/// Pin what the emulated helpers of `which` read at call time.
fn pin_dc(cx: &mut Prepare<'_>, g: &Bound<'_, PyDict>, minor: u8, which: Which) -> Result<(), Refusal> {
    let (globals, builtins): (&[&str], &[&str]) = match which {
        Which::Asdict => (
            &["_asdict_inner", "fields", "_FIELDS", "_FIELD", "_ATOMIC_TYPES", "_is_dataclass_instance", "Field"],
            &["type", "hasattr", "getattr", "issubclass", "list", "dict", "tuple", "TypeError", "AttributeError"],
        ),
        Which::Astuple => (
            &["_astuple_inner", "fields", "_FIELDS", "_FIELD", "_ATOMIC_TYPES", "_is_dataclass_instance", "Field"],
            &["type", "hasattr", "getattr", "isinstance", "list", "dict", "tuple", "TypeError", "AttributeError"],
        ),
    };
    for k in globals {
        cx.pin(g, k, &DEPS_DC)?;
    }
    match which {
        Which::Asdict => {
            let f = super::dget(g, "asdict")?;
            let kw = f.getattr("__kwdefaults__")?.cast_into::<PyDict>().map_err(PyErr::from)?;
            cx.pin(&kw, "dict_factory", &DEPS_DC)?;
        }
        Which::Astuple => {
            let f = super::dget(g, "astuple")?;
            let kw = f.getattr("__kwdefaults__")?.cast_into::<PyDict>().map_err(PyErr::from)?;
            cx.pin(&kw, "tuple_factory", &DEPS_DC)?;
        }
    }
    for k in builtins {
        cx.pin_global(g, k, &DEPS_DC)?;
    }
    if minor >= 15 {
        cx.pin_global(g, "frozendict", &DEPS_DC)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Which {
    Asdict,
    Astuple,
}

/// The module still holds the function objects (and code objects) `d` was validated with.
unsafe fn same_functions(d: &DcSt, g: &Bound<'_, PyDict>) -> Result<bool, Refusal> {
    let pairs = [
        ("asdict", d.asdict),
        ("_asdict_inner", d.asdict_inner),
        ("astuple", d.astuple),
        ("_astuple_inner", d.astuple_inner),
        ("fields", d.fields),
        ("_is_dataclass_instance", d.is_dc),
    ];
    for (k, f) in pairs {
        let cur = super::dget(g, k)?;
        // SAFETY: a function object (identity-checked).
        if cur.as_ptr() != f.0 || unsafe { (*f.0.cast::<ffi::PyFunctionObject>()).func_code } != f.1 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn prepare_asdict(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    ensure_dc(py, cx, Which::Asdict)?;
    let f = cx.module_attr("asdict")?;
    cx.vectorcall(&f, vc_asdict, &accel::FUNCTION_VECTORCALL)
}

fn prepare_astuple(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    ensure_dc(py, cx, Which::Astuple)?;
    let f = cx.module_attr("astuple")?;
    cx.vectorcall(&f, vc_astuple, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------------------

/// One `asdict`/`astuple` call: the recursion budget (`py_recursion_remaining` at the call)
/// and the emulated Python frames entered. Every method returns Err to restart.
struct Cx {
    d: &'static DcSt,
    budget: i32,
    used: i32,
}

impl Cx {
    #[inline(always)]
    fn push(&mut self) -> R<()> {
        if self.budget - self.used <= 0 {
            return Err(());
        }
        self.used += 1;
        Ok(())
    }

    #[inline(always)]
    fn pop(&mut self) {
        self.used -= 1;
    }

    #[inline(always)]
    fn touch(&self) -> R<()> {
        if self.budget - self.used <= 0 { Err(()) } else { Ok(()) }
    }

    /// `type(obj) in _ATOMIC_TYPES`, decided purely.
    #[inline(always)]
    unsafe fn atomic(&self, t: *mut ffi::PyTypeObject) -> R<bool> {
        if self.d.atomic.contains(&t) {
            return Ok(true);
        }
        // SAFETY: live type.
        if unsafe { meta_hash(t.cast()) } != default_type_hash() {
            return Err(());
        }
        Ok(false)
    }

    /// `hasattr(cls, _FIELDS)`, decided purely.
    #[inline(always)]
    unsafe fn is_dc_type(&self, cls: *mut ffi::PyTypeObject) -> R<bool> {
        let d = self.d;
        // SAFETY: live type objects.
        unsafe {
            let meta = ty(cls.cast());
            let type_getattro = ffi::PyType_Type.tp_getattro.map(|f| f as usize);
            if (*meta).tp_getattro.map(|f| f as usize) != type_getattro || !_PyType_Lookup(meta, d.s_fields).is_null() {
                return Err(());
            }
            let f = _PyType_Lookup(cls, d.s_fields);
            if f.is_null() {
                return Ok(false);
            }
            // A plain value (the fields dict): no `__get__` runs.
            if (*ty(f)).tp_descr_get.is_some() {
                return Err(());
            }
            Ok(true)
        }
    }

    /// `_is_dataclass_instance(obj)` (its frame included).
    #[inline(always)]
    unsafe fn is_dc_instance(&self, obj: O) -> R<bool> {
        self.touch()?;
        // SAFETY: live object.
        unsafe { self.is_dc_type(ty(obj)) }
    }

    /// `fields(obj)`: a new tuple of the `Field`s whose `_field_type is _FIELD`.
    unsafe fn fields(&mut self, obj: O) -> R<O> {
        let d = self.d;
        // SAFETY: live objects; generic attribute access, `Field` member descriptors.
        unsafe {
            if !intact(d.fields, false) || !generic_getattr(ty(obj)) {
                return Err(());
            }
            self.push()?;
            let fs = Own(getattr_opt(obj, d.s_fields)?);
            if fs.get().is_null() || ty(fs.get()) != &raw mut ffi::PyDict_Type {
                return Err(());
            }
            let out = Own::new(ffi::PyList_New(0))?;
            // The generator expression: created, then resumed by tuple() per item and once
            // more to find it exhausted.
            self.touch()?;
            let mut it = DictIt::new(fs.get());
            loop {
                self.touch()?;
                let Some((k, v)) = it.next()? else { break };
                decref(k);
                let v = Own(v);
                if ty(v.get()) != d.field_cls.cast() {
                    return Err(());
                }
                let ft = Own::new(ffi::PyObject_GetAttr(v.get(), d.s_field_type))?;
                if ft.get() == d.field && ffi::PyList_Append(out.get(), v.get()) < 0 {
                    return Err(());
                }
            }
            self.pop();
            nn(ffi::PyList_AsTuple(out.get()))
        }
    }

    /// `getattr(obj, f.name)` when it runs no user code: an instance attribute, a slot, or
    /// a class attribute without a Python `__get__`.
    unsafe fn field_value(&self, obj: O, f: O) -> R<O> {
        let d = self.d;
        // SAFETY: live objects; `Field.name` is a member descriptor.
        unsafe {
            let name = Own::new(ffi::PyObject_GetAttr(f, d.s_name))?;
            if ty(name.get()) != &raw mut ffi::PyUnicode_Type {
                return Err(());
            }
            let cls = ty(obj);
            let a = _PyType_Lookup(cls, name.get());
            if !a.is_null() {
                let at = ty(a);
                let pure = at == &raw mut ffi::PyMemberDescr_Type
                    || at == &raw mut ffi::PyFunction_Type
                    || (*at).tp_descr_get.is_none();
                if !pure {
                    return Err(());
                }
            }
            nn(ffi::PyObject_GetAttr(obj, name.get()))
        }
    }

    /// `copy.deepcopy(obj)` from `_asdict_inner`/`_astuple_inner`: the copy group's engine,
    /// when `copy` is already imported, its `deepcopy` is the installed kernel, and the copy
    /// runs no user code.
    unsafe fn deepcopy(&self, obj: O) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            let g = Own(dict_get(d.globals, d.s_copy_name)?);
            if g.get().is_null() || ty(g.get()) != &raw mut ffi::PyModule_Type {
                return Err(()); // missing, or a lazy import still to run
            }
            let md = ffi::PyModule_GetDict(g.get());
            let f = Own(dict_get(md, d.s_deepcopy_attr)?);
            let st = super::rt::ST.load(Relaxed);
            if f.get().is_null()
                || st.is_null()
                || !super::deepcopy_direct(&*st, f.get())
                || !accel::tools_quiet(&TOOLS_DC, f.get())
            {
                return Err(());
            }
            let st: &'static St = &*st;
            let e = deep::Engine::new(st, null_mut(), super::atomic_fast(st), self.budget - self.used);
            e.run(obj, &mut Vec::new()).map_err(|_| ())
        }
    }

    /// Subclasses of tuple/list/dict/frozendict: their constructors may be user code.
    #[inline(always)]
    unsafe fn builtin_subclass(t: *mut ffi::PyTypeObject) -> bool {
        // SAFETY: live type.
        unsafe {
            ffi::PyType_GetFlags(t)
                & (ffi::Py_TPFLAGS_TUPLE_SUBCLASS | ffi::Py_TPFLAGS_DICT_SUBCLASS | ffi::Py_TPFLAGS_LIST_SUBCLASS)
                != 0
                || (!super::frozendict_type().is_null() && ffi::PyType_IsSubtype(t, super::frozendict_type()) != 0)
        }
    }

    /// `_asdict_inner(obj, dict)`.
    unsafe fn asdict_inner(&mut self, obj: O, level: u32) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            if level >= MAX_NATIVE_DEPTH {
                return Err(());
            }
            if is_quick(ty(obj)) {
                self.touch()?;
                return Ok(incref(obj));
            }
            self.push()?;
            let t = ty(obj);
            let r = if self.atomic(t)? {
                incref(obj)
            } else if t != &raw mut ffi::PyList_Type
                && t != &raw mut ffi::PyDict_Type
                && t != &raw mut ffi::PyTuple_Type
                && self.is_dc_type(t)?
            {
                // {f.name: _asdict_inner(getattr(obj, f.name), dict) for f in fields(obj)}
                let fs = Own::new(self.fields(obj)?)?;
                let out = Own::new(ffi::PyDict_New())?;
                for i in 0..ffi::PyTuple_GET_SIZE(fs.get()) {
                    let f = ffi::PyTuple_GET_ITEM(fs.get(), i);
                    let key = Own::new(ffi::PyObject_GetAttr(f, d.s_name))?;
                    let v = Own::new(self.field_value(obj, f)?)?;
                    let c = Own::new(self.asdict_inner(v.get(), level + 1)?)?;
                    if !pure_key(key.get()) || ffi::PyDict_SetItem(out.get(), key.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                out.into_raw()
            } else if t == &raw mut ffi::PyList_Type {
                let out = Own::new(ffi::PyList_New(0))?;
                let mut i = 0;
                while let Some(v) = list_item(obj, i) {
                    i += 1;
                    let v = Own(v);
                    let c = Own::new(self.asdict_inner(v.get(), level + 1)?)?;
                    if ffi::PyList_Append(out.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                out.into_raw()
            } else if t == &raw mut ffi::PyDict_Type {
                let out = Own::new(ffi::PyDict_New())?;
                let src = Own(incref(obj));
                let mut it = DictIt::new(src.get());
                while let Some((k, v)) = it.next()? {
                    let (k, v) = (Own(k), Own(v));
                    let kc = Own::new(self.asdict_inner(k.get(), level + 1)?)?;
                    let vc = Own::new(self.asdict_inner(v.get(), level + 1)?)?;
                    if !pure_key(kc.get()) || ffi::PyDict_SetItem(out.get(), kc.get(), vc.get()) < 0 {
                        return Err(());
                    }
                }
                out.into_raw()
            } else if t == &raw mut ffi::PyTuple_Type {
                let out = Own::new(ffi::PyList_New(0))?;
                for i in 0..ffi::PyTuple_GET_SIZE(obj) {
                    let c = Own::new(self.asdict_inner(ffi::PyTuple_GET_ITEM(obj, i), level + 1)?)?;
                    if ffi::PyList_Append(out.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                nn(ffi::PyList_AsTuple(out.get()))?
            } else if Self::builtin_subclass(t) {
                return Err(());
            } else {
                self.deepcopy(obj)?
            };
            self.pop();
            Ok(r)
        }
    }

    /// `_astuple_inner(obj, tuple)`.
    unsafe fn astuple_inner(&mut self, obj: O, level: u32) -> R<O> {
        // SAFETY: live objects.
        unsafe {
            if level >= MAX_NATIVE_DEPTH {
                return Err(());
            }
            if is_quick(ty(obj)) {
                self.touch()?;
                return Ok(incref(obj));
            }
            self.push()?;
            let t = ty(obj);
            let r = if self.atomic(t)? {
                incref(obj)
            } else if t != &raw mut ffi::PyList_Type
                && t != &raw mut ffi::PyDict_Type
                && t != &raw mut ffi::PyTuple_Type
                && self.is_dc_instance(obj)?
            {
                // tuple([_astuple_inner(getattr(obj, f.name)) for f in fields(obj)])
                let fs = Own::new(self.fields(obj)?)?;
                let out = Own::new(ffi::PyList_New(0))?;
                for i in 0..ffi::PyTuple_GET_SIZE(fs.get()) {
                    let f = ffi::PyTuple_GET_ITEM(fs.get(), i);
                    let v = Own::new(self.field_value(obj, f)?)?;
                    let c = Own::new(self.astuple_inner(v.get(), level + 1)?)?;
                    if ffi::PyList_Append(out.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                nn(ffi::PyList_AsTuple(out.get()))?
            } else if t == &raw mut ffi::PyTuple_Type {
                // `tuple(_astuple_inner(v) for v in obj)`: a generator expression, created
                // then resumed per item and once more.
                self.touch()?;
                let n = ffi::PyTuple_GET_SIZE(obj);
                let out = Own::new(ffi::PyList_New(0))?;
                for i in 0..n {
                    self.push()?;
                    let c = Own::new(self.astuple_inner(ffi::PyTuple_GET_ITEM(obj, i), level + 1)?)?;
                    self.pop();
                    if ffi::PyList_Append(out.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                self.touch()?;
                nn(ffi::PyList_AsTuple(out.get()))?
            } else if t == &raw mut ffi::PyDict_Type {
                // `dict((_astuple_inner(k), _astuple_inner(v)) for k, v in obj.items())`.
                self.touch()?;
                let out = Own::new(ffi::PyDict_New())?;
                let src = Own(incref(obj));
                let mut it = DictIt::new(src.get());
                loop {
                    self.push()?;
                    let Some((k, v)) = it.next()? else {
                        self.pop();
                        break;
                    };
                    let (k, v) = (Own(k), Own(v));
                    let kc = Own::new(self.astuple_inner(k.get(), level + 1)?)?;
                    let vc = Own::new(self.astuple_inner(v.get(), level + 1)?)?;
                    self.pop();
                    if !pure_key(kc.get()) || ffi::PyDict_SetItem(out.get(), kc.get(), vc.get()) < 0 {
                        return Err(());
                    }
                }
                out.into_raw()
            } else if t == &raw mut ffi::PyList_Type {
                // `list(_astuple_inner(v) for v in obj)`: the generator's frames as for
                // tuples; the list is built as `list(map(copies.__getitem__, range(n)))`,
                // whose length hint is a generator's (none: the default 8), so `list_extend`
                // allocates it exactly as CPython does.
                self.touch()?;
                let copies = Own::new(ffi::PyList_New(0))?;
                let mut i = 0;
                while let Some(v) = list_item(obj, i) {
                    i += 1;
                    let v = Own(v);
                    self.push()?;
                    let c = Own::new(self.astuple_inner(v.get(), level + 1)?)?;
                    self.pop();
                    if ffi::PyList_Append(copies.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                self.touch()?;
                list_like_genexpr(copies.get())?
            } else if Self::builtin_subclass(t) {
                // Subclass constructors may be user code.
                return Err(());
            } else {
                self.deepcopy(obj)?
            };
            self.pop();
            Ok(r)
        }
    }
}

/// `list(<generator yielding the items of copies>)`, with the same allocation: `map` has no
/// length hint either, and `list.__getitem__` over `range(n)` runs no Python code.
unsafe fn list_like_genexpr(copies: O) -> R<O> {
    // SAFETY: an exact list; C callables only.
    unsafe {
        let getitem = Own::new(ffi::PyObject_GetAttrString(copies, c"__getitem__".as_ptr()))?;
        let n = Own::new(ffi::PyLong_FromSsize_t(ffi::PyList_GET_SIZE(copies)))?;
        let r = Own::new(ffi::PyObject_CallOneArg((&raw mut ffi::PyRange_Type).cast(), n.get()))?;
        let m = Own::new(ffi::PyObject_CallFunctionObjArgs(
            (&raw mut ffi::PyMap_Type).cast(),
            getitem.get(),
            r.get(),
            std::ptr::null_mut::<ffi::PyObject>(),
        ))?;
        nn(ffi::PySequence_List(m.get()))
    }
}

#[inline(always)]
fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// The shared kernel of `asdict` / `astuple`.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
unsafe fn kernel(
    func: O,
    args: *const O,
    nargsf: usize,
    kw: O,
    c: &'static Counters,
    f: (O, O, O),
    inner: (O, O, O),
    factory_kw: O,
    factory: *mut ffi::PyTypeObject,
    tuple: bool,
) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let ok_args = nargs(nargsf) == 1
            && (kw.is_null()
                || (ffi::PyTuple_GET_SIZE(kw) == 1
                    && ffi::PyTuple_GET_ITEM(kw, 0) == factory_kw
                    && *args.add(1) == factory.cast()));
        if !ok_args {
            c.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        if !DEPS_DC.ok() || !same_code(f, true) || !same_code(inner, false) {
            c.fallback_streak(Reason::Deps);
            return original(func, args, nargsf, kw);
        }
        if !accel::tools_quiet(&TOOLS_DC, func) || !accel::code_locally_quiet(inner.1) {
            c.fallback_streak(Reason::Tools);
            return original(func, args, nargsf, kw);
        }
        let mut cx = Cx { d: dst(), budget: recursion_remaining(), used: 0 };
        let obj = *args;
        let r = (|| {
            cx.push()?; // asdict's / astuple's frame
            if !cx.is_dc_instance(obj)? {
                return Err(()); // CPython raises the TypeError
            }
            if tuple { cx.astuple_inner(obj, 0) } else { cx.asdict_inner(obj, 0) }
        })();
        match r {
            Ok(y) => {
                c.hit_streak();
                y
            }
            Err(()) => {
                ffi::PyErr_Clear();
                c.fallback_streak(Reason::Deopt);
                original(func, args, nargsf, kw)
            }
        }
    }
}

unsafe extern "C" fn vc_asdict(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    let d = dst();
    // SAFETY: as kernel.
    unsafe {
        kernel(
            func,
            args,
            nargsf,
            kw,
            &C_ASDICT,
            d.asdict,
            d.asdict_inner,
            d.s_dict_factory,
            &raw mut ffi::PyDict_Type,
            false,
        )
    }
}

unsafe extern "C" fn vc_astuple(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    let d = dst();
    // SAFETY: as kernel.
    unsafe {
        kernel(
            func,
            args,
            nargsf,
            kw,
            &C_ASTUPLE,
            d.astuple,
            d.astuple_inner,
            d.s_tuple_factory,
            &raw mut ffi::PyTuple_Type,
            true,
        )
    }
}
