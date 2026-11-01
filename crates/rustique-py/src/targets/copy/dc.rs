//! Group `dataclasses`: `asdict`, `astuple` and `replace` as vectorcall kernels.
//!
//! `asdict(obj)` / `astuple(obj)` with the default factory run `_asdict_inner` /
//! `_astuple_inner` natively for atomic values, dataclass instances (through the same
//! `fields(obj)` and `getattr(obj, f.name)` calls) and exact `list`/`dict`/`tuple`; every
//! other value goes to CPython's own `_asdict_inner`/`_astuple_inner` (subclasses,
//! namedtuples, defaultdicts) or to `copy.deepcopy` (native through the copy group's engine
//! when it is installed and intact). Beyond [`MAX_NATIVE_DEPTH`] levels the subtree is
//! handed to CPython's helper, so deep structures need no deep C stack.
//!
//! `replace(obj, **changes)` runs `_replace`'s field loop natively (same attribute reads,
//! same `changes` insertion order, same errors) and calls `obj.__class__(**changes)`.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering::Relaxed};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use super::rt::*;
use super::{deep, original};
use crate::accel::{self, Counters, DepFlag, Prepare, Reason, Refusal, TargetSpec, Tools};

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

pub static C_ASDICT: Counters = Counters::new();
pub static C_ASTUPLE: Counters = Counters::new();
pub static C_REPLACE: Counters = Counters::new();
pub static DEPS_DC: DepFlag = DepFlag::new();
static TOOLS_DC: Tools = Tools::new();

