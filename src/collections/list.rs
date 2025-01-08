use pyo3::basic::CompareOp;
use pyo3::exceptions::{PyIndexError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyList, PySlice, PyTuple, PyType};

fn any_to_list(value: &Bound<'_, PyAny>) -> PyResult<List> {
    if let Ok(list) = value.extract::<List>() {
        return Ok(list);
    }

    if let Ok(list) = value.downcast::<PyList>() {
        let data: Vec<PyObject> = list.iter().map(|item| item.into()).collect();
        return Ok(List { data, _type: None });
    }

    Err(PyTypeError::new_err(
        "Expected Rustique List or Python list",
    ))
}

#[derive(Clone)]
#[pyclass(subclass)]
pub struct List {
    data: Vec<PyObject>,
    _type: Option<Py<PyType>>,
}

#[pymethods]
impl List {
    #[new]
    #[pyo3(signature = (*values, _type=None))]
    pub fn __new__(
        values: &Bound<'_, PyTuple>,
        _type: Option<Bound<'_, PyType>>,
    ) -> PyResult<Self> {
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
        Ok(List {
            data: vec,
            _type: _type.map(|t| t.into()),
        })
    }

    fn type_validate(&self, py: Python, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Some(ref t) = self._type {
            if !value.is_instance(t.bind(py))? {
                return Err(PyTypeError::new_err(format!(
                    "Expected item of type {}, got {}",
                    t.bind(py).name()?,
                    value.get_type().name()?
                )));
            }
        }
        Ok(())
    }

