//! Group `re` (docs/targets/re.md).
//!
//! - `re._compiler.compile`: CPython's regular-expression compiler (pure Python, ~20 µs per
//!   pattern; 7.5% of real programs' import time). The kernel compiles in Rust
//!   (`rustique_core::re`, a port of `re._parser` + `re._compiler`) and calls CPython's own
//!   `_sre.compile` with exactly the arguments the original would pass. Patterns that raise,
//!   warn, use `LOCALE`/`DEBUG`/`\N{...}`, non-ASCII group names or deep nesting fall back.
//! - `re.match` (`re.prefixmatch`), `re.fullmatch`, `re.search`, `re.sub`, `re.subn`,
//!   `re.split`, `re.findall`, `re.finditer`, `re.compile`: the `_compile` cache hit on
//!   CPython's own `_cache2`, then the same `Pattern` method. Misses, callable replacements,
//!   positional `count`/`maxsplit` and unusual argument types fall back.
//!
//! Private state skipped: none (CPython's caches are used as they are).

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule, PyType};

use crate::accel::{self, Counters, DepFlag, GroupSpec, Prepare, Refusal, TargetSpec, Tools};
use crate::refuse;

mod compile;
mod fps;
mod funcs;
mod state;

use funcs::Method;
use state::{O, Tag};

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

/// The compiler's dependencies (`re._compiler`/`re._parser`/`_sre` namespaces, builtins).
pub(crate) static DEPS_COMPILE: DepFlag = DepFlag::new();
/// The module functions' dependencies (`re` globals, builtins, `re._parser` for templates).
pub(crate) static DEPS_FUNCS: DepFlag = DepFlag::new();

pub(crate) static COMPILE_COUNTERS: Counters = Counters::new();
pub(crate) static COMPILE_TOOLS: Tools = Tools::new();

/// Per-target runtime of a module function.
pub(crate) struct FuncRt {
    pub counters: Counters,
    pub tools: Tools,
    /// Check mode re-runs the original (results compared structurally: not for functions
    /// returning `Match` objects or iterators).
    pub checkable: bool,
}

impl FuncRt {
    const fn new(checkable: bool) -> Self {
        FuncRt { counters: Counters::new(), tools: Tools::new(), checkable }
    }
}

/// Shared validation of the compiler target.
pub static SHARED_COMPILE: GroupSpec = GroupSpec { name: "re.compiler", prepare: shared_compile };
/// Shared validation of the module functions.
pub static SHARED_FUNCS: GroupSpec = GroupSpec { name: "re.functions", prepare: shared_funcs };

