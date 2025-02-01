// rust_types.rs

use std::mem;
use std::ptr;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyList;

// Import our custom memory-management API.
use crate::memory::{allocate_memory, deallocate_memory, PyMemoryBlock};

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

// rust_types.rs

//////////////////////////////////////////////////////////////////
///                          RustList                          ///
//////////////////////////////////////////////////////////////////

/// A Python‑exposed dynamic array type whose backing storage is allocated
/// via our custom memory manager. Each element is a Python object (PyObject).
///
/// Internally, we maintain:
/// - a raw pointer `ptr` into a block of memory (which holds capacity many PyObject’s),
/// - the current `length` (number of items stored),
/// - and the allocated `capacity` (the number of slots available).
///
///
#[pyclass(unsendable)]
pub struct RustList {
    // Pointer to the beginning of an array of PyObject.
    ptr: *mut PyObject,
    capacity: usize,
    length: usize,
    // The memory block holding the storage. When growing the array, we reallocate
    // a new block and return the old one to our custom memory manager.
    mem_block: Option<PyMemoryBlock>,
}

#[pymethods]
impl RustList {
    /// Create a new, empty RustList.
    #[new]
    pub fn new() -> PyResult<Self> {
        // Choose an initial capacity.
        let capacity = 4;
        let size = capacity * mem::size_of::<PyObject>();
        // Allocate a memory block large enough to hold `capacity` PyObject values.
        let mut mem_block = allocate_memory(size)?;
        // Convert the underlying u8 slice to a pointer for PyObject storage.
        let ptr = mem_block
            .data
            .as_mut()
            .expect("Memory block should contain data")
            .as_mut_ptr() as *mut PyObject;
        // (We assume the block is zeroed—unused slots need not be initialized.)
        Ok(RustList {
            ptr,
            capacity,
            length: 0,
            mem_block: Some(mem_block),
        })
    }

    /// Append a new Python object to the list.
    pub fn append(&mut self, item: PyObject) -> PyResult<()> {
        // Grow the underlying storage if needed.
        if self.length == self.capacity {
            self.grow()?;
        }
        unsafe {
            let slot = self.ptr.add(self.length);
            // Write the new item into the slot.
            ptr::write(slot, item);
        }
        self.length += 1;
        Ok(())
    }

    /// Retrieve the element at the given index.
    ///
    /// Supports negative indexing (e.g. -1 returns the last element).
    pub fn __getitem__(&self, idx: isize, py: Python) -> PyResult<PyObject> {
        let len = self.length as isize;
        let index = if idx < 0 { len + idx } else { idx };
        if index < 0 || index >= len {
            Err(PyValueError::new_err("Index out of range"))
        } else {
            unsafe {
                // Instead of consuming the element, we borrow it and clone_ref to increase its refcount.
                let item_ref = &*self.ptr.add(index as usize);
                Ok(item_ref.clone_ref(py))
            }
        }
    }

    /// Set the element at the given index to a new value.
    pub fn __setitem__(&mut self, idx: isize, value: PyObject, _py: Python) -> PyResult<()> {
        let len = self.length as isize;
        let index = if idx < 0 { len + idx } else { idx };
        if index < 0 || index >= len {
            Err(PyValueError::new_err("Index out of range"))
        } else {
            unsafe {
                let slot = self.ptr.add(index as usize);
                // Drop the old value in this slot.
                ptr::drop_in_place(slot);
                // Write the new value into the slot.
                ptr::write(slot, value);
            }
            Ok(())
        }
    }

    /// Return the number of elements in the list.
    pub fn __len__(&self) -> PyResult<usize> {
        Ok(self.length)
    }

    /// Return a string representation of the list.
    pub fn __repr__(&self, py: Python) -> PyResult<String> {
        let mut elems = Vec::new();
        for i in 0..self.length {
            unsafe {
                let item = &*self.ptr.add(i);
                // Use Python's repr() on the object.
                let repr_obj = item.bind_borrowed(py).repr()?;
                elems.push(repr_obj.to_string());
            }
        }
        Ok(format!("RustList([{}])", elems.join(", ")))
    }
}

impl RustList {
    /// Grow the underlying storage (doubling its capacity).
    fn grow(&mut self) -> PyResult<()> {
        let new_capacity = self.capacity * 2;
        let new_size = new_capacity * mem::size_of::<PyObject>();
        // Allocate a new memory block.
        let mut new_mem_block = allocate_memory(new_size)?;
        let new_ptr = new_mem_block
            .data
            .as_mut()
            .expect("New memory block must have data")
            .as_mut_ptr() as *mut PyObject;
        unsafe {
            // Copy existing elements into the new storage.
            for i in 0..self.length {
                let src = self.ptr.add(i);
                let dst = new_ptr.add(i);
                // Move the PyObject from the old location to the new one.
                ptr::write(dst, ptr::read(src));
            }
        }
        // Return the old memory block to our custom memory manager.
        if let Some(mut old_block) = self.mem_block.take() {
            let _ = deallocate_memory(&mut old_block);
        }
        // Update our fields.
        self.ptr = new_ptr;
        self.capacity = new_capacity;
        self.mem_block = Some(new_mem_block);
        Ok(())
    }
}