pub static ASDICT: TargetSpec = TargetSpec {
    name: "dataclasses.asdict",
    group: "dataclasses",
    module: Some("dataclasses"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_ASDICT,
    prepare: prepare_asdict,
    ..TargetSpec::DEFAULT
};

pub static ASTUPLE: TargetSpec = TargetSpec {
    name: "dataclasses.astuple",
    group: "dataclasses",
    module: Some("dataclasses"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_ASTUPLE,
    prepare: prepare_astuple,
    ..TargetSpec::DEFAULT
};

pub static REPLACE: TargetSpec = TargetSpec {
    name: "dataclasses.replace",
    group: "dataclasses",
    module: Some("dataclasses"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_REPLACE,
    prepare: prepare_replace,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&ASDICT, &ASTUPLE, &REPLACE];

/// Native levels of `_asdict_inner` before the rest of the subtree is left to CPython.
const MAX_NATIVE_DEPTH: u32 = 200;

pub mod fp {
    use crate::accel::Fingerprints;
    pub const ASDICT: Fingerprints = &[(3, 14, "8cb2b653a31d83e2"), (3, 15, "6ab2d1d00c3afaa6")];
    pub const ASDICT_INNER: Fingerprints = &[(3, 14, "f9d557aaf4422477"), (3, 15, "eaf04aae2ab27cf8")];
    pub const ASTUPLE: Fingerprints = &[(3, 14, "24dfc52b6050833c"), (3, 15, "f1ed77e772114c3b")];
    pub const ASTUPLE_INNER: Fingerprints = &[(3, 14, "9b8baca788b6c06e"), (3, 15, "7394d90919ed73c1")];
    pub const FIELDS: Fingerprints = &[(3, 14, "adedad7ba627ef28"), (3, 15, "4ebe82b3a648d524")];
    pub const IS_DC: Fingerprints = &[(3, 14, "5ac7413f6e2936a1"), (3, 15, "f0f502b43cd141f5")];
    pub const REPLACE: Fingerprints = &[(3, 14, "355af68d3afe00e1"), (3, 15, "d89da0a772d242e4")];
    pub const REPLACE_INNER: Fingerprints = &[(3, 14, "6ccb9cae5dd2acbe"), (3, 15, "7297909faa9d328e")];
}

pub struct DcSt {
    // (function, code, defaults-or-kwdefaults)
    asdict: (O, O, O),
    asdict_inner: (O, O, O),
    astuple: (O, O, O),
    astuple_inner: (O, O, O),
    fields: (O, O, O),
    is_dc: (O, O, O),
    replace: (O, O, O),
    replace_inner: (O, O, O),
    atomic: [*mut ffi::PyTypeObject; 15],
    atomic_set: O,
    globals: O,
    s_copy_name: O,
    reify: Option<ReifyFn>,
    s_fields: O,
    field: O,
    field_classvar: O,
    field_initvar: O,
    missing: O,
    s_field_type: O,
    s_values: O,
    s_name: O,
    s_init: O,
    s_default: O,
    s_class: O,
    s_deepcopy_attr: O,
    s_dict_factory: O,
    s_tuple_factory: O,
}

// SAFETY: immutable after install; referenced objects kept alive.
unsafe impl Send for DcSt {}
unsafe impl Sync for DcSt {}

static DST: AtomicPtr<DcSt> = AtomicPtr::new(null_mut());

type ReifyFn = unsafe extern "C" fn(*mut ffi::PyThreadState, O, O, O) -> O;
/// `_PyLazyImport_Reify` (3.15, PyAPI_FUNC): what `LOAD_GLOBAL` does with a lazy import.
static REIFY: accel::Original<ReifyFn> = accel::Original::new();

#[inline(always)]
fn dst() -> &'static DcSt {
    // SAFETY: set before the kernels are installed; never freed.
    unsafe { &*DST.load(Relaxed) }
}

#[inline(always)]
unsafe fn intact(f: (O, O, O), kw: bool) -> bool {
    // SAFETY: live function (strong ref in DcSt).
    unsafe {
        let fo = f.0.cast::<ffi::PyFunctionObject>();
        (*fo).func_code == f.1
            && (if kw { (*fo).func_kwdefaults == f.2 } else { (*fo).func_defaults == f.2 })
            && accel::code_locally_quiet(f.1)
    }
}

fn func3(o: &Bound<'_, PyAny>, kw: bool) -> PyResult<(O, O, O)> {
    let d = o.getattr(if kw { "__kwdefaults__" } else { "__defaults__" })?;
    Ok((o.clone().into_ptr(), o.getattr("__code__")?.into_ptr(), if d.is_none() { null_mut() } else { d.into_ptr() }))
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
fn ensure_dc(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let m = cx.module()?.clone();
    let copy = cx.loaded_module("copy")?;
    super::ensure_state(py, cx, &copy)?;
    let (_, minor) = cx.version();
    let g = m.dict();

    let checks: [(&str, crate::accel::Fingerprints); 8] = [
        ("asdict", fp::ASDICT),
        ("_asdict_inner", fp::ASDICT_INNER),
        ("astuple", fp::ASTUPLE),
        ("_astuple_inner", fp::ASTUPLE_INNER),
        ("fields", fp::FIELDS),
        ("_is_dataclass_instance", fp::IS_DC),
        ("replace", fp::REPLACE),
        ("_replace", fp::REPLACE_INNER),
    ];
    for (k, f) in checks {
        let o = super::dget(&g, k)?;
        cx.fingerprint_code(&o, f)?;
    }
    let atomic = dc_atomic_types();
    let at = super::dget(&g, "_ATOMIC_TYPES")?;
    // SAFETY: a static type object.
    // SAFETY: live object.
    if unsafe { ty(at.as_ptr()) } != &raw mut ffi::PyFrozenSet_Type || !super::same_type_set(&at, &atomic)? {
        return Err("dataclasses._ATOMIC_TYPES differs".into());
    }
    // `dataclasses` imports copy lazily on 3.15 (PEP 810): the global is resolved like
    // LOAD_GLOBAL does it, per call (see `Cx::deepcopy`).
    let reify = if minor >= 15 { Some(cx.symbol(c"_PyLazyImport_Reify", &REIFY)?) } else { None };
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
        replace: func3(&super::dget(&g, "replace")?, false)?,
        replace_inner: func3(&super::dget(&g, "_replace")?, false)?,
        atomic,
        atomic_set: at.into_ptr(),
        globals: g.clone().into_ptr(),
        s_copy_name: PyString::intern(py, "copy").into_ptr(),
        reify,
        s_fields: super::dget(&g, "_FIELDS")?.into_ptr(),
        field: super::dget(&g, "_FIELD")?.into_ptr(),
        field_classvar: super::dget(&g, "_FIELD_CLASSVAR")?.into_ptr(),
        field_initvar: super::dget(&g, "_FIELD_INITVAR")?.into_ptr(),
        missing: super::dget(&g, "MISSING")?.into_ptr(),
        s_field_type: PyString::intern(py, "_field_type").into_ptr(),
        s_values: PyString::intern(py, "values").into_ptr(),
        s_name: PyString::intern(py, "name").into_ptr(),
        s_init: PyString::intern(py, "init").into_ptr(),
        s_default: PyString::intern(py, "default").into_ptr(),
        s_class: PyString::intern(py, "__class__").into_ptr(),
        s_deepcopy_attr: PyString::intern(py, "deepcopy").into_ptr(),
        s_dict_factory: PyString::intern(py, "dict_factory").into_ptr(),
        s_tuple_factory: PyString::intern(py, "tuple_factory").into_ptr(),
    };
    if PyString::intern(py, "__dataclass_fields__").as_ptr() != s.s_fields {
        return Err("dataclasses._FIELDS differs".into());
    }
    for k in [
        "_asdict_inner",
        "_astuple_inner",
        "fields",
        "_FIELDS",
        "_FIELD",
        "_FIELD_CLASSVAR",
        "_FIELD_INITVAR",
        "MISSING",
        "_ATOMIC_TYPES",
        "_is_dataclass_instance",
        "_replace",
    ] {
        cx.pin(&g, k, &DEPS_DC)?;
    }
    let kw = asdict.getattr("__kwdefaults__")?.cast_into::<PyDict>().map_err(PyErr::from)?;
    cx.pin(&kw, "dict_factory", &DEPS_DC)?;
    let kw = astuple.getattr("__kwdefaults__")?.cast_into::<PyDict>().map_err(PyErr::from)?;
    cx.pin(&kw, "tuple_factory", &DEPS_DC)?;
    let mut names = vec![
        "type",
        "hasattr",
        "getattr",
        "issubclass",
        "isinstance",
        "tuple",
        "list",
        "dict",
        "TypeError",
        "AttributeError",
    ];
    if minor >= 15 {
        names.push("frozendict");
    }
    for k in names {
        cx.pin_global(&g, k, &DEPS_DC)?;
    }
    DST.store(Box::leak(Box::new(s)), Relaxed);
    Ok(())
}

fn prepare_asdict(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    ensure_dc(py, cx)?;
    let f = cx.module_attr("asdict")?;
    cx.vectorcall(&f, vc_asdict, &accel::FUNCTION_VECTORCALL)
}

fn prepare_astuple(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    ensure_dc(py, cx)?;
    let f = cx.module_attr("astuple")?;
    cx.vectorcall(&f, vc_astuple, &accel::FUNCTION_VECTORCALL)
}

fn prepare_replace(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    ensure_dc(py, cx)?;
    let f = cx.module_attr("replace")?;
    cx.vectorcall(&f, vc_replace, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------------------

struct Cx {
    st: &'static St,
    d: &'static DcSt,
    depth: Depth,
    /// `_asdict_inner` / `_astuple_inner` (whichever this call uses) is intact.
    inner_ok: bool,
}

impl Cx {
    /// `hasattr(t, _FIELDS)`.
    #[inline(always)]
    unsafe fn has_fields(&self, t: O) -> R<bool> {
        // SAFETY: live object.
        unsafe {
            let a = getattr_opt(t, self.d.s_fields)?;
            if a.is_null() {
                return Ok(false);
            }
            decref(a);
            Ok(true)
        }
    }

    /// `_is_dataclass_instance(obj)` (its frame included).
    #[inline(always)]
    unsafe fn is_dc_instance(&mut self, obj: O) -> R<bool> {
        // SAFETY: live object.
        unsafe {
            self.depth.push()?;
            let r = self.has_fields(ty(obj).cast());
            self.depth.pop();
            r
        }
    }

    /// `type(obj) in _ATOMIC_TYPES`.
    #[inline(always)]
    unsafe fn atomic(&self, t: *mut ffi::PyTypeObject) -> R<bool> {
        let a = &self.d.atomic;
        if a.contains(&t) {
            return Ok(true);
        }
        // SAFETY: live type; pinned frozenset.
        unsafe {
            if meta_hash(t.cast()) == self.st.default_type_hash {
                return Ok(false);
            }
            match ffi::PySet_Contains(self.d.atomic_set, t.cast()) {
                r if r < 0 => Err(()),
                r => Ok(r != 0),
            }
        }
    }

    /// `fields(obj)`: a new tuple of the `Field`s whose `_field_type is _FIELD`.
    unsafe fn fields(&mut self, obj: O) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            // With generic attribute access the lookup below cannot fail (the class has
            // `__dataclass_fields__`); anything else (and its `raise ... from None`) is
            // left to CPython's `fields`.
            let generic = (*ty(obj)).tp_getattro.map(|f| f as usize)
                == (*(&raw mut ffi::PyBaseObject_Type)).tp_getattro.map(|f| f as usize);
            if !generic || !intact(d.fields, false) {
                return call1(d.fields.0, obj);
            }
            self.depth.push()?;
            let r = (|| {
                let fs = ffi::PyObject_GetAttr(obj, d.s_fields);
                if fs.is_null() {
                    // `except AttributeError: raise TypeError(...) from None`
                    if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) == 0 {
                        return Err(());
                    }
                    let ctx = ffi::PyErr_GetRaisedException();
                    let msg = c"must be called with a dataclass type or instance";
                    let m = Own::new(ffi::PyUnicode_FromString(msg.as_ptr()))?;
                    let te = ffi::PyObject_CallOneArg(ffi::PyExc_TypeError, m.get());
                    if te.is_null() {
                        xdecref(ctx);
                        return Err(());
                    }
                    ffi::PyException_SetContext(te, ctx);
                    ffi::PyException_SetCause(te, null_mut());
                    ffi::PyErr_SetRaisedException(te);
                    return Err(());
                }
                let fs = Own(fs);
                if ty(fs.get()) != &raw mut ffi::PyDict_Type {
                    // `fields.values()` of something else: the generic protocol.
                    let vals = Own::new(call_method0(fs.get(), d.s_values)?)?;
                    let it = Own::new(ffi::PyObject_GetIter(vals.get()))?;
                    let out = Own::new(ffi::PyList_New(0))?;
                    self.depth.touch()?;
                    loop {
                        self.depth.push()?;
                        let step = (|| -> R<bool> {
                            let Some(v) = iter_next(it.get())? else { return Ok(false) };
                            let v = Own(v);
                            let ft = Own::new(ffi::PyObject_GetAttr(v.get(), d.s_field_type))?;
                            if ft.get() == d.field && ffi::PyList_Append(out.get(), v.get()) < 0 {
                                return Err(());
                            }
                            Ok(true)
                        })();
                        self.depth.pop();
                        if !step? {
                            break;
                        }
                    }
                    return nn(ffi::PyList_AsTuple(out.get()));
                }
                let out = Own::new(ffi::PyList_New(0))?;
                // The generator expression: created, then resumed by tuple().
                self.depth.touch()?;
                let mut it = DictIt::new(fs.get());
                loop {
                    self.depth.push()?;
                    let step = (|| -> R<bool> {
                        let Some((k, v)) = it.next()? else { return Ok(false) };
                        decref(k);
                        let v = Own(v);
                        let ft = Own::new(ffi::PyObject_GetAttr(v.get(), d.s_field_type))?;
                        if ft.get() == d.field && ffi::PyList_Append(out.get(), v.get()) < 0 {
                            return Err(());
                        }
                        Ok(true)
                    })();
                    self.depth.pop();
                    if !step? {
                        break;
                    }
                }
                nn(ffi::PyList_AsTuple(out.get()))
            })();
            self.depth.pop();
            r
        }
    }

    /// `copy.deepcopy(obj)` as `_asdict_inner` calls it: `LOAD_GLOBAL copy` (resolving a
    /// lazy import), `LOAD_ATTR deepcopy`, call. `None`: the global is missing (the caller
    /// lets CPython's helper raise).
    unsafe fn deepcopy(&mut self, obj: O) -> R<Option<O>> {
        let st = self.st;
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            let g = dict_get(d.globals, d.s_copy_name)?;
            if g.is_null() {
                return Ok(None);
            }
            let mut g = Own(g);
            if ty(g.get()) != &raw mut ffi::PyModule_Type
                && std::ffi::CStr::from_ptr((*ty(g.get())).tp_name) == c"lazy_import"
            {
                let Some(reify) = d.reify else { return Ok(None) };
                g = Own::new(reify(ffi::PyThreadState_Get(), g.get(), d.s_copy_name, d.globals))?;
            }
            let f = Own::new(ffi::PyObject_GetAttr(g.get(), d.s_deepcopy_attr))?;
            if super::deepcopy_direct(st, f.get()) && accel::tools_quiet(&TOOLS_DC, f.get()) {
                super::C_DEEPCOPY.hit_streak();
                let mut e = deep::Engine::new(st, null_mut(), super::atomic_fast(st));
                return e.run(obj).map(Some);
            }
            call1(f.get(), obj).map(Some)
        }
    }
}

