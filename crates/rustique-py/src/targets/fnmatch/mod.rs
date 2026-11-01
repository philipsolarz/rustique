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

use crate::accel::{self, DepFlag, Fingerprints, GroupSpec, Prepare, Refusal, TargetSpec};
use crate::refuse;
use crate::targets::posixpath::rt::{self, O, Rt, Unit, bytes_view, exact_bytes, exact_str, str_view};

pub(crate) const FPS: &[(&str, Fingerprints)] = &[
    ("fnmatch.fnmatch", &[(3, 14, "dfa5c09b17d2a628"), (3, 15, "b91a7302efd00ab7")]),
    ("fnmatch.fnmatchcase", &[(3, 14, "49051a1ea4f359eb"), (3, 15, "5e74499978ac5f66")]),
    ("fnmatch.filter", &[(3, 14, "8ff7d447607dd2c2"), (3, 15, "d48457eb4862dc31")]),
    ("fnmatch._compile_pattern", &[(3, 14, "210606416e460057"), (3, 15, "482b0d2dd8fb8ec5")]),
    ("fnmatch.translate", &[(3, 14, "452f333f7d491a42"), (3, 15, "bae9e41617d16327")]),
    ("fnmatch._translate", &[(3, 14, "2974d59e2c491214"), (3, 15, "c5c5507425325b18")]),
    ("fnmatch._join_translated_parts", &[(3, 14, "2cc2a232f906617d"), (3, 15, "d29bdee58b96b3a6")]),
    ("re.escape", &[(3, 14, "3e4b805b83e2d9b8"), (3, 15, "a7b4140652fb66b2")]),
    ("re.compile", &[(3, 14, "a2ebcd8a548a953a"), (3, 15, "28f9681cad925e2b")]),
    ("re._compile", &[(3, 14, "638524c6d0b543cc"), (3, 15, "da7d49c39bd21137")]),
    ("re._parser._parse", &[(3, 14, "e5e3cd0d55da7193"), (3, 15, "aa2ecdd42bfc8f61")]),
    ("re._parser._class_escape", &[(3, 14, "ac571e89293380c0"), (3, 15, "5c7ab5562ea230eb")]),
    ("posixpath.normcase", &[(3, 14, "0c979efa01cde1ee"), (3, 15, "20a2c8c5e6360108")]),
];

/// The group's dependency flag, pinned once by [`SHARED`].
static DEPS: DepFlag = DepFlag::new();

static RT_FNMATCH: Rt = Rt::new(&DEPS);
static RT_FNMATCHCASE: Rt = Rt::new(&DEPS);
static RT_FILTER: Rt = Rt::new(&DEPS);

/// Declarations shared by the three targets (prepared once, kept while any is installed).
pub static SHARED: GroupSpec = GroupSpec { name: "fnmatch", prepare: shared };

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
            // Steady-state JIT cells measured within 1.05 (fallbacks suspend the target).
            jit_safe: true,
            checkable: true,
            shared: Some(&SHARED),
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

/// The group's shared declarations: everything `fnmatchcase(name, pat)` reaches on a
/// cache miss (the whole `fnmatch` namespace by snapshot, the builtins it uses,
/// `re.compile`, `re._compile`, the parts of `re._parser` whose set semantics the port
/// reproduces) and `os.path.normcase` as `fnmatch`/`filter` reach it (`fnmatch.os` →
/// `os.path` → `posixpath.normcase` → `os.fspath`): validated and pinned once.
fn shared(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if !rt::init_str_layout(py) {
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
        cx.pin(&red, name, &DEPS)?;
    }
    let parser = cx.loaded_module("re._parser")?.dict();
    for name in ["_parse", "_class_escape"] {
        let f = cx.get(&parser, name)?;
        rt::check_fp(cx, FPS, &format!("re._parser.{name}"), &f)?;
    }
    // Every fnmatch global (functions, `re`, `os`, `posixpath`, the caches) and the
    // absence of module globals shadowing builtins.
    cx.pin_dict(&g, &DEPS)?;
    for name in ["isinstance", "str", "bytes", "len", "range"] {
        cx.pin_global(&g, name, &DEPS)?;
    }
    // os.path.normcase
    let os = cx.loaded_module("os")?;
    let pp = cx.loaded_module("posixpath")?;
    if !cx.get(&g, "os")?.is(&os) || !cx.get(&g, "posixpath")?.is(&pp) {
        return Err(refuse!("fnmatch.os / fnmatch.posixpath are not the modules"));
    }
    let osd = os.dict();
    if !cx.get(&osd, "path")?.is(&pp) {
        return Err(refuse!("os.path is not posixpath"));
    }
    let ppd = pp.dict();
    rt::check_fp(cx, FPS, "posixpath.normcase", &cx.get(&ppd, "normcase")?)?;
    if !cx.get(&ppd, "os")?.is(&os) {
        return Err(refuse!("posixpath.os is not the os module"));
    }
    rt::check_c(cx, crate::targets::posixpath::FPS, "os.fspath", &cx.get(&osd, "fspath")?)?;
    cx.pin(&osd, "path", &DEPS)?;
    cx.pin(&ppd, "normcase", &DEPS)?;
    cx.pin(&ppd, "os", &DEPS)?;
    cx.pin(&osd, "fspath", &DEPS)?;
    Ok(())
}

