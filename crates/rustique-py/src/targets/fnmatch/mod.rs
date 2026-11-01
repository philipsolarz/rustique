//! Group `fnmatch`: `fnmatch.fnmatch`, `fnmatch.fnmatchcase` and `fnmatch.filter` without
//! building or running a regular expression.
//!
//! CPython translates the shell pattern (`fnmatch._translate`), compiles the regex
//! (`re.compile`, cached by the private `lru_cache` `_compile_pattern`) and matches it.
//! The kernel ports `_translate` and evaluates the regex's meaning directly
//! (`rustique_core::paths::glob`); patterns outside the grammar the port reproduces exactly
//! (anything `re` would warn about) fall back. Exact subset: the pattern an exact `str` or
//! `bytes`; names `str` (any subclass: `os.fspath` returns them unchanged and `re` reads
//! their buffer directly) for a str pattern, exact `bytes` for a bytes pattern; `filter`
//! over an exact list or tuple. Private state skipped: `_compile_pattern`'s cache,
//! `re`'s compile cache, `_re_escape`'s cache. See docs/targets/fnmatch.md.

use std::cell::RefCell;

use pyo3::ffi;
use pyo3::prelude::*;

use rustique_core::paths::glob::Program;

use crate::accel::{self, Fingerprints, Prepare, Refusal, TargetSpec};
use crate::refuse;
use crate::targets::posixpath::rt::{self, O, Rt, bytes_view, exact_bytes, exact_str, str_view};

pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("fnmatch.fnmatch", &[(3, 14, "5986cba5802aba46"), (3, 15, "517d6a638fefa7e6")]),
    ("fnmatch.fnmatchcase", &[(3, 14, "83b7cb4758b87aeb"), (3, 15, "17934a096dd6f3aa")]),
    ("fnmatch.filter", &[(3, 14, "7bf973b1f53842dd"), (3, 15, "b5c0ce249e6a934f")]),
    ("fnmatch._compile_pattern", &[(3, 14, "79c0f2ffdc7d95ec"), (3, 15, "2e43dc16daf8b00d")]),
    ("fnmatch.translate", &[(3, 14, "c4fed7b889a0400f"), (3, 15, "13b3a820d251b35f")]),
    ("fnmatch._translate", &[(3, 14, "ca38f8a1c8f7e749"), (3, 15, "c567b67d0743b4de")]),
    ("fnmatch._join_translated_parts", &[(3, 14, "0fc493831425b153"), (3, 15, "4fd1d664ea2fd617")]),
    ("re.escape", &[(3, 14, "4112f39d484aa40d"), (3, 15, "c721420afec3ed91")]),
    ("re.compile", &[(3, 14, "84ef376a8f8d11c3"), (3, 15, "f744bd5c1c6c821f")]),
    ("re._compile", &[(3, 14, "ee2108f8069d004a"), (3, 15, "97249c2720f5b6a5")]),
    ("re._parser._parse", &[(3, 14, "c983b21f16b51ed5"), (3, 15, "9adcebe371aec858")]),
    ("re._parser._class_escape", &[(3, 14, "7fc2558ee17ca545"), (3, 15, "86b79b5449bf7d98")]),
    ("posixpath.normcase", &[(3, 14, "4430685c31fdf17c"), (3, 15, "b8ae21472a1996ce")]),
];

static RT_FNMATCH: Rt = Rt::new();
static RT_FNMATCHCASE: Rt = Rt::new();
static RT_FILTER: Rt = Rt::new();