// ---------------------------------------------------------------------------------------
// asdict
// ---------------------------------------------------------------------------------------

impl Cx {
    /// `_asdict_inner(obj, dict)`; `level` counts native levels.
    unsafe fn asdict_inner(&mut self, obj: O, level: u32) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            if level >= MAX_NATIVE_DEPTH || !self.inner_ok {
                return call2(d.asdict_inner.0, obj, (&raw mut ffi::PyDict_Type).cast());
            }
            // An atomic value: the helper's frame could be entered and returns it.
            if is_quick(self.st, ty(obj)) && *self.depth.rem > 0 {
                return Ok(incref(obj));
            }
            self.depth.push()?;
            let r = self.asdict_body(obj, level);
            self.depth.pop();
            r
        }
    }

    unsafe fn asdict_body(&mut self, obj: O, level: u32) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            let t = ty(obj);
            if self.atomic(t)? {
                return Ok(incref(obj));
            }
            let is_builtin =
                t == &raw mut ffi::PyList_Type || t == &raw mut ffi::PyDict_Type || t == &raw mut ffi::PyTuple_Type;
            if !is_builtin && self.has_fields(t.cast())? {
                let fs = Own::new(self.fields(obj)?)?;
                let out = Own::new(ffi::PyDict_New())?;
                for i in 0..ffi::PyTuple_GET_SIZE(fs.get()) {
                    let f = ffi::PyTuple_GET_ITEM(fs.get(), i);
                    let key = Own::new(ffi::PyObject_GetAttr(f, d.s_name))?;
                    let name = Own::new(ffi::PyObject_GetAttr(f, d.s_name))?;
                    let v = Own::new(getattr_builtin(obj, name.get())?)?;
                    let c = Own::new(self.asdict_inner(v.get(), level + 1)?)?;
                    if ffi::PyDict_SetItem(out.get(), key.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                return Ok(out.into_raw());
            }
            if t == &raw mut ffi::PyList_Type {
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
                return Ok(out.into_raw());
            }
            if t == &raw mut ffi::PyDict_Type {
                let out = Own::new(ffi::PyDict_New())?;
                let src = Own(incref(obj));
                let mut it = DictIt::new(src.get());
                while let Some((k, v)) = it.next()? {
                    let (k, v) = (Own(k), Own(v));
                    let kc = Own::new(self.asdict_inner(k.get(), level + 1)?)?;
                    let vc = Own::new(self.asdict_inner(v.get(), level + 1)?)?;
                    if ffi::PyDict_SetItem(out.get(), kc.get(), vc.get()) < 0 {
                        return Err(());
                    }
                }
                return Ok(out.into_raw());
            }
            if t == &raw mut ffi::PyTuple_Type {
                let out = Own::new(ffi::PyList_New(0))?;
                for i in 0..ffi::PyTuple_GET_SIZE(obj) {
                    let c = Own::new(self.asdict_inner(ffi::PyTuple_GET_ITEM(obj, i), level + 1)?)?;
                    if ffi::PyList_Append(out.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                return nn(ffi::PyList_AsTuple(out.get()));
            }
            // Subclasses of tuple / dict / frozendict / list: CPython's helper, from the
            // `issubclass` tests on (they are pure for these types, so re-running the
            // tests above in Python is unobservable).
            let sub = ffi::PyType_GetFlags(t)
                & (ffi::Py_TPFLAGS_TUPLE_SUBCLASS | ffi::Py_TPFLAGS_DICT_SUBCLASS | ffi::Py_TPFLAGS_LIST_SUBCLASS)
                != 0
                || (self.st.minor >= 15 && ffi::PyType_IsSubtype(t, self.st.frozendict_type) != 0);
            if !sub && let Some(y) = self.deepcopy(obj)? {
                return Ok(y);
            }
            self.depth.pop();
            let r = call2(d.asdict_inner.0, obj, (&raw mut ffi::PyDict_Type).cast());
            self.depth.push().ok();
            r
        }
    }
}

/// `getattr(obj, name)` (the builtin, two arguments).
#[inline(always)]
unsafe fn getattr_builtin(obj: O, name: O) -> R<O> {
    // SAFETY: live objects.
    unsafe {
        if ffi::PyUnicode_Check(name) == 0 {
            ffi::PyErr_Format(
                ffi::PyExc_TypeError,
                c"attribute name must be string, not '%.200s'".as_ptr(),
                (*ty(name)).tp_name,
            );
            return Err(());
        }
        nn(ffi::PyObject_GetAttr(obj, name))
    }
}

#[inline(always)]
fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Common guard of the three kernels. Returns the context, or the fallback reason.
#[inline(always)]
unsafe fn enter(f: (O, O, O), kw: bool, func: O) -> Result<Cx, Reason> {
    // SAFETY: attached thread.
    unsafe {
        let st = super::rt::ST.load(Relaxed);
        if st.is_null() || !DEPS_DC.ok() || func != f.0 || !intact(f, kw) {
            return Err(Reason::Deps);
        }
        if !accel::tools_quiet(&TOOLS_DC, func) {
            return Err(Reason::Tools);
        }
        let st: &'static St = &*st;
        Ok(Cx { st, d: dst(), depth: Depth::new(st), inner_ok: false })
    }
}

unsafe extern "C" fn vc_asdict(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let d = dst();
        let ok_args = nargs(nargsf) == 1
            && (kw.is_null()
                || (ffi::PyTuple_GET_SIZE(kw) == 1
                    && ffi::PyTuple_GET_ITEM(kw, 0) == d.s_dict_factory
                    && *args.add(1) == (&raw mut ffi::PyDict_Type).cast()));
        if !ok_args {
            C_ASDICT.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let mut cx = match enter(d.asdict, true, func) {
            Ok(cx) => cx,
            Err(r) => {
                C_ASDICT.fallback_streak(r);
                return original(func, args, nargsf, kw);
            }
        };
        C_ASDICT.hit_streak();
        cx.inner_ok = intact(d.asdict_inner, false);
        let obj = *args;
        let r = (|| {
            cx.depth.push()?; // asdict's frame
            if !cx.is_dc_instance(obj)? {
                ffi::PyErr_SetString(
                    ffi::PyExc_TypeError,
                    c"asdict() should be called on dataclass instances".as_ptr(),
                );
                return Err(());
            }
            cx.asdict_inner(obj, 0)
        })();
        cx.depth.unwind();
        r.unwrap_or(null_mut())
    }
}

// ---------------------------------------------------------------------------------------
// astuple
// ---------------------------------------------------------------------------------------

impl Cx {
    unsafe fn astuple_inner(&mut self, obj: O, level: u32) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            if level >= MAX_NATIVE_DEPTH || !self.inner_ok {
                return call2(d.astuple_inner.0, obj, (&raw mut ffi::PyTuple_Type).cast());
            }
            // An atomic value: the helper's frame could be entered and returns it.
            if is_quick(self.st, ty(obj)) && *self.depth.rem > 0 {
                return Ok(incref(obj));
            }
            self.depth.push()?;
            let r = self.astuple_body(obj, level);
            self.depth.pop();
            r
        }
    }

    unsafe fn astuple_body(&mut self, obj: O, level: u32) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            let t = ty(obj);
            if self.atomic(t)? {
                return Ok(incref(obj));
            }
            let is_builtin =
                t == &raw mut ffi::PyList_Type || t == &raw mut ffi::PyDict_Type || t == &raw mut ffi::PyTuple_Type;
            if !is_builtin && self.is_dc_instance(obj)? {
                let fs = Own::new(self.fields(obj)?)?;
                let out = Own::new(ffi::PyList_New(0))?;
                for i in 0..ffi::PyTuple_GET_SIZE(fs.get()) {
                    let f = ffi::PyTuple_GET_ITEM(fs.get(), i);
                    let name = Own::new(ffi::PyObject_GetAttr(f, d.s_name))?;
                    let v = Own::new(getattr_builtin(obj, name.get())?)?;
                    let c = Own::new(self.astuple_inner(v.get(), level + 1)?)?;
                    if ffi::PyList_Append(out.get(), c.get()) < 0 {
                        return Err(());
                    }
                }
                return nn(ffi::PyList_AsTuple(out.get()));
            }
            if t == &raw mut ffi::PyTuple_Type {
                // `type(obj)(_astuple_inner(v) for v in obj)`: a generator expression,
                // created then resumed once per item and once more.
                self.depth.touch()?;
                let n = ffi::PyTuple_GET_SIZE(obj);
                let out = Own::new(ffi::PyList_New(0))?;
                for i in 0..=n {
                    self.depth.push()?;
                    let r = if i < n {
                        self.astuple_inner(ffi::PyTuple_GET_ITEM(obj, i), level + 1).and_then(|c| {
                            let c = Own(c);
                            if ffi::PyList_Append(out.get(), c.get()) < 0 { Err(()) } else { Ok(()) }
                        })
                    } else {
                        Ok(())
                    };
                    self.depth.pop();
                    r?;
                }
                return nn(ffi::PyList_AsTuple(out.get()));
            }
            if t == &raw mut ffi::PyDict_Type {
                // `type(obj)((_astuple_inner(k), _astuple_inner(v)) for k, v in obj.items())`
                // (`hasattr(dict, 'default_factory')` is False).
                self.depth.touch()?;
                let out = Own::new(ffi::PyDict_New())?;
                let src = Own(incref(obj));
                let mut it = DictIt::new(src.get());
                loop {
                    self.depth.push()?;
                    let r = (|| -> R<bool> {
                        let Some((k, v)) = it.next()? else { return Ok(false) };
                        let (k, v) = (Own(k), Own(v));
                        let kc = Own::new(self.astuple_inner(k.get(), level + 1)?)?;
                        let vc = Own::new(self.astuple_inner(v.get(), level + 1)?)?;
                        if ffi::PyDict_SetItem(out.get(), kc.get(), vc.get()) < 0 {
                            return Err(());
                        }
                        Ok(true)
                    })();
                    self.depth.pop();
                    if !r? {
                        break;
                    }
                }
                return Ok(out.into_raw());
            }
            let sub = ffi::PyType_GetFlags(t)
                & (ffi::Py_TPFLAGS_TUPLE_SUBCLASS | ffi::Py_TPFLAGS_DICT_SUBCLASS | ffi::Py_TPFLAGS_LIST_SUBCLASS)
                != 0
                || (self.st.minor >= 15 && ffi::PyType_IsSubtype(t, self.st.frozendict_type) != 0);
            if !sub && let Some(y) = self.deepcopy(obj)? {
                return Ok(y);
            }
            self.depth.pop();
            let r = call2(d.astuple_inner.0, obj, (&raw mut ffi::PyTuple_Type).cast());
            self.depth.push().ok();
            r
        }
    }
}