#[allow(clippy::needless_update)]
pub static TARGET_COMPILE: TargetSpec = TargetSpec {
    name: "re._compiler.compile",
    group: "re",
    module: Some("re"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &COMPILE_COUNTERS,
    prepare: prep_compile,
    // A fallback costs <6% of the cheapest compile (an error pattern), also under the JIT.
    jit_safe: true,
    checkable: true,
    shared: Some(&SHARED_COMPILE),
    // A hit saves 25k-1M Ir, a fallback costs ~0.3-2.7k: suspend only on a stream of
    // declined patterns.
    hit_credit: 255,
    fallback_debit: 1,
    release: Some(release),
    ..TargetSpec::DEFAULT
};

macro_rules! func_target {
    ($spec:ident, $rt:ident, $kernel:ident, $prep:ident, $name:literal, $attr:literal, $method:expr, $checkable:expr) => {
        static $rt: FuncRt = FuncRt::new($checkable);
        unsafe extern "C" fn $kernel(f: O, args: *const O, nargsf: usize, kw: O) -> O {
            // SAFETY: installed as this function's vectorcall by the registry.
            unsafe { funcs::kernel(&$rt, &$spec, $method, f, args, nargsf, kw) }
        }
        fn $prep(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
            prep_func(py, cx, $attr, $kernel)
        }
        #[allow(clippy::needless_update)]
        pub static $spec: TargetSpec = TargetSpec {
            name: $name,
            group: "re",
            module: Some("re"),
            versions: VERSIONS,
            free_threaded: true,
            counters: &$rt.counters,
            prepare: $prep,
            // Fallbacks (callable `sub` replacements, misses) lose the caller's inlined frame
            // push (+250-350 Ir under the JIT): refuse there until measured.
            jit_safe: false,
            checkable: $checkable,
            shared: Some(&SHARED_FUNCS),
            // A hit saves ~1,100 Ir, a fallback costs ~250-340 (docs/targets/re.md): stay
            // installed while hits are at least a quarter of the calls.
            hit_credit: 3,
            fallback_debit: 1,
            release: Some(release),
            ..TargetSpec::DEFAULT
        };
    };
}

func_target!(MATCH, RT_MATCH, k_match, prep_match, "re.match", "match", Method::Match, false);
func_target!(
    FULLMATCH,
    RT_FULLMATCH,
    k_fullmatch,
    prep_fullmatch,
    "re.fullmatch",
    "fullmatch",
    Method::Fullmatch,
    false
);
func_target!(SEARCH, RT_SEARCH, k_search, prep_search, "re.search", "search", Method::Search, false);
func_target!(SUB, RT_SUB, k_sub, prep_sub, "re.sub", "sub", Method::Sub, true);
func_target!(SUBN, RT_SUBN, k_subn, prep_subn, "re.subn", "subn", Method::Subn, true);
func_target!(SPLIT, RT_SPLIT, k_split, prep_split, "re.split", "split", Method::Split, true);
func_target!(FINDALL, RT_FINDALL, k_findall, prep_findall, "re.findall", "findall", Method::Findall, true);
func_target!(FINDITER, RT_FINDITER, k_finditer, prep_finditer, "re.finditer", "finditer", Method::Finditer, false);
func_target!(COMPILE_FN, RT_COMPILE_FN, k_compile_fn, prep_compile_fn, "re.compile", "compile", Method::Compile, true);

pub static TARGETS: &[&TargetSpec] =
    &[&TARGET_COMPILE, &MATCH, &FULLMATCH, &SEARCH, &SUB, &SUBN, &SPLIT, &FINDALL, &FINDITER, &COMPILE_FN];

/// Python-visible test helpers.
pub fn register(accel: &Bound<'_, PyModule>) -> PyResult<()> {
    accel.add_function(wrap_pyfunction!(compile::_re_compile_args, accel)?)?;
    accel.add_function(wrap_pyfunction!(compile::_re_template_ok, accel)?)?;
    Ok(())
}

fn release(_py: Python<'_>) {
    state::release();
}

// ---------------------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------------------

/// `d[name]` with a plain dict read (no interning, no heap allocation: validation runs at a
/// first call), refusing when absent.
fn dget<'py>(py: Python<'py>, d: &Bound<'py, PyDict>, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    let mut buf = [0u8; 64];
    if name.len() >= buf.len() {
        return Err(refuse!("name too long: {name}"));
    }
    buf[..name.len()].copy_from_slice(name.as_bytes());
    // SAFETY: attached thread; an exact dict and a NUL-terminated ASCII key: no Python code
    // runs.
    let v = unsafe { ffi::PyDict_GetItemString(d.as_ptr(), buf.as_ptr().cast()) };
    if v.is_null() {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return Err(refuse!(Deps: "missing {name}"));
    }
    // SAFETY: a borrowed reference kept alive by the dict.
    Ok(unsafe { Bound::from_borrowed_ptr(py, v) })
}

/// The `int` value of `v` (an `int` or `_NamedIntConstant`), if it fits `u64`.
fn int_value(v: &Bound<'_, PyAny>) -> Option<u64> {
    // SAFETY: attached thread; a live object.
    unsafe {
        if ffi::PyLong_Check(v.as_ptr()) == 0 {
            return None;
        }
        let x = ffi::PyLong_AsUnsignedLongLong(v.as_ptr());
        if x == u64::MAX && !ffi::PyErr_Occurred().is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(x)
    }
}

fn int_of(py: Python<'_>, d: &Bound<'_, PyDict>, name: &str) -> Result<u64, Refusal> {
    int_value(&dget(py, d, name)?).ok_or_else(|| refuse!(Deps: "{name} is not a small int"))
}

fn as_type<'py>(o: Bound<'py, PyAny>, what: &str) -> Result<Bound<'py, PyType>, Refusal> {
    o.cast_into::<PyType>().map_err(|_| refuse!(Deps: "{what} is not a class"))
}

/// The version tag of `ty` (refuse when none can be assigned).
fn tag_of(ty: &Bound<'_, PyType>, what: &str) -> Result<Tag, Refusal> {
    // SAFETY: attached thread; a live type.
    unsafe { Tag::take(ty.as_type_ptr()) }.ok_or_else(|| refuse!(Layout: "{what}: no type version tag"))
}

