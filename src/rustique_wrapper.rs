// rustique_wrapper.rs

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyAny;

// Import the Rust-based types defined elsewhere (for example, in rust_types.rs).
// use crate::rust_types::{RustDict, RustFloat, RustInt, RustList};
use crate::rust_types_advanced::{RustDict, RustFloat, RustInt, RustList};
// Import the memory-management API from our memory module.
use crate::memory::{allocate_memory, deallocate_memory, PyMemoryBlock};

/// Check if a given object is already a Rustique type.
///
/// Rustique types currently supported include:
/// - `RustInt`
/// - `RustFloat`
/// - `RustList`
/// - `RustDict`
///
/// Returns `True` if the object is an instance of one of these types; otherwise, returns `False`.
#[pyfunction]
pub fn is_rustique_object(obj: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = obj.py();
    let rust_int_type = py.get_type::<RustInt>();
    let rust_float_type = py.get_type::<RustFloat>();
    let rust_list_type = py.get_type::<RustList>();
    let rust_dict_type = py.get_type::<RustDict>();

    if obj.is_instance(&rust_int_type)?
        || obj.is_instance(&rust_float_type)?
        || obj.is_instance(&rust_list_type)?
        || obj.is_instance(&rust_dict_type)?
    {
        Ok(true)
    } else {
        Ok(false)
    }
}

/// A wrapper for generic Python objects that allows their memory operations to be routed
/// through the Rust memory-management subsystem. This mechanism is optional and can be used
/// to “adopt” a non‑Rustique object so that, for example, auxiliary data can be stored in
/// custom allocated memory.
///
/// # Attributes
///
/// * `inner` – The original Python object being wrapped.
/// * `memory_block` – An optional memory block allocated via our Rust memory-management system.
///
/// # Example
///
/// ```python
/// import rustique_wrapper
///
/// # Suppose `obj` is a generic Python object:
/// wrapped = rustique_wrapper.PyRustiqueWrapper(obj)
/// wrapped.initialize_memory(128)  # allocate 128 bytes via the Rust allocator
/// # ... perform operations ...
/// wrapped.release_memory()  # release the allocated memory
/// ```
#[pyclass]
pub struct PyRustiqueWrapper {
    /// The original (generic) Python object.
    inner: PyObject,
    /// Optionally, a memory block allocated for this object.
    memory_block: Option<PyMemoryBlock>,
}

#[pymethods]
impl PyRustiqueWrapper {
    /// Create a new wrapper for a generic Python object.
    #[new]
    pub fn new(obj: PyObject) -> Self {
        PyRustiqueWrapper {
            inner: obj,
            memory_block: None,
        }
    }

    /// Retrieve the wrapped Python object.
    #[getter]
    pub fn inner(&self) -> PyObject {
        self.inner.clone()
    }

    /// Initialize memory for the wrapped object by allocating a memory block of the specified size.
    ///
    /// In a full implementation, you might copy or relocate data into this block. Here, we simply
    /// simulate allocation via the Rust memory-management system.
    pub fn initialize_memory(&mut self, size: usize) -> PyResult<()> {
        let block = allocate_memory(size)?;
        self.memory_block = Some(block);
        Ok(())
    }

    /// Release the allocated memory block.
    pub fn release_memory(&mut self) -> PyResult<()> {
        if let Some(ref mut block) = self.memory_block {
            deallocate_memory(block)?;
            self.memory_block = None;
            Ok(())
        } else {
            Err(PyValueError::new_err(
                "No memory block is currently allocated",
            ))
        }
    }

    /// Return a string representation of the wrapper.
    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("PyRustiqueWrapper(inner={:?})", self.inner))
    }
}

/// The module-level documentation includes supported types and a short guide for future extensibility.
///
/// **Supported Types:**
/// - `RustInt`
/// - `RustFloat`
/// - `RustList`
/// - `RustDict`
///
/// **Extensibility Plan:**
///
/// 1. **Detecting Additional Types:**  
///    Extend `is_rustique_object` by adding checks for new types as they are implemented.
///
/// 2. **Wrapping Mechanism Enhancements:**  
///    Extend `PyRustiqueWrapper` to support custom initialization, copy or move semantics,
///    and integrate more tightly with the memory-management system for different kinds of objects.
///
/// 3. **Interfacing with Python’s GC:**  
///    Optionally, provide hooks to allow wrapped objects to interact with Python’s garbage collector,
///    ensuring that memory allocated by the Rust subsystem is correctly released.
///
/// 4. **Documentation and Testing:**  
///    Maintain clear documentation and add unit tests for each new type and wrapper behavior.
#[pymodule]
pub fn register_rustique_wrapper(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(is_rustique_object, m)?)?;
    m.add_class::<PyRustiqueWrapper>()?;

    m.add("__doc__", "\
Rustique Wrapper Module
-------------------------

This module provides an abstraction layer for integrating non-Rustique (generic) Python objects with the \
Rust-based memory-management system.

**Supported Rustique Types:**
- RustInt
- RustFloat
- RustList
- RustDict

**Extensibility:**
To extend support to additional types, add new type checks in `is_rustique_object` and create additional \
wrapper classes (or extend `PyRustiqueWrapper`) to handle custom memory operations for those types.")?;

    Ok(())
}
