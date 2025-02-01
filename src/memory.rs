// memory.rs

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

/// A simple memory manager that caches allocated memory blocks.
/// The cache maps a block size to a vector of available (cached) blocks.
struct MemoryManager {
    cache: HashMap<usize, Vec<Box<[u8]>>>,
}

impl MemoryManager {
    /// Create a new MemoryManager with an empty cache.
    fn new() -> Self {
        MemoryManager {
            cache: HashMap::new(),
        }
    }

    /// Allocate a memory block of the requested size.
    ///
    /// If a block of the same size is available in the cache, reuse it;
    /// otherwise, allocate a new zero-initialized block.
    fn allocate(&mut self, size: usize) -> Box<[u8]> {
        if let Some(blocks) = self.cache.get_mut(&size) {
            if let Some(block) = blocks.pop() {
                // Found a cached block; reuse it.
                return block;
            }
        }
        // No cached block available; allocate a new one.
        vec![0u8; size].into_boxed_slice()
    }

    /// Deallocate a memory block by caching it for future reuse.
    fn deallocate(&mut self, block: Box<[u8]>) {
        let size = block.len();
        self.cache.entry(size).or_default().push(block);
    }

    /// Cleanup the entire memory cache.
    fn cleanup(&mut self) {
        self.cache.clear();
    }
}

// Create a global, lazily initialized memory manager protected by a Mutex for thread safety.
static MEMORY_MANAGER: Lazy<Mutex<MemoryManager>> = Lazy::new(|| Mutex::new(MemoryManager::new()));

/// A Python-exposed memory block.
/// This wraps an allocated memory block (a boxed slice) so that Python code can hold a reference
/// to it and later pass it back for deallocation.
#[pyclass]
pub struct PyMemoryBlock {
    /// The internal memory block. We wrap it in an Option so that once deallocated it becomes None.
    pub data: Option<Box<[u8]>>,
}

#[pymethods]
impl PyMemoryBlock {
    /// Create a new (empty) memory block.
    /// (Normally, use `allocate_memory` to obtain a new block.)
    #[new]
    fn new() -> Self {
        PyMemoryBlock { data: None }
    }

    /// Return the size of the memory block.
    #[getter]
    fn size(&self) -> PyResult<usize> {
        Ok(self.data.as_ref().map(|b| b.len()).unwrap_or(0))
    }

    /// Get the contents of the memory block as Python bytes.
    /// (Note that the returned bytes are a copy of the internal data.)
    pub fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Py<PyBytes>> {
        let data = self
            .data
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not allocated"))?;
        Ok(PyBytes::new(py, data).into())
    }
}

/// Initialize the memory system.
/// (In this simple example, initialization happens automatically via lazy initialization.)
#[pyfunction]
pub fn init_memory_system() -> PyResult<()> {
    // The MEMORY_MANAGER is created lazily on first access.
    Ok(())
}

/// Allocate a memory block of the given size.
///
/// Returns a Python-wrapped memory block that can be used by Python code.
#[pyfunction]
pub fn allocate_memory(size: usize) -> PyResult<PyMemoryBlock> {
    // Lock the memory manager for thread-safe access.
    let mut manager = MEMORY_MANAGER.lock().unwrap();
    let block = manager.allocate(size);
    Ok(PyMemoryBlock { data: Some(block) })
}

/// Deallocate a memory block previously allocated.
///
/// After deallocation, the internal pointer is removed (set to None) to avoid double-free.
#[pyfunction]
pub fn deallocate_memory(block: &mut PyMemoryBlock) -> PyResult<()> {
    let mut manager = MEMORY_MANAGER.lock().unwrap();
    if let Some(data) = block.data.take() {
        manager.deallocate(data);
        Ok(())
    } else {
        Err(PyValueError::new_err("Memory block already deallocated"))
    }
}

/// Clean up the memory system by clearing the internal cache.
#[pyfunction]
pub fn cleanup_memory_system() -> PyResult<()> {
    let mut manager = MEMORY_MANAGER.lock().unwrap();
    manager.cleanup();
    Ok(())
}

/// Define the Python module. This makes the functions and classes available in Python.
#[pymodule]
pub fn register_memory(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(init_memory_system, m)?)?;
    m.add_function(wrap_pyfunction!(allocate_memory, m)?)?;
    m.add_function(wrap_pyfunction!(deallocate_memory, m)?)?;
    m.add_function(wrap_pyfunction!(cleanup_memory_system, m)?)?;
    m.add_class::<PyMemoryBlock>()?;

    // Optionally, add a simple docstring.
    m.add(
        "__doc__",
        "Rust-based memory management module for Python objects.",
    )?;
    Ok(())
}
