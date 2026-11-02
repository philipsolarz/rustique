//! What a target declares (docs/ACCELERATION.md §9): [`TargetSpec`], the typed handle on a
//! site's original entry point [`Original`], and the reason a target refuses to install
//! ([`Refusal`]).

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::prelude::*;

use super::prepare::Prepare;
use super::stats::Counters;

/// `prepare` of a target: validate (fingerprints, layouts, self-test) and declare the
/// sites; the registry swaps them all-or-nothing after this returns `Ok`.
pub type PrepareFn = for<'a, 'py> fn(Python<'py>, &'a mut Prepare<'py>) -> Result<(), Refusal>;

/// One accelerated piece of behaviour.
///
/// Write specs with `..TargetSpec::DEFAULT` so fields added later stay additive:
///
/// ```ignore
/// pub static DUMPS: TargetSpec = TargetSpec {
///     name: "json.dumps",
///     group: "json",
///     module: Some("json"),
///     versions: &[(3, 14), (3, 15)],
///     free_threaded: true,
///     counters: &DUMPS_COUNTERS,
///     prepare: prepare_dumps,
///     ..TargetSpec::DEFAULT
/// };
/// ```
#[derive(Debug)]
pub struct TargetSpec {
    /// Stable dotted name (`"json.dumps"`), unique across all targets.
    pub name: &'static str,
    /// The unit users switch on and off (`"json"`). Groups starting with `_` are hidden:
    /// selected only when named explicitly in `enable(only=...)`.
    pub group: &'static str,
    /// The module whose import triggers install (`prepare` then sees it as
    /// [`Prepare::module`]); `None`: installed at `enable()` (builtins, static types).
    pub module: Option<&'static str>,
    /// Validated `(major, minor)` versions.
    pub versions: &'static [(u8, u8)],
    /// Validated on free-threaded builds too.
    pub free_threaded: bool,
    /// Validate at install, never arm (first-call validation, docs/ACCELERATION.md §3.2):
    /// for targets that must act from install on although their sites fire rarely (a
    /// policy such as `gc.adaptive`). Default `false`.
    pub eager: bool,
    /// Opt-in tier (`Some("interp")`, `Some("gc")`): not in the default set; installed only
    /// when the tier is requested (`RUSTIQUE_TIERS=interp,gc`, `rustique.enable(tiers=[...])`),
    /// and then like any target (`only`/`exclude` still apply; a tier name works in both).
    /// Default `None`.
    pub opt_in: Option<&'static str>,
    /// Hit / fallback counters of the target's kernels.
    pub counters: &'static Counters,
    /// Validate and declare the sites.
    pub prepare: PrepareFn,
    /// Meets the never-slower rule under the JIT too. When the JIT is enabled in the
    /// process (at `enable()`), targets with `jit_safe: false` refuse with reason `"jit"`
    /// (vectorcall fallbacks cost +250–350 Ir there). Default `true`.
    pub jit_safe: bool,
    /// The exact subset runs no user code and has no side effects, so check mode may run
    /// the original a second time and compare (`check::verify`). Default `false`.
    pub checkable: bool,
    /// Declarations shared with other targets (typically one module's dependencies):
    /// prepared once, before the first target that names it, and kept while any of them is
    /// installed. Default `None`.
    pub shared: Option<&'static GroupSpec>,
    /// Net-benefit policy of vectorcall targets (`Counters::hit_streak` /
    /// `fallback_streak`): a hit credits `hit_credit`, a non-`Deps` fallback debits
    /// `fallback_debit`, and the target is suspended when the debt reaches
    /// `64 × fallback_debit`. It stays installed while hits are at least
    /// `debit / (credit + debit)` of its calls: set the ratio from the measured
    /// penalty/savings (a kernel saving 400 Ir per hit and costing 200 Ir per fallback:
    /// credit 2, debit 1). Defaults 4 / 1 (20%).
    pub hit_credit: u8,
    pub fallback_debit: u8,
    /// Credits v2 (docs/atlas/w2-floors.md §3): the extra debit of a [`Reason::Abandon`]
    /// fallback (a restart after kernel work, which is wasted), on top of `fallback_debit`;
    /// set it from the work an abandoned call wastes relative to the fallback penalty
    /// (`continue` and root `deopt` restarts waste nothing). The decision window
    /// ([`stats::WINDOW`](super::stats::WINDOW)) applies to every target. Default 1.
    ///
    /// [`Reason::Abandon`]: super::stats::Reason::Abandon
    pub abandon_debit: u8,
    /// Called once at interpreter exit (the registry's atexit hook; once per distinct
    /// function when several targets share one), after every site was restored, every pin
    /// dropped and every dependency flag cleared: drop the target's own static references
    /// to Python objects (verified objects, cached functions), so finalization does not
    /// keep module objects alive through them. A kernel may still be entered afterwards on
    /// a stale path (a call already past the vectorcall pointer on another thread) and
    /// must fall back (its flags are false); a static it reads without a flag check must
    /// stay valid or be checked for null. Default `None`.
    pub release: Option<fn(Python<'_>)>,
    /// Called each time the target is restored after having been installed (`disable()`,
    /// the exit hook, a refusal after install) and when it is suspended, after its sites
    /// were restored (and, except for a suspension, its pins dropped), on an attached
    /// thread, outside the pin tables lock: restore state the target changed that is not a
    /// site (calibrated internal state, routed slots, caches). A suspension may be decided
    /// inside a call (the falling-back kernel performs it): the hook must run no Python
    /// code and release no reference that could. Must tolerate running more than once; a
    /// resumed target gets `on_install` again. Default `None`.
    pub on_disable: Option<fn(Python<'_>)>,
    /// Called after the target was installed (at `enable()`, or after a lazy activation on
    /// the importing path), outside the registry lock and every watcher, on an attached
    /// thread, once per install: e.g. adopt objects created before `enable()` with
    /// `accel::dynamic::install`. The target may have been disabled again meanwhile (the
    /// hook is skipped then). Default `None`.
    pub on_install: Option<fn(Python<'_>)>,
}

/// Declarations shared by several targets: dependency pins (with the group's own
/// `DepFlag`s), fingerprints and validations, made once instead of once per target. Its
/// `prepare` gets the same [`Prepare`] context as a target's (same module) but must not
/// declare sites. If it refuses, every target that names it is refused with that reason.
/// Its pins stay active while at least one of those targets is installed; a pin that drops
/// suspends (vectorcall targets) or falls back every target that checks the group's flag.
#[derive(Debug)]
pub struct GroupSpec {
    /// A unique name (`"pathlib"`), for status and diagnostics.
    pub name: &'static str,
    pub prepare: PrepareFn,
}

static NO_COUNTERS: Counters = Counters::new();

fn no_prepare(_py: Python<'_>, _cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    Err(Refusal::new("target declares no prepare function"))
}

impl TargetSpec {
    /// Defaults for every field: use as `..TargetSpec::DEFAULT`.
    pub const DEFAULT: TargetSpec = TargetSpec {
        name: "",
        group: "",
        module: None,
        versions: &[],
        free_threaded: false,
        eager: false,
        opt_in: None,
        counters: &NO_COUNTERS,
        prepare: no_prepare,
        jit_safe: true,
        checkable: false,
        shared: None,
        hit_credit: crate::accel::stats::HIT_CREDIT,
        fallback_debit: crate::accel::stats::FALLBACK_DEBIT,
        abandon_debit: crate::accel::stats::FALLBACK_DEBIT,
        release: None,
        on_disable: None,
        on_install: None,
    };

    /// Hidden targets (group starting with `_`): only selected when named explicitly.
    pub fn hidden(&self) -> bool {
        self.group.starts_with('_')
    }
}

/// Opt-in tiers known to this build even when none of their targets is compiled in
/// (`enable(tiers=["interp"])` must not raise where the tier is unavailable: its targets
/// would refuse anyway).
pub const KNOWN_TIERS: &[&str] = &["interp", "gc"];

/// Machine-readable class of a refusal (`status()["targets"][i]["refusal"]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// CPython version not validated (or pre-release).
    Version,
    /// Build not validated (debug, free-threaded without validation).
    Build,
    /// The JIT is on and the target is not `jit_safe`.
    Jit,
    /// Operating system or C library not validated (e.g. glibc-only behaviour on macOS,
    /// musl or Windows; no runtime symbol lookup on Windows).
    Platform,
    /// An original's fingerprint differs from the recorded one.
    Fingerprint,
    /// An object layout the kernel reads differs.
    Layout,
    /// Someone else changed an entry point (CAS failure, wrapper mismatch).
    Conflict,
    /// A behavioural self-test failed.
    Selftest,
    /// A required exported symbol is missing.
    Symbol,
    /// A dependency is missing or not the validated object.
    Deps,
    /// The running CPython patch release was not recorded (validated) for this build of
    /// Rustique (`RUSTIQUE_UNVALIDATED_PATCH=1` installs the targets whose fingerprints
    /// match a recorded release, reported "unvalidated").
    UnvalidatedPatch,
    /// The target is not needed here (what it fixes or speeds up does not exist on this
    /// build, e.g. a free-threaded-only contention fix on a GIL build): not a failure;
    /// reported apart from refusals.
    NotNeeded,
    /// Anything else (including errors raised while preparing).
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Version => "version",
            Category::Build => "build",
            Category::Jit => "jit",
            Category::Platform => "platform",
            Category::Fingerprint => "fingerprint",
            Category::Layout => "layout",
            Category::Conflict => "conflict",
            Category::Selftest => "selftest",
            Category::Symbol => "symbol",
            Category::Deps => "deps",
            Category::UnvalidatedPatch => "unvalidated-patch",
            Category::NotNeeded => "not-needed",
            Category::Other => "other",
        }
    }
}

/// Why a target did not install: a category and a human-readable reason, shown in
/// `rustique.status()`. Build with [`Refusal::new`] (category `Other`), [`Refusal::with`],
/// `refuse!(...)`, or `?` on a `PyResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    reason: Cow<'static, str>,
    category: Category,
}

