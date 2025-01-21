use pyo3::prelude::*;

use pyo3::prelude::*;

#[pyclass]
pub struct SequenceIterator {
    pub index: usize,
    pub length: usize,
    pub elements: Vec<PyObject>,
}

#[pymethods]
impl SequenceIterator {
    pub fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    pub fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyObject> {
        if slf.index < slf.length {
            let obj = slf.elements[slf.index].clone();
            slf.index += 1;
            Some(obj)
        } else {
            None
        }
    }
}

#[pyclass(subclass)]
pub struct Sequence {
    pub elements: Vec<PyObject>,
}

#[pymethods]
impl Sequence {
    #[new]
    pub fn new(elements: Vec<PyObject>) -> Self {
        Sequence { elements }
    }

    pub fn __len__(&self) -> usize {
        self.elements.len()
    }

    pub fn __getitem__(&self, index: usize) -> Option<PyObject> {
        self.elements.get(index).cloned()
    }
}
