// memory.rs

use once_cell::sync::Lazy;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// Threshold (in bytes) under which allocations are considered “small”
const SMALL_BLOCK_THRESHOLD: usize = 256;

/// Global memory manager for larger allocations.
/// It maintains a cache of previously allocated blocks keyed by size.
pub struct MemoryManager {
    cache: HashMap<usize, Vec<Box<[u8]>>>,
}

impl MemoryManager {
    pub fn new() -> Self {
        MemoryManager {
            cache: HashMap::new(),
        }
    }

    /// Allocate a block of the given size from the global cache, or allocate a new block.
    pub fn allocate(&mut self, size: usize) -> Box<[u8]> {
        if let Some(blocks) = self.cache.get_mut(&size) {
            if let Some(block) = blocks.pop() {
                // Cache hit
                return block;
            }
        }
        // Cache miss: allocate a new zero-initialized block.
        vec![0u8; size].into_boxed_slice()
    }

    /// Deallocate a block by returning it to the global cache.
    pub fn deallocate(&mut self, block: Box<[u8]>) {
        let size = block.len();
        self.cache.entry(size).or_default().push(block);
    }

    /// Cleanup the global cache.
    pub fn cleanup(&mut self) {
        self.cache.clear();
    }
}

/// A lazily initialized global memory manager protected by a Mutex.
static GLOBAL_MEMORY_MANAGER: Lazy<Mutex<MemoryManager>> =
    Lazy::new(|| Mutex::new(MemoryManager::new()));

/// Thread-local cache for small memory blocks.
/// Using thread-local storage avoids global locking overhead and improves data locality.
thread_local! {
    static THREAD_LOCAL_CACHE: RefCell<HashMap<usize, Vec<Box<[u8]>>>> = RefCell::new(HashMap::new());
}

/// Allocate a memory block using an optimized scheme.
/// - For small sizes (≤ SMALL_BLOCK_THRESHOLD), we use the thread-local cache.
/// - For larger sizes, we fall back to the global memory manager.
pub fn optimized_allocate(size: usize) -> Box<[u8]> {
    if size <= SMALL_BLOCK_THRESHOLD {
        THREAD_LOCAL_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(blocks) = cache.get_mut(&size) {
                if let Some(block) = blocks.pop() {
                    // Return a cached small block.
                    return block;
                }
            }
            // Cache miss: allocate a new small block.
            vec![0u8; size].into_boxed_slice()
        })
    } else {
        let mut manager = GLOBAL_MEMORY_MANAGER.lock().unwrap();
        manager.allocate(size)
    }
}

/// Deallocate a memory block using the optimized scheme.
/// - Small blocks are returned to the thread-local cache.
/// - Larger blocks are returned to the global memory manager.
pub fn optimized_deallocate(block: Box<[u8]>) {
    let size = block.len();
    if size <= SMALL_BLOCK_THRESHOLD {
        THREAD_LOCAL_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.entry(size).or_default().push(block);
        });
    } else {
        let mut manager = GLOBAL_MEMORY_MANAGER.lock().unwrap();
        manager.deallocate(block);
    }
}

/// Cleanup all caches: both global and thread-local.
pub fn optimized_cleanup() {
    {
        let mut manager = GLOBAL_MEMORY_MANAGER.lock().unwrap();
        manager.cleanup();
    }
    THREAD_LOCAL_CACHE.with(|cache| {
        cache.borrow_mut().clear();
    });
}

/// A Python-exposed memory block that wraps an allocated memory region.
#[pyclass]
pub struct PyMemoryBlock {
    /// The internal memory block.
    pub data: Option<Box<[u8]>>,
}

#[pymethods]
impl PyMemoryBlock {
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
    pub fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Py<PyBytes>> {
        let data = self
            .data
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not allocated"))?;
        Ok(PyBytes::new(py, data).into())
    }
}

/// Initialize the memory system. (Initialization is lazy.)
#[pyfunction]
pub fn init_memory_system() -> PyResult<()> {
    // Optionally add logging or instrumentation here.
    Ok(())
}

/// Allocate a memory block of the given size using the optimized allocator.
#[pyfunction]
pub fn allocate_memory(size: usize) -> PyResult<PyMemoryBlock> {
    let block = optimized_allocate(size);
    Ok(PyMemoryBlock { data: Some(block) })
}

/// Deallocate a memory block.
#[pyfunction]
pub fn deallocate_memory(block: &mut PyMemoryBlock) -> PyResult<()> {
    if let Some(data) = block.data.take() {
        optimized_deallocate(data);
        Ok(())
    } else {
        Err(PyValueError::new_err("Memory block already deallocated"))
    }
}

/// Cleanup the entire memory system, clearing both global and thread-local caches.
#[pyfunction]
pub fn cleanup_memory_system() -> PyResult<()> {
    optimized_cleanup();
    Ok(())
}

/// Expose the memory-management API to Python.
#[pymodule]
pub fn memory(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(init_memory_system, m)?)?;
    m.add_function(wrap_pyfunction!(allocate_memory, m)?)?;
    m.add_function(wrap_pyfunction!(deallocate_memory, m)?)?;
    m.add_function(wrap_pyfunction!(cleanup_memory_system, m)?)?;
    m.add_class::<PyMemoryBlock>()?;
    m.add(
        "__doc__",
        "Optimized Rust-based memory management module for Python objects.",
    )?;
    Ok(())
}
