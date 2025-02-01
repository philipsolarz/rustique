// context_manager.rs

use pyo3::prelude::*;

// Import the memory-management API from the memory module.
use crate::memory::{cleanup_memory_system, init_memory_system};

/// A Python context manager that activates the Rust-based memory-management environment.
/// When entering the context, it initializes the memory system.
/// Upon exiting, it cleans up any pending memory operations and frees resources.
#[pyclass]
pub struct RustiqueContext;

#[pymethods]
impl RustiqueContext {
    /// Create a new RustiqueContext.
    #[new]
    pub fn new() -> Self {
        RustiqueContext
    }

    /// __enter__ is invoked at the start of the Python with‑block.
    /// It initializes the memory management system and returns self.
    fn __enter__(slf: PyRefMut<Self>) -> PyResult<PyRefMut<Self>> {
        init_memory_system()?;
        Ok(slf)
    }

    /// __exit__ is automatically called when the Python with‑block ends.
    /// It cleans up the memory system and frees any pending resources.
    #[pyo3(signature = (_exc_type=None, _exc_value=None, _traceback=None))]
    fn __exit__(
        &mut self,
        _exc_type: Option<PyObject>,
        _exc_value: Option<PyObject>,
        _traceback: Option<PyObject>,
    ) -> PyResult<()> {
        cleanup_memory_system()?;
        Ok(())
    }

    /// Return a string representation of the context manager.
    fn __repr__(&self) -> PyResult<String> {
        Ok("RustiqueContext()".to_string())
    }
}

/// Expose the context manager as a Python module named `rustique_context`.
#[pymodule]
pub fn register_context_manager(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<RustiqueContext>()?;
    m.add(
        "__doc__",
        "A Python context manager for activating the Rust-based memory-management environment.",
    )?;
    Ok(())
}