macro_rules! target {
    ($stat:ident, $name:literal, $rt:ident, $prep:ident) => {
        // `..DEFAULT` keeps the spec additive when the registry grows fields.
        #[allow(clippy::needless_update)]
        pub static $stat: TargetSpec = TargetSpec {
            name: $name,
            group: "fnmatch",
            module: Some("fnmatch"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$rt.counters,
            prepare: $prep,
            jit_safe: false,
            checkable: true,
            ..TargetSpec::DEFAULT
        };
    };
}

target!(FNMATCH, "fnmatch.fnmatch", RT_FNMATCH, prep_fnmatch);
target!(FNMATCHCASE, "fnmatch.fnmatchcase", RT_FNMATCHCASE, prep_fnmatchcase);
target!(FILTER, "fnmatch.filter", RT_FILTER, prep_filter);

pub static TARGETS: &[&TargetSpec] = &[&FNMATCH, &FNMATCHCASE, &FILTER];

// ---------------------------------------------------------------------------------------
// Validation: what `_compile_pattern(pat)` and `os.path.normcase` read
// ---------------------------------------------------------------------------------------

/// Fingerprint and pin everything `fnmatchcase(name, pat)` reaches on a cache miss:
/// the whole `fnmatch` namespace (snapshot), the builtins it uses, `re.compile`,
/// `re._compile` and the parts of `re._parser` whose set semantics the port reproduces.
fn prep_compile(cx: &mut Prepare<'_>, rt: &'static Rt) -> Result<(), Refusal> {
    if !rt::init_str_layout(cx.py()) {
        return Err(refuse!("str layout not calibrated"));
    }
    let g = cx.module()?.dict();
    let cp = cx.get(&g, "_compile_pattern")?;
    let wrapped = cp.getattr("__wrapped__")?;
    rt::check_fp(cx, FPS, "fnmatch._compile_pattern", &wrapped)?;
    for name in ["translate", "_translate", "_join_translated_parts"] {
        let f = cx.get(&g, name)?;
        rt::check_fp(cx, FPS, &format!("fnmatch.{name}"), &f)?;
    }
    let re = cx.loaded_module("re")?;
    if !cx.get(&g, "re")?.is(&re) {
        return Err(refuse!("fnmatch.re is not the re module"));
    }
    let esc = cx.get(&g, "_re_escape")?.getattr("__wrapped__")?;
    let red = re.dict();
    if !esc.is(&cx.get(&red, "escape")?) {
        return Err(refuse!("fnmatch._re_escape does not wrap re.escape"));
    }
    rt::check_fp(cx, FPS, "re.escape", &esc)?;
    for name in ["compile", "_compile"] {
        let f = cx.get(&red, name)?;
        rt::check_fp(cx, FPS, &format!("re.{name}"), &f)?;
        cx.pin(&red, name, &rt.deps)?;
    }
    let parser = cx.loaded_module("re._parser")?.dict();
    for name in ["_parse", "_class_escape"] {
        let f = cx.get(&parser, name)?;
        rt::check_fp(cx, FPS, &format!("re._parser.{name}"), &f)?;
    }
    // Every fnmatch global (functions, `re`, `os`, `posixpath`, the caches) and the
    // absence of module globals shadowing builtins.
    cx.pin_dict(&g, &rt.deps)?;
    for name in ["isinstance", "str", "bytes", "len", "range"] {
        cx.pin_global(&g, name, &rt.deps)?;
    }
    Ok(())
}

/// `os.path.normcase` as fnmatch reaches it: `fnmatch.os` → `os.path` → `posixpath`,
/// whose `normcase` returns `os.fspath(s)`.
fn prep_normcase(cx: &mut Prepare<'_>, rt: &'static Rt) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let os = cx.loaded_module("os")?;
    let pp = cx.loaded_module("posixpath")?;
    if !cx.get(&g, "os")?.is(&os) || !cx.get(&g, "posixpath")?.is(&pp) {
        return Err(refuse!("fnmatch.os / fnmatch.posixpath are not the modules"));
    }
    let osd = os.dict();
    if !cx.get(&osd, "path")?.is(&pp) {
        return Err(refuse!("os.path is not posixpath"));
    }
    cx.pin(&osd, "path", &rt.deps)?;
    let ppd = pp.dict();
    let nc = cx.get(&ppd, "normcase")?;
    rt::check_fp(cx, FPS, "posixpath.normcase", &nc)?;
    cx.pin(&ppd, "normcase", &rt.deps)?;
    if !cx.get(&ppd, "os")?.is(&os) {
        return Err(refuse!("posixpath.os is not the os module"));
    }
    cx.pin(&ppd, "os", &rt.deps)?;
    let fspath = cx.get(&osd, "fspath")?;
    rt::check_c(cx, crate::targets::posixpath::FPS, "os.fspath", &fspath)?;
    cx.pin(&osd, "fspath", &rt.deps)?;
    Ok(())
}

