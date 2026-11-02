//! Group `copy`: `copy.deepcopy` and `copy.copy`; and (in [`dc`]) group `dataclasses`:
//! `dataclasses.asdict`, `astuple`, `replace`. All are vectorcall kernels on the existing
//! function objects (contract §3.7). See `docs/targets/copy.md`.

pub mod dc;
mod deep;
mod known;
mod memo;
mod reduce;
mod rt;
mod shallow;
mod uuidk;

use std::ffi::c_int;
use std::sync::atomic::Ordering::Relaxed;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

use crate::accel::{self, Counters, DepFlag, GroupSpec, Prepare, Reason, Refusal, TargetSpec, Tools};
use rt::*;

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

static C_DEEPCOPY: Counters = Counters::new();
static C_COPY: Counters = Counters::new();
/// The module globals and builtins `copy.py` reads at call time are the objects validated.
static DEPS_COPY: DepFlag = DepFlag::new();
static TOOLS_DEEPCOPY: Tools = Tools::new();
static TOOLS_COPY: Tools = Tools::new();

pub static DEEPCOPY: TargetSpec = TargetSpec {
    name: "copy.deepcopy",
    group: "copy",
    module: Some("copy"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_DEEPCOPY,
    prepare: prepare_deepcopy,
    shared: Some(&SHARED),
    // Measured (callgrind, docs/targets/copy.md §5): a direct fallback costs ~420 Ir over
    // CPython (the C call the interpreter cannot inline); a hit saves 400 Ir (an atomic
    // root) to 20k+ (a record): installed while at least a third of the calls hit.
    hit_credit: 2,
    fallback_debit: 1,
    ..TargetSpec::DEFAULT
};

pub static COPY: TargetSpec = TargetSpec {
    name: "copy.copy",
    group: "copy",
    module: Some("copy"),
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_COPY,
    prepare: prepare_copy,
    shared: Some(&SHARED),
    // Measured: a fallback costs 420 Ir (3.15) to 690 Ir (3.14) over CPython (the C call
    // the interpreter cannot inline, then the original's own frame); a hit saves 250 Ir (an
    // atomic) to ~420 (a record or container; 8k for a plain object): installed while two
    // thirds of the calls hit.
    hit_credit: 1,
    fallback_debit: 2,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&DEEPCOPY, &COPY];

/// The group's shared declarations: validation of `copy`/`copyreg` (fingerprints, type
/// sets, layouts: [`St`]) and the pins of [`DEPS_COPY`], made once for both targets.
static SHARED: GroupSpec = GroupSpec { name: "copy", prepare: prepare_shared };

/// `_PyObject_GetMethod` of the running interpreter.
static GET_METHOD: accel::Original<reduce::GetMethodFn> = accel::Original::new();

#[inline(always)]
unsafe fn original(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: the saved `_PyFunction_Vectorcall`.
    unsafe { accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kw) }
}

#[inline(always)]
fn nargs(nargsf: usize) -> usize {
    nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET
}

/// Is the set `deepcopy` checks atomic types against intact (3.14's is a mutable set)?
#[inline(always)]
unsafe fn atomic_fast(st: &St) -> bool {
    match &st.atomic_snap {
        None => true,
        // SAFETY: live set.
        Some(s) => unsafe { s.matches(st.atomic) || same_types(st.atomic, &st.atomic_types) },
    }
}

/// `t in _atomic_types` for a quick type, decided from the inspected slots alone (3.14).
#[inline(always)]
unsafe fn quick_atomic(st: &St, t: *mut ffi::PyTypeObject) -> bool {
    match &st.atomic_snap {
        None => true,
        // SAFETY: live set.
        Some(s) => unsafe { s.probe(st.atomic, t).unwrap_or_else(|| atomic_fast(st)) },
    }
}

/// `t in s` for copy's 3.14 sets: `Some(answer)` when decided from the inspected slots.
#[inline(always)]
pub(crate) unsafe fn copy_set_probe(snap: &Option<SetSnap>, set: O, t: *mut ffi::PyTypeObject) -> Option<bool> {
    match snap {
        None => None,
        // SAFETY: live set.
        Some(s) => unsafe { s.probe(set, t) },
    }
}

/// Does the (mutable, 3.14) set hold exactly `known` (after an add/discard, its table can
/// differ from the snapshot while its content is the validated one)?
#[cold]
unsafe fn same_types(set: O, known: &[*mut ffi::PyTypeObject]) -> bool {
    // SAFETY: a live set; the known entries are builtin types (pointer hashes).
    unsafe {
        ffi::PySet_Size(set) == known.len() as isize && known.iter().all(|&t| ffi::PySet_Contains(set, t.cast()) == 1)
    }
}

/// Are `copy.copy`'s two type sets intact (3.14's are mutable sets)?
#[inline(always)]
pub(crate) unsafe fn copy_sets_fast(st: &St) -> bool {
    // SAFETY: live sets.
    unsafe {
        match (&st.copy_atomic_snap, &st.copy_containers_snap) {
            (Some(a), Some(b)) => {
                (a.matches(st.copy_atomic) || same_types(st.copy_atomic, &st.copy_atomic_types))
                    && (b.matches(st.copy_containers) || same_types(st.copy_containers, &st.copy_container_types))
            }
            _ => true,
        }
    }
}

/// Is the `copy.deepcopy` kernel installed and its guards satisfied, so that callers inside
/// Rustique (dataclasses) may run the engine directly instead of calling the function?
#[inline(always)]
unsafe fn deepcopy_direct(st: &St, f: O) -> bool {
    // SAFETY: live function object.
    unsafe {
        f == st.deepcopy_fn && {
            let fo = f.cast::<ffi::PyFunctionObject>();
            (*fo).vectorcall.map(|v| v as usize) == Some(vc_deepcopy as ffi::vectorcallfunc as usize)
                && DEPS_COPY.ok()
                && (*fo).func_code == st.deepcopy_code
                && (*fo).func_defaults == st.deepcopy_defaults
                && accel::code_locally_quiet(st.deepcopy_code)
        }
    }
}

