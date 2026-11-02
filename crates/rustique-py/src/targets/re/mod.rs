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

use crate::accel::{self, Category, Counters, DepFlag, GroupSpec, Prepare, Refusal, TargetSpec, Tools};
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
    // A fallback costs <2% of the cheapest compile, also under the JIT.
    jit_safe: true,
    checkable: true,
    shared: Some(&SHARED_COMPILE),
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
        fn $prep(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
            prep_func(cx, $attr, $kernel)
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
func_target!(FULLMATCH, RT_FULLMATCH, k_fullmatch, prep_fullmatch, "re.fullmatch", "fullmatch", Method::Fullmatch, false);
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

/// Require the recorded fingerprint of `obj` (named `name` in [`fps::FPS`]).
fn check_fp(cx: &mut Prepare<'_>, name: &str, obj: &Bound<'_, PyAny>) -> Result<(), Refusal> {
    let fps = fps::FPS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f).ok_or_else(|| refuse!("{name}: no fingerprint"))?;
    cx.fingerprint_code(obj, fps)
}

/// A class's own `__dict__` entry `name` (plain dict read).
fn class_attr<'py>(cx: &Prepare<'py>, ty: &Bound<'py, PyType>, name: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    cx.type_attr(ty, name)
}

/// The `int` value of `d[name]` (an `int` or `_NamedIntConstant`).
fn int_of(cx: &Prepare<'_>, d: &Bound<'_, PyDict>, name: &str) -> Result<u64, Refusal> {
    let v = cx.get(d, name)?;
    // SAFETY: attached thread; a live object.
    let x = unsafe {
        if ffi::PyLong_Check(v.as_ptr()) == 0 {
            return Err(refuse!(Deps: "{name} is not an int"));
        }
        ffi::PyLong_AsUnsignedLongLong(v.as_ptr())
    };
    if x == u64::MAX {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return Err(refuse!(Deps: "{name} out of range"));
    }
    Ok(x)
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
fn pin_builtins(cx: &mut Prepare<'_>, globals: &Bound<'_, PyDict>, names: &[&str], flag: &'static DepFlag) -> Result<(), Refusal> {
    for n in names {
        cx.pin_global(globals, n, flag)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------------------

/// Everything `re._compiler.compile` reaches on its success path, validated and pinned once:
/// the `_sre` constants and functions, `re._constants`' values against the port's table,
/// the fingerprints of every function and method of the parser and compiler, the
/// `_EXTRA_CASES` table; the module namespaces (snapshot pins), the classes (version tags)
/// and the builtins they use.
fn shared_compile(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    use rustique_core::re::consts as k;
    let red = cx.module()?.dict();
    let co_m = cx.loaded_module("re._compiler")?;
    let pa_m = cx.loaded_module("re._parser")?;
    let cn_m = cx.loaded_module("re._constants")?;
    let sre_m = cx.loaded_module("_sre")?;
    let (co, pa, cn, sre) = (co_m.dict(), pa_m.dict(), cn_m.dict(), sre_m.dict());
    if !cx.get(&red, "_compiler")?.is(&co_m) || !cx.get(&co, "_parser")?.is(&pa_m) || !cx.get(&co, "_sre")?.is(&sre_m) {
        return Err(refuse!(Deps: "re._compiler's modules are not the imported ones"));
    }
    // _sre: the code-word size and limits the port assumes
    if int_of(cx, &sre, "MAGIC")? != k::MAGIC as u64 || int_of(cx, &cn, "MAGIC")? != k::MAGIC as u64 {
        return Err(refuse!(Version: "_sre.MAGIC differs from the port's"));
    }
    if int_of(cx, &sre, "CODESIZE")? != 4 {
        return Err(refuse!(Platform: "_sre.CODESIZE is not 4"));
    }
    let maxrepeat = int_of(cx, &sre, "MAXREPEAT")?;
    let maxgroups = int_of(cx, &sre, "MAXGROUPS")?;
    if maxrepeat > u32::MAX as u64 || maxgroups > u32::MAX as u64 {
        return Err(refuse!(Platform: "_sre limits out of range"));
    }
    for (name, v) in k::OPCODE_NAMES.iter().chain(k::ATCODE_NAMES).chain(k::CHCODE_NAMES).chain(k::FLAG_NAMES) {
        if int_of(cx, &cn, name)? != *v as u64 {
            return Err(refuse!(Version: "re._constants.{name} differs from the port's"));
        }
    }
    for (name, v) in [("MAXREPEAT", maxrepeat), ("MAXGROUPS", maxgroups)] {
        if int_of(cx, &cn, name)? != v {
            return Err(refuse!(Deps: "re._constants.{name} is not _sre's"));
        }
    }
    // fingerprints of the call graph
    for name in [
        "compile", "isstring", "_code", "_compile_info", "_compile", "_compile_charset", "_optimize_charset",
        "_mk_bitmap", "_bytes_to_codes", "_simple", "_generate_overlap_table", "_get_iscased",
        "_get_literal_prefix", "_get_charset_prefix", "_combine_flags",
    ] {
        let f = cx.get(&co, name)?;
        check_fp(cx, &format!("re._compiler.{name}"), &f)?;
    }
    for name in ["parse", "fix_flags", "_parse_sub", "_parse", "_parse_flags", "_escape", "_class_escape", "_uniq"] {
        let f = cx.get(&pa, name)?;
        check_fp(cx, &format!("re._parser.{name}"), &f)?;
    }
    let mut tags = Vec::new();
    for (class, methods) in [
        ("Tokenizer", &["__init__", "_Tokenizer__next", "match", "get", "getwhile", "getuntil", "tell", "seek", "checkgroupname", "pos"][..]),
        ("State", &["__init__", "opengroup", "closegroup", "checkgroup", "checklookbehindgroup", "groups"][..]),
        ("SubPattern", &["__init__", "__len__", "__delitem__", "__getitem__", "__setitem__", "insert", "append", "getwidth"][..]),
    ] {
        let ty = as_type(cx.get(&pa, class)?, class)?;
        for m in methods {
            let mut f = class_attr(cx, &ty, m)?;
            if matches!(*m, "pos" | "groups") {
                f = f.getattr("fget")?; // property
            }
            let rec = if *m == "_Tokenizer__next" { "__next" } else { m };
            check_fp(cx, &format!("re._parser.{class}.{rec}"), &f)?;
        }
        tags.push(tag_of(&ty, class)?);
    }
    let nic = as_type(cx.get(&cn, "_NamedIntConstant")?, "_NamedIntConstant")?;
    tags.push(tag_of(&nic, "_NamedIntConstant")?);
    // _EXTRA_CASES (the compiler's own reference: `from ._casefix import _EXTRA_CASES`)
    let extra = cx.get(&co, "_EXTRA_CASES")?;
    let extra = extra.cast_into::<PyDict>().map_err(|_| refuse!(Deps: "_EXTRA_CASES is not a dict"))?;
    let mut pairs = Vec::with_capacity(extra.len());
    for (key, v) in extra.iter() {
        let key: u32 = key.extract().map_err(|_| refuse!(Deps: "_EXTRA_CASES key"))?;
        let v: Vec<u32> = v.extract().map_err(|_| refuse!(Deps: "_EXTRA_CASES value"))?;
        pairs.push((key, v));
    }
    let hash = extra_cases_hash(&pairs);
    let (major, minor) = cx.version();
    let want = fps::EXTRA_CASES.iter().find(|(a, b, _)| (*a, *b) == (major, minor)).map(|e| e.2);
    if want != Some(hash.as_str()) {
        return Err(refuse!(Fingerprint: "re._casefix._EXTRA_CASES: {hash} != recorded {want:?}"));
    }
    // `_sre` functions the compiler calls (their C fingerprints are their identity: pinned)
    let sre_compile = cx.get(&sre, "compile")?;
    // pins: the namespaces as validated (any rebinding, addition or deletion falls back)
    cx.pin_dict(&co, &DEPS_COMPILE)?;
    cx.pin_dict(&pa, &DEPS_COMPILE)?;
    cx.pin_dict(&sre, &DEPS_COMPILE)?;
    let builtins = [
        "isinstance", "str", "bytes", "len", "range", "IndexError", "int", "chr", "ord", "min", "list", "dict",
        "slice", "tuple", "any", "map", "bytearray", "memoryview", "ValueError",
    ];
    pin_builtins(cx, &co, &builtins, &DEPS_COMPILE)?;
    pin_builtins(cx, &pa, &builtins, &DEPS_COMPILE)?;
    let mut refs = vec![keep(&sre_compile)];
    refs.extend(tags.iter().map(|t| {
        // SAFETY: attached thread; the classes stay referenced by the state.
        unsafe { ffi::Py_NewRef(t.ty.cast::<ffi::PyObject>()) }
    }));
    state::publish_compile(state::Compile {
        sre_compile: sre_compile.as_ptr(),
        maxrepeat: maxrepeat as u32,
        maxgroups: maxgroups as u32,
        cases: state::Cases { extra: rustique_core::re::ExtraCases::new(pairs) },
        tags,
        refs,
    });
    Ok(())
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

fn prep_compile(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let co = cx.loaded_module("re._compiler")?.dict();
    let f = cx.get(&co, "compile")?;
    cx.vectorcall(&f, compile::k_compile, &accel::FUNCTION_VECTORCALL)
}

// ---------------------------------------------------------------------------------------
// The module functions
// ---------------------------------------------------------------------------------------

/// What the module functions read on a cache hit (`re` globals, `RegexFlag` and the
/// `Pattern` type) and on a template `sub` (`parse_template`, `_compile_template`).
fn shared_funcs(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let red = cx.module()?.dict();
    let pa_m = cx.loaded_module("re._parser")?;
    let pa = pa_m.dict();
    let sre = cx.loaded_module("_sre")?.dict();
    let (major, minor) = cx.version();
    // fingerprints: every function, `_compile` (its hit path), the template path
    let match_name = if minor >= 15 { "prefixmatch" } else { "match" };
    for name in [match_name, "fullmatch", "search", "sub", "subn", "split", "findall", "finditer", "compile", "_compile"] {
        let f = cx.get(&red, name)?;
        check_fp(cx, &format!("re.{}", if name == "prefixmatch" { "match" } else { name }), &f)?;
    }
    if minor >= 15 && !cx.get(&red, "match")?.is(&cx.get(&red, "prefixmatch")?) {
        return Err(refuse!(Deps: "re.match is not re.prefixmatch"));
    }
    let ct = cx.get(&red, "_compile_template")?;
    check_fp(cx, "re._compile_template", &ct.getattr("__wrapped__")?)?;
    check_fp(cx, "re._parser.parse_template", &cx.get(&pa, "parse_template")?)?;
    if !cx.get(&red, "_parser")?.is(&pa_m) {
        return Err(refuse!(Deps: "re._parser is not the imported module"));
    }
    let tok = as_type(cx.get(&pa, "Tokenizer")?, "Tokenizer")?;
    for m in ["__init__", "_Tokenizer__next", "match", "get", "getuntil", "checkgroupname"] {
        let rec = if m == "_Tokenizer__next" { "__next" } else { m };
        check_fp(cx, &format!("re._parser.Tokenizer.{rec}"), &class_attr(cx, &tok, m)?)?;
    }
    // classes: Pattern (immutable C type), RegexFlag (+ its metaclass), _ZeroSentinel
    let pattern_t = as_type(cx.get(&red, "Pattern")?, "re.Pattern")?;
    // SAFETY: attached thread; type flags of a live type.
    if unsafe { ffi::PyType_GetFlags(pattern_t.as_type_ptr()) } & ffi::Py_TPFLAGS_IMMUTABLETYPE == 0 {
        return Err(refuse!(Deps: "re.Pattern is not immutable"));
    }
    let regexflag = as_type(cx.get(&red, "RegexFlag")?, "re.RegexFlag")?;
    let enumtype = regexflag.get_type();
    let zero = cx.get(&red, "_zero_sentinel")?;
    let zero_t = zero.get_type();
    // `flags.value` is `self._value_`: plain attribute lookup (no descriptor named
    // `_value_` in the MRO, generic getattr)
    // SAFETY: attached thread; live types.
    unsafe {
        let rf = regexflag.as_type_ptr();
        if (*rf).tp_getattro != Some(ffi::PyObject_GenericGetAttr) {
            return Err(refuse!(Deps: "RegexFlag has a custom __getattribute__"));
        }
        let name = pyo3::intern!(py, "_value_");
        if !ffi::_PyType_Lookup(rf, name.as_ptr()).is_null() {
            return Err(refuse!(Deps: "RegexFlag defines _value_ on a class"));
        }
    }
    check_fp(cx, "re._compile", &cx.get(&red, "_compile")?)?;
    let tags =
        vec![tag_of(&regexflag, "RegexFlag")?, tag_of(&enumtype, "EnumType")?, tag_of(&zero_t, "_ZeroSentinel")?];
    // Pattern methods called on a hit (the C method descriptors of the immutable type)
    let pd = pattern_t.getattr("__dict__")?;
    let get_m = |n: &str| -> Result<Bound<'_, PyAny>, Refusal> { Ok(pd.get_item(n)?) };
    let methods = [
        get_m(match_name)?,
        get_m("fullmatch")?,
        get_m("search")?,
        get_m("findall")?,
        get_m("finditer")?,
        get_m("sub")?,
        get_m("subn")?,
        get_m("split")?,
        // `re.compile` calls no method: any live object will do
        pattern_t.clone().into_any(),
    ];
    // pins
    for name in ["_compile", "_cache2", "RegexFlag", "_zero_sentinel", "Pattern", "_compile_template", "_parser", "_sre"] {
        cx.pin(&red, name, &DEPS_FUNCS)?;
    }
    pin_builtins(cx, &red, &["isinstance", "type"], &DEPS_FUNCS)?;
    cx.pin_dict(&pa, &DEPS_FUNCS)?;
    cx.pin_dict(&sre, &DEPS_FUNCS)?;
    let cache2 = cx.get(&red, "_cache2")?;
    if !cache2.is_exact_instance_of::<PyDict>() {
        return Err(refuse!(Deps: "re._cache2 is not a dict"));
    }
    let names = ["flags", "count", "maxsplit", "_value_", "groups", "groupindex", "pattern"]
        .map(|n| crate::accel::prepare::interned(py, n).into_any());
    let mut refs: Vec<O> = vec![keep(&cache2), keep(&pattern_t), keep(&regexflag), keep(&zero), keep(&zero_t)];
    refs.extend(tags.iter().map(|t| {
        // SAFETY: attached thread; live classes.
        unsafe { ffi::Py_NewRef(t.ty.cast::<ffi::PyObject>()) }
    }));
    refs.extend(methods.iter().map(keep));
    refs.extend(names.iter().map(keep));
    let _ = major;
    state::publish_funcs(state::Funcs {
        cache2: cache2.as_ptr(),
        pattern_t: pattern_t.as_type_ptr(),
        regexflag_t: regexflag.as_type_ptr(),
        zero: zero.as_ptr(),
        zero_t: zero_t.as_type_ptr(),
        tags,
        methods: methods.map(|m| m.as_ptr()),
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

fn prep_func(cx: &mut Prepare<'_>, attr: &str, kernel: ffi::vectorcallfunc) -> Result<(), Refusal> {
    let red = cx.module()?.dict();
    let attr = if attr == "match" && cx.version().1 >= 15 { "prefixmatch" } else { attr };
    let f = cx.get(&red, attr)?;
    cx.vectorcall(&f, kernel, &accel::FUNCTION_VECTORCALL)
}

#[allow(dead_code)]
fn category_unused() -> Category {
    Category::Other
}