impl Refusal {
    pub fn new(reason: impl Into<Cow<'static, str>>) -> Self {
        Refusal { reason: reason.into(), category: Category::Other }
    }
    /// Not needed on this build (category `not-needed`, reported apart from refusals).
    pub fn not_needed(reason: impl Into<Cow<'static, str>>) -> Self {
        Refusal { reason: reason.into(), category: Category::NotNeeded }
    }
    pub fn with(category: Category, reason: impl Into<Cow<'static, str>>) -> Self {
        Refusal { reason: reason.into(), category }
    }
    /// The same refusal under another category (keeps an explicit one: only `Other` is
    /// replaced).
    pub fn or_category(mut self, category: Category) -> Self {
        if self.category == Category::Other {
            self.category = category;
        }
        self
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
    pub fn category(&self) -> Category {
        self.category
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl From<PyErr> for Refusal {
    fn from(e: PyErr) -> Self {
        Python::attach(|py| Refusal::new(format!("error: {}", e.value(py))))
    }
}

impl From<String> for Refusal {
    fn from(s: String) -> Self {
        Refusal::new(s)
    }
}

impl From<&'static str> for Refusal {
    fn from(s: &'static str) -> Self {
        Refusal::new(s)
    }
}

/// `return Err(refuse!("format", args))` shorthand.
#[macro_export]
macro_rules! refuse {
    ($cat:ident: $($arg:tt)*) => {
        $crate::accel::Refusal::with($crate::accel::spec::Category::$cat, format!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::accel::Refusal::from(format!($($arg)*))
    };
}

/// A C function pointer type a site can hold (`ml_meth` conventions, type slots,
/// `vectorcallfunc`). Implemented for every `unsafe extern "C" fn` of 1 to 5 arguments.
///
/// # Safety
/// Implementors are plain function pointers (pointer-sized, never null).
pub unsafe trait FnPtr: Copy + 'static {
    /// Number of C arguments (checked against `ml_flags` and slot signatures).
    const ARITY: usize;
    fn addr(self) -> usize;
    /// # Safety
    /// `addr` is a non-null function of this exact signature.
    unsafe fn from_addr(addr: usize) -> Self;
}

macro_rules! impl_fnptr {
    ($n:expr; $($a:ident),*) => {
        // SAFETY: function pointers are pointer-sized and non-null.
        unsafe impl<R: 'static, $($a: 'static),*> FnPtr for unsafe extern "C" fn($($a),*) -> R {
            const ARITY: usize = $n;
            #[inline(always)]
            fn addr(self) -> usize {
                self as usize
            }
            #[inline(always)]
            unsafe fn from_addr(addr: usize) -> Self {
                // SAFETY: per the caller.
                unsafe { std::mem::transmute::<usize, Self>(addr) }
            }
        }
    };
}
impl_fnptr!(1; A);
impl_fnptr!(2; A, B);
impl_fnptr!(3; A, B, C);
impl_fnptr!(4; A, B, C, D);
impl_fnptr!(5; A, B, C, D, E);

/// The entry point a site held when its target was validated: what the kernel falls back
/// to. One static per site (sites with the same original may share one). Filled by the
/// registry before the kernel becomes reachable and never cleared (kernels may still run
/// after `disable()`: a concurrent call, or a subinterpreter's captured slot wrapper).
pub struct Original<F> {
    addr: AtomicUsize,
    _f: PhantomData<F>,
}

// SAFETY: an atomic address; `F` is a function pointer type.
unsafe impl<F> Sync for Original<F> {}

impl<F: FnPtr> Original<F> {
    pub const fn new() -> Self {
        Original { addr: AtomicUsize::new(0), _f: PhantomData }
    }

    /// The original function. Only call from a kernel installed through this `Original`
    /// (it is set before the kernel is reachable).
    #[inline(always)]
    pub fn get(&self) -> F {
        let a = self.addr.load(Ordering::Relaxed);
        debug_assert!(a != 0, "Original read before install");
        // SAFETY: set by the registry to the site's original, of signature `F`, before the
        // kernel could be reached; never cleared.
        unsafe { F::from_addr(a) }
    }

    /// Whether the registry captured it already.
    pub fn is_set(&self) -> bool {
        self.addr.load(Ordering::Relaxed) != 0
    }

    pub fn addr(&self) -> usize {
        self.addr.load(Ordering::Relaxed)
    }

    /// Record `addr` (first capture), or check it equals the captured one.
    pub(crate) fn capture(&self, addr: usize) -> Result<(), Refusal> {
        match self.addr.compare_exchange(0, addr, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(()),
            Err(prev) if prev == addr => Ok(()),
            Err(_) => Err(Refusal::with(
                Category::Conflict,
                "entry point differs from the original captured at first install (patched by someone else?)",
            )),
        }
    }
}

impl<F: FnPtr> Default for Original<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> fmt::Debug for Original<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Original({:#x})", self.addr.load(Ordering::Relaxed))
    }
}
