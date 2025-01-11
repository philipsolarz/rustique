use indexmap::IndexMap;
use pyo3::{
    ffi,
    prelude::*,
    types::{PyDict, PyList, PyTuple},
    AsPyPointer,
};

#[pyclass]
pub struct Dict3 {
    dict: IndexMap<i32, i32>,
}

#[pymethods]
impl Dict3 {
    #[new]
    fn new() -> Self {
        Dict3 {
            dict: IndexMap::new(),
        }
    }

    fn __setitem__(&mut self, key: i32, value: i32) {
        self.dict.insert(key, value);
    }

    fn __getitem__(&self, key: i32) -> Option<&i32> {
        self.dict.get(&key)
    }

    fn insert_bulk(&mut self, pairs: &Bound<'_, PyList>) -> PyResult<()> {
        for pair in pairs.iter() {
            let tuple = pair.downcast::<PyTuple>()?;
            if tuple.len() == 2 {
                let key: i32 = tuple.get_item(0)?.extract()?;
                let value: i32 = tuple.get_item(1)?.extract()?;
                self.dict.insert(key, value);
            } else {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "Each item in the list must be a tuple of length 2",
                ));
            }
        }
        Ok(())
    }
}

#[pyclass]
pub struct Dict2 {
    dict: Py<PyDict>,
}

#[pymethods]
impl Dict2 {
    #[new]
    fn new(py: Python) -> Self {
        Dict2 {
            dict: PyDict::new(py).into(),
        }
    }

    fn __setitem__(&mut self, py: Python, key: PyObject, value: PyObject) -> PyResult<()> {
        unsafe {
            let dict_ptr = self.dict.as_ptr();
            let key_ptr = key.as_ptr();
            let value_ptr = value.as_ptr();

            let result = ffi::PyDict_SetItem(dict_ptr, key_ptr, value_ptr);

            if result != 0 {
                Err(PyErr::fetch(py))
            } else {
                Ok(())
            }
        }
    }

    // fn insert_bulk(&mut self, py: Python, pairs: &Bound<'_, PyList>) -> PyResult<()> {
    //     unsafe {
    //         let dict_ptr = self.dict.as_ptr();

    //         for pair in pairs.iter() {
    //             let tuple = pair.downcast::<PyTuple>()?;
    //             if tuple.len() == 2 {
    //                 let key = tuple.get_item(0)?;
    //                 let value = tuple.get_item(1)?;

    //                 let key_ptr = key.as_ptr();
    //                 let value_ptr = value.as_ptr();

    //                 let result = ffi::PyDict_SetItem(dict_ptr, key_ptr, value_ptr);

    //                 if result != 0 {
    //                     return Err(PyErr::fetch(py));
    //                 }
    //             } else {
    //                 return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
    //                     "Each item in the list must be a tuple of length 2",
    //                 ));
    //             }
    //         }
    //     }
    //     Ok(())
    // }

    fn insert_bulk(&mut self, py: Python, pairs: &Bound<'_, PyList>) -> PyResult<()> {
        unsafe {
            let dict_ptr = self.dict.as_ptr();
            let mut failed = false;

            for pair in pairs.iter() {
                let tuple = pair.downcast::<PyTuple>()?;
                if tuple.len() == 2 {
                    let key = tuple.get_item(0)?;
                    let value = tuple.get_item(1)?;

                    let key_ptr = key.as_ptr();
                    let value_ptr = value.as_ptr();

                    if ffi::PyDict_SetItem(dict_ptr, key_ptr, value_ptr) != 0 {
                        failed = true;
                    }
                } else {
                    return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                        "Each item in the list must be a tuple of length 2",
                    ));
                }
            }

            if failed {
                return Err(PyErr::fetch(py));
            }
        }
        Ok(())
    }
}

#[pymodule]
pub fn register_pydict(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict2>()?;
    m.add_class::<Dict3>()?;
    Ok(())
}
