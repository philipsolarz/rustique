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

/// `d[name]` with a plain dict read (no interning: install cost), refusing when absent.
fn dget<'py>(py: Python<'py>, d: &Bound<'py, PyDict>, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    let key = std::ffi::CString::new(name).map_err(|_| refuse!("bad name {name}"))?;
    // SAFETY: attached thread; an exact dict and a str key: no Python code runs.
    let v = unsafe { ffi::PyDict_GetItemString(d.as_ptr(), key.as_ptr()) };
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

/// First-call validation: every recorded fingerprint (the objects collected at install).
///
/// # Safety
/// Attached thread; the objects are alive (held by the state's references).
unsafe fn check_fingerprints(lazy: &state::Lazy) -> Result<(), String> {
    // SAFETY: per the caller.
    let py = unsafe { Python::assume_attached() };
    let (major, minor) = crate::accel::version::running();
    for &(name, obj) in &lazy.fps {
        let want = fps::FPS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, f)| *f)
            .ok_or_else(|| format!("{name}: no fingerprint"))?;
        // SAFETY: a live object held by the state.
        let obj = unsafe { Bound::from_borrowed_ptr(py, obj) };
        let got = crate::accel::fingerprint::any(&obj).map_err(|e| format!("{name}: {e}"))?;
        match crate::accel::fingerprint::expected_for(want, major, minor) {
            Some(w) if w == got => {}
            Some(w) => return Err(format!("fingerprint: {name}: {got} != expected {w}")),
            None => return Err(format!("version: {name}: no fingerprint recorded for {major}.{minor}")),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------------------

const COMPILER_FNS: &[&str] = &[
    "compile",
    "isstring",
    "_code",
    "_compile_info",
    "_compile",
    "_compile_charset",
    "_optimize_charset",
    "_mk_bitmap",
    "_bytes_to_codes",
    "_simple",
    "_generate_overlap_table",
    "_get_iscased",
    "_get_literal_prefix",
    "_get_charset_prefix",
    "_combine_flags",
];
const PARSER_FNS: &[&str] =
    &["parse", "fix_flags", "_parse_sub", "_parse", "_parse_flags", "_escape", "_class_escape", "_uniq"];
const PARSER_METHODS: &[(&str, &[&str])] = &[
    (
        "Tokenizer",
        &[
            "__init__",
            "_Tokenizer__next",
            "match",
            "get",
            "getwhile",
            "getuntil",
            "tell",
            "seek",
            "checkgroupname",
            "pos",
        ],
    ),
    ("State", &["__init__", "opengroup", "closegroup", "checkgroup", "checklookbehindgroup", "groups"]),
    (
        "SubPattern",
        &["__init__", "__len__", "__delitem__", "__getitem__", "__setitem__", "insert", "append", "getwidth"],
    ),
];

/// The fingerprint table's name for a parser method (`_Tokenizer__next` → `__next`).
fn method_fp_name(class: &str, m: &str) -> &'static str {
    let rec = if m == "_Tokenizer__next" { "__next" } else { m };
    let full = format!("re._parser.{class}.{rec}");
    fps::FPS.iter().find(|(n, _)| *n == full).map(|(n, _)| *n).unwrap_or("?")
}

fn fn_fp_name(module: &str, f: &str) -> &'static str {
    let full = format!("{module}.{f}");
    fps::FPS.iter().find(|(n, _)| *n == full).map(|(n, _)| *n).unwrap_or("?")
}

/// What `re._compiler.compile` reaches on its success path. At install (cheap): the modules'
/// identities, `_sre`'s code-word size and limits, the version tags of the parser's classes,
/// the pins (snapshots of the `re._compiler`, `re._parser` and `_sre` namespaces, the
/// builtins they use). On the first call ([`validate_compile`]): the fingerprints of every
/// function and method of the parser and compiler, `re._constants`' values against the port's
/// table, the `_EXTRA_CASES` table.
fn shared_compile(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use rustique_core::re::consts as k;
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
    let mut refs: Vec<O> = Vec::new();
    let mut fps: Vec<(&'static str, O)> = Vec::new();
    for name in COMPILER_FNS {
        let f = dget(py, &co, name)?;
        fps.push((fn_fp_name("re._compiler", name), keep(&f)));
    }
    for name in PARSER_FNS {
        let f = dget(py, &pa, name)?;
        fps.push((fn_fp_name("re._parser", name), keep(&f)));
    }
    let mut tags = Vec::new();
    for (class, methods) in PARSER_METHODS {
        let ty = as_type(dget(py, &pa, class)?, class)?;
        let td = crate::accel::prepare::type_dict(&ty)?;
        for m in *methods {
            let mut f = dget(py, &td, m)?;
            if matches!(*m, "pos" | "groups") {
                f = f.getattr("fget")?; // property (a C getter of the property object)
            }
            fps.push((method_fp_name(class, m), keep(&f)));
        }
        tags.push(tag_of(&ty, class)?);
    }
    let nic = as_type(dget(py, &cn, "_NamedIntConstant")?, "_NamedIntConstant")?;
    tags.push(tag_of(&nic, "_NamedIntConstant")?);
    let extra = dget(py, &co, "_EXTRA_CASES")?;
    if !extra.is_exact_instance_of::<PyDict>() {
        return Err(refuse!(Deps: "_EXTRA_CASES is not a dict"));
    }
    let sre_compile = dget(py, &sre, "compile")?;
    // pins: the namespaces as validated (any rebinding, addition or deletion falls back)
    cx.pin_dict(&co, &DEPS_COMPILE)?;
    cx.pin_dict(&pa, &DEPS_COMPILE)?;
    cx.pin_dict(&sre, &DEPS_COMPILE)?;
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
    pin_builtins(cx, &co, &builtins, &DEPS_COMPILE)?;
    pin_builtins(cx, &pa, &builtins, &DEPS_COMPILE)?;
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
        cases: std::sync::OnceLock::new(),
        constants: cn.as_ptr(),
        extra: extra.as_ptr(),
        lazy: state::Lazy::new(fps),
        tags,
        refs,
    });
    Ok(())
}

