//! The acceleration framework (docs/ACCELERATION.md): put a Rust kernel behind an existing
//! CPython entry point, exactly or not at all.
//!
//! - [`spec`]: [`TargetSpec`] (what a target declares), [`Original`] (typed handle on a
//!   site's original entry point, the kernel's fallback), [`Refusal`].
//! - [`prepare`]: [`Prepare`], the validation and declaration API a target's `prepare`
//!   uses (sites, dependency pins, fingerprints, self-tests).
//! - [`registry`]: selection, version gate, all-or-nothing install, lazy activation on
//!   import, `disable()`, status, counters.
//! - [`site`]: patch sites (`ml_meth`, type slots, the vectorcall of a Python function).
//! - [`guard`]: per-call guards (tools listening, interpreter, dependency pins).
//! - [`stats`]: per-target hit / fallback counters, free when nobody looks.
//! - [`version`]: interpreter facts and the gate.
//! - [`fingerprint`]: canonical fingerprints of Python and C originals.
//! - [`check`]: check mode (compare fast-path results with the original's).
//!
//! A target in one picture:
//!
//! ```ignore
//! use crate::accel::{self, Original, Prepare, Reason, Refusal, TargetSpec, Counters, DepFlag, Tools};
//!
//! static COUNTERS: Counters = Counters::new();
//! static DEPS: DepFlag = DepFlag::new();
//! static TOOLS: Tools = Tools::new();
//!
//! unsafe extern "C" fn k_join(f: *mut PyObject, a: *const *mut PyObject, n: usize, kw: *mut PyObject) -> *mut PyObject {
//!     if /* exact subset */ && DEPS.ok() && unsafe { accel::tools_quiet(&TOOLS, f) } {
//!         COUNTERS.hit();
//!         return /* fast path */;
//!     }
//!     COUNTERS.fallback(Reason::Args);
//!     unsafe { accel::FUNCTION_VECTORCALL.get()(f, a, n, kw) }
//! }
//!
//! fn prepare(py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
//!     let join = cx.module_attr("join")?;
//!     cx.fingerprint_code(&join, &[(3, 14, "…"), (3, 15, "…")])?;
//!     let g = cx.module()?.dict();
//!     cx.pin(&g, "_get_sep", &DEPS)?;
//!     cx.pin_global(&g, "len", &DEPS)?;
//!     cx.vectorcall(&join, k_join, &accel::FUNCTION_VECTORCALL)
//! }
//!
//! pub static JOIN: TargetSpec = TargetSpec {
//!     name: "posixpath.join", group: "paths", module: Some("posixpath"),
//!     versions: &[(3, 14), (3, 15)], free_threaded: true, counters: &COUNTERS,
//!     prepare, jit_safe: false, ..TargetSpec::DEFAULT
//! };
//! ```

pub mod check;
pub mod dynamic;
pub mod exit;
pub mod fingerprint;
pub mod guard;
pub mod prepare;
pub mod py;
pub mod registry;
pub mod site;
pub mod spec;
pub mod stats;
pub mod stubs;
pub mod switch;
pub mod version;

pub use fingerprint::Fingerprints;
pub use guard::{ClassCache, DepFlag, Invalidate, Tools, code_locally_quiet, in_main_interpreter, tools_quiet};
pub use prepare::Prepare;
pub use registry::degrade;
pub use site::SlotName;
pub use spec::{Category, FnPtr, GroupSpec, Original, Refusal, TargetSpec};
pub use stats::{Counters, Reason};
pub use version::FREE_THREADED;

/// `_PyFunction_Vectorcall`, shared by every vectorcall site: pass it to
/// [`Prepare::vectorcall`] and fall back through `FUNCTION_VECTORCALL.get()`.
pub static FUNCTION_VECTORCALL: Original<pyo3::ffi::vectorcallfunc> = Original::new();
