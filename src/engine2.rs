use std::{
    alloc::{alloc, dealloc, Layout},
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
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

#[derive(Debug)]
struct RustiqueObject {
    value: RustiqueValue,
    references: RefCell<Vec<Weak<RustiqueObject>>>,
}

impl RustiqueObject {
    fn new(value: RustiqueValue) -> Arc<Self> {
        Arc::new(RustiqueObject {
            value: value,
            references: RefCell::new(Vec::new()),
        })
    }

    fn add_reference(&self, other: &Arc<Self>) {
        self.references.borrow_mut().push(Arc::downgrade(other));
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
        for weak_ref in obj.references.borrow().iter() {
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
    objects: RefCell<HashSet<Arc<RustiqueObject>>>,
}

#[pymethods]
impl RustiqueGC {
    #[new]
    fn new() -> Self {
        Self {
            objects: RefCell::new(HashSet::new()),
        }
    }

    fn create_object(&self, value: String) -> PyRustiqueObject {
        let obj = RustiqueObject::new(&value);
        self.objects.borrow_mut().insert(Arc::clone(&obj));
        PyRustiqueObject { obj }
    }

    fn collect_garbage(&self) {
        gc_collect(&mut self.objects.borrow_mut());
    }
}

#[pyclass]
struct PyRustiqueObject {
    obj: Arc<RustiqueObject>,
}

impl PyRustiqueObject {
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

#[pyclass]
struct RustiqueTuple {
    items: Vec<Arc<RustiqueObject>>,
}

#[pymethods]
impl RustiqueTuple {
    #[new]
    fn new(items: Vec<PyRustiqueObject>) -> Self {
        RustiqueTuple {
            items: items.into_iter().map(|obj| Arc::clone(&obj.obj)).collect(),
        }
    }

    fn get(&self, index: usize) -> Option<PyRustiqueObject> {
        self.items.get(index).map(|obj| PyRustiqueObject {
            obj: Arc::clone(obj),
        })
    }

    fn length(&self) -> usize {
        self.items.len()
    }
}

#[pymodule]
pub fn register_engine2(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<RustiqueGC>()?;
    m.add_class::<PyRustiqueObject>()?;
    m.add_class::<RustiqueList>()?;
    m.add_class::<RustiqueDict>()?;
    m.add_class::<RustiqueTuple>()?;
    Ok(())
}