    fn __richcmp__(
        &self,
        py: Python,
        #[pyo3(from_py_with = "any_to_list")] other: List,
        op: CompareOp,
    ) -> PyResult<PyObject> {
        match op {
            CompareOp::Eq => {
                if self.data.len() != other.data.len() {
                    return Ok(false.into_py(py));
                }

                // if self._type.is_some() && (self._type != other._type) {
                //     return Ok(false.into_py(py));
                // }

                for (a, b) in self.data.iter().zip(other.data.iter()) {
                    if a.bind(py)
                        .rich_compare(b, CompareOp::Eq)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        continue;
                    } else {
                        return Ok(false.into_py(py));
                    }
                }

                Ok(true.into_py(py))
            }
            CompareOp::Ne => {
                if self.data.len() != other.data.len() {
                    return Ok(true.into_py(py));
                }

                for (a, b) in self.data.iter().zip(other.data.iter()) {
                    if a.bind(py)
                        .rich_compare(b, CompareOp::Eq)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        continue;
                    } else {
                        return Ok(true.into_py(py));
                    }
                }

                Ok(false.into_py(py))
            }
            CompareOp::Lt => {
                for (a, b) in self.data.iter().zip(other.data.iter()) {
                    if a.bind(py)
                        .rich_compare(b, CompareOp::Lt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(true.into_py(py));
                    } else if a
                        .bind(py)
                        .rich_compare(b, CompareOp::Gt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(false.into_py(py));
                    }
                }

                Ok(false.into_py(py))
            }
            CompareOp::Le => {
                for (a, b) in self.data.iter().zip(other.data.iter()) {
                    if a.bind(py)
                        .rich_compare(b, CompareOp::Lt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(true.into_py(py));
                    } else if a
                        .bind(py)
                        .rich_compare(b, CompareOp::Gt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(false.into_py(py));
                    }
                }

                Ok(true.into_py(py))
            }
            CompareOp::Gt => {
                for (a, b) in self.data.iter().zip(other.data.iter()) {
                    if a.bind(py)
                        .rich_compare(b, CompareOp::Gt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(true.into_py(py));
                    } else if a
                        .bind(py)
                        .rich_compare(b, CompareOp::Lt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(false.into_py(py));
                    }
                }

                Ok(false.into_py(py))
            }
            CompareOp::Ge => {
                for (a, b) in self.data.iter().zip(other.data.iter()) {
                    if a.bind(py)
                        .rich_compare(b, CompareOp::Gt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(true.into_py(py));
                    } else if a
                        .bind(py)
                        .rich_compare(b, CompareOp::Lt)
                        .and_then(|res| res.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(false.into_py(py));
                    }
                }

                Ok(true.into_py(py))
            }
        }
    }

    /// Representation of the list
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
        Ok(format!("List{}({})", type_annotation, values.join(", ")))
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
        Ok(format!("List{}({})", type_annotation, values.join(", ")))
    }

    pub fn __len__(&self) -> usize {
        self.data.len()
    }

    pub fn __getitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        if let Ok(idx) = index.extract::<isize>() {
            let actual_idx = if idx < 0 {
                (self.data.len() as isize + idx) as usize
            } else {
                idx as usize
            };

            if actual_idx >= self.data.len() {
                return Err(PyIndexError::new_err(format!(
                    "Index out of range: got {}, length is {}",
                    idx,
                    self.data.len()
                )));
            }

            return Ok(self.data[actual_idx].clone());
        }

        if let Ok(slice) = index.downcast::<PySlice>() {
            let indices = slice.indices(self.data.len() as isize)?;

            let start = indices.start;
            let stop = indices.stop;
            let step = indices.step;

            let mut new_data = Vec::new();
            let mut i = start;

            let positive_step = step > 0;
            while (positive_step && i < stop) || (!positive_step && i > stop) {
                new_data.push(self.data[i as usize].clone());
                i += step;
            }

            return Ok(Py::new(
                index.py(),
                List {
                    data: new_data,
                    _type: self._type.clone(),
                },
            )?
            .into_py(index.py()));
        }

        Err(PyTypeError::new_err(format!(
            "Invalid index type: {}",
            index.get_type().name()?
        )))
    }

    pub fn __setitem__(
        &mut self,
        py: Python,
        index: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        if let Ok(idx) = index.extract::<isize>() {
            let actual_idx = if idx < 0 {
                (self.data.len() as isize + idx) as usize
            } else {
                idx as usize
            };

            if actual_idx >= self.data.len() {
                return Err(PyIndexError::new_err(format!(
                    "Index out of range: got {}, length is {}",
                    idx,
                    self.data.len()
                )));
            }

            self.type_validate(py, value)?;

            self.data[actual_idx] = Py::from(value.clone());
            return Ok(());
        }

        if let Ok(slice) = index.downcast::<PySlice>() {
            let indices = slice.indices(self.data.len() as isize)?;

            let start = indices.start;
            let stop = indices.stop;
            let step = indices.step;
            let slicelen = indices.slicelength;

            let sequence = value
                .downcast::<PyList>()
                .map_err(|_| PyTypeError::new_err("Assigned value must be a list or sequence"))?;

            if sequence.len() != slicelen as usize {
                return Err(PyValueError::new_err(format!(
                    "Attempt to assign sequence of size {} to extended slice of size {}",
                    sequence.len(),
                    slicelen
                )));
            }

            let mut i = start;
            for item in sequence.iter() {
                self.type_validate(py, &item)?;

                self.data[i as usize] = Py::from(item);
                i += step;
            }
            return Ok(());
        }

        Err(PyTypeError::new_err(format!(
            "Invalid index type: {}",
            index.get_type().name()?
        )))
    }

    pub fn __delitem__(&mut self, index: isize) {
        self.data.remove(index as usize);
    }

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
        List {
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
        if let Some(ref t) = self._type {
            if !value.is_instance(t.bind(py))? {
                return Err(PyTypeError::new_err(format!(
                    "Expected item of type {}, got {}",
                    t.bind(py).name()?,
                    value.get_type().name()?
                )));
            }
        }

        self.data.insert(index as usize, Py::from(value.clone()));
        Ok(())
    }

    pub fn pop(&mut self, py: Python) -> PyResult<PyObject> {
        self.data
            .pop()
            .ok_or_else(|| PyErr::new::<PyIndexError, _>("pop from empty list"))
    }

    pub fn remove(&mut self, index: isize) {
        self.data.remove(index as usize);
    }

    pub fn reverse(&mut self) {
        self.data.reverse();
    }
}

#[pymodule]
pub fn register_list(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<List>()?;
    Ok(())
}
