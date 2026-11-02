//! What the `re` kernels read at run time: the validated objects (strong references, taken by
//! the group's `prepare`, dropped by `release` at exit), the interpreter's case mappings,
//! and the class version tags that stand for "no class the originals use was modified".

use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use pyo3::ffi;

use rustique_core::re::{CaseMap, Env, ExtraCases};

pub(crate) type O = *mut ffi::PyObject;

python_extern! {
    fn _PyUnicode_ToLowercase(ch: u32) -> u32;
    fn _PyUnicode_ToUppercase(ch: u32) -> u32;
    fn PyUnstable_Type_AssignVersionTag(ty: *mut ffi::PyTypeObject) -> std::ffi::c_int;
}

/// The running interpreter's case mappings: what `_sre.unicode_tolower` /
/// `_sre.unicode_iscased` call (`Py_UNICODE_TOLOWER` / `Py_UNICODE_TOUPPER`), and
/// `_casefix._EXTRA_CASES` as validated at install.
pub(crate) struct Cases {
    pub extra: ExtraCases,
}

impl CaseMap for Cases {
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
        self.extra.get(c)
    }
    fn extra_is_empty(&self) -> bool {
        self.extra.is_empty()
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

/// The compile kernel's state.
pub(crate) struct Compile {
    /// `_sre.compile` (pinned: `_sre.__dict__['compile']`).
    pub sre_compile: O,
    pub maxrepeat: u32,
    pub maxgroups: u32,
    pub cases: Cases,
    /// `Tokenizer`, `State`, `SubPattern`, `_NamedIntConstant`.
    pub tags: Vec<Tag>,
    /// Strong references held (dropped by `release`).
    pub refs: Vec<O>,
}

impl Compile {
    pub fn env(&self) -> Env<'_> {
        Env { maxrepeat: self.maxrepeat, maxgroups: self.maxgroups, case: &self.cases }
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
    /// `re._zero_sentinel` and its type
    pub zero: O,
    pub zero_t: *mut ffi::PyTypeObject,
    /// `RegexFlag`, `type(RegexFlag)` (`EnumType`), `_ZeroSentinel`.
    pub tags: Vec<Tag>,
    /// `Pattern.__dict__` method descriptors, by [`super::funcs::Method`].
    pub methods: [O; 9],
    /// Interned names.
    pub n_flags: O,
    pub n_count: O,
    pub n_maxsplit: O,
    pub n_value_: O,
    pub n_groups: O,
    pub n_groupindex: O,
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
unsafe impl Sync for Compile {}
unsafe impl Send for Funcs {}
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
