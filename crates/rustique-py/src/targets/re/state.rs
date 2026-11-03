//! What the `re` kernels read at run time: the validated objects (strong references, taken by
//! the group's `prepare`, dropped by `release` at exit), the interpreter's case mappings,
//! and the class version tags that stand for "no class the originals use was modified".

use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, Ordering};

use pyo3::ffi;

use rustique_core::re::{CaseMap, Env, ExtraCases};

pub(crate) type O = *mut ffi::PyObject;

python_extern! {
    fn _PyUnicode_ToLowercase(ch: u32) -> u32;
    fn _PyUnicode_ToUppercase(ch: u32) -> u32;
    fn PyUnstable_Type_AssignVersionTag(ty: *mut ffi::PyTypeObject) -> std::ffi::c_int;
    pub(crate) fn _PyType_Lookup(ty: *mut ffi::PyTypeObject, name: *mut ffi::PyObject) -> *mut ffi::PyObject;
}

/// The running interpreter's case mappings: what `_sre.unicode_tolower` /
/// `_sre.unicode_iscased` call (`Py_UNICODE_TOLOWER` / `Py_UNICODE_TOUPPER`), and
/// `_casefix._EXTRA_CASES` once validated. The table is read only when a compile first needs
/// it (Unicode IGNORECASE): without it, the port's lookups record that it was `wanted` (the
/// compile is then redone with the table, or declined).
pub(crate) struct Cases<'a> {
    pub extra: Option<&'a ExtraCases>,
    pub wanted: std::cell::Cell<bool>,
}

impl CaseMap for Cases<'_> {
    #[inline]
    fn lower(&self, c: u32) -> u32 {
        // SAFETY: a pure function of the code point (CPython's Unicode database).
        unsafe { _PyUnicode_ToLowercase(c) }
    }
    #[inline]
    fn upper(&self, c: u32) -> u32 {
        // SAFETY: as above.
        unsafe { _PyUnicode_ToUppercase(c) }
    }
    #[inline]
    fn extra(&self, c: u32) -> Option<&[u32]> {
        match self.extra {
            Some(t) => t.get(c),
            None => {
                self.wanted.set(true);
                None
            }
        }
    }
    fn extra_is_empty(&self) -> bool {
        match self.extra {
            Some(t) => t.is_empty(),
            None => {
                self.wanted.set(true);
                false
            }
        }
    }
}

/// A class whose version tag must stay what it was at install (any modification of the
/// class or of a class in its MRO resets the tag: `PyType_Modified`).
pub(crate) struct Tag {
    pub ty: *mut ffi::PyTypeObject,
    pub tag: u32,
}

impl Tag {
    /// The current tag of `ty` (assigned if it had none); `None` if none can be assigned.
    ///
    /// # Safety
    /// Attached thread; `ty` a live type.
    pub unsafe fn take(ty: *mut ffi::PyTypeObject) -> Option<Tag> {
        // SAFETY: per the caller.
        unsafe {
            if PyUnstable_Type_AssignVersionTag(ty) == 0 && read_tag(ty) == 0 {
                return None;
            }
            let tag = read_tag(ty);
            (tag != 0).then_some(Tag { ty, tag })
        }
    }
    #[inline(always)]
    pub fn intact(&self) -> bool {
        // SAFETY: the type is kept alive by the state's references.
        unsafe { read_tag(self.ty) == self.tag }
    }
}

/// `tp_version_tag`, read atomically (free-threaded builds update it concurrently).
///
/// # Safety
/// `ty` a live type.
#[inline(always)]
unsafe fn read_tag(ty: *mut ffi::PyTypeObject) -> u32 {
    // SAFETY: an aligned `unsigned int` field of a live type object.
    unsafe { (*(&raw const (*ty).tp_version_tag).cast::<AtomicU32>()).load(Ordering::Relaxed) }
}

/// Validation done on a group's first kernel call instead of at install (ACCELERATION §5.7:
/// a failure degrades the targets, whose kernels then fall back): the fingerprints of the
/// Python functions the kernels stand in for, and the tables they read. The install itself
/// only checks identities and declares sites and pins (`docs/targets/re.md`, install cost).
pub(crate) struct Lazy {
    state: AtomicU8,
    /// Calls seen while pending (the deferral counter).
    calls: AtomicU32,
    /// (name in `fps::FPS`, object) of every original to fingerprint (references in the
    /// state's `refs`); checked against the v3 data of `owner`.
    pub fps: Vec<(&'static str, O)>,
    /// The `GroupSpec` name the v3 data records these originals under.
    pub owner: &'static str,
}

const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const OK: u8 = 2;
const FAILED: u8 = 3;

impl Lazy {
    pub fn new(owner: &'static str, fps: Vec<(&'static str, O)>) -> Self {
        Lazy { state: AtomicU8::new(PENDING), calls: AtomicU32::new(0), fps, owner }
    }

    /// Validated (running `check` once, on the `after`-th call that gets here: the first
    /// calls of a process fall back without validating, so a program that compiles one or two
    /// patterns does not pay the validation). A thread that finds another one validating
    /// falls back rather than waits (no lock is held while CPython code runs).
    #[inline(always)]
    pub fn ready(&self, after: u32, check: impl FnOnce() -> Result<(), String>, fail: impl FnOnce(&str)) -> bool {
        match self.state.load(Ordering::Acquire) {
            OK => true,
            PENDING => {
                if after > 1 && self.calls.fetch_add(1, Ordering::Relaxed) + 1 < after {
                    return false;
                }
                self.run(check, fail)
            }
            _ => false,
        }
    }

