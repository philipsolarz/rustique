use pyo3::prelude::*;
use pyo3::types::{PyAny, PySequence};
use pyo3::exceptions::PyIndexError;

#[pyclass]
struct List {
    elements: Vec<PyObject>
}

#[pymethods]
impl List {
    #[new]
    #[pyo3(signature = (elements=None))]
    fn new(elements: Option<Vec<i32>>) -> Self {
        List {
            elements: elements.unwrap_or_default()
        }
    }

    fn append(&mut self, element: i32) {
        self.elements.push(element);
    }

    fn sum(&self) -> i32 {
        self.elements.iter().sum()
    }

    fn __len__(&self) -> usize {
        self.elements.len()
    }

    fn __getitem__(&self, index: i32) -> PyResult<i32> {
        let i = index as usize;
        if i < self.elements.len() {
            Ok(self.elements[i])
        } else {
            Err(pyo3::exceptions::PyIndexError::new_err("Index out of range!"))
        }
    }

    fn __setitem__(&mut self, index: i32, value: i32) -> PyResult<()> {
        let i = index as usize;
        if i < self.elements.len() {
            self.elements[i] = value;
            Ok(())
        } else {
            Err(pyo3::exceptions::PyIndexError::new_err("Index out of range!"))
        }
    }

    fn __delitem__(&mut self, index: i32) -> PyResult<()> {
        let i = index as usize;
        if i < self.elements.len() {
            self.elements.remove(i);
            Ok(())
        } else {
            Err(pyo3::exceptions::PyIndexError::new_err("Index out of range!"))
        }
    }

    // fn __iter__(&self) -> PyResult<iter::Iter<i32>> {
    //     Ok(self.elements.iter().cloned().into_iter())
    // }

    fn __contains__(&self, element: i32) -> bool {
        self.elements.contains(&element)
    }

    fn __str__(&self) -> String {
        format!("{:?}", self.elements)
    }

    fn __repr__(&self) -> String {
        format!("List({:?})", self.elements)
    }

    fn __add__(&self, other: &List) -> List {
        List {
            elements: self.elements.iter().chain(other.elements.iter()).cloned().collect()
        }
    }

    fn __mul__(&self, other: i32) -> List {
        List {
            elements: self.elements.iter().cloned().cycle().take(self.elements.len() * other as usize).collect()
        }
    }


}

/// A Python module implemented in Rust.
#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // m.add_function(wrap_pyfunction!(sum_as_string, m)?)?;
    m.add_class::<List>()?;
    Ok(())
}