// ---------------------------------------------------------------------------------------
// Deep recursion through fallbacks
// ---------------------------------------------------------------------------------------
//
// While the kernel is installed, every call of `deepcopy`/`copy` from Python code is a C
// call (the interpreter inlines only functions whose vectorcall is its own). A call that
// falls back runs the original, whose helpers (or the hooks) call the function again: one
// C-level recursion per level, where CPython would inline Python-to-Python calls and go as
// deep as the recursion limit allows. So fallbacks nesting deeper than [`DEEP_FALLBACK`] on
// one thread put the original vectorcall back at once — the deeper levels are inlined by
// the interpreter exactly as in CPython — and have the registry record a suspension
// (resumed by the next `enable()`, contract §5.3).

// Nesting of fallback calls on this thread, per target.
thread_local! {
    static NEST_DEEPCOPY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static NEST_COPY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Fallback calls nested on one thread before the original vectorcall is put back
/// (bounded C stack: a few hundred kB at most).
const DEEP_FALLBACK: u32 = 128;

/// Call the original after a fallback (`why`), with one thread-local access: count it, and
/// debit the net-benefit score unless it is a call the original's helpers (or hooks) make
/// inside a fallback of the same target (those repeat the outer one level by level);
/// `always_debit` for abandoned restarts (work was wasted). Nesting deeper than
/// [`DEEP_FALLBACK`] puts the original vectorcall back ([`leave_kernel`]).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
unsafe fn fall(
    c: &'static accel::Counters,
    nest: &'static std::thread::LocalKey<std::cell::Cell<u32>>,
    why: Reason,
    always_debit: bool,
    func: O,
    args: *const O,
    nargsf: usize,
    kw: O,
) -> O {
    nest.with(|n| {
        let depth = n.get();
        if depth == 0 || always_debit {
            c.fallback_streak(why);
        } else {
            c.fallback(why);
        }
        n.set(depth + 1);
        if depth + 1 == DEEP_FALLBACK {
            // SAFETY: a live function object (the target's).
            unsafe { leave_kernel(c, func) };
        }
        // SAFETY: vectorcall arguments.
        let r = unsafe { original(func, args, nargsf, kw) };
        n.set(n.get() - 1);
        r
    })
}

/// Classes whose instances, as roots, always need user code (a `__deepcopy__`/`__copy__`
/// hook, a registered copier or reductor): calls with such a root go straight to the
/// original, before any other work. Keyed by `(type, tp_version_tag)`: any change to the
/// class gives it a new tag. A stale or torn entry can only send a call to the original,
/// which is always correct.
pub(crate) struct RootCache {
    ty: [std::sync::atomic::AtomicUsize; 64],
    tag: [std::sync::atomic::AtomicU32; 64],
    /// [`REGISTRATIONS`] when the entry was made.
    reg: [std::sync::atomic::AtomicU64; 64],
}

/// Bumped on any change of `copy._deepcopy_dispatch` or `copyreg.dispatch_table` (dict
/// watcher): a decision that rested on a registration goes stale with it.
static REGISTRATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl RootCache {
    const fn new() -> RootCache {
        RootCache {
            ty: [const { std::sync::atomic::AtomicUsize::new(0) }; 64],
            tag: [const { std::sync::atomic::AtomicU32::new(0) }; 64],
            reg: [const { std::sync::atomic::AtomicU64::new(u64::MAX) }; 64],
        }
    }

    #[inline(always)]
    fn slot(cls: *mut ffi::PyTypeObject) -> usize {
        ((cls as usize) >> 6) & 63
    }

    #[inline(always)]
    unsafe fn blocked(&self, cls: *mut ffi::PyTypeObject) -> bool {
        let i = Self::slot(cls);
        // SAFETY: a live type object.
        let tag = unsafe { (*cls).tp_version_tag };
        self.ty[i].load(Relaxed) == cls as usize
            && tag != 0
            && self.tag[i].load(Relaxed) == tag
            && self.reg[i].load(Relaxed) == REGISTRATIONS.load(Relaxed)
    }

    #[cold]
    unsafe fn block(&self, cls: *mut ffi::PyTypeObject) {
        // SAFETY: a live type object.
        let tag = unsafe { (*cls).tp_version_tag };
        if tag != 0 {
            let i = Self::slot(cls);
            self.ty[i].store(cls as usize, Relaxed);
            self.tag[i].store(tag, Relaxed);
            self.reg[i].store(REGISTRATIONS.load(Relaxed), Relaxed);
        }
    }
}

static ROOTS_DEEPCOPY: RootCache = RootCache::new();
static ROOTS_COPY: RootCache = RootCache::new();

python_extern! {
    /// Public C API (funcobject.h), not in pyo3's bindings.
    fn PyFunction_SetVectorcall(func: *mut ffi::PyFunctionObject, vectorcall: ffi::vectorcallfunc);
}

#[cold]
#[inline(never)]
unsafe fn leave_kernel(c: &'static accel::Counters, func: O) {
    // SAFETY: a live function object; its version is already cleared (the kernel is
    // installed), so this is the plain store `PyFunction_SetVectorcall` would make. The
    // registry's suspension then finds the original in place and only records it.
    unsafe {
        let fo = func.cast::<ffi::PyFunctionObject>();
        if (*fo).vectorcall.map(|v| v as usize) != Some(accel::FUNCTION_VECTORCALL.get() as usize) {
            PyFunction_SetVectorcall(fo, accel::FUNCTION_VECTORCALL.get());
        }
    }
    c.suspend_requested.store(true, std::sync::atomic::Ordering::Release);
    crate::accel::registry::schedule_resync();
}

/// Threads currently running a restarted `deepcopy` with a non-empty skip path.
static RESTARTING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The containers (by address) on the paths to user code found by the kernel calls being
/// restarted on this thread (a stack: nested restarts push, and truncate when they return).
/// Only an optimisation: a false match calls the original, which is always correct.
mod skip {
    use std::cell::RefCell;
    use std::sync::atomic::Ordering::Relaxed;

    use super::RESTARTING;

    struct Path {
        stack: Vec<usize>,
        index: hashbrown::HashMap<usize, u32, foldhash::fast::FixedState>,
    }

    thread_local! {
        static PATH: RefCell<Path> = const {
            RefCell::new(Path {
                stack: Vec::new(),
                index: hashbrown::HashMap::with_hasher(foldhash::fast::FixedState::with_seed(0)),
            })
        };
    }

    pub fn contains(x: super::O) -> bool {
        PATH.with(|p| {
            let p = p.borrow();
            !p.stack.is_empty() && p.index.contains_key(&(x as usize))
        })
    }

    pub fn push(path: &[usize]) -> usize {
        PATH.with(|p| {
            let mut p = p.borrow_mut();
            let mark = p.stack.len();
            if mark == 0 && !path.is_empty() {
                RESTARTING.fetch_add(1, Relaxed);
            }
            for &a in path {
                p.stack.push(a);
                *p.index.entry(a).or_insert(0) += 1;
            }
            mark
        })
    }

    pub fn truncate(mark: usize) {
        PATH.with(|p| {
            let mut p = p.borrow_mut();
            if p.stack.len() == mark {
                return;
            }
            while p.stack.len() > mark {
                let a = p.stack.pop().expect("entry");
                if let Some(c) = p.index.get_mut(&a) {
                    *c -= 1;
                    if *c == 0 {
                        p.index.remove(&a);
                    }
                }
            }
            if mark == 0 {
                RESTARTING.fetch_sub(1, Relaxed);
            }
        })
    }
}

/// Kernel for `copy.deepcopy(x, memo=None)`.
unsafe extern "C" fn vc_deepcopy(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    let fb = |why, always| {
        // SAFETY: vectorcall arguments.
        unsafe { fall(&C_DEEPCOPY, &NEST_DEEPCOPY, why, always, func, args, nargsf, kw) }
    };
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let st = st();
        let n = nargs(nargsf);
        let memo = if kw.is_null() {
            match n {
                1 => std::ptr::null_mut(),
                2 => *args.add(1),
                _ => return fb(Reason::Args, false),
            }
        } else if n == 1 && ffi::PyTuple_GET_SIZE(kw) == 1 && ffi::PyTuple_GET_ITEM(kw, 0) == st.s_memo {
            *args.add(1)
        } else {
            return fb(Reason::Args, false);
        };
        let fo = func.cast::<ffi::PyFunctionObject>();
        if !DEPS_COPY.ok() || (*fo).func_code != st.deepcopy_code || (*fo).func_defaults != st.deepcopy_defaults {
            return fb(Reason::Deps, false);
        }
        if !accel::tools_quiet(&TOOLS_DEEPCOPY, func) {
            return fb(Reason::Tools, false);
        }
        let x = *args;
        let cls = ty(x);
        let budget = recursion_remaining();
        // `if cls in _atomic_types: return x` (deepcopy's frame only; any memo, untouched).
        if is_quick(cls) && budget > 0 && quick_atomic(st, cls) {
            C_DEEPCOPY.hit_streak();
            return incref(x);
        }
        // A root whose class needs user code: straight to the original, no work done.
        if ROOTS_DEEPCOPY.blocked(cls) {
            return fb(Reason::Deopt, false);
        }
        // Any other memo's `__contains__`/`__getitem__` could be user code.
        let memo = if memo == ffi::Py_None() { std::ptr::null_mut() } else { memo };
        if !memo.is_null() && ty(memo) != &raw mut ffi::PyDict_Type {
            return fb(Reason::Args, false);
        }
        let fast = atomic_fast(st);
        // Inside a restarted call whose kernel already found user code below `x`: the
        // original's helpers reach `x` again, and the kernel would find it again.
        if RESTARTING.load(Relaxed) != 0 && skip::contains(x) {
            return fb(Reason::Deopt, false);
        }
        let mut path = Vec::new();
        let e = deep::Engine::new(st, memo, fast, budget);
        match e.run(x, &mut path) {
            Ok(y) => {
                C_DEEPCOPY.hit_streak();
                y
            }
            Err(deep::RESTART_ABANDONED) => {
                // User code (or an error, or the recursion limit) below: CPython's own call,
                // from scratch, with the unchanged arguments; the abandoned work always
                // debits the net-benefit score. While it runs, the containers on the path
                // to the node that needs user code go straight to the original when the
                // helpers reach them again (each would restart at the same node: no user
                // code ran before it), so a deep restart does not re-traverse its prefix
                // once per level.
                let mark = skip::push(&path);
                let r = fb(Reason::Deopt, true);
                skip::truncate(mark);
                r
            }
            Err(k) => {
                // User code at the root itself: nothing was wasted (a hook object reached
                // again through a fallback's helpers is only counted). When the root's
                // class decided it, later roots of that class skip the kernel's work.
                if k == deep::RESTART_ROOT_CLASS {
                    ROOTS_DEEPCOPY.block(cls);
                }
                fb(Reason::Deopt, false)
            }
        }
    }
}

