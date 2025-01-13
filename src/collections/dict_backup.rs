use indexmap::IndexMap;
use pyo3::{
    ffi::{PyObject_Hash, PyObject_RichCompareBool, Py_EQ},
    prelude::*,
    types::{PyBytes, PyDict, PyIterator, PyList, PyString, PyTuple},
};
use std::hash::{Hash, Hasher};
use std::{ffi::c_long, ops::Index};

use crate::{
    list::List,
    tuple::{self, Tuple},
};

#[derive(Debug, Clone)]
pub struct Value {
    value: PyObject,
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

#[derive(Debug, Clone)]
#[pyclass]
pub struct Dict {
    dict: IndexMap<Key, Value>,
}

#[pymethods]
impl Dict {
    #[new]
    #[pyo3(signature = (*args, **kwargs))]
    pub fn new(
        py: Python,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let mut dict = IndexMap::new();

        if args.len() == 1 {
            let arg = args.get_item(0)?;
            if let Ok(mapping) = arg.extract::<Dict>() {
                dict = mapping.dict.clone();
            } else if let Ok(mapping) = arg.downcast::<PyDict>() {
                dict.reserve(mapping.len());
                for (key, value) in mapping.iter() {
                    dict.insert(
                        Key { key: key.into() },
                        Value {
                            value: value.into(),
                        },
                    );
                }
            } else if let Ok(list) = arg.extract::<List>() {
                dict.reserve(list.list.len());
                for item in list.list {
                    if let Ok(tuple) = item.bind(py).extract::<Tuple>() {
                        if tuple.tuple.len() == 2 {
                            dict.insert(
                                Key {
                                    key: tuple.tuple[0].clone(),
                                },
                                Value {
                                    value: tuple.tuple[1].clone(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.tuple.len());
                        }
                    } else if let Ok(tuple) = item.bind(py).downcast::<PyTuple>() {
                        if tuple.len() == 2 {
                            let key = tuple.get_item(0)?;
                            let value = tuple.get_item(1)?;
                            dict.insert(
                                Key { key: key.into() },
                                Value {
                                    value: value.into(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
                        }
                    } else if let Ok(list) = item.bind(py).extract::<List>() {
                        if list.list.len() == 2 {
                            let key = list.list[0].clone();
                            let value = list.list[1].clone();
                            if key.bind(py).is_instance_of::<PyString>()
                                || key.bind(py).is_instance_of::<PyBytes>()
                            {
                                dict.insert(
                                    Key { key: key.into() },
                                    Value {
                                        value: value.into(),
                                    },
                                );
                            } else {
                                eprintln!(
                                    "Expected key to be a string or bytes, got {:?}",
                                    key.bind(py).get_type().name()
                                );
                            }
                        } else {
                            eprintln!("Expected list of length 2, got {:?}", list.list.len());
                        }
                    } else if let Ok(list) = item.bind(py).downcast::<PyList>() {
                        if list.len() == 2 {
                            let key = list.get_item(0)?;
                            let value = list.get_item(1)?;
                            if key.is_instance_of::<PyString>() || key.is_instance_of::<PyBytes>() {
                                dict.insert(
                                    Key { key: key.into() },
                                    Value {
                                        value: value.into(),
                                    },
                                );
                            } else {
                                eprintln!(
                                    "Expected key to be a string or bytes, got {:?}",
                                    key.get_type().name()
                                );
                            }
                        } else {
                            eprintln!("Expected list of length 2, got {:?}", list.len());
                        }
                    } else {
                        eprintln!("Expected tuple, got {:?}", item.bind(py).get_type().name());
                    }
                    if let Ok(tuple) = item.bind(py).downcast::<PyTuple>() {
                        if tuple.len() == 2 {
                            let key = tuple.get_item(0)?;
                            let value = tuple.get_item(1)?;
                            dict.insert(
                                Key { key: key.into() },
                                Value {
                                    value: value.into(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
                        }
                    } else {
                        eprintln!("Expected tuple, got {:?}", item.bind(py).get_type().name());
                    }
                }
            } else if let Ok(list) = arg.downcast::<PyList>() {
                dict.reserve(list.len());
                for item in list.iter() {
                    if let Ok(tuple) = item.extract::<Tuple>() {
                        if tuple.tuple.len() == 2 {
                            dict.insert(
                                Key {
                                    key: tuple.tuple[0].clone(),
                                },
                                Value {
                                    value: tuple.tuple[1].clone(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.tuple.len());
                        }
                    } else if let Ok(tuple) = item.downcast::<PyTuple>() {
                        if tuple.len() == 2 {
                            let key = tuple.get_item(0)?;
                            let value = tuple.get_item(1)?;
                            dict.insert(
                                Key { key: key.into() },
                                Value {
                                    value: value.into(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
                        }
                    } else if let Ok(list) = item.extract::<List>() {
                        if list.list.len() == 2 {
                            let key = list.list[0].clone();
                            let value = list.list[1].clone();
                            if key.bind(py).is_instance_of::<PyString>()
                                || key.bind(py).is_instance_of::<PyBytes>()
                            {
                                dict.insert(
                                    Key { key: key.into() },
                                    Value {
                                        value: value.into(),
                                    },
                                );
                            } else {
                                eprintln!(
                                    "Expected key to be a string or bytes, got {:?}",
                                    key.bind(py).get_type().name()
                                );
                            }
                        } else {
                            eprintln!("Expected list of length 2, got {:?}", list.list.len());
                        }
                    } else if let Ok(list) = item.downcast::<PyList>() {
                        if list.len() == 2 {
                            let key = list.get_item(0)?;
                            let value = list.get_item(1)?;
                            if key.is_instance_of::<PyString>() || key.is_instance_of::<PyBytes>() {
                                dict.insert(
                                    Key { key: key.into() },
                                    Value {
                                        value: value.into(),
                                    },
                                );
                            } else {
                                eprintln!(
                                    "Expected key to be a string or bytes, got {:?}",
                                    key.get_type().name()
                                );
                            }
                        } else {
                            eprintln!("Expected list of length 2, got {:?}", list.len());
                        }
                    } else {
                        eprintln!("Expected tuple, got {:?}", item.get_type().name());
                    }
                }
            } else if let Ok(iterable) = arg.downcast::<PyIterator>() {
                for item in iterable {
                    let item = item?;
                    if let Ok(tuple) = item.extract::<Tuple>() {
                        if tuple.tuple.len() == 2 {
                            dict.insert(
                                Key {
                                    key: tuple.tuple[0].clone(),
                                },
                                Value {
                                    value: tuple.tuple[1].clone(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.tuple.len());
                        }
                    } else if let Ok(tuple) = item.downcast::<PyTuple>() {
                        if tuple.len() == 2 {
                            let key = tuple.get_item(0)?;
                            let value = tuple.get_item(1)?;
                            dict.insert(
                                Key { key: key.into() },
                                Value {
                                    value: value.into(),
                                },
                            );
                        } else {
                            eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
                        }
                    } else if let Ok(list) = item.extract::<List>() {
                        if list.list.len() == 2 {
                            let key = list.list[0].clone();
                            let value = list.list[1].clone();
                            if key.bind(py).is_instance_of::<PyString>()
                                || key.bind(py).is_instance_of::<PyBytes>()
                            {
                                dict.insert(
                                    Key { key: key.into() },
                                    Value {
                                        value: value.into(),
                                    },
                                );
                            } else {
                                eprintln!(
                                    "Expected key to be a string or bytes, got {:?}",
                                    key.bind(py).get_type().name()
                                );
                            }
                        } else {
                            eprintln!("Expected list of length 2, got {:?}", list.list.len());
                        }
                    } else if let Ok(list) = item.downcast::<PyList>() {
                        if list.len() == 2 {
                            let key = list.get_item(0)?;
                            let value = list.get_item(1)?;
                            if key.is_instance_of::<PyString>() || key.is_instance_of::<PyBytes>() {
                                dict.insert(
                                    Key { key: key.into() },
                                    Value {
                                        value: value.into(),
                                    },
                                );
                            } else {
                                eprintln!(
                                    "Expected key to be a string or bytes, got {:?}",
                                    key.get_type().name()
                                );
                            }
                        } else {
                            eprintln!("Expected list of length 2, got {:?}", list.len());
                        }
                    } else {
                        eprintln!("Expected tuple, got {:?}", item.get_type().name());
                    }
                }
            } else {
                eprintln!(
                    "Expected mapping or iterable, got {:?}",
                    arg.get_type().name()
                );
            }
        } else if args.len() > 1 {
            eprintln!("Expected 1 argument, got {}", args.len());
        }

        if let Some(kwargs) = kwargs {
            for (key, value) in kwargs.iter() {
                dict.insert(
                    Key { key: key.into() },
                    Value {
                        value: value.into(),
                    },
                );
            }
        }

        Ok(Dict { dict })
    }

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
                    Value {
                        value: PyObject::from_borrowed_ptr(py, value_ptr),
                    },
                );
            }
        }
    }
}

#[pyclass]
pub struct Dict2 {
    dict: IndexMap<Key, Value>,
}

#[pymethods]
impl Dict2 {
    // #[new]
    // fn new() -> Self {
    //     Dict {
    //         dict: IndexMap::new(),
    //     }
    // }

    // fn __setitem__(&mut self, py: Python, keys: List, values: List) {
    //     let num_items = keys.list.len().min(values.list.len());
    //     self.dict.reserve(num_items);

    //     unsafe {
    //         for (key, value) in keys.list.iter().zip(values.list.iter()) {
    //             let key_ptr = key.as_ptr();
    //             let value_ptr = value.as_ptr();
    //             self.dict.insert(
    //                 Key {
    //                     key: PyObject::from_borrowed_ptr(py, key_ptr),
    //                 },
    //                 PyObject::from_borrowed_ptr(py, value_ptr),
    //             );
    //         }
    //     }
    // }

    #[new]
    fn new(keys: List, values: List) -> Self {
        let mut dict = IndexMap::new();
        let num_items = keys.list.len().min(values.list.len());
        dict.reserve(num_items);

        for (key, value) in keys.list.iter().zip(values.list.iter()) {
            dict.insert(
                Key { key: key.clone() },
                Value {
                    value: value.clone(),
                },
            );
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

#[pymodule]
pub fn register_dict(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict>()?;
    m.add_class::<Dict2>()?;
    Ok(())
}