/// A strong reference for a state's `refs`.
fn keep(o: &Bound<'_, PyAny>) -> O {
    o.clone().into_ptr()
}

/// The builtins the originals look up on their success path, pinned with `flag` through
/// `globals` (absent from the module, present in builtins).
fn pin_builtins(
    cx: &mut Prepare<'_>,
    globals: &Bound<'_, PyDict>,
    names: &[&str],
    flag: &'static DepFlag,
) -> Result<(), Refusal> {
    for n in names {
        cx.pin_global(globals, n, flag)?;
    }
    Ok(())
}

/// The inline (format v2) fingerprints of `name`, the fallback where the v3 data has no entry.
fn v2_of(name: &str) -> crate::accel::Fingerprints {
    fps::FPS.iter().find(|(n, _)| *n == name).map_or(&[], |(_, f)| *f)
}

/// First-call validation: every original against the recorded v3 fingerprints of `lazy.owner`
/// (`accel::fingerprint::check`; the inline v2 ones where the data has no entry).
///
/// # Safety
/// Attached thread; the objects are alive (held by the state's references).
unsafe fn check_fingerprints(lazy: &state::Lazy) -> Result<(), String> {
    // SAFETY: per the caller.
    let py = unsafe { Python::assume_attached() };
    for &(name, obj) in &lazy.fps {
        // SAFETY: a live object held by the state.
        let obj = unsafe { Bound::from_borrowed_ptr(py, obj) };
        if !crate::accel::fingerprint::check(lazy.owner, &obj, v2_of(name)) {
            return Err(format!("fingerprint: {name} differs from the recorded code"));
        }
    }
    Ok(())
}

/// Record mode (`benches/accel/record_patch.py`): report every original through `Prepare`,
/// so the v3 data of `owner` (the `GroupSpec`) can be recorded per patch release; then drop
/// the references (`true`: the caller returns without publishing a state).
fn record_fingerprints(cx: &mut Prepare<'_>, fps: &[(&'static str, O)]) -> Result<bool, Refusal> {
    if !cx.record {
        return Ok(false);
    }
    let py = cx.py();
    let mut r = Ok(());
    for &(name, obj) in fps {
        // SAFETY: a live object held by the caller's references.
        let obj = unsafe { Bound::from_borrowed_ptr(py, obj) };
        if r.is_ok() {
            r = cx.fingerprint_code(&obj, v2_of(name));
        }
    }
    for &(_, obj) in fps {
        // SAFETY: the caller's strong references, not used after this.
        unsafe { ffi::Py_DECREF(obj) };
    }
    r.map(|()| true)
}

// ---------------------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------------------

