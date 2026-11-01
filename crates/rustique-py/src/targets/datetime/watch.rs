//! The datetime group's generation: bumped by Rustique's dict watcher
//! (`accel::guard::watch`) whenever a dependency entry is added, replaced or removed.
//! Kernels cache the generation at which they last validated their dependencies by value
//! and compare it with one relaxed load per call.
//!
//! Ordering: CPython notifies watchers *before* applying a change. Validators read
//! [`current`] first and then read each watched dict inside that dict's critical section
//! (`env::item`; free-threaded builds: the writer holds it across notify and store), so they
//! either see the new value or validate under the old generation.

use std::sync::atomic::{AtomicU64, Ordering};

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Starts at 1 so that 0 means "never validated".
pub static GEN: AtomicU64 = AtomicU64::new(1);

#[inline(always)]
pub fn current() -> u64 {
    GEN.load(Ordering::Acquire)
}

/// Watch `dict[key]` for every key of `keys`; a new watch means "validate again".
pub fn watch(dict: &Bound<'_, PyDict>, keys: &[&str]) -> PyResult<()> {
    for k in keys {
        crate::accel::guard::watch(dict, k, &GEN)?;
    }
    GEN.fetch_add(1, Ordering::AcqRel);
    Ok(())
}