unsafe extern "C" fn vc_astuple(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let d = dst();
        let ok_args = nargs(nargsf) == 1
            && (kw.is_null()
                || (ffi::PyTuple_GET_SIZE(kw) == 1
                    && ffi::PyTuple_GET_ITEM(kw, 0) == d.s_tuple_factory
                    && *args.add(1) == (&raw mut ffi::PyTuple_Type).cast()));
        if !ok_args {
            C_ASTUPLE.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let mut cx = match enter(d.astuple, true, func) {
            Ok(cx) => cx,
            Err(r) => {
                C_ASTUPLE.fallback_streak(r);
                return original(func, args, nargsf, kw);
            }
        };
        C_ASTUPLE.hit_streak();
        cx.inner_ok = intact(d.astuple_inner, false);
        let obj = *args;
        let r = (|| {
            cx.depth.push()?;
            if !cx.is_dc_instance(obj)? {
                ffi::PyErr_SetString(
                    ffi::PyExc_TypeError,
                    c"astuple() should be called on dataclass instances".as_ptr(),
                );
                return Err(());
            }
            cx.astuple_inner(obj, 0)
        })();
        cx.depth.unwind();
        r.unwrap_or(null_mut())
    }
}

// ---------------------------------------------------------------------------------------
// replace
// ---------------------------------------------------------------------------------------

