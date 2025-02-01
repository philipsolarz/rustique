// rust_types.rs

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyList;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

// Import the memory-management API defined in memory.rs.
use crate::memory::{allocate_memory, PyMemoryBlock};

/// A Python‑exposed Rust integer type.
/// Internally, the integer value is stored in an 8‑byte memory block allocated
/// via our custom memory management subsystem.
#[pyclass]
pub struct RustInt {
    mem_block: Option<PyMemoryBlock>,
}

#[pymethods]
impl RustInt {
    /// Create a new RustInt with the given value.
    #[new]
    pub fn new(value: i64) -> PyResult<Self> {
        // Allocate 8 bytes.
        let mut block = allocate_memory(8)?;
        if let Some(ref mut data) = block.data {
            if data.len() != 8 {
                return Err(PyValueError::new_err("Allocated block is not 8 bytes"));
            }
            // Write the i64 value in little-endian format.
            data.as_mut()[..8].copy_from_slice(&value.to_le_bytes());
        } else {
            return Err(PyValueError::new_err("Failed to allocate memory"));
        }
        Ok(RustInt {
            mem_block: Some(block),
        })
    }

    /// Return the integer value.
    #[getter]
    pub fn value(&self) -> PyResult<i64> {
        if let Some(ref block) = self.mem_block {
            if let Some(ref data) = block.data {
                if data.len() != 8 {
                    return Err(PyValueError::new_err("Block size mismatch"));
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[..8]);
                Ok(i64::from_le_bytes(bytes))
            } else {
                Err(PyValueError::new_err("Memory block is deallocated"))
            }
        } else {
            Err(PyValueError::new_err("No memory block"))
        }
    }

    /// Support addition: self + other.
    fn __add__(&self, other: &RustInt) -> PyResult<RustInt> {
        let sum = self.value()? + other.value()?;
        RustInt::new(sum)
    }

    /// String representation.
    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("RustInt({})", self.value()?))
    }
}

/// A Python‑exposed Rust floating‑point type.
/// Uses the custom memory manager to allocate an 8‑byte block for a f64 value.
#[pyclass]
pub struct RustFloat {
    mem_block: Option<PyMemoryBlock>,
}

#[pymethods]
impl RustFloat {
    #[new]
    pub fn new(value: f64) -> PyResult<Self> {
        let mut block = allocate_memory(8)?;
        if let Some(ref mut data) = block.data {
            if data.len() != 8 {
                return Err(PyValueError::new_err("Allocated block is not 8 bytes"));
            }
            data.as_mut()[..8].copy_from_slice(&value.to_le_bytes());
        } else {
            return Err(PyValueError::new_err("Failed to allocate memory"));
        }
        Ok(RustFloat {
            mem_block: Some(block),
        })
    }

    #[getter]
    pub fn value(&self) -> PyResult<f64> {
        if let Some(ref block) = self.mem_block {
            if let Some(ref data) = block.data {
                if data.len() != 8 {
                    return Err(PyValueError::new_err("Block size mismatch"));
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[..8]);
                Ok(f64::from_le_bytes(bytes))
            } else {
                Err(PyValueError::new_err("Memory block is deallocated"))
            }
        } else {
            Err(PyValueError::new_err("No memory block"))
        }
    }

    fn __add__(&self, other: &RustFloat) -> PyResult<RustFloat> {
        let sum = self.value()? + other.value()?;
        RustFloat::new(sum)
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("RustFloat({})", self.value()?))
    }
}

/// A Python‑exposed Rust list type.
/// This is a wrapper around a Rust Vec that holds Python objects.
/// (In a complete implementation, the vector’s memory would also be allocated via
/// our custom memory management system.)
#[pyclass]
pub struct RustList {
    data: Vec<PyObject>,
}

#[pymethods]
impl RustList {
    #[new]
    pub fn new() -> Self {
        RustList { data: Vec::new() }
    }

    /// Append an element to the list.
    pub fn append(&mut self, item: PyObject) {
        self.data.push(item);
    }

    /// Get an element by index.
    pub fn __getitem__(&self, idx: isize, py: Python) -> PyResult<PyObject> {
        let len = self.data.len() as isize;
        let index = if idx < 0 { len + idx } else { idx };
        if index < 0 || index >= len {
            Err(PyValueError::new_err("Index out of range"))
        } else {
            Ok(self.data[index as usize].clone_ref(py))
        }
    }

    /// Set an element by index.
    pub fn __setitem__(&mut self, idx: isize, value: PyObject) -> PyResult<()> {
        let len = self.data.len() as isize;
        let index = if idx < 0 { len + idx } else { idx };
        if index < 0 || index >= len {
            Err(PyValueError::new_err("Index out of range"))
        } else {
            self.data[index as usize] = value;
            Ok(())
        }
    }

    /// Return the number of elements.
    pub fn __len__(&self) -> PyResult<usize> {
        Ok(self.data.len())
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("RustList({:?})", self.data))
    }
}

#[derive(Debug, Clone)]
struct PyObjectKey {
    pub obj: PyObject,
}

impl PartialEq for PyObjectKey {
    fn eq(&self, other: &Self) -> bool {
        Python::with_gil(|py| {
            self.obj
                .bind_borrowed(py)
                .rich_compare(other.obj.bind_borrowed(py), pyo3::basic::CompareOp::Eq)
                .and_then(|v| v.is_truthy())
                .unwrap_or(false)
        })
    }
}

impl Eq for PyObjectKey {}

impl Hash for PyObjectKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Python::with_gil(|py| {
            let hash_value = self.obj.bind_borrowed(py).hash().unwrap_or(0);
            hash_value.hash(state);
        });
    }
}

impl From<PyObject> for PyObjectKey {
    fn from(obj: PyObject) -> Self {
        PyObjectKey { obj }
    }
}

/// A Python‑exposed Rust dictionary type.
/// Wraps a Rust HashMap. In a full integration the memory for this container would be managed
/// through our custom memory management system.
#[pyclass]
pub struct RustDict {
    data: HashMap<PyObjectKey, PyObject>,
}

#[pymethods]
impl RustDict {
    #[new]
    pub fn new() -> Self {
        RustDict {
            data: HashMap::new(),
        }
    }

    /// Set a key/value pair.
    pub fn __setitem__(&mut self, key: PyObject, value: PyObject) {
        self.data.insert(key.into(), value);
    }

    /// Get a value by key.
    pub fn __getitem__(&self, key: PyObject, py: Python) -> PyResult<PyObject> {
        self.data
            .get(&key.into())
            .map(|v| v.clone_ref(py))
            .ok_or_else(|| PyValueError::new_err("Key not found"))
    }

    /// Delete a key.
    pub fn __delitem__(&mut self, key: PyObject) -> PyResult<()> {
        self.data
            .remove(&key.into())
            .map(|_| ())
            .ok_or_else(|| PyValueError::new_err("Key not found"))
    }

    /// Return a list of keys.
    pub fn keys(&self, py: Python) -> PyResult<PyObject> {
        let keys: Vec<PyObject> = self.data.keys().map(|k| k.obj.clone_ref(py)).collect();
        Ok(PyList::new(py, keys)?.to_object(py))
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("RustDict({:?})", self.data))
    }
}

/// Expose the custom types to Python in the module `rust_types`.
#[pymodule]
pub fn register_rust_types(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<RustInt>()?;
    m.add_class::<RustFloat>()?;
    m.add_class::<RustList>()?;
    m.add_class::<RustDict>()?;
    Ok(())
}
