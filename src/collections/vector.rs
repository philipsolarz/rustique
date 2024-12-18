use pyo3::exceptions::{PyIndexError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyList, PyTuple, PyType};



#[pyclass(subclass)]
pub struct Vector {
    data: Vec<PyObject>,
    _type: Option<Py<PyType>>,
}

#[pymethods]
impl Vector {

    #[new]
    #[pyo3(signature = (*values, _type=None))]
    pub fn __new__(values: &Bound<'_, PyTuple>, _type: Option<Bound<'_, PyType>>) -> PyResult<Self> {
        let mut vec = Vec::new();

        for item in values.as_slice() {
            // println!("item: {:?}, item_type: {:?}, expected_type: {:?}", item, item.get_type(), _type);
            if let Some(ref t) = _type {
                if !item.is_instance(t)? {
                    return Err(PyTypeError::new_err(format!(
                        "Expected item of type {}, got {} at index {}",
                        t.name()?,
                        item.get_type().name()?,
                        vec.len(),
                    )));
                }
            }
            vec.push(Py::from(item.clone()));

        }
        Ok(Vector { 
            data: vec,
            _type: _type.map(|t| t.into()),
        })
    }

    /// Representation of the vector
    pub fn __repr__(&self, py: Python) -> PyResult<String> {
        let values: Vec<String> = self
            .data
            .iter()
            .map(|item| item.bind(py).repr().unwrap().to_string())
            .collect();

        let type_annotation = if let Some(ref t) = self._type {
            format!("[{}]", t.bind(py).name()?)
        } else {
            String::new()
        };
        Ok(format!(
            "Vector{}({})",
            type_annotation,
            values.join(", ")
        ))
    }

    pub fn __str__(&self, py: Python) -> PyResult<String> {
        let values: Vec<String> = self
            .data
            .iter()
            .map(|item| item.bind(py).str().unwrap().to_string())
            .collect();

        let type_annotation = if let Some(ref t) = self._type {
            format!("[{}]", t.bind(py).name()?)
        } else {
            String::new()
        };
        Ok(format!(
            "Vector{}({})",
            type_annotation,
            values.join(", ")
        ))
    }

    pub fn __len__(&self) -> usize {
        self.data.len()
    }

    pub fn __getitem__(&self, index: isize) -> PyObject {
        self.data[index as usize].clone()
    }

    pub fn __setitem__(&mut self, py: Python, index: isize, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Some(ref t) = self._type {
            if !value.is_instance(t.bind(py)).unwrap() {
                return Err(PyTypeError::new_err(format!(
                    "Expected item of type {}, got {}",
                    t.bind(py).name()?,
                    value.get_type().name()?
                )));
            }
        }

        self.data[index as usize] = Py::from(value.clone());
        Ok(())
    }

    pub fn __delitem__(&mut self, index: isize) {
        self.data.remove(index as usize);
    }

    /// Append any Python object
    pub fn append(&mut self, py: Python, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Some(ref t) = self._type {
            if !value.is_instance(t.bind(py))? {
                return Err(PyTypeError::new_err(format!(
                    "Expected item of type {}, got {}",
                    t.bind(py).name()?,
                    value.get_type().name()?
                )));
            }
        }

        self.data.push(Py::from(value.clone()));
        Ok(())
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn count(&self, py: Python, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        if let Some(ref t) = self._type {
            if !value.is_instance(t.bind(py))? {
                return Err(PyTypeError::new_err(format!(
                    "Expected item of type {}, got {}",
                    t.bind(py).name()?,
                    value.get_type().name()?
                )));
            }
        }
    
        Ok(self
            .data
            .iter()
            .filter(|item| {
                item.bind(py)
                    .rich_compare(value, pyo3::basic::CompareOp::Eq)
                    .and_then(|res| res.is_truthy())
                    .unwrap_or(false)
            })
            .count())
    }

    pub fn copy(&self) -> Self {
        Vector {
            data: self.data.clone(),
            _type: self._type.clone(),
        }
    }

    pub fn extend(&mut self, py: Python, values: &Bound<'_, PyList>) -> PyResult<()> {
        for item in values.iter() {
            self.append(py, &item)?;
        }
        Ok(())
    }

    pub fn index(&self, py: Python, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        if let Some(ref t) = self._type {
            if !value.is_instance(t.bind(py))? {
                return Err(PyTypeError::new_err(format!(
                    "Expected item of type {}, got {}",
                    t.bind(py).name()?,
                    value.get_type().name()?
                )));
            }
        }
    
        for (i, item) in self.data.iter().enumerate() {
            if item
                .bind(py)
                .rich_compare(value, pyo3::basic::CompareOp::Eq)?
                .is_truthy()?
            {
                return Ok(i);
            }
        }
    
        Err(PyErr::new::<PyValueError, _>("Value not found"))
    }

    pub fn insert(&mut self, py: Python, index: isize, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.__setitem__(py, index, value)
    }

    pub fn pop(&mut self, py: Python) -> PyResult<PyObject> {
        self.data.pop().ok_or_else(|| {
            PyErr::new::<PyIndexError, _>("pop from empty list")
        })
    }

    pub fn remove(&mut self, index: isize) {
        self.data.remove(index as usize);
    }

    pub fn reverse(&mut self) {
        self.data.reverse();
    }

}

#[pymodule]
pub fn register_vector(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Vector>()?;
    Ok(())
}