/// `TypeError(f"... {x} ...")` with `x` formatted as an f-string replacement field.
#[cold]
unsafe fn raise_fmt(pre: &str, x: O, repr: bool, post: &str) -> R<O> {
    // SAFETY: live object.
    unsafe {
        let v = if repr { Own::new(ffi::PyObject_Repr(x))? } else { Own(incref(x)) };
        let s = Own::new(ffi::PyObject_Format(v.get(), null_mut()))?;
        let a = Own::new(ffi::PyUnicode_FromStringAndSize(pre.as_ptr().cast(), pre.len() as isize))?;
        let b = Own::new(ffi::PyUnicode_FromStringAndSize(post.as_ptr().cast(), post.len() as isize))?;
        let ab = Own::new(ffi::PyUnicode_Concat(a.get(), s.get()))?;
        let msg = Own::new(ffi::PyUnicode_Concat(ab.get(), b.get()))?;
        let e = Own::new(ffi::PyObject_CallOneArg(ffi::PyExc_TypeError, msg.get()))?;
        raise_obj(e.get())
    }
}

impl Cx {
    /// `_replace(self, **changes)` once `changes` (a new dict, owned by the caller) is
    /// built; its frame is entered by the caller.
    unsafe fn replace_body(&mut self, obj: O, changes: O) -> R<O> {
        let d = self.d;
        // SAFETY: live objects.
        unsafe {
            let fs = Own::new(ffi::PyObject_GetAttr(obj, d.s_fields))?;
            if ty(fs.get()) != &raw mut ffi::PyDict_Type {
                // Shadowed by the instance with something else: everything so far was a
                // pure lookup, so CPython's `_replace` can do the whole loop.
                let args = Own::new(ffi::PyTuple_Pack(1, obj))?;
                return nn(ffi::PyObject_Call(d.replace_inner.0, args.get(), changes));
            }
            let mut it = DictIt::new(fs.get());
            while let Some((k, f)) = it.next()? {
                decref(k);
                let f = Own(f);
                let ft = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_field_type))?;
                if ft.get() == d.field_classvar {
                    continue;
                }
                let init = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_init))?;
                if !truth(init.get())? {
                    let name = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_name))?;
                    match ffi::PySequence_Contains(changes, name.get()) {
                        r if r < 0 => return Err(()),
                        0 => {}
                        _ => {
                            let name = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_name))?;
                            raise_fmt(
                                "field ",
                                name.get(),
                                false,
                                " is declared with init=False, it cannot be specified with replace()",
                            )?;
                        }
                    }
                    continue;
                }
                let name = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_name))?;
                match ffi::PySequence_Contains(changes, name.get()) {
                    r if r < 0 => return Err(()),
                    0 => {
                        let ft = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_field_type))?;
                        if ft.get() == d.field_initvar {
                            let def = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_default))?;
                            if def.get() == d.missing {
                                let name = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_name))?;
                                raise_fmt("InitVar ", name.get(), true, " must be specified with replace()")?;
                            }
                        }
                        let n1 = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_name))?;
                        let v = Own::new(getattr_builtin(obj, n1.get())?)?;
                        let n2 = Own::new(ffi::PyObject_GetAttr(f.get(), d.s_name))?;
                        if ffi::PyObject_SetItem(changes, n2.get(), v.get()) < 0 {
                            return Err(());
                        }
                    }
                    _ => {}
                }
            }
            let cls = Own::new(ffi::PyObject_GetAttr(obj, d.s_class))?;
            let empty = Own::new(ffi::PyTuple_New(0))?;
            nn(ffi::PyObject_Call(cls.get(), empty.get(), changes))
        }
    }
}