impl Drop for RustList {
    fn drop(&mut self) {
        unsafe {
            // Drop all stored elements.
            for i in 0..self.length {
                ptr::drop_in_place(self.ptr.add(i));
            }
        }
        // Return the memory block to our custom memory manager.
        if let Some(mut block) = self.mem_block.take() {
            let _ = deallocate_memory(&mut block);
        }
    }
}

//////////////////////////////////////////////////////////////////
///                          RustDict                          ///
//////////////////////////////////////////////////////////////////

/// Internal structure representing one dictionary entry.
/// We use a C‑compatible layout and allocate an array of these from our custom memory manager.
///
/// When `occupied` is false (i.e. zero), the slot is considered empty.
/// When true, the slot contains a key and a value (both Python objects).
#[repr(C)]
struct DictEntry {
    occupied: bool,
    // We use MaybeUninit so that we do not assume the key/value are valid
    // unless the slot is occupied.
    key: mem::MaybeUninit<PyObject>,
    value: mem::MaybeUninit<PyObject>,
}

/// A Python‑exposed dictionary type implemented as a simple open‑addressing hash table.
/// Its backing storage is allocated via our custom memory manager.
///
/// (This implementation uses linear probing for collision resolution and rehashes the table
/// when the load factor exceeds ~70%.)
#[pyclass(unsendable)]
pub struct RustDict {
    // Pointer to an array of DictEntry.
    ptr: *mut DictEntry,
    capacity: usize,
    count: usize,
    mem_block: Option<PyMemoryBlock>,
}

#[pymethods]
impl RustDict {
    /// Create a new, empty RustDict.
    #[new]
    pub fn new() -> PyResult<Self> {
        let capacity = 8;
        let size = capacity * mem::size_of::<DictEntry>();
        let mem_block = allocate_memory(size)?;
        let ptr = mem_block
            .data
            .as_ref()
            .expect("Memory block must have data")
            .as_ptr() as *mut DictEntry;
        // Because our allocator returns zeroed memory, all `occupied` flags are false.
        Ok(RustDict {
            ptr,
            capacity,
            count: 0,
            mem_block: Some(mem_block),
        })
    }

    /// Set a key–value pair in the dictionary.
    ///
    /// If the key already exists, its value is replaced.
    pub fn __setitem__(
        &mut self,
        key: &Bound<'_, PyAny>,
        value: PyObject,
        py: Python,
    ) -> PyResult<()> {
        // Rehash if the table is getting too full (load factor ≥ 0.7).
        unsafe {
            if (self.count + 1) * 10 >= self.capacity * 7 {
                self.rehash(py, self.capacity * 2)?;
            }
            let index = self.find_slot(&key, py);
            let entry = self.ptr.add(index);
            if (*entry).occupied {
                // Replace existing value: drop the old one.
                ptr::drop_in_place((*entry).value.as_mut_ptr());
                ptr::write((*entry).value.as_mut_ptr(), value);
            } else {
                // Insert new key and value.
                ptr::write((*entry).key.as_mut_ptr(), key.clone().into());
                ptr::write((*entry).value.as_mut_ptr(), value);
                (*entry).occupied = true;
                self.count += 1;
            }
        }
        Ok(())
    }

    /// Retrieve the value associated with a given key.
    pub fn __getitem__(&self, key: &Bound<'_, PyAny>, py: Python) -> PyResult<PyObject> {
        unsafe {
            let index = self.find_slot_readonly(&key, py);
            let entry = self.ptr.add(index);
            if (*entry).occupied {
                let value = &*((*entry).value.as_ptr());
                Ok(value.clone_ref(py))
            } else {
                Err(PyValueError::new_err("Key not found"))
            }
        }
    }

    /// Delete a key (and its value) from the dictionary.
    ///
    /// For simplicity, after deletion we rehash the entire table to repair any probing chains.
    pub fn __delitem__(&mut self, key: &Bound<'_, PyAny>, py: Python) -> PyResult<()> {
        unsafe {
            let index = self.find_slot_readonly(&key, py);
            let entry = self.ptr.add(index);
            if (*entry).occupied {
                // Drop the key and value.
                ptr::drop_in_place((*entry).key.as_mut_ptr());
                ptr::drop_in_place((*entry).value.as_mut_ptr());
                (*entry).occupied = false;
                self.count -= 1;
                // Rehash the table (using the same capacity) to fix the probing chain.
                self.rehash(py, self.capacity)?;
                Ok(())
            } else {
                Err(PyValueError::new_err("Key not found"))
            }
        }
    }

    /// Return a Python list of all keys in the dictionary.
    pub fn keys(&self, py: Python) -> PyResult<PyObject> {
        let mut key_list = Vec::new();
        unsafe {
            for i in 0..self.capacity {
                let entry = self.ptr.add(i);
                if (*entry).occupied {
                    let key = &*((*entry).key.as_ptr());
                    key_list.push(key.clone_ref(py));
                }
            }
        }
        Ok(PyList::new(py, key_list)?.to_object(py))
    }