/// Kernel for `copy.copy(x)`.
unsafe extern "C" fn vc_copy(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    let fb = |why| {
        // SAFETY: vectorcall arguments.
        unsafe { fall(&C_COPY, &NEST_COPY, why, false, func, args, nargsf, kw) }
    };
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let st = st();
        if !kw.is_null() || nargs(nargsf) != 1 {
            return fb(Reason::Args);
        }
        let fo = func.cast::<ffi::PyFunctionObject>();
        if !DEPS_COPY.ok() || (*fo).func_code != st.copy_code || !(*fo).func_defaults.is_null() {
            return fb(Reason::Deps);
        }
        if !accel::tools_quiet(&TOOLS_COPY, func) {
            return fb(Reason::Tools);
        }
        let x = *args;
        let cls = ty(x);
        // The root's class decides everything in `copy.copy`: one whose hook or registered
        // reductor is user code goes straight to the original.
        if ROOTS_COPY.blocked(cls) {
            return fb(Reason::Deopt);
        }
        match shallow::copy(st, x, recursion_remaining()) {
            Ok(y) => {
                C_COPY.hit_streak();
                y
            }
            Err(block) => {
                // Decided before anything was built.
                if block {
                    ROOTS_COPY.block(cls);
                }
                fb(Reason::Deopt)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------

/// Canonical code fingerprints (`rustique._rustique._accel.fingerprint`) of every Python
/// function the kernels replace or emulate.
mod fp {
    use crate::accel::Fingerprints;
    pub const DEEPCOPY: Fingerprints = &[(3, 14, "a8e15d1610cfc3b7"), (3, 15, "307dc8f19f9b5db4")];
    pub const COPY: Fingerprints = &[(3, 14, "14c52708c4954d3f"), (3, 15, "2eea511e3b3c11d5")];
    pub const DEEPCOPY_LIST: Fingerprints = &[(3, 14, "b7639b0a182a4223"), (3, 15, "0ebca81111a34cee")];
    pub const DEEPCOPY_TUPLE: Fingerprints = &[(3, 14, "21bef80950ffc7aa"), (3, 15, "7334dda0989c80a9")];
    pub const DEEPCOPY_DICT: Fingerprints = &[(3, 14, "8ae3d261d6c844e1"), (3, 15, "355063d5f74669c2")];
    pub const DEEPCOPY_FROZENDICT: Fingerprints = &[(3, 15, "7e2fe4b5411f95f3")];
    pub const DEEPCOPY_METHOD: Fingerprints = &[(3, 14, "512dee8b68330b17"), (3, 15, "5a261da2c95b4fce")];
    pub const KEEP_ALIVE: Fingerprints = &[(3, 14, "e3a4068b84c7e5ac"), (3, 15, "51a2113d512c2be0")];
    pub const RECONSTRUCT: Fingerprints = &[(3, 14, "eef7983ee166e632"), (3, 15, "cfebe2bd44245931")];
    pub const SLOTNAMES: Fingerprints = &[(3, 14, "fa693b043c4d7ca4"), (3, 15, "d2fbfedc0f8bcb0d")];
    pub const NEWOBJ: Fingerprints = &[(3, 14, "e9a487187e3c3309"), (3, 15, "dc09633e5824ebb5")];
}

/// The CPython 3.14 type sets in copy.py are mutable sets: a snapshot of the hash table.
pub(crate) struct SetSnap {
    fill: isize,
    used: isize,
    mask: isize,
    table: usize,
    entries: Vec<[usize; 2]>,
    /// For the types of [`probed`]: `(type, first, last, found)` — the slots CPython's lookup
    /// of that type inspects (`set_do_lookup`: home slot, then the linear run) and its
    /// outcome, when it ends inside the first run without calling `__eq__`.
    probes: Vec<(usize, usize, usize, bool)>,
}

/// `PySetObject` (public header); `table` entries are `setentry {key, hash}`.
#[repr(C)]
struct RawSet {
    ob_base: ffi::PyObject,
    fill: isize,
    used: isize,
    mask: isize,
    table: *mut [usize; 2],
}

/// `LINEAR_PROBES` of Objects/setobject.c.
const LINEAR_PROBES: usize = 9;

/// The types whose membership the kernels ask on every call (quick atomics, containers).
fn probed() -> [*mut ffi::PyTypeObject; 9] {
    // SAFETY: static type objects and singletons.
    unsafe {
        [
            &raw mut ffi::PyUnicode_Type,
            &raw mut ffi::PyLong_Type,
            ty(ffi::Py_None()),
            &raw mut ffi::PyFloat_Type,
            &raw mut ffi::PyBool_Type,
            &raw mut ffi::PyList_Type,
            &raw mut ffi::PyDict_Type,
            &raw mut ffi::PySet_Type,
            &raw mut ffi::PyByteArray_Type,
        ]
    }
}

/// CPython's `set` lookup of `key` (hash `h`) over `entries` (`setentry {key, hash}`),
/// restricted to its first linear run (`set_do_lookup`: the home slot, then the next
/// [`LINEAR_PROBES`] slots when they fit below the mask): `Some((last slot inspected,
/// found))` when it ends there by identity or at an empty slot; `None` when it would go on
/// (perturbation) or compare with `__eq__` (an equal hash, another key).
fn first_run_lookup(entries: &[[usize; 2]], mask: usize, key: usize, h: usize) -> Option<(usize, bool)> {
    let home = h & mask;
    let run = if home + LINEAR_PROBES <= mask { LINEAR_PROBES } else { 0 };
    for (j, &[ek, eh]) in entries.iter().enumerate().skip(home).take(run + 1) {
        if eh == 0 && ek == 0 {
            return Some((j, false));
        }
        if eh == h {
            return if ek == key { Some((j, true)) } else { None };
        }
    }
    None
}

/// 3.14 install self-test: the `set` layout and lookup [`SetSnap`] mirrors — the
/// `PySetObject` header, `setentry {key, hash}`, and linear probing over
/// [`LINEAR_PROBES`] slots — checked on a crafted set against CPython's own lookup.
fn calibrate_set_probing(py: Python<'_>) -> Result<(), Refusal> {
    let bad = |what: &str| Refusal::with(accel::Category::Layout, format!("set table layout differs: {what}"));
    let s = pyo3::types::PySet::empty(py)?;
    // Fillers at their home slots 32..60 grow the table past 64 slots, leaving 0..31 free.
    for i in 32..60 {
        s.add(i)?;
    }
    let Ok(zero) = 0i64.into_pyobject(py);
    let zero = zero.into_any();
    s.add(&zero)?;
    // SAFETY: a live exact set (public PySetObject layout); read under its lock.
    let (mask, entries) = unsafe {
        with_cs(s.as_ptr(), || {
            let r = &*s.as_ptr().cast::<RawSet>();
            (r.mask as usize, std::slice::from_raw_parts(r.table, (r.mask + 1) as usize).to_vec())
        })
    };
    if mask < 63 || r_used(&s) != 29 {
        return Err(bad("table size"));
    }
    // A key colliding with 0 at slot 0: linear probing puts it in slot 1.
    let Ok(coll) = ((mask + 1) as i64).into_pyobject(py);
    let coll = coll.into_any();
    s.add(&coll)?;
    // SAFETY: as above.
    let entries2 = unsafe {
        with_cs(s.as_ptr(), || {
            let r = &*s.as_ptr().cast::<RawSet>();
            if r.mask as usize != mask {
                return None;
            }
            Some(std::slice::from_raw_parts(r.table, (r.mask + 1) as usize).to_vec())
        })
    }
    .ok_or_else(|| bad("table resized"))?;
    if entries[0] != [zero.as_ptr() as usize, 0]
        || entries2[1] != [coll.as_ptr() as usize, mask + 1]
        || entries2[33][1] != 33
    {
        return Err(bad("setentry {key, hash} / linear probing"));
    }
    // The mirrored lookup answers as CPython's for present, colliding and absent keys.
    for k in [0i64, (mask + 1) as i64, 2 * (mask + 1) as i64, 33, 64, 31] {
        let Ok(o) = k.into_pyobject(py);
        let o = o.into_any();
        let h = o.hash()? as usize;
        let want = s.contains(&o)?;
        match first_run_lookup(&entries2, mask, o.as_ptr() as usize, h) {
            Some((_, got)) if got == want => {}
            None if !want => {} // an equal-hash miss would be left to CPython anyway
            _ => return Err(bad(&format!("lookup of {k}"))),
        }
    }
    Ok(())
}

fn r_used(s: &Bound<'_, pyo3::types::PySet>) -> isize {
    // SAFETY: a live exact set.
    unsafe { (*s.as_ptr().cast::<RawSet>()).used }
}

impl SetSnap {
    unsafe fn take(set: O) -> SetSnap {
        // SAFETY: a live set (public PySetObject layout).
        unsafe {
            with_cs(set, || {
                let s = &*set.cast::<RawSet>();
                let entries = std::slice::from_raw_parts(s.table, (s.mask + 1) as usize).to_vec();
                let mask = s.mask as usize;
                let mut probes = Vec::new();
                for t in probed() {
                    // Type objects hash by address (metatype `type`).
                    let h = ffi::PyObject_Hash(t.cast()) as usize;
                    if let Some((last, found)) = first_run_lookup(&entries, mask, t as usize, h) {
                        probes.push((t as usize, h & mask, last, found));
                    }
                }
                SetSnap { fill: s.fill, used: s.used, mask: s.mask, table: s.table as usize, entries, probes }
            })
        }
    }

    #[inline(always)]
    unsafe fn matches(&self, set: O) -> bool {
        // SAFETY: as take.
        unsafe {
            with_cs(set, || {
                let s = &*set.cast::<RawSet>();
                s.fill == self.fill
                    && s.used == self.used
                    && s.mask == self.mask
                    && s.table as usize == self.table
                    && std::slice::from_raw_parts(s.table, (s.mask + 1) as usize) == &self.entries[..]
            })
        }
    }

    /// `t in set` for a probed type, exactly as CPython's lookup answers it, when the slots
    /// it inspects are unchanged (`None`: ask [`Self::matches`]).
    #[inline(always)]
    unsafe fn probe(&self, set: O, t: *mut ffi::PyTypeObject) -> Option<bool> {
        let &(_, first, last, found) = self.probes.iter().find(|p| p.0 == t as usize)?;
        // SAFETY: as take.
        unsafe {
            with_cs(set, || {
                let s = &*set.cast::<RawSet>();
                (s.mask == self.mask
                    && s.table as usize == self.table
                    && std::slice::from_raw_parts(s.table.add(first), last - first + 1) == &self.entries[first..=last])
                    .then_some(found)
            })
        }
    }
}

fn types(list: &[*mut ffi::PyTypeObject]) -> Vec<*mut ffi::PyTypeObject> {
    list.to_vec()
}

/// `copy._atomic_types` in the kernels' order (most frequent first).
fn atomic_types() -> Vec<*mut ffi::PyTypeObject> {
    // SAFETY: static type objects and singletons.
    unsafe {
        types(&[
            &raw mut ffi::PyUnicode_Type,
            &raw mut ffi::PyLong_Type,
            ty(ffi::Py_None()),
            &raw mut ffi::PyFloat_Type,
            &raw mut ffi::PyBool_Type,
            &raw mut ffi::PyComplex_Type,
            &raw mut ffi::PyBytes_Type,
            &raw mut ffi::PyType_Type,
            &raw mut ffi::PyRange_Type,
            &raw mut ffi::PyCode_Type,
            &raw mut ffi::PyCFunction_Type,
            &raw mut ffi::PyFunction_Type,
            &raw mut ffi::_PyWeakref_RefType,
            &raw mut ffi::PyProperty_Type,
            &raw mut ffi::PyEllipsis_Type,
            ty(ffi::Py_NotImplemented()),
        ])
    }
}

fn copy_atomic_types(minor: u8) -> Vec<*mut ffi::PyTypeObject> {
    // SAFETY: static type objects and singletons.
    unsafe {
        let mut v = types(&[
            ty(ffi::Py_None()),
            &raw mut ffi::PyLong_Type,
            &raw mut ffi::PyFloat_Type,
            &raw mut ffi::PyBool_Type,
            &raw mut ffi::PyComplex_Type,
            &raw mut ffi::PyUnicode_Type,
            &raw mut ffi::PyTuple_Type,
            &raw mut ffi::PyBytes_Type,
            &raw mut ffi::PyFrozenSet_Type,
            &raw mut ffi::PyType_Type,
            &raw mut ffi::PyRange_Type,
            &raw mut ffi::PySlice_Type,
            &raw mut ffi::PyProperty_Type,
            &raw mut ffi::PyCFunction_Type,
            &raw mut ffi::PyEllipsis_Type,
            ty(ffi::Py_NotImplemented()),
            &raw mut ffi::PyFunction_Type,
            &raw mut ffi::PyCode_Type,
            &raw mut ffi::_PyWeakref_RefType,
            &raw mut ffi::PySuper_Type,
        ]);
        if minor >= 15 {
            v.push(frozendict_type());
        }
        v
    }
}

#[cfg(Py_3_15)]
fn frozendict_type() -> *mut ffi::PyTypeObject {
    &raw mut ffi::PyFrozenDict_Type
}

#[cfg(not(Py_3_15))]
fn frozendict_type() -> *mut ffi::PyTypeObject {
    std::ptr::null_mut()
}

/// Does `set` hold exactly `want` (types: hashing and comparing them runs no Python code)?
fn same_type_set(set: &Bound<'_, PyAny>, want: &[*mut ffi::PyTypeObject]) -> PyResult<bool> {
    let n = set.len()?;
    if n != want.len() {
        return Ok(false);
    }
    for &t in want {
        // SAFETY: a live set and a static type.
        if unsafe { ffi::PySet_Contains(set.as_ptr(), t.cast()) } != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn intern(py: Python<'_>, s: &str) -> O {
    pyo3::types::PyString::intern(py, s).into_ptr()
}

/// The offset of `py_recursion_remaining` in `PyThreadState` (public cpython/pystate.h
/// layout: 3.15 added `gil_requested`), checked against `sys.getrecursionlimit()`.
pub(crate) fn tstate_offset(cx: &Prepare<'_>, minor: u8) -> Result<usize, Refusal> {
    let off = if minor >= 15 { 52 } else { 48 };
    if REM_OFF.load(Relaxed) == off {
        return Ok(off);
    }
    let sys = cx.loaded_module("sys")?;
    let limit: c_int = sys.getattr("getrecursionlimit")?.call0()?.extract()?;
    // SAFETY: attached thread; reads within PyThreadState.
    let (rem, lim) = unsafe {
        let ts = ffi::PyThreadState_Get().cast::<u8>();
        (*ts.add(off).cast::<c_int>(), *ts.add(off + 4).cast::<c_int>())
    };
    if lim != limit || rem <= 0 || rem > limit {
        return Err(Refusal::with(
            accel::Category::Layout,
            format!("PyThreadState layout differs (limit {lim} vs {limit}, remaining {rem})"),
        ));
    }
    REM_OFF.store(off, Relaxed);
    Ok(off)
}

/// `d[k]` of an exact dict with a str key (no Python code runs).
fn dget<'py>(d: &Bound<'py, PyDict>, k: &str) -> Result<Bound<'py, PyAny>, Refusal> {
    d.get_item(k)?.ok_or_else(|| crate::refuse!("{k} missing"))
}

/// (function, code, defaults) of a pure-Python function, read from the struct (strong refs).
fn func3(o: &Bound<'_, PyAny>) -> PyResult<(O, O, O)> {
    if o.cast::<pyo3::types::PyFunction>().is_err() {
        return Err(pyo3::exceptions::PyTypeError::new_err("not a function"));
    }
    // SAFETY: an exact function object; its code/defaults are kept alive by the strong refs.
    unsafe {
        let f = o.as_ptr().cast::<ffi::PyFunctionObject>();
        let d = (*f).func_defaults;
        Ok((incref(o.as_ptr()), incref((*f).func_code), if d.is_null() { d } else { incref(d) }))
    }
}

/// `cx.fingerprint_code` without building the description string of a passing check (a
/// third of its cost) unless fingerprints are being recorded.
pub(crate) fn check_fp(
    cx: &mut Prepare<'_>,
    f: &Bound<'_, PyAny>,
    expected: crate::accel::Fingerprints,
) -> Result<(), Refusal> {
    if !cx.record {
        let (major, minor) = cx.version();
        let got = cx.fingerprint_of(f)?;
        if crate::accel::fingerprint::expected_for(expected, major, minor) == Some(got.as_str()) {
            return Ok(());
        }
    }
    cx.fingerprint_code(f, expected)
}

/// [`St`] if it was built from this very copy module and nothing it validated changed.
fn current_state(cd: &Bound<'_, PyDict>, rd: &Bound<'_, PyDict>) -> Result<Option<&'static St>, Refusal> {
    let p = ST.load(Relaxed);
    if p.is_null() {
        return Ok(None);
    }
    // SAFETY: never freed.
    let st: &'static St = unsafe { &*p };
    if st.copy_dict != cd.as_ptr() {
        return Ok(None);
    }
    let names = [
        "_deepcopy_list",
        "_deepcopy_tuple",
        "_deepcopy_dict",
        "_deepcopy_frozendict",
        "_deepcopy_method",
        "_keep_alive",
        "_reconstruct",
    ];
    for (i, n) in names.iter().enumerate() {
        let (f, code, _) = st.helpers[i];
        if f.is_null() {
            continue;
        }
        let cur = dget(cd, n)?;
        // SAFETY: a function object (identity-checked).
        if cur.as_ptr() != f || unsafe { (*f.cast::<ffi::PyFunctionObject>()).func_code } != code {
            return Ok(None);
        }
    }
    for (i, n) in [(H_NEWOBJ, "__newobj__"), (H_SLOTNAMES, "_slotnames")] {
        let (f, code, _) = st.helpers[i];
        let cur = dget(rd, n)?;
        // SAFETY: as above.
        if cur.as_ptr() != f || unsafe { (*f.cast::<ffi::PyFunctionObject>()).func_code } != code {
            return Ok(None);
        }
    }
    let same = |k: &str, want: O| -> Result<bool, Refusal> { Ok(dget(cd, k)?.as_ptr() == want) };
    // SAFETY: function objects.
    let code_of = |f: &Bound<'_, PyAny>| unsafe { (*f.as_ptr().cast::<ffi::PyFunctionObject>()).func_code };
    let ok = same("deepcopy", st.deepcopy_fn)?
        && code_of(&dget(cd, "deepcopy")?) == st.deepcopy_code
        && code_of(&dget(cd, "copy")?) == st.copy_code
        && same("_atomic_types", st.atomic)?
        && same("_copy_atomic_types", st.copy_atomic)?
        && same("_copy_builtin_containers", st.copy_containers)?
        && same("_deepcopy_dispatch", st.dispatch)?
        && same("dispatch_table", st.dispatch_table)?;
    Ok(if ok { Some(st) } else { None })
}

/// Validate the copy module and build [`St`] (shared by both targets and by the
/// dataclasses group); refuses on any difference from the validated sources.
fn ensure_state(py: Python<'_>, cx: &mut Prepare<'_>, copy: &Bound<'_, PyModule>) -> Result<&'static St, Refusal> {
    let (_, minor) = cx.version();
    tstate_offset(cx, minor)?;
    let copyreg = cx.loaded_module("copyreg")?;
    let cd = copy.dict();
    let rd = copyreg.dict();
    let get = |k: &str| dget(&cd, k);
    // The group's other target already validated this module: same functions and code
    // objects (the fingerprinted ones), same sets.
    if !cx.record
        && let Some(st) = current_state(&cd, &rd)?
    {
        return Ok(st);
    }
    let deepcopy = get("deepcopy")?;
    let copy_fn = get("copy")?;
    check_fp(cx, &deepcopy, fp::DEEPCOPY)?;
    check_fp(cx, &copy_fn, fp::COPY)?;
    let h_list = get("_deepcopy_list")?;
    let h_tuple = get("_deepcopy_tuple")?;
    let h_dict = get("_deepcopy_dict")?;
    let h_method = get("_deepcopy_method")?;
    let keep_alive = get("_keep_alive")?;
    let reconstruct = get("_reconstruct")?;
    let newobj = dget(&rd, "__newobj__")?;
    let slotnames = dget(&rd, "_slotnames")?;
    check_fp(cx, &slotnames, fp::SLOTNAMES)?;
    check_fp(cx, &h_list, fp::DEEPCOPY_LIST)?;
    check_fp(cx, &h_tuple, fp::DEEPCOPY_TUPLE)?;
    check_fp(cx, &h_dict, fp::DEEPCOPY_DICT)?;
    check_fp(cx, &h_method, fp::DEEPCOPY_METHOD)?;
    check_fp(cx, &keep_alive, fp::KEEP_ALIVE)?;
    check_fp(cx, &reconstruct, fp::RECONSTRUCT)?;
    check_fp(cx, &newobj, fp::NEWOBJ)?;
    let h_frozendict = if minor >= 15 {
        let f = get("_deepcopy_frozendict")?;
        check_fp(cx, &f, fp::DEEPCOPY_FROZENDICT)?;
        Some(f)
    } else {
        None
    };
    // The type sets copy.py consults must hold exactly the types the kernels assume.
    let atomic = atomic_types();
    let copy_atomic = copy_atomic_types(minor);
    let containers = vec![
        &raw mut ffi::PyList_Type,
        &raw mut ffi::PyDict_Type,
        &raw mut ffi::PySet_Type,
        &raw mut ffi::PyByteArray_Type,
    ];
    if !same_type_set(&get("_atomic_types")?, &atomic)?
        || !same_type_set(&get("_copy_atomic_types")?, &copy_atomic)?
        || !same_type_set(&get("_copy_builtin_containers")?, &containers)?
    {
        return Err("copy's type sets were modified".into());
    }
    if !get("dispatch_table")?.is(&dget(&rd, "dispatch_table")?) {
        return Err("copy.dispatch_table is not copyreg.dispatch_table".into());
    }
    let nul = std::ptr::null_mut();
    let helpers = [
        func3(&h_list)?,
        func3(&h_tuple)?,
        func3(&h_dict)?,
        match &h_frozendict {
            Some(f) => func3(f)?,
            None => (nul, nul, nul),
        },
        func3(&h_method)?,
        func3(&keep_alive)?,
        func3(&reconstruct)?,
        func3(&newobj)?,
        func3(&slotnames)?,
    ];
    let (_, deepcopy_code, deepcopy_defaults) = func3(&deepcopy)?;
    let (_, copy_code, _) = func3(&copy_fn)?;
    let atomic_set = get("_atomic_types")?;
    let get_method = cx.symbol(c"_PyObject_GetMethod", &GET_METHOD)?;
    let object_attr = |name: &str| -> O {
        let n = pyo3::types::PyString::intern(py, name);
        // SAFETY: a ready static type; borrowed result kept alive by `object` (made strong).
        unsafe {
            let o = _PyType_Lookup(&raw mut ffi::PyBaseObject_Type, n.as_ptr());
            if o.is_null() { o } else { incref(o) }
        }
    };
    let mut st = St {
        minor,
        copy_dict: cd.clone().into_ptr(),
        deepcopy_fn: deepcopy.into_ptr(),
        deepcopy_code,
        deepcopy_defaults,
        copy_code,
        atomic: atomic_set.into_ptr(),
        atomic_snap: None,
        copy_atomic_snap: None,
        copy_containers_snap: None,
        copy_atomic: get("_copy_atomic_types")?.into_ptr(),
        copy_containers: get("_copy_builtin_containers")?.into_ptr(),
        copy_atomic_types: copy_atomic,
        copy_container_types: containers,
        dispatch: get("_deepcopy_dispatch")?.into_ptr(),
        dispatch_table: get("dispatch_table")?.into_ptr(),
        h_list: helpers[H_LIST].0,
        h_tuple: helpers[H_TUPLE].0,
        h_dict: helpers[H_DICT].0,
        h_frozendict: helpers[H_FROZENDICT].0,
        h_method: helpers[H_METHOD].0,
        helpers,
        newobj: helpers[H_NEWOBJ].0,
        object_new: object_attr("__new__"),
        obj_reduce_ex: object_attr("__reduce_ex__"),
        obj_reduce: object_attr("__reduce__"),
        obj_getstate: object_attr("__getstate__"),
        get_method,
        get_method_addr: get_method as usize,
        s_getstate: intern(py, "__getstate__"),
        s_getnewargs: intern(py, "__getnewargs__"),
        s_getnewargs_ex: intern(py, "__getnewargs_ex__"),
        s_slotnames: intern(py, "__slotnames__"),
        s_slots: intern(py, "__slots__"),
        s_getinitargs: intern(py, "__getinitargs__"),
        s_del: intern(py, "__del__"),
        atomic_types: atomic.try_into().map_err(|_| Refusal::new("atomic types"))?,
        // SAFETY: static type objects.
        method_type: &raw mut PyMethod_Type,
        frozendict_type: frozendict_type(),
        // SAFETY: static type object read.
        s_deepcopy: intern(py, "__deepcopy__"),
        s_copy: intern(py, "__copy__"),
        s_copy_method: intern(py, "copy"),
        s_memo: intern(py, "memo"),
        s_reduce_ex: intern(py, "__reduce_ex__"),
        s_reduce: intern(py, "__reduce__"),
        s_setstate: intern(py, "__setstate__"),
        s_dict: intern(py, "__dict__"),
        s_new: intern(py, "__new__"),
    };
    if minor < 15 {
        calibrate_set_probing(py)?;
    }
    if minor < 15 {
        // SAFETY: a live set.
        st.atomic_snap = Some(unsafe { SetSnap::take(st.atomic) });
        // SAFETY: live sets.
        st.copy_atomic_snap = Some(unsafe { SetSnap::take(st.copy_atomic) });
        // SAFETY: live sets.
        st.copy_containers_snap = Some(unsafe { SetSnap::take(st.copy_containers) });
    }
    let st: &'static St = Box::leak(Box::new(st));
    ST.store((st as *const St).cast_mut(), Relaxed);
    Ok(st)
}

/// Pin what copy.py reads at call time (shared flag of the group): copy's globals as one
/// snapshot (every binding the kernels validated, and the absence of any global that would
/// shadow a builtin), the builtins it uses, `copyreg.__newobj__`/`_slotnames`, and
/// `sys.modules['copyreg']` (what `object.__reduce_ex__` imports).
fn pin_deps(cx: &mut Prepare<'_>, copy: &Bound<'_, PyModule>) -> Result<(), Refusal> {
    let (_, minor) = cx.version();
    let g = copy.dict();
    cx.pin_dict(&g, &DEPS_COPY)?;
    let builtins = cx.builtins_of(&g)?;
    let mut names =
        vec!["type", "id", "issubclass", "getattr", "isinstance", "hasattr", "len", "setattr", "zip", "tuple"];
    if minor >= 15 {
        names.push("frozendict");
    }
    for k in names {
        cx.pin(&builtins, k, &DEPS_COPY)?;
    }
    let copyreg = cx.loaded_module("copyreg")?;
    cx.pin(&copyreg.dict(), "__newobj__", &DEPS_COPY)?;
    cx.pin(&copyreg.dict(), "_slotnames", &DEPS_COPY)?;
    let modules = cx.loaded_module("sys")?.dict();
    let modules = cx.get(&modules, "modules")?.cast_into::<PyDict>().map_err(PyErr::from)?;
    cx.pin(&modules, "copyreg", &DEPS_COPY)?;
    Ok(())
}

fn prepare_shared(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let copy = cx.module()?.clone();
    ensure_state(py, cx, &copy)?;
    for d in [dget(&copy.dict(), "_deepcopy_dispatch")?, dget(&cx.loaded_module("copyreg")?.dict(), "dispatch_table")?]
    {
        let d = d.cast_into::<PyDict>().map_err(PyErr::from)?;
        accel::guard::watch_dict_generation(&d, &REGISTRATIONS)?;
    }
    pin_deps(cx, &copy)
}

/// [`St`], as the group's shared prepare built it from this very module.
fn group_state(cx: &Prepare<'_>) -> Result<&'static St, Refusal> {
    let p = ST.load(Relaxed);
    // SAFETY: never freed.
    if p.is_null() || unsafe { (*p).copy_dict } != cx.module()?.dict().as_ptr() {
        return Err("copy group state missing".into());
    }
    // SAFETY: as above.
    Ok(unsafe { &*p })
}

fn prepare_deepcopy(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    group_state(cx)?;
    let f = cx.module_attr("deepcopy")?;
    cx.vectorcall(&f, vc_deepcopy, &accel::FUNCTION_VECTORCALL)
}

fn prepare_copy(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    group_state(cx)?;
    let f = cx.module_attr("copy")?;
    cx.vectorcall(&f, vc_copy, &accel::FUNCTION_VECTORCALL)
}

/// `tstate->py_recursion_remaining` as the kernels see it (tests).
#[pyfunction]
fn copy_recursion_remaining() -> PyResult<c_int> {
    let p = ST.load(Relaxed);
    if p.is_null() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("copy group never prepared"));
    }
    // SAFETY: attached thread.
    Ok(unsafe { recursion_remaining() })
}

/// Python-visible helpers (on `rustique._rustique._accel`).
/// Whether roots of `cls` go straight to the original in `deepcopy` / `copy.copy` (tests).
#[pyfunction]
fn copy_root_blocked(cls: &Bound<'_, pyo3::types::PyType>) -> (bool, bool) {
    let t = cls.as_type_ptr();
    // SAFETY: a live type object.
    unsafe { (ROOTS_DEEPCOPY.blocked(t), ROOTS_COPY.blocked(t)) }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(copy_recursion_remaining, m)?)?;
    m.add_function(wrap_pyfunction!(copy_root_blocked, m)?)
}