/// The originals of the compiler path: (fingerprint name, where: 0 `re._compiler` function,
/// 1 `re._parser` function, else a `re._parser` class), attribute, property getter.
const COMPILE_ORIGINALS: &[(&str, &str, &str, bool)] = &[
    ("re._compiler.compile", "", "compile", false),
    ("re._compiler.isstring", "", "isstring", false),
    ("re._compiler._code", "", "_code", false),
    ("re._compiler._compile_info", "", "_compile_info", false),
    ("re._compiler._compile", "", "_compile", false),
    ("re._compiler._compile_charset", "", "_compile_charset", false),
    ("re._compiler._optimize_charset", "", "_optimize_charset", false),
    ("re._compiler._mk_bitmap", "", "_mk_bitmap", false),
    ("re._compiler._bytes_to_codes", "", "_bytes_to_codes", false),
    ("re._compiler._simple", "", "_simple", false),
    ("re._compiler._generate_overlap_table", "", "_generate_overlap_table", false),
    ("re._compiler._get_iscased", "", "_get_iscased", false),
    ("re._compiler._get_literal_prefix", "", "_get_literal_prefix", false),
    ("re._compiler._get_charset_prefix", "", "_get_charset_prefix", false),
    ("re._compiler._combine_flags", "", "_combine_flags", false),
    ("re._parser.parse", "-", "parse", false),
    ("re._parser.fix_flags", "-", "fix_flags", false),
    ("re._parser._parse_sub", "-", "_parse_sub", false),
    ("re._parser._parse", "-", "_parse", false),
    ("re._parser._parse_flags", "-", "_parse_flags", false),
    ("re._parser._escape", "-", "_escape", false),
    ("re._parser._class_escape", "-", "_class_escape", false),
    ("re._parser._uniq", "-", "_uniq", false),
    ("re._parser.Tokenizer.__init__", "Tokenizer", "__init__", false),
    ("re._parser.Tokenizer.__next", "Tokenizer", "_Tokenizer__next", false),
    ("re._parser.Tokenizer.match", "Tokenizer", "match", false),
    ("re._parser.Tokenizer.get", "Tokenizer", "get", false),
    ("re._parser.Tokenizer.getwhile", "Tokenizer", "getwhile", false),
    ("re._parser.Tokenizer.getuntil", "Tokenizer", "getuntil", false),
    ("re._parser.Tokenizer.tell", "Tokenizer", "tell", false),
    ("re._parser.Tokenizer.seek", "Tokenizer", "seek", false),
    ("re._parser.Tokenizer.checkgroupname", "Tokenizer", "checkgroupname", false),
    ("re._parser.Tokenizer.pos", "Tokenizer", "pos", true),
    ("re._parser.State.__init__", "State", "__init__", false),
    ("re._parser.State.opengroup", "State", "opengroup", false),
    ("re._parser.State.closegroup", "State", "closegroup", false),
    ("re._parser.State.checkgroup", "State", "checkgroup", false),
    ("re._parser.State.checklookbehindgroup", "State", "checklookbehindgroup", false),
    ("re._parser.State.groups", "State", "groups", true),
    ("re._parser.SubPattern.__init__", "SubPattern", "__init__", false),
    ("re._parser.SubPattern.__len__", "SubPattern", "__len__", false),
    ("re._parser.SubPattern.__delitem__", "SubPattern", "__delitem__", false),
    ("re._parser.SubPattern.__getitem__", "SubPattern", "__getitem__", false),
    ("re._parser.SubPattern.__setitem__", "SubPattern", "__setitem__", false),
    ("re._parser.SubPattern.insert", "SubPattern", "insert", false),
    ("re._parser.SubPattern.append", "SubPattern", "append", false),
    ("re._parser.SubPattern.getwidth", "SubPattern", "getwidth", false),
];

/// Deferral of the compiler's validation: the first `COMPILE_VALIDATE_AFTER - 1` compiles of a
/// process run CPython's compiler (a module's import typically compiles 0–3 patterns; the
/// validation, ~0.5M instructions, pays off from a handful of compiles on).
const COMPILE_VALIDATE_AFTER: u32 = 4;

