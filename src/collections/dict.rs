use indexmap::IndexMap;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict, PyDictKeys, PyList, PyNone, PyString, PyTuple, PyType};
use std::collections::HashMap;
use std::fmt::Write;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub struct PyKey {
    obj: PyObject,
}

impl PartialEq for PyKey {
    fn eq(&self, other: &Self) -> bool {
        Python::with_gil(|py| {
            let result = self.obj.call_method1(py, "__eq__", (&other.obj,))?;
            result.extract::<bool>(py)
        })
        .unwrap_or(false)
    }
}

impl Eq for PyKey {}

impl Hash for PyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Python::with_gil(|py| match self.obj.call_method0(py, "__hash__") {
            Ok(hash) => {
                let hash = hash.extract::<isize>(py).unwrap();
                hash.hash(state);
            }
            Err(err) => {
                eprintln!("Failed to compute hash for PyKey: {:?}", err);
            }
        })
    }
}

#[derive(Debug, Clone)]
#[pyclass(subclass)]
pub struct Dict {
    dict: IndexMap<PyKey, PyObject>,
}

#[pymethods]
impl Dict {
    #[new]
    #[pyo3(signature = (*args, **kwargs))]
    pub fn new(args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        let mut dict = IndexMap::new();

        if args.len() == 1 {
            let arg = args.get_item(0)?;

            if let Ok(mapping) = arg.downcast::<PyDict>() {
                for (key, value) in mapping.iter() {
                    dict.insert(PyKey { obj: key.into() }, value.into());
                }
            } else if let Ok(iterable) = arg.try_iter() {
                for item in iterable {
                    let item = item?;

                    if let Ok(tuple) = item.downcast::<PyTuple>() {
                        if tuple.len() == 2 {
                            let key = tuple.get_item(0)?;
                            let value = tuple.get_item(1)?;
                            dict.insert(PyKey { obj: key.into() }, value.into());
                        } else {
                            return Err(PyValueError::new_err(
                                "Dict must be initialized with a sequence of 2-tuples",
                            ));
                        }
                    } else if let Ok(list) = item.downcast::<PyList>() {
                        if list.len() == 2 {
                            let key = list.get_item(0)?;
                            let value = list.get_item(1)?;
                            if key.is_instance_of::<PyString>() || key.is_instance_of::<PyBytes>() {
                                dict.insert(PyKey { obj: key.into() }, value.into());
                            } else {
                                return Err(PyValueError::new_err(
                                    "Keys must be strings or bytes in iterable of lists",
                                ));
                            }
                        } else {
                            return Err(PyValueError::new_err(
                                "Each list in the iterable must contain exactly two elements",
                            ));
                        }
                    }
                }
            } else {
                return Err(PyValueError::new_err(
                    "Dict must be initialized with a mapping, iterable of tuples, or iterable of lists",
                ));
            }
        } else if args.len() > 1 {
            return Err(PyValueError::new_err(
                "Dict must be initialized with a mapping or an iterable of 2-tuples",
            ));
        }

        if let Some(kwargs) = kwargs {
            for (key, value) in kwargs.iter() {
                dict.insert(PyKey { obj: key.into() }, value.into());
            }
        }

        Ok(Dict { dict })
    }

    #[pyo3(signature = (keys, value=None))]
    #[classmethod]
    pub fn fromkeys(
        cls: &Bound<'_, PyType>,
        keys: &Bound<'_, PyAny>,
        value: Option<PyObject>,
    ) -> PyResult<Self> {
        let mut dict = IndexMap::new();

        let value = value.unwrap_or_else(|| Python::with_gil(|py| py.None().into()));

        if let Ok(iterable) = keys.try_iter() {
            for key in iterable {
                let key = key?;
                dict.insert(PyKey { obj: key.into() }, value.clone());
            }
        } else {
            return Err(PyValueError::new_err("Expected an iterable"));
        }

        Ok(Dict { dict })
    }

    pub fn copy(&self) -> Self {
        Dict {
            dict: self.dict.clone(),
        }
    }

