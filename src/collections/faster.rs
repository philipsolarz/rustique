use indexmap::IndexMap;
use pyo3::{
    ffi::{self, c_str, PyObject_Hash, PyObject_RichCompareBool, Py_EQ},
    prelude::*,
    types::{PyDict, PyList},
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::raw::c_long;

#[pyclass]
pub struct ListIterator {
    index: usize,
    length: usize,
    list: Vec<PyObject>,
}

#[pymethods]
impl ListIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyObject> {
        let py = slf.py();
        if slf.index < slf.length {
            // let list_ptr = slf.list.as_ptr();
            let item_ptr = slf.list[slf.index].as_ptr();
            // let item_ptr = unsafe { ffi::PyList_GetItem(list_ptr, slf.index as isize) };
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

#[pyclass]
pub struct ListIterator2 {
    list: Option<List>,
}

#[pymethods]
impl ListIterator2 {
    #[new]
    pub fn new(list: List) -> Self {
        Self { list: Some(list) }
    }

    // fn __iter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
    //     println!("ListIterator2.__iter__ called");
    //     Ok(slf)
    // }

    fn __next__(mut slf: PyRefMut<'_, Self>) -> PyResult<Option<List>> {
        if let Some(list) = slf.list.take() {
            Ok(Some(list))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone)]
#[pyclass]
pub struct List {
    list: Vec<PyObject>,
}

#[pymethods]
impl List {
    #[new]
    fn new() -> Self {
        List { list: Vec::new() }
    }

    fn __repr__(&self) -> String {
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

    fn append(&mut self, item: PyObject) {
        self.list.push(item);
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<ListIterator>> {
        let py = slf.py();
        // let list_ptr = slf.list.as_ptr();
        let length = slf.list.len();
        // let length = unsafe { ffi::PyList_Size(list_ptr) as usize };
        Py::new(
            py,
            ListIterator {
                index: 0,
                length,
                list: slf.list.clone(),
            },
        )
    }

    // fn __iter__(slf: PyRef<'_, Self>, py: Python) -> PyResult<ListIterator2> {
    //     let x = slf.clone();
    //     Ok(ListIterator2 { list: Some(x) })
    //     // Ok(ListIterator2 {
    //     //     list: Some(slf.into_pyobject(py)?.extract::<List>()?),
    //     // })
    // }
}

#[derive(Debug, Clone)]
pub struct Key {
    key: PyObject,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        unsafe {
            let result = PyObject_RichCompareBool(self.key.as_ptr(), other.key.as_ptr(), Py_EQ);
            result == 1
        }
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        unsafe {
            let hash = PyObject_Hash(self.key.as_ptr()) as c_long;
            if hash != -1 {
                hash.hash(state);
            } else {
                eprintln!("Failed to compute hash for Key.");
            }
        }
    }
}

#[pyclass]
pub struct Dict2 {
    dict: IndexMap<Key, PyObject>,
}

#[pymethods]
impl Dict2 {
    // #[new]
    // fn new() -> Self {
    //     Dict {
    //         dict: IndexMap::new(),
    //     }
    // }

    fn __setitem__(&mut self, py: Python, keys: List, values: List) {
        let num_items = keys.list.len().min(values.list.len());
        self.dict.reserve(num_items);

        unsafe {
            for (key, value) in keys.list.iter().zip(values.list.iter()) {
                let key_ptr = key.as_ptr();
                let value_ptr = value.as_ptr();
                self.dict.insert(
                    Key {
                        key: PyObject::from_borrowed_ptr(py, key_ptr),
                    },
                    PyObject::from_borrowed_ptr(py, value_ptr),
                );
            }
        }
    }

    #[new]
    fn new(keys: List, values: List) -> Self {
        let mut dict = IndexMap::new();
        let num_items = keys.list.len().min(values.list.len());
        dict.reserve(num_items);

        for (key, value) in keys.list.iter().zip(values.list.iter()) {
            dict.insert(Key { key: key.clone() }, value.clone());
        }

        Dict2 { dict }
    }

    // fn __setitem__(&mut self, keys: List, values: List) {
    //     let num_items = keys.list.len().min(values.list.len());
    //     self.dict.reserve(num_items);

    //     for (key, value) in keys.list.iter().zip(values.list.iter()) {
    //         self.dict.insert(Key { key: key.clone() }, value.clone());
    //     }
    // }
}

// #[pyclass]
// pub struct ListIterator {
//     index: usize,
//     length: usize,
//     list: Py<PyList>,
// }

// #[pymethods]
// impl ListIterator {
//     fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
//         slf
//     }

//     fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyObject> {
//         let py = slf.py();
//         if slf.index < slf.length {
//             let list_ptr = slf.list.as_ptr();
//             let item_ptr = unsafe { ffi::PyList_GetItem(list_ptr, slf.index as isize) };
//             if !item_ptr.is_null() {
//                 let obj = unsafe { PyObject::from_borrowed_ptr(py, item_ptr) };
//                 slf.index += 1;
//                 Some(obj)
//             } else {
//                 None
//             }
//         } else {
//             None
//         }
//     }
// }

// #[pyclass]
// pub struct List {
//     list: Py<PyList>,
// }

// #[pymethods]
// impl List {
//     #[new]
//     pub fn new(py: Python) -> Self {
//         List {
//             list: PyList::empty(py).into(),
//         }
//     }

//     pub fn append(&mut self, py: Python, item: PyObject) -> PyResult<()> {
//         unsafe {
//             let list_ptr = self.list.as_ptr();
//             let item_ptr = item.as_ptr();

//             let result = ffi::PyList_Append(list_ptr, item_ptr);

//             if result != 0 {
//                 Err(PyErr::fetch(py))
//             } else {
//                 Ok(())
//             }
//         }
//     }

//     fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<ListIterator>> {
//         let py = slf.py();
//         let list_ptr = slf.list.as_ptr();
//         let length = unsafe { ffi::PyList_Size(list_ptr) as usize };
//         Py::new(
//             py,
//             ListIterator {
//                 index: 0,
//                 length,
//                 list: slf.list.clone(),
//             },
//         )
//     }

//     fn vectorize(&self, py: Python) -> PyResult<Py<PyList>> {
//         let list_ptr = self.list.as_ptr();
//         let length = unsafe { ffi::PyList_Size(list_ptr) as usize };

//         // Create a new Python list to hold the bulk result
//         let py_list = unsafe { ffi::PyList_New(length as isize) };
//         if py_list.is_null() {
//             return Err(PyErr::fetch(py));
//         }

//         // Populate the Python list in a single loop
//         for i in 0..length {
//             let item_ptr = unsafe { ffi::PyList_GetItem(list_ptr, i as isize) };
//             if !item_ptr.is_null() {
//                 // Increment reference and set item (PyList_SetItem steals the reference)
//                 unsafe {
//                     ffi::Py_INCREF(item_ptr);
//                     ffi::PyList_SetItem(py_list, i as isize, item_ptr);
//                 }
//             }
//         }

//         Ok(unsafe { Py::from_owned_ptr(py, py_list) })
//     }
// }

// #[pyclass]
// pub struct Dict {
//     dict: Py<PyDict>,
// }

// #[pymethods]
// impl Dict {
//     #[new]
//     fn new(py: Python) -> Self {
//         Dict {
//             dict: PyDict::new(py).into(),
//         }
//     }

//     fn __setitem__(&mut self, py: Python, key: PyObject, value: PyObject) -> PyResult<()> {
//         unsafe {
//             let dict_ptr = self.dict.as_ptr();
//             let key_ptr = key.as_ptr();
//             let value_ptr = value.as_ptr();

//             let result = ffi::PyDict_SetItem(dict_ptr, key_ptr, value_ptr);

//             if result != 0 {
//                 Err(PyErr::fetch(py))
//             } else {
//                 Ok(())
//             }
//         }
//     }
// }

#[pymodule]
pub fn register_faster(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict2>()?;
    // m.add_class::<Dict2>()?;
    // m.add_class::<List>()?;
    // m.add_class::<List2>()?;
    // m.add_class::<ListIterator>()?;
    // m.add_class::<ListIterator2>()?;
    Ok(())
}
