//! # rustique-core
//!
//! The Python-independent parts of Rustique's kernels: CPython-identical algorithms that
//! the engine (`rustique-py`, `targets/`) runs behind CPython's own entry points, testable
//! without an interpreter: `repr(float)` ([`float_repr`]), JSON scanning and escaping
//! ([`json`]), `strftime`/`strptime` and calendar arithmetic ([`datetime`]), path
//! manipulation and glob patterns ([`paths`]), the Mersenne Twister and `random`'s
//! variates ([`random`]), exact sums for `statistics` ([`stats`]), `logging` formatting
//! ([`logging`]), `textwrap` ([`textwrap`]), `urllib.parse` ([`urlparse`]) and the `re` compiler ([`re`]).
//!
//! Nothing in this crate links against Python; the `rustique-py` crate adds the bindings.

pub mod datetime;
pub mod float_repr;
pub mod json;
pub mod logging;
pub mod paths;
pub mod random;
pub mod re;
pub mod stats;
pub mod textwrap;
pub mod urlparse;

/// The slow, exact formatter that [`float_repr`] is verified against.
#[cfg(test)]
#[allow(dead_code)]
mod pyfmt;