/// What `re._compiler.compile` reaches on its success path. At install (cheap): the modules'
/// identities, `_sre`'s code-word size and limits, the version tags of the parser's classes,
/// the pins (snapshots of the `re._compiler`, `re._parser` and `_sre` namespaces, the
/// builtins they use). On the first call ([`validate_compile`]): the fingerprints of every
/// function and method of the parser and compiler, `re._constants`' values against the port's
/// table, the `_EXTRA_CASES` table.
fn shared_compile(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use rustique_core::re::consts as k;
    if cx.arming() {
        // Armed until the first call (accel's first-call validation): the targets only
        // declare their sites now; everything below runs at the full prepare.
        return Ok(());
    }
    let red = cx.module()?.dict();
    let co_m = cx.loaded_module("re._compiler")?;
    let pa_m = cx.loaded_module("re._parser")?;
    let cn_m = cx.loaded_module("re._constants")?;
    let sre_m = cx.loaded_module("_sre")?;
    let (co, pa, cn, sre) = (co_m.dict(), pa_m.dict(), cn_m.dict(), sre_m.dict());
    if !dget(py, &red, "_compiler")?.is(&co_m)
        || !dget(py, &co, "_parser")?.is(&pa_m)
        || !dget(py, &co, "_sre")?.is(&sre_m)
    {
        return Err(refuse!(Deps: "re._compiler's modules are not the imported ones"));
    }
    // _sre: the code-word size and limits the port assumes
    if int_of(py, &sre, "MAGIC")? != k::MAGIC as u64 || int_of(py, &cn, "MAGIC")? != k::MAGIC as u64 {
        return Err(refuse!(Version: "_sre.MAGIC differs from the port's"));
    }
    if int_of(py, &sre, "CODESIZE")? != 4 {
        return Err(refuse!(Platform: "_sre.CODESIZE is not 4"));
    }
    let maxrepeat = int_of(py, &sre, "MAXREPEAT")?;
    let maxgroups = int_of(py, &sre, "MAXGROUPS")?;
    if maxrepeat > u32::MAX as u64 || maxgroups > u32::MAX as u64 {
        return Err(refuse!(Platform: "_sre limits out of range"));
    }
    let mut refs: Vec<O> = Vec::with_capacity(COMPILE_ORIGINALS.len() + 8);
    let mut fps: Vec<(&'static str, O)> = Vec::with_capacity(COMPILE_ORIGINALS.len());
    let mut tags = Vec::with_capacity(4);
    let mut class: Option<(&str, Bound<'_, PyDict>)> = None;
    for &(name, owner, attr, property) in COMPILE_ORIGINALS {
        let f = match owner {
            "" => dget(py, &co, attr)?,
            "-" => dget(py, &pa, attr)?,
            cls => {
                if class.as_ref().is_none_or(|(c, _)| *c != cls) {
                    let ty = as_type(dget(py, &pa, cls)?, cls)?;
                    tags.push(tag_of(&ty, cls)?);
                    class = Some((cls, crate::accel::prepare::type_dict(&ty)?));
                }
                let td = &class.as_ref().expect("set above").1;
                let f = dget(py, td, attr)?;
                // a property: its getter (a C member of the property object)
                if property { f.getattr(pyo3::intern!(py, "fget"))? } else { f }
            }
        };
        fps.push((name, keep(&f)));
    }
    if record_fingerprints(cx, &fps)? {
        return Ok(());
    }
    let nic = as_type(dget(py, &cn, "_NamedIntConstant")?, "_NamedIntConstant")?;
    tags.push(tag_of(&nic, "_NamedIntConstant")?);
    let extra = dget(py, &co, "_EXTRA_CASES")?;
    if !extra.is_exact_instance_of::<PyDict>() {
        return Err(refuse!(Deps: "_EXTRA_CASES is not a dict"));
    }
    let sre_compile = dget(py, &sre, "compile")?;
    // pins: the compiler's and parser's namespaces as validated (any rebinding, addition or
    // deletion falls back); of `_sre`, what the compiler reads at call time
    cx.pin_dict(&co, &DEPS_COMPILE)?;
    cx.pin_dict(&pa, &DEPS_COMPILE)?;
    for name in ["compile", "unicode_iscased", "unicode_tolower", "ascii_iscased", "ascii_tolower", "CODESIZE"] {
        cx.pin(&sre, name, &DEPS_COMPILE)?;
    }
    let builtins = [
        "isinstance",
        "str",
        "bytes",
        "len",
        "range",
        "IndexError",
        "int",
        "chr",
        "ord",
        "min",
        "list",
        "dict",
        "slice",
        "tuple",
        "any",
        "map",
        "bytearray",
        "memoryview",
        "ValueError",
    ];
    // The snapshot pins already cover "absent from the module" (an added global shadowing a
    // builtin deviates); pin the builtins' own entries once.
    let bco = cx.builtins_of(&co)?;
    if !bco.is(&cx.builtins_of(&pa)?) {
        return Err(refuse!(Deps: "re._parser and re._compiler see different builtins"));
    }
    for n in builtins {
        cx.pin(&bco, n, &DEPS_COMPILE)?;
    }
    refs.push(keep(&sre_compile));
    refs.push(keep(cn.as_any()));
    refs.push(keep(&extra));
    refs.extend(fps.iter().map(|(_, o)| *o));
    refs.extend(tags.iter().map(|t| {
        // SAFETY: attached thread; the classes stay referenced by the state.
        unsafe { ffi::Py_NewRef(t.ty.cast::<ffi::PyObject>()) }
    }));
    state::publish_compile(state::Compile {
        sre_compile: sre_compile.as_ptr(),
        maxrepeat: maxrepeat as u32,
        maxgroups: maxgroups as u32,
        extra_table: std::sync::OnceLock::new(),
        constants: cn.as_ptr(),
        extra: extra.as_ptr(),
        lazy: state::Lazy::new(SHARED_COMPILE.name, fps),
        tags,
        refs,
    });
    Ok(())
}

/// The compiler's deferred validation (see [`shared_compile`]): the fingerprints of the
/// parser's and compiler's originals and `re._constants`' values against the port's table.
/// `_EXTRA_CASES` is checked when a compile first needs it ([`extra_table`]).
///
/// # Safety
/// Attached thread.
pub(crate) unsafe fn validate_compile(st: &state::Compile) -> Result<(), String> {
    use rustique_core::re::consts as k;
    // SAFETY: per the caller.
    unsafe { check_fingerprints(&st.lazy)? };
    // SAFETY: attached thread; the state holds the dict.
    let py = unsafe { Python::assume_attached() };
    // SAFETY: a live dict held by the state.
    let cn = unsafe { Bound::from_borrowed_ptr(py, st.constants) };
    let Ok(cn) = cn.cast_into::<PyDict>() else {
        return Err("deps: re._constants has no dict".into());
    };
    let named = k::OPCODE_NAMES.iter().chain(k::ATCODE_NAMES).chain(k::CHCODE_NAMES).chain(k::FLAG_NAMES);
    let limits = [("MAXREPEAT", st.maxrepeat), ("MAXGROUPS", st.maxgroups)];
    for (name, v) in named.copied().chain(limits) {
        if dget(py, &cn, name).ok().and_then(|o| int_value(&o)) != Some(v as u64) {
            return Err(format!("version: re._constants.{name} differs from the port's"));
        }
    }
    Ok(())
}

/// The compiler target is validated (running the deferred validation at its
/// [`COMPILE_VALIDATE_AFTER`]-th call).
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub(crate) unsafe fn compile_ready(st: &state::Compile) -> bool {
    st.lazy.ready(
        COMPILE_VALIDATE_AFTER,
        // SAFETY: per the caller.
        || unsafe { validate_compile(st) },
        |r| crate::accel::degrade(&TARGET_COMPILE, r),
    )
}

/// [`compile_ready`], validating now (the test helper).
///
/// # Safety
/// Attached thread.
pub(crate) unsafe fn compile_ready_now(st: &state::Compile) -> bool {
    st.lazy.force(
        // SAFETY: per the caller.
        || unsafe { validate_compile(st) },
        |r| crate::accel::degrade(&TARGET_COMPILE, r),
    )
}

/// `_EXTRA_CASES`, read and checked against its recorded content hash the first time a
/// compile needs it (Unicode IGNORECASE); `None` if it differs (the target degrades).
///
/// # Safety
/// Attached thread.
pub(crate) unsafe fn extra_table(st: &state::Compile) -> Option<&rustique_core::re::ExtraCases> {
    let r = st.extra_table.get_or_init(|| {
        // SAFETY: attached thread; a live dict held by the state.
        let py = unsafe { Python::assume_attached() };
        // SAFETY: as above.
        let extra = unsafe { Bound::from_borrowed_ptr(py, st.extra) };
        let extra = extra.cast_into::<PyDict>().map_err(|_| "deps: _EXTRA_CASES is not a dict".to_string())?;
        let mut pairs = Vec::with_capacity(extra.len());
        for (key, v) in extra.iter() {
            let key: u32 = key.extract().map_err(|_| "deps: _EXTRA_CASES key".to_string())?;
            let v: Vec<u32> = v.extract().map_err(|_| "deps: _EXTRA_CASES value".to_string())?;
            pairs.push((key, v));
        }
        let hash = extra_cases_hash(&pairs);
        let (major, minor) = crate::accel::version::running();
        let want = crate::accel::fingerprint::expected_for(fps::EXTRA_CASES, major, minor);
        if want != Some(hash.as_str()) {
            return Err(format!("fingerprint: re._casefix._EXTRA_CASES: {hash} != recorded {want:?}"));
        }
        Ok(rustique_core::re::ExtraCases::new(pairs))
    });
    match r {
        Ok(t) => Some(t),
        Err(reason) => {
            crate::accel::degrade(&TARGET_COMPILE, reason);
            None
        }
    }
}

/// FNV-1a 64 over the little-endian u32 words `k, len(v), *v` of the sorted items
/// (`benches/targets/re_record.py` computes the same).
fn extra_cases_hash(pairs: &[(u32, Vec<u32>)]) -> String {
    let mut sorted: Vec<&(u32, Vec<u32>)> = pairs.iter().collect();
    sorted.sort_by_key(|p| p.0);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut word = |w: u32| {
        for b in w.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (k, v) in sorted {
        word(*k);
        word(v.len() as u32);
        for &x in v {
            word(x);
        }
    }
    format!("{h:016x}")
}

fn prep_compile(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let co = cx.loaded_module("re._compiler")?.dict();
    let f = dget(py, &co, "compile")?;
    cx.vectorcall(&f, compile::k_compile, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// The module functions
// ---------------------------------------------------------------------------------------

/// What the module functions read on a cache hit (`re` globals, `RegexFlag` and the
/// `Pattern` type) and on a template `sub` (`parse_template`, `_compile_template`). The
/// fingerprints are checked on the first call ([`funcs_ready`]).
fn shared_funcs(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if cx.arming() {
        return Ok(()); // see `shared_compile`
    }
    let red = cx.module()?.dict();
    let pa_m = cx.loaded_module("re._parser")?;
    let pa = pa_m.dict();
    let sre = cx.loaded_module("_sre")?.dict();
    let minor = cx.version().1;
    let mut fps: Vec<(&'static str, O)> = Vec::with_capacity(19);
    let match_name = if minor >= 15 { "prefixmatch" } else { "match" };
    for (fp, name) in [
        ("re.match", match_name),
        ("re.fullmatch", "fullmatch"),
        ("re.search", "search"),
        ("re.sub", "sub"),
        ("re.subn", "subn"),
        ("re.split", "split"),
        ("re.findall", "findall"),
        ("re.finditer", "finditer"),
        ("re.compile", "compile"),
        ("re._compile", "_compile"),
    ] {
        fps.push((fp, keep(&dget(py, &red, name)?)));
    }
    if minor >= 15 && !dget(py, &red, "match")?.is(&dget(py, &red, "prefixmatch")?) {
        return Err(refuse!(Deps: "re.match is not re.prefixmatch"));
    }
    let ct = dget(py, &red, "_compile_template")?;
    fps.push(("re._compile_template", keep(&ct.getattr("__wrapped__")?)));
    fps.push(("re._parser.parse_template", keep(&dget(py, &pa, "parse_template")?)));
    if !dget(py, &red, "_parser")?.is(&pa_m) {
        return Err(refuse!(Deps: "re._parser is not the imported module"));
    }
    let tok = as_type(dget(py, &pa, "Tokenizer")?, "Tokenizer")?;
    let td = crate::accel::prepare::type_dict(&tok)?;
    for (fp, m) in [
        ("re._parser.Tokenizer.__init__", "__init__"),
        ("re._parser.Tokenizer.__next", "_Tokenizer__next"),
        ("re._parser.Tokenizer.match", "match"),
        ("re._parser.Tokenizer.get", "get"),
        ("re._parser.Tokenizer.getuntil", "getuntil"),
        ("re._parser.Tokenizer.checkgroupname", "checkgroupname"),
    ] {
        fps.push((fp, keep(&dget(py, &td, m)?)));
    }
    if record_fingerprints(cx, &fps)? {
        return Ok(());
    }
    // classes: Pattern (immutable C type), RegexFlag (+ its metaclass), _ZeroSentinel
    let pattern_t = as_type(dget(py, &red, "Pattern")?, "re.Pattern")?;
    // SAFETY: attached thread; type flags of a live type.
    if unsafe { ffi::PyType_GetFlags(pattern_t.as_type_ptr()) } & ffi::Py_TPFLAGS_IMMUTABLETYPE == 0 {
        return Err(refuse!(Deps: "re.Pattern is not immutable"));
    }
    let regexflag = as_type(dget(py, &red, "RegexFlag")?, "re.RegexFlag")?;
    let enumtype = regexflag.get_type();
    let zero = dget(py, &red, "_zero_sentinel")?;
    let zero_t = zero.get_type();
    // `flags.value` is `self._value_`: plain attribute lookup (no descriptor named
    // `_value_` in the MRO, generic getattr)
    // SAFETY: attached thread; live types.
    unsafe {
        let rf = regexflag.as_type_ptr();
        if (*rf).tp_getattro.map(|f| f as usize) != Some(ffi::PyObject_GenericGetAttr as *const () as usize) {
            return Err(refuse!(Deps: "RegexFlag has a custom __getattribute__"));
        }
        let name = pyo3::intern!(py, "_value_");
        if !state::_PyType_Lookup(rf, name.as_ptr()).is_null() {
            return Err(refuse!(Deps: "RegexFlag defines _value_ on a class"));
        }
    }
    let tags =
        vec![tag_of(&regexflag, "RegexFlag")?, tag_of(&enumtype, "EnumType")?, tag_of(&zero_t, "_ZeroSentinel")?];
    // Pattern methods called on a hit (the C method descriptors of the immutable type)
    let pd = crate::accel::prepare::type_dict(&pattern_t)?;
    let methods = [
        dget(py, &pd, match_name)?,
        dget(py, &pd, "fullmatch")?,
        dget(py, &pd, "search")?,
        dget(py, &pd, "findall")?,
        dget(py, &pd, "finditer")?,
        dget(py, &pd, "sub")?,
        dget(py, &pd, "subn")?,
        dget(py, &pd, "split")?,
        // `re.compile` calls no method: any live object will do
        pattern_t.clone().into_any(),
    ];
    // pins: the `re` globals the hit path and the template path read, what `parse_template`
    // and `_compile_template` read from `re._parser` and `_sre`, the builtins
    for name in
        ["_compile", "_cache2", "RegexFlag", "_zero_sentinel", "Pattern", "_compile_template", "_parser", "_sre"]
    {
        cx.pin(&red, name, &DEPS_FUNCS)?;
    }
    pin_builtins(cx, &red, &["isinstance", "type"], &DEPS_FUNCS)?;
    for name in ["parse_template", "Tokenizer", "MAXGROUPS", "OCTDIGITS", "DIGITS", "ESCAPES", "ASCIILETTERS"] {
        cx.pin(&pa, name, &DEPS_FUNCS)?;
    }
    pin_builtins(cx, &pa, &["chr", "int", "len", "KeyError", "isinstance", "str"], &DEPS_FUNCS)?;
    cx.pin(&sre, "template", &DEPS_FUNCS)?;
    cx.pin(&sre, "compile", &DEPS_FUNCS)?;
    let cache2 = dget(py, &red, "_cache2")?;
    if !cache2.is_exact_instance_of::<PyDict>() {
        return Err(refuse!(Deps: "re._cache2 is not a dict"));
    }
    let names = ["flags", "count", "maxsplit", "_value_", "groups", "pattern"]
        .map(|n| crate::accel::prepare::interned(py, n).into_any());
    let mut refs: Vec<O> = vec![keep(&cache2), keep(&pattern_t), keep(&regexflag), keep(&zero)];
    refs.extend(fps.iter().map(|(_, o)| *o));
    refs.extend(tags.iter().map(|t| {
        // SAFETY: attached thread; live classes.
        unsafe { ffi::Py_NewRef(t.ty.cast::<ffi::PyObject>()) }
    }));
    refs.extend(methods.iter().map(keep));
    refs.extend(names.iter().map(keep));
    state::publish_funcs(state::Funcs {
        cache2: cache2.as_ptr(),
        pattern_t: pattern_t.as_type_ptr(),
        regexflag_t: regexflag.as_type_ptr(),
        zero: zero.as_ptr(),
        tags,
        methods: methods.map(|m| m.as_ptr()),
        lazy: state::Lazy::new(SHARED_FUNCS.name, fps),
        n_flags: names[0].as_ptr(),
        n_count: names[1].as_ptr(),
        n_maxsplit: names[2].as_ptr(),
        n_value_: names[3].as_ptr(),
        n_groups: names[4].as_ptr(),
        n_pattern: names[5].as_ptr(),
        refs,
    });
    Ok(())
}

/// The module functions are validated (the fingerprints, checked at the first cache *hit*: a
/// miss falls back before anything depends on them, so module-level `re.compile` calls at
/// import never pay it; a failure degrades all of them).
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub(crate) unsafe fn funcs_ready(st: &state::Funcs) -> bool {
    st.lazy.ready(
        1,
        // SAFETY: per the caller.
        || unsafe { check_fingerprints(&st.lazy) },
        |r| {
            for spec in &TARGETS[1..] {
                crate::accel::degrade(spec, r);
            }
        },
    )
}

fn prep_func(py: Python<'_>, cx: &mut Prepare<'_>, attr: &str, kernel: ffi::vectorcallfunc) -> Result<(), Refusal> {
    let red = cx.module()?.dict();
    let attr = if attr == "match" && cx.version().1 >= 15 { "prefixmatch" } else { attr };
    let f = dget(py, &red, attr)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}
