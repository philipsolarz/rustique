use pyo3::prelude::*;

#[pyclass]
pub struct ListIterator {
    index: usize,
    length: usize,
    list: Vec<PyObject>,
}

#[pymethods]
impl ListIterator {
    pub fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    pub fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyObject> {
        let py = slf.py();
        if slf.index < slf.length {
            let item_ptr = slf.list[slf.index].as_ptr();
            if !item_ptr.is_null() {
                let obj = unsafe { PyObject::from_borrowed_ptr(py, item_ptr) };
                slf.index += 1;
                Some(obj)
            } else {
                None
            }
        } else {
            None
        }
    }
}

#[derive(Clone)]
#[pyclass]
pub struct List {
    pub list: Vec<PyObject>,
}

#[pymethods]
impl List {
    #[new]
    pub fn new() -> Self {
        List { list: Vec::new() }
    }

    pub fn __repr__(&self) -> String {
        Python::with_gil(|py| {
            let reprs: Vec<String> = self
                .list
                .iter()
                .map(|obj| {
                    obj.call_method0(py, "__repr__")
                        .and_then(|repr_obj| repr_obj.extract::<String>(py))
                        .unwrap_or_else(|_| "<error>".to_string())
                })
                .collect();

            format!("List([{}])", reprs.join(", "))
        })
    }

    pub fn append(&mut self, item: PyObject) {
        self.list.push(item);
    }

    pub fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<ListIterator>> {
        let py = slf.py();
        let length = slf.list.len();
        Py::new(
            py,
            ListIterator {
                index: 0,
                length,
                list: slf.list.clone(),
            },
        )
    }
}

#[pymodule]
pub fn register_list(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<List>()?;
    Ok(())
}