    /// Return a string representation of the dictionary.
    pub fn __repr__(&self, py: Python) -> PyResult<String> {
        let mut pairs = Vec::new();
        unsafe {
            for i in 0..self.capacity {
                let entry = self.ptr.add(i);
                if (*entry).occupied {
                    let key = &*((*entry).key.as_ptr());
                    let value = &*((*entry).value.as_ptr());
                    let key_repr = key.bind_borrowed(py).repr()?.to_string();
                    let value_repr = value.bind_borrowed(py).repr()?.to_string();
                    pairs.push(format!("{}: {}", key_repr, value_repr));
                }
            }
        }
        Ok(format!("RustDict({{{}}})", pairs.join(", ")))
    }
}

impl RustDict {
    /// (Mutable) Linear probing: find the slot index where the given key is located,
    /// or where an empty slot is available.
    ///
    /// This function assumes the GIL is held (via the provided `py` context).
    unsafe fn find_slot(&mut self, key: &Bound<'_, PyAny>, py: Python) -> usize {
        let hash = key.hash().unwrap_or(0) as usize;
        let mut index = hash % self.capacity;
        loop {
            let entry = self.ptr.add(index);
            if !(*entry).occupied {
                return index;
            } else {
                let existing_key = &*((*entry).key.as_ptr());
                let cmp = existing_key
                    .bind_borrowed(py)
                    .rich_compare(key, pyo3::basic::CompareOp::Eq)
                    .unwrap();
                if cmp.is_truthy().unwrap() {
                    return index;
                }
            }
            index = (index + 1) % self.capacity;
        }
    }

    /// (Read-only) Linear probing for lookups.
    ///
    /// Returns the slot index where the key is found or an empty slot.
    unsafe fn find_slot_readonly(&self, key: &Bound<'_, PyAny>, py: Python) -> usize {
        let hash = key.hash().unwrap_or(0) as usize;
        let mut index = hash % self.capacity;
        loop {
            let entry = self.ptr.add(index);
            if !(*entry).occupied {
                return index;
            } else {
                let existing_key = &*((*entry).key.as_ptr());
                let cmp = existing_key
                    .bind_borrowed(py)
                    .rich_compare(key, pyo3::basic::CompareOp::Eq)
                    .unwrap();
                if cmp.is_truthy().unwrap() {
                    return index;
                }
            }
            index = (index + 1) % self.capacity;
        }
    }

    /// Rehash the dictionary into a new table with the specified capacity.
    ///
    /// This routine allocates a new memory block, reinserts all occupied entries,
    /// then returns the old block to our custom memory manager.
    unsafe fn rehash(&mut self, py: Python, new_capacity: usize) -> PyResult<()> {
        let new_size = new_capacity * mem::size_of::<DictEntry>();
        let mut new_mem_block = allocate_memory(new_size)?;
        let new_ptr = new_mem_block
            .data
            .as_mut()
            .expect("New memory block must have data")
            .as_mut_ptr() as *mut DictEntry;
        // Since the block is zeroed, all new slots have occupied == false.
        let old_capacity = self.capacity;
        let old_ptr = self.ptr;
        let mut new_count = 0;
        // Reinsert each occupied entry into the new table.
        for i in 0..old_capacity {
            let old_entry = old_ptr.add(i);
            if (*old_entry).occupied {
                // Move the key and value out of the old entry.
                let key = ptr::read((*old_entry).key.as_ptr());
                let value = ptr::read((*old_entry).value.as_ptr());
                let hash = key.bind(py).hash().unwrap_or(0) as usize;
                let mut index = hash % new_capacity;
                loop {
                    let new_entry = new_ptr.add(index);
                    if !(*new_entry).occupied {
                        ptr::write((*new_entry).key.as_mut_ptr(), key);
                        ptr::write((*new_entry).value.as_mut_ptr(), value);
                        (*new_entry).occupied = true;
                        new_count += 1;
                        break;
                    }
                    index = (index + 1) % new_capacity;
                }
            }
        }
        // Deallocate the old memory block.
        if let Some(mut old_block) = self.mem_block.take() {
            let _ = deallocate_memory(&mut old_block);
        }
        // Update our fields.
        self.ptr = new_ptr;
        self.capacity = new_capacity;
        self.count = new_count;
        self.mem_block = Some(new_mem_block);
        Ok(())
    }
}

impl Drop for RustDict {
    fn drop(&mut self) {
        unsafe {
            // For each slot that is occupied, drop the key and value.
            for i in 0..self.capacity {
                let entry = self.ptr.add(i);
                if (*entry).occupied {
                    ptr::drop_in_place((*entry).key.as_mut_ptr());
                    ptr::drop_in_place((*entry).value.as_mut_ptr());
                }
            }
        }
        if let Some(mut block) = self.mem_block.take() {
            let _ = deallocate_memory(&mut block);
        }
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
