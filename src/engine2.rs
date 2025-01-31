use std::{
    alloc::{alloc, dealloc, Layout},
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, RwLock, Weak},
};

use pyo3::prelude::*;

struct Arena {
    buffer: RefCell<Vec<*mut u8>>,
}

impl Arena {
    fn new() -> Self {
        Self {
            buffer: RefCell::new(Vec::new()),
        }
    }

    fn allocate<T>(&self, value: T) -> *mut T {
        let layout = Layout::new::<T>();
        unsafe {
            let ptr = alloc(layout) as *mut T;
            ptr.write(value);
            self.buffer.borrow_mut().push(ptr as *mut u8);
            ptr
        }
    }

    fn deallocate_all(&self) {
        for &ptr in self.buffer.borrow().iter() {
            unsafe {
                dealloc(ptr, Layout::for_value(&*ptr));
            }
        }
        self.buffer.borrow_mut().clear();
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        self.deallocate_all();
    }
}

#[derive(Debug)]
enum RustiqueValue {
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Arc<RustiqueObject>>),
    Dict(HashMap<String, Arc<RustiqueObject>>),
    Tuple(Vec<Arc<RustiqueObject>>),
}

// ✅ Implement PartialEq Manually
impl PartialEq for RustiqueValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (RustiqueValue::Int(a), RustiqueValue::Int(b)) => a == b,
            (RustiqueValue::Float(a), RustiqueValue::Float(b)) => a.to_bits() == b.to_bits(), // ✅ Float comparison workaround
            (RustiqueValue::Str(a), RustiqueValue::Str(b)) => a == b,
            _ => false,
        }
    }
}

// ✅ Implement Eq Manually
impl Eq for RustiqueValue {}

// ✅ Implement Hash Manually
impl Hash for RustiqueValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            RustiqueValue::Int(i) => i.hash(state),
            RustiqueValue::Float(f) => f.to_bits().hash(state), // ✅ Convert f64 to bits before hashing
            RustiqueValue::Str(s) => s.hash(state),
            _ => {}
        }
    }
}

#[derive(Debug)]
struct RustiqueObject {
    value: RustiqueValue,
    references: RwLock<Vec<Weak<RustiqueObject>>>,
}

// ✅ Implement PartialEq for RustiqueObject
impl PartialEq for RustiqueObject {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

// ✅ Implement Eq for RustiqueObject
impl Eq for RustiqueObject {}

// ✅ Implement Hash for RustiqueObject
impl Hash for RustiqueObject {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl RustiqueObject {
    fn new(value: RustiqueValue) -> Arc<Self> {
        Arc::new(RustiqueObject {
            value: value,
            references: RwLock::new(Vec::new()),
        })
    }

    fn add_reference(&self, other: &Arc<Self>) {
        self.references.write().unwrap().push(Arc::downgrade(other));
    }
}

fn gc_collect(objects: &mut HashSet<Arc<RustiqueObject>>) {
    let mut reachable = HashSet::new();

    // Mark Phase: Recursively find reachable objects
    fn mark(obj: &Arc<RustiqueObject>, reachable: &mut HashSet<Arc<RustiqueObject>>) {
        if reachable.contains(obj) {
            return;
        }
        reachable.insert(Arc::clone(obj));

        for weak_ref in obj.references.read().unwrap().iter() {
            if let Some(strong_ref) = weak_ref.upgrade() {
                mark(&strong_ref, reachable);
            }
        }
    }

    for obj in objects.iter() {
        mark(obj, &mut reachable);
    }

    // Sweep Phase: Deallocate unreachable objects
    objects.retain(|obj| reachable.contains(obj));
}

#[pyclass]
struct RustiqueGC {
    objects: RwLock<HashSet<Arc<RustiqueObject>>>,
}

#[pymethods]
impl RustiqueGC {
    #[new]
    fn new() -> Self {
        Self {
            objects: RwLock::new(HashSet::new()),
        }
    }

    fn create_object(&self, value: String) -> PyRustiqueObject {
        let obj = RustiqueObject::new(RustiqueValue::Str(value));
        self.objects.write().unwrap().insert(Arc::clone(&obj));
        PyRustiqueObject { obj }
    }

    fn collect_garbage(&self) {
        gc_collect(&mut self.objects.write().unwrap());
    }
}

#[pyclass]
struct PyRustiqueObject {
    obj: Arc<RustiqueObject>,
}

#[pymethods]
impl PyRustiqueObject {
    #[new]
    fn new(value: String) -> Self {
        PyRustiqueObject {
            obj: RustiqueObject::new(RustiqueValue::Str(value)),
        }
    }

    fn add_reference(&self, other: &PyRustiqueObject) {
        self.obj.add_reference(&other.obj);
    }

    fn value(&self) -> String {
        format!("{:?}", self.obj.value)
    }
}

#[pyclass]
struct RustiqueList {
    items: RefCell<Vec<Arc<RustiqueObject>>>,
}

#[pymethods]
impl RustiqueList {
    #[new]
    fn new() -> Self {
        RustiqueList {
            items: RefCell::new(Vec::new()),
        }
    }

    fn append(&self, obj: &PyRustiqueObject) {
        self.items.borrow_mut().push(Arc::clone(&obj.obj));
    }

    fn get(&self, index: usize) -> Option<PyRustiqueObject> {
        self.items.borrow().get(index).map(|obj| PyRustiqueObject {
            obj: Arc::clone(obj),
        })
    }

    fn length(&self) -> usize {
        self.items.borrow().len()
    }
}

#[pyclass]
struct RustiqueDict {
    items: RefCell<HashMap<String, Arc<RustiqueObject>>>,
}

#[pymethods]
impl RustiqueDict {
    #[new]
    fn new() -> Self {
        RustiqueDict {
            items: RefCell::new(HashMap::new()),
        }
    }

    fn set(&self, key: String, value: &PyRustiqueObject) {
        self.items.borrow_mut().insert(key, Arc::clone(&value.obj));
    }

    fn get(&self, key: String) -> Option<PyRustiqueObject> {
        self.items.borrow().get(&key).map(|obj| PyRustiqueObject {
            obj: Arc::clone(obj),
        })
    }

    fn keys(&self) -> Vec<String> {
        self.items.borrow().keys().cloned().collect()
    }

    fn values(&self) -> Vec<String> {
        self.items
            .borrow()
            .values()
            .map(|obj| format!("{:?}", obj.value))
            .collect()
    }
}

#[pymodule]
pub fn register_engine2(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<RustiqueGC>()?;
    m.add_class::<PyRustiqueObject>()?;
    m.add_class::<RustiqueList>()?;
    m.add_class::<RustiqueDict>()?;
    Ok(())
}