fn prep_fnmatchcase(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = cx.get(&g, "fnmatchcase")?;
    rt::check_fp(cx, FPS, "fnmatch.fnmatchcase", &f)?;
    prep_compile(cx, &RT_FNMATCHCASE)?;
    cx.vectorcall(&f, k_fnmatchcase, &accel::FUNCTION_VECTORCALL)
}

fn prep_fnmatch(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = cx.get(&g, "fnmatch")?;
    rt::check_fp(cx, FPS, "fnmatch.fnmatch", &f)?;
    let fc = cx.get(&g, "fnmatchcase")?;
    rt::check_fp(cx, FPS, "fnmatch.fnmatchcase", &fc)?;
    prep_compile(cx, &RT_FNMATCH)?;
    prep_normcase(cx, &RT_FNMATCH)?;
    cx.vectorcall(&f, k_fnmatch, &accel::FUNCTION_VECTORCALL)
}

fn prep_filter(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = cx.get(&g, "filter")?;
    rt::check_fp(cx, FPS, "fnmatch.filter", &f)?;
    prep_compile(cx, &RT_FILTER)?;
    prep_normcase(cx, &RT_FILTER)?;
    cx.vectorcall(&f, k_filter, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

thread_local! {
    /// Reused token buffers (compilation allocates nothing for simple patterns after the
    /// first call on a thread).
    static PROG: RefCell<Program> = RefCell::new(Program::default());
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Str,
    Bytes,
}

/// The pattern's kind when it is in the exact subset (exact str or bytes).
#[inline(always)]
unsafe fn pat_kind(p: O) -> Option<Kind> {
    // SAFETY: live object.
    unsafe {
        if exact_str(p) {
            Some(Kind::Str)
        } else if exact_bytes(p) {
            Some(Kind::Bytes)
        } else {
            None
        }
    }
}

/// A name `re` matches without running Python code: any `str` for a str pattern (its
/// buffer is read directly), an exact `bytes` for a bytes pattern (a subclass could define
/// `__buffer__`).
#[inline(always)]
unsafe fn name_ok(k: Kind, n: O) -> bool {
    // SAFETY: live object.
    unsafe {
        match k {
            Kind::Str => ffi::PyUnicode_Check(n) != 0,
            Kind::Bytes => exact_bytes(n),
        }
    }
}

/// Compile `pat` into the thread's program; false when outside the reproduced grammar.
unsafe fn compile(k: Kind, pat: O, prog: &mut Program) -> bool {
    // SAFETY: live exact str/bytes.
    unsafe {
        match k {
            Kind::Str => crate::with_text!(str_view(pat).text, |s| prog.compile_into(s)).is_some(),
            Kind::Bytes => prog.compile_into(bytes_view(pat)).is_some(),
        }
    }
}

#[inline(always)]
unsafe fn matches(k: Kind, prog: &Program, name: O) -> bool {
    // SAFETY: `name_ok(k, name)`.
    unsafe {
        match k {
            Kind::Str => crate::with_text!(str_view(name).text, |s| prog.matches(s)),
            Kind::Bytes => prog.matches(bytes_view(name)),
        }
    }
}

#[inline(always)]
fn bool_obj(b: bool) -> O {
    // SAFETY: immortal singletons.
    unsafe { ffi::Py_NewRef(if b { ffi::Py_True() } else { ffi::Py_False() }) }
}

/// `fnmatchcase(name, pat)` / `fnmatch(name, pat)` (normcase is the identity on str/bytes).
unsafe fn match_one(rt: &Rt, func: O, name: O, pat: O) -> Option<O> {
    // SAFETY: live objects; attached thread.
    unsafe {
        let k = pat_kind(pat)?;
        if !name_ok(k, name) {
            return None;
        }
        if let Some(g) = rt::guards(rt, func) {
            return Some(g);
        }
        PROG.with(|p| {
            let mut prog = p.borrow_mut();
            if !compile(k, pat, &mut prog) {
                return None;
            }
            Some(bool_obj(matches(k, &prog, name)))
        })
    }
}

crate::paths_kernel!(k_fnmatchcase, RT_FNMATCHCASE, FNMATCHCASE, |func, args, nargsf, n, kw| {
    if !kw.is_null() || n != 2 { None } else { match_one(&RT_FNMATCHCASE, func, *args, *args.add(1)) }
});

crate::paths_kernel!(k_fnmatch, RT_FNMATCH, FNMATCH, |func, args, nargsf, n, kw| {
    // `os.path.normcase(name)` returns any str/bytes (subclasses included) unchanged; a
    // bytes subclass is excluded by `name_ok` (its buffer could run `__buffer__`).
    if !kw.is_null() || n != 2 { None } else { match_one(&RT_FNMATCH, func, *args, *args.add(1)) }
});

crate::paths_kernel!(k_filter, RT_FILTER, FILTER, |func, args, nargsf, n, kw| {
    if !kw.is_null() || n != 2 { None } else { filter(func, *args, *args.add(1)) }
});

/// `[name for name in names if match(name)]` over an exact list or tuple whose items all
/// match without running Python code (checked before anything else happens).
unsafe fn filter(func: O, names: O, pat: O) -> Option<O> {
    // SAFETY: live objects; attached thread.
    unsafe {
        let k = pat_kind(pat)?;
        let is_list = ffi::PyList_CheckExact(names) != 0;
        if !is_list && ffi::PyTuple_CheckExact(names) == 0 {
            return None;
        }
        if let Some(g) = rt::guards(&RT_FILTER, func) {
            return Some(g);
        }
        PROG.with(|p| {
            let mut prog = p.borrow_mut();
            if !compile(k, pat, &mut prog) {
                return None;
            }
            let py = Python::assume_attached();
            let seq = Bound::from_borrowed_ptr(py, names);
            // Hold the list's lock while reading it (free-threaded builds; a no-op with
            // the GIL). Matched items get a strong reference before the result list is
            // allocated (an allocation may run a finalizer that mutates `names`).
            let picked: Option<Vec<O>> = pyo3::sync::critical_section::with_critical_section(&seq, || {
                let len = if is_list { ffi::PyList_GET_SIZE(names) } else { ffi::PyTuple_GET_SIZE(names) } as usize;
                let item = |i: usize| {
                    if is_list {
                        ffi::PyList_GET_ITEM(names, i as ffi::Py_ssize_t)
                    } else {
                        ffi::PyTuple_GET_ITEM(names, i as ffi::Py_ssize_t)
                    }
                };
                if !(0..len).all(|i| name_ok(k, item(i))) {
                    return None;
                }
                let mut out = Vec::new();
                for i in 0..len {
                    let it = item(i);
                    if matches(k, &prog, it) {
                        out.push(ffi::Py_NewRef(it));
                    }
                }
                Some(out)
            });
            let picked = picked?;
            let list = ffi::PyList_New(picked.len() as ffi::Py_ssize_t);
            if list.is_null() {
                for o in picked {
                    ffi::Py_DECREF(o);
                }
                return Some(list);
            }
            for (i, o) in picked.into_iter().enumerate() {
                ffi::PyList_SET_ITEM(list, i as ffi::Py_ssize_t, o);
            }
            Some(list)
        })
    }
}

// ---------------------------------------------------------------------------------------
// Test helpers on `rustique._rustique._accel`
// ---------------------------------------------------------------------------------------

/// `None` when the pattern is outside the reproduced grammar (the kernel falls back),
/// else whether `name` matches: for differential tests of the port alone.
#[pyfunction]
fn _paths_fnmatch_port(name: &Bound<'_, PyAny>, pat: &Bound<'_, PyAny>) -> PyResult<Option<bool>> {
    rt::init_str_layout(name.py());
    // SAFETY: live objects; attached thread.
    unsafe {
        let Some(k) = pat_kind(pat.as_ptr()) else { return Ok(None) };
        if !name_ok(k, name.as_ptr()) {
            return Ok(None);
        }
        let mut prog = Program::default();
        if !compile(k, pat.as_ptr(), &mut prog) {
            return Ok(None);
        }
        Ok(Some(matches(k, &prog, name.as_ptr())))
    }
}

pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    accel.add_function(wrap_pyfunction!(_paths_fnmatch_port, accel)?)?;
    Ok(())
}