    #[pyo3(signature = (key, default=None))]
    pub fn get(&self, key: PyObject, default: Option<PyObject>) -> PyObject {
        Python::with_gil(|py| {
            self.dict
                .get(&PyKey { obj: key }) // Try to get the value from the dictionary
                .cloned() // Clone the value if it exists
                .unwrap_or_else(|| default.unwrap_or_else(|| py.None().into())) // Return default or None
        })
    }

    pub fn __contains__(&self, key: PyObject) -> bool {
        self.dict.contains_key(&PyKey { obj: key })
    }

    #[pyo3(signature = (key, default=None))]
    pub fn pop(&mut self, key: PyObject, default: Option<PyObject>) -> PyResult<PyObject> {
        let key = PyKey { obj: key };
        let value = self.dict.remove(&key).unwrap_or_else(|| {
            default.unwrap_or_else(|| {
                Python::with_gil(|py| {
                    let msg = format!("Key not found: {:?}", key.obj);
                    PyValueError::new_err(msg).into()
                })
            })
        });

        Ok(value)
    }

    pub fn popitem(&mut self) -> PyResult<(PyObject, PyObject)> {
        let (key, value) = self.dict.pop().ok_or_else(|| {
            Python::with_gil(|py| PyValueError::new_err("popitem(): dictionary is empty"))
        })?;

        Ok((key.obj, value))
    }

    pub fn clear(&mut self) {
        self.dict.clear();
    }

    pub fn update(&mut self, other: &Dict) {
        self.dict.extend(
            other
                .dict
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }

    pub fn setdefault(&mut self, key: PyObject, default: PyObject) -> PyObject {
        let key = PyKey { obj: key };
        let value = self.dict.entry(key).or_insert_with(|| default.into());
        value.clone()
    }

    pub fn __repr__(&self) -> PyResult<String> {
        self.__str__()
    }

    pub fn __str__(&self) -> PyResult<String> {
        Python::with_gil(|py| {
            let entries: Result<Vec<String>, PyErr> = self
                .dict
                .iter()
                .map(|(key, value)| {
                    let key_repr = key
                        .obj
                        .call_method0(py, "__repr__")?
                        .extract::<String>(py)?;
                    let value_repr = value.call_method0(py, "__repr__")?.extract::<String>(py)?;
                    Ok(format!("{}: {}", key_repr, value_repr))
                })
                .collect();

            entries.map(|entries| format!("{{{}}}", entries.join(", ")))
        })
    }
}

#[pymodule]
pub fn register_dict(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict>()?;
    Ok(())
}

// fn any_to_dict(value: &Bound<'_, PyAny>) -> PyResult<Dict> {
//     if let Ok(dict) = value.extract::<Dict>() {
//         return Ok(dict);
//     }

//     if let Ok(dict) = value.downcast::<PyDict>() {
//         let data: HashMap<PyObject, PyObject> = dict.iter().map(|item| item.into()).collect();
//         return Ok(Dict { data, _type: None });
//     }

//     Err(PyTypeError::new_err(
//         "Expected Rustique List or Python list",
//     ))
// }

// #[derive(Clone)]
// #[pyclass(subclass)]
// pub struct Dict {
//     dict: HashMap<PyObject, PyObject>,
//     // _type: Option<Py<PyType>>,
// }

// #[pymethods]
// impl Dict {
//     #[new]
//     #[pyo3(signature=(
//         *tuples,
//         // _type=None
//     ))]
//     pub fn new(
//         tuples: &Bound<'_, PyTuple>,
//         // _type: Option<Bound<'_, PyType>>
//     ) -> PyResult<Self> {
//         let mut dict = HashMap::new();
//         for item in tuples.iter() {
//             let tuple = item.extract::<(PyObject, PyObject)>()?;
//             let key = tuple.0;
//             let value = tuple.1;
//             dict.insert(key, value);
//         }
//         Ok(Dict {
//             dict,
//             // _type
//         })
//     }

//     pub fn insert(&mut self, key: PyObject, value: PyObject) {
//         self.dict.insert(key, value);
//     }

//     pub fn get(&self, key: PyObject) -> Option<PyObject> {
//         self.dict.get(&key).cloned()
//     }

//     pub fn __repr__(&self) -> String {
//         format!("{:?}", self.dict)
//     }
// }
