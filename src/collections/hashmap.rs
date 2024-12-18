use pyo3::prelude::*;
use std::collections::HashMap;

#[pyclass]
pub struct HashMapWrapper {
    map: HashMap<String, i32>,
}

#[pymethods]
impl HashMapWrapper {
    #[new]
    pub fn new() -> Self {
        HashMapWrapper {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, key: String, value: i32) {
        self.map.insert(key, value);
    }

    pub fn get(&self, key: String) -> Option<i32> {
        self.map.get(&key).cloned()
    }

    pub fn __repr__(&self) -> String {
        format!("{:?}", self.map)
    }
}

pub fn register_hashmap(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<HashMapWrapper>()?;
    Ok(())
}