/// The compiler's first-call validation (see [`shared_compile`]).
///
/// # Safety
/// Attached thread.
pub(crate) unsafe fn validate_compile(st: &state::Compile) -> Result<(), String> {
    use rustique_core::re::consts as k;
    // SAFETY: per the caller.
    unsafe { check_fingerprints(&st.lazy)? };
    // SAFETY: attached thread; the state holds the dicts.
    let py = unsafe { Python::assume_attached() };
    let cn = unsafe { Bound::from_borrowed_ptr(py, st.constants) }
        .cast_into::<PyDict>()
        .map_err(|_| "constants".to_string())?;
    for (name, v) in k::OPCODE_NAMES.iter().chain(k::ATCODE_NAMES).chain(k::CHCODE_NAMES).chain(k::FLAG_NAMES) {
        let got = dget(py, &cn, name).ok().and_then(|o| int_value(&o));
        if got != Some(*v as u64) {
            return Err(format!("version: re._constants.{name} differs from the port's"));
        }
    }
    for (name, v) in [("MAXREPEAT", st.maxrepeat), ("MAXGROUPS", st.maxgroups)] {
        if dget(py, &cn, name).ok().and_then(|o| int_value(&o)) != Some(v as u64) {
            return Err(format!("deps: re._constants.{name} is not _sre's"));
        }
    }
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
    let _ = st.cases.set(state::Cases { extra: rustique_core::re::ExtraCases::new(pairs) });
    Ok(())
}

/// The compiler target is validated (running the first-call validation if needed).
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub(crate) unsafe fn compile_ready(st: &state::Compile) -> bool {
    // SAFETY: per the caller.
    st.lazy.ready(|| unsafe { validate_compile(st) }, |r| crate::accel::degrade(&TARGET_COMPILE, r))
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
    let red = cx.module()?.dict();
    let pa_m = cx.loaded_module("re._parser")?;
    let pa = pa_m.dict();
    let sre = cx.loaded_module("_sre")?.dict();
    let minor = cx.version().1;
    let mut fps: Vec<(&'static str, O)> = Vec::new();
    let match_name = if minor >= 15 { "prefixmatch" } else { "match" };
    for name in
        [match_name, "fullmatch", "search", "sub", "subn", "split", "findall", "finditer", "compile", "_compile"]
    {
        let f = dget(py, &red, name)?;
        fps.push((fn_fp_name("re", if name == "prefixmatch" { "match" } else { name }), keep(&f)));
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
    for m in ["__init__", "_Tokenizer__next", "match", "get", "getuntil", "checkgroupname"] {
        fps.push((method_fp_name("Tokenizer", m), keep(&dget(py, &td, m)?)));
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
    let names = ["flags", "count", "maxsplit", "_value_", "groups", "groupindex", "pattern"]
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
        lazy: state::Lazy::new(fps),
        n_flags: names[0].as_ptr(),
        n_count: names[1].as_ptr(),
        n_maxsplit: names[2].as_ptr(),
        n_value_: names[3].as_ptr(),
        n_groups: names[4].as_ptr(),
        n_groupindex: names[5].as_ptr(),
        n_pattern: names[6].as_ptr(),
        refs,
    });
    Ok(())
}

/// The module functions are validated (running the first-call validation if needed; a
/// failure degrades all of them).
///
/// # Safety
/// Attached thread.
#[inline(always)]
pub(crate) unsafe fn funcs_ready(st: &state::Funcs) -> bool {
    st.lazy.ready(
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