    /// Validated now (tests, helpers).
    pub fn force(&self, check: impl FnOnce() -> Result<(), String>, fail: impl FnOnce(&str)) -> bool {
        self.ready(1, check, fail)
    }

    #[cold]
    fn run(&self, check: impl FnOnce() -> Result<(), String>, fail: impl FnOnce(&str)) -> bool {
        if self.state.compare_exchange(PENDING, RUNNING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return self.state.load(Ordering::Acquire) == OK;
        }
        match check() {
            Ok(()) => {
                self.state.store(OK, Ordering::Release);
                true
            }
            Err(reason) => {
                fail(&reason);
                self.state.store(FAILED, Ordering::Release);
                false
            }
        }
    }
}

/// `_EXTRA_CASES` as validated on first need, or why it is not usable.
pub(crate) type ExtraTable = OnceLock<Result<ExtraCases, String>>;

/// The compile kernel's state.
pub(crate) struct Compile {
    /// `_sre.compile` (pinned: `_sre.__dict__['compile']`).
    pub sre_compile: O,
    pub maxrepeat: u32,
    pub maxgroups: u32,
    /// Built from `_EXTRA_CASES` when a compile first needs it.
    pub extra_table: ExtraTable,
    /// `re._constants.__dict__` and `re._compiler._EXTRA_CASES` (checked on first call).
    pub constants: O,
    pub extra: O,
    pub lazy: Lazy,
    /// `Tokenizer`, `State`, `SubPattern`, `_NamedIntConstant`.
    pub tags: Vec<Tag>,
    /// Strong references held (dropped by `release`).
    pub refs: Vec<O>,
}

impl Compile {
    /// The port's environment over `cases`.
    pub fn env<'a>(&self, cases: &'a Cases<'a>) -> Env<'a> {
        Env { maxrepeat: self.maxrepeat, maxgroups: self.maxgroups, case: cases }
    }
    #[inline(always)]
    pub fn tags_intact(&self) -> bool {
        self.tags.iter().all(Tag::intact)
    }
}

/// The module functions' state.
pub(crate) struct Funcs {
    /// `re._cache2` (pinned)
    pub cache2: O,
    /// `re.Pattern`
    pub pattern_t: *mut ffi::PyTypeObject,
    /// `re.RegexFlag`
    pub regexflag_t: *mut ffi::PyTypeObject,
    /// `re._zero_sentinel`
    pub zero: O,
    /// `RegexFlag`, `type(RegexFlag)` (`EnumType`), `_ZeroSentinel`.
    pub tags: Vec<Tag>,
    /// `Pattern.__dict__` method descriptors, by [`super::funcs::Method`].
    pub methods: [O; 9],
    pub lazy: Lazy,
    /// Interned names.
    pub n_flags: O,
    pub n_count: O,
    pub n_maxsplit: O,
    pub n_value_: O,
    pub n_groups: O,
    pub n_pattern: O,
    pub refs: Vec<O>,
}

impl Funcs {
    #[inline(always)]
    pub fn tags_intact(&self) -> bool {
        self.tags.iter().all(Tag::intact)
    }
}

// SAFETY: the raw pointers are immutable after install and refer to objects kept alive by
// `refs` (or immortal); the kernels only use them with an attached thread.
unsafe impl Send for Compile {}
// SAFETY: as above (the only mutable parts are atomics and a `OnceLock`).
unsafe impl Sync for Compile {}
// SAFETY: as above.
unsafe impl Send for Funcs {}
// SAFETY: as above.
unsafe impl Sync for Funcs {}

pub(crate) static COMPILE: AtomicPtr<Compile> = AtomicPtr::new(std::ptr::null_mut());
pub(crate) static FUNCS: AtomicPtr<Funcs> = AtomicPtr::new(std::ptr::null_mut());

/// The current state, if installed (never freed: a kernel may hold it while a re-prepare
/// replaces it; a replaced state keeps its references).
#[inline(always)]
pub(crate) fn compile_state<'a>() -> Option<&'a Compile> {
    // SAFETY: published with Release by `publish_*`; never freed.
    unsafe { COMPILE.load(Ordering::Acquire).as_ref() }
}

#[inline(always)]
pub(crate) fn funcs_state<'a>() -> Option<&'a Funcs> {
    // SAFETY: as above.
    unsafe { FUNCS.load(Ordering::Acquire).as_ref() }
}

pub(crate) fn publish_compile(s: Compile) {
    COMPILE.store(Box::into_raw(Box::new(s)), Ordering::Release);
}

pub(crate) fn publish_funcs(s: Funcs) {
    FUNCS.store(Box::into_raw(Box::new(s)), Ordering::Release);
}

/// Interpreter exit (after the sites were restored and the dependency flags cleared): drop
/// the references so that finalization frees what CPython frees. The state memory stays
/// (a kernel already past its flag check on another thread may still read it).
pub(crate) fn release() {
    for p in [COMPILE.swap(std::ptr::null_mut(), Ordering::AcqRel)] {
        // SAFETY: published by `publish_compile`; references owned by the state.
        if let Some(s) = unsafe { p.as_mut() } {
            for o in s.refs.drain(..) {
                // SAFETY: attached thread at exit; owned references.
                unsafe { ffi::Py_DECREF(o) };
            }
        }
    }
    let p = FUNCS.swap(std::ptr::null_mut(), Ordering::AcqRel);
    // SAFETY: as above.
    if let Some(s) = unsafe { p.as_mut() } {
        for o in s.refs.drain(..) {
            // SAFETY: as above.
            unsafe { ffi::Py_DECREF(o) };
        }
    }
}