/// A target: its own function's fingerprint and its vectorcall site.
fn prep_fn(cx: &mut Prepare<'_>, attr: &str, kernel: ffi::vectorcallfunc) -> Result<(), Refusal> {
    let g = cx.module()?.dict();
    let f = cx.get(&g, attr)?;
    rt::check_fp(cx, FPS, &format!("fnmatch.{attr}"), &f)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}

fn prep_fnmatchcase(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "fnmatchcase", k_fnmatchcase)
}

fn prep_fnmatch(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    // Its original calls the global `fnmatchcase` (covered by the namespace snapshot).
    rt::check_fp(cx, FPS, "fnmatch.fnmatchcase", &cx.module_attr("fnmatchcase")?)?;
    prep_fn(cx, "fnmatch", k_fnmatch)
}

fn prep_filter(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prep_fn(cx, "filter", k_filter)
}

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

/// Compiled patterns of this thread, keyed by content (CPython keeps the regexes in the
/// `lru_cache` of `_compile_pattern`; this cache is the kernel's own and private). Patterns
/// outside the reproduced grammar are cached as `None`.
struct Cached {
    bytes: bool,
    units: Vec<u32>,
    prog: Option<Program>,
}

const CACHE_SIZE: usize = 32;

#[derive(Default)]
struct Cache {
    entries: Vec<Cached>,
    next: usize,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

fn same<T: Unit>(units: &[u32], s: &[T]) -> bool {
    units.len() == s.len() && units.iter().zip(s).all(|(&a, &b)| a == b.cp())
}

/// Run `f` on the compiled program of `pat`; `None` when the pattern is outside the
/// reproduced grammar.
unsafe fn with_program<R>(k: Kind, pat: O, f: impl FnOnce(&Program) -> R) -> Option<R> {
    // SAFETY: live exact str/bytes; no Python code runs while the cache is borrowed.
    unsafe {
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            let bytes = k == Kind::Bytes;
            let hit = match k {
                Kind::Str => {
                    let t = str_view(pat).text;
                    c.entries.iter().position(|e| !e.bytes && crate::with_text!(t, |s| same(&e.units, s)))
                }
                Kind::Bytes => {
                    let b = bytes_view(pat);
                    c.entries.iter().position(|e| e.bytes && same(&e.units, b))
                }
            };
            let i = match hit {
                Some(i) => i,
                None => {
                    let mut prog = Program::default();
                    let ok = compile(k, pat, &mut prog);
                    let units: Vec<u32> = match k {
                        Kind::Str => crate::with_text!(str_view(pat).text, |s| s.iter().map(|u| u.cp()).collect()),
                        Kind::Bytes => bytes_view(pat).iter().map(|&u| u32::from(u)).collect(),
                    };
                    let entry = Cached { bytes, units, prog: ok.then_some(prog) };
                    if c.entries.len() < CACHE_SIZE {
                        c.entries.push(entry);
                        c.entries.len() - 1
                    } else {
                        let i = c.next;
                        c.entries[i] = entry;
                        c.next = (i + 1) % CACHE_SIZE;
                        i
                    }
                }
            };
            c.entries[i].prog.as_ref().map(f)
        })
    }
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
        with_program(k, pat, |prog| bool_obj(matches(k, prog, name)))
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
        with_program(k, pat, |prog| {
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
                    if matches(k, prog, it) {
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
        .flatten()
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