unsafe extern "C" fn vc_replace(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let d = dst();
        if nargs(nargsf) != 1 {
            C_REPLACE.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        let mut cx = match enter(d.replace, false, func) {
            Ok(cx) if intact(d.replace_inner, false) && intact(d.is_dc, false) => cx,
            Ok(_) => {
                C_REPLACE.fallback_streak(Reason::Deps);
                return original(func, args, nargsf, kw);
            }
            Err(r) => {
                C_REPLACE.fallback_streak(r);
                return original(func, args, nargsf, kw);
            }
        };
        let obj = *args;
        // `__dataclass_fields__` must be a dict for the native loop: probe it purely
        // (class attribute, no instance override) before anything observable.
        let t = ty(obj);
        let fs = _PyType_Lookup(t, d.s_fields);
        if fs.is_null()
            || ty(fs) != &raw mut ffi::PyDict_Type
            || (*t).tp_getattro.map(|f| f as usize)
                != (*(&raw mut ffi::PyBaseObject_Type)).tp_getattro.map(|f| f as usize)
        {
            C_REPLACE.fallback_streak(Reason::Args);
            return original(func, args, nargsf, kw);
        }
        C_REPLACE.hit_streak();
        let r = (|| {
            cx.depth.push()?; // replace's frame
            // replace(obj, /, **changes): a new dict of the keyword arguments.
            let changes = Own::new(ffi::PyDict_New())?;
            if !kw.is_null() {
                for i in 0..ffi::PyTuple_GET_SIZE(kw) {
                    if ffi::PyDict_SetItem(changes.get(), ffi::PyTuple_GET_ITEM(kw, i), *args.add(1 + i as usize)) < 0 {
                        return Err(());
                    }
                }
            }
            if !cx.is_dc_instance(obj)? {
                ffi::PyErr_SetString(
                    ffi::PyExc_TypeError,
                    c"replace() should be called on dataclass instances".as_ptr(),
                );
                return Err(());
            }
            // _replace(obj, **changes): the callee gets its own copy of the dict.
            let inner = Own::new(ffi::PyDict_Copy(changes.get()))?;
            drop(changes);
            cx.depth.push()?;
            let y = cx.replace_body(obj, inner.get());
            cx.depth.pop();
            y
        })();
        cx.depth.unwind();
        r.unwrap_or(null_mut())
    }
}
