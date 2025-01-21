// use crate::{
//     list::List,
//     tuple::{self, Tuple},
// };
// use indexmap::IndexMap;
use pyo3::{
    ffi::{PyObject_Hash, PyObject_RichCompareBool, Py_EQ},
    prelude::*,
    types::{PyBytes, PyDict, PyIterator, PyList, PyMapping, PySequence, PyString, PyTuple},
};
use std::hash::{Hash, Hasher};
// use std::time::Instant;
use std::{ffi::c_long, ops::Index};

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyTuple};
use std::collections::HashMap;

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

// #[derive(Debug, Clone)]
// #[pyclass]
// pub struct Dict {
//     dict: IndexMap<Key, Value>,
// }

// #[pymethods]
// impl Dict {
//     #[new]
//     #[pyo3(signature = (*args, **kwargs))]
//     pub fn new(
//         py: Python,
//         args: &Bound<'_, PyTuple>,
//         kwargs: Option<&Bound<'_, PyDict>>,
//     ) -> PyResult<Self> {
//         // let start_time = Instant::now();
//         let mut dict = IndexMap::new();

//         if args.len() == 1 {
//             let arg = args.get_item(0)?;

//             if let Ok(mapping) = arg.downcast::<Dict>() {
//                 dict = mapping.borrow().dict.clone();
//             } else if let Ok(mapping) = arg.downcast::<PyDict>() {
//                 dict.reserve(mapping.len());
//                 for (key, value) in mapping.iter() {
//                     dict.insert(
//                         Key { key: key.into() },
//                         Value {
//                             value: value.into(),
//                         },
//                     );
//                 }
//             } else if let Ok(mapping) = arg.downcast::<PyMapping>() {
//                 dict.reserve(mapping.len()?);
//                 // mapping has get_item, keys, values, items
//                 todo!();
//             } else if let Ok(list) = arg.downcast::<List>() {
//                 dict.reserve(list.borrow().list.len());
//                 for item in &list.borrow().list {
//                     if let Ok(tuple) = item.bind(py).downcast::<Tuple>() {
//                         if tuple.borrow().tuple.len() == 2 {
//                             dict.insert(
//                                 Key {
//                                     key: tuple.borrow().tuple[0].clone(),
//                                 },
//                                 Value {
//                                     value: tuple.borrow().tuple[1].clone(),
//                                 },
//                             );
//                         } else {
//                             eprintln!(
//                                 "Expected tuple of length 2, got {:?}",
//                                 tuple.borrow().tuple.len()
//                             );
//                         }
//                     } else if let Ok(list) = item.bind(py).downcast::<List>() {
//                         if list.borrow().list.len() == 2 {
//                             dict.insert(
//                                 Key {
//                                     key: list.borrow().list[0].clone(),
//                                 },
//                                 Value {
//                                     value: list.borrow().list[1].clone(),
//                                 },
//                             );
//                             // let key = list.borrow().list[0].clone();
//                             // let value = list.borrow().list[1].clone();
//                             // if key.bind(py).is_instance_of::<PyString>()
//                             //     || key.bind(py).is_instance_of::<PyBytes>()
//                             // {
//                             //     dict.insert(
//                             //         Key { key: key.into() },
//                             //         Value {
//                             //             value: value.into(),
//                             //         },
//                             //     );
//                             // } else {
//                             //     eprintln!(
//                             //         "Expected key to be a string or bytes, got {:?}",
//                             //         key.bind(py).get_type().name()
//                             //     );
//                             // }
//                         } else {
//                             eprintln!(
//                                 "Expected list of length 2, got {:?}",
//                                 list.borrow().list.len()
//                             );
//                         }
//                     } else if let Ok(sequence) = item.bind(py).downcast::<PySequence>() {
//                         if sequence.len()? == 2 {
//                             let key = sequence.get_item(0)?;
//                             let value = sequence.get_item(1)?;
//                             dict.insert(
//                                 Key { key: key.into() },
//                                 Value {
//                                     value: value.into(),
//                                 },
//                             );
//                         } else {
//                             eprintln!("Expected sequence of length 2, got {:?}", sequence.len());
//                         }
//                     } else if let Ok(tuple) = item.bind(py).downcast::<PyTuple>() {
//                         if tuple.len() == 2 {
//                             let key = tuple.get_item(0)?;
//                             let value = tuple.get_item(1)?;
//                             dict.insert(
//                                 Key { key: key.into() },
//                                 Value {
//                                     value: value.into(),
//                                 },
//                             );
//                         } else {
//                             eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
//                         }
//                     } else if let Ok(list) = item.bind(py).downcast::<PyList>() {
//                         if list.len() == 2 {
//                             let key = list.get_item(0)?;
//                             let value = list.get_item(1)?;
//                             if key.is_instance_of::<PyString>() || key.is_instance_of::<PyBytes>() {
//                                 dict.insert(
//                                     Key { key: key.into() },
//                                     Value {
//                                         value: value.into(),
//                                     },
//                                 );
//                             } else {
//                                 eprintln!(
//                                     "Expected key to be a string or bytes, got {:?}",
//                                     key.get_type().name()
//                                 );
//                             }
//                         } else {
//                             eprintln!("Expected list of length 2, got {:?}", list.len());
//                         }
//                     } else {
//                         eprintln!("Expected tuple, got {:?}", item.bind(py).get_type().name());
//                     }
//                 }
//             } else if let Ok(list) = arg.downcast::<PyList>() {
//                 dict.reserve(list.len());
//                 for item in list.iter() {
//                     if let Ok(tuple) = item.extract::<Tuple>() {
//                         if tuple.tuple.len() == 2 {
//                             dict.insert(
//                                 Key {
//                                     key: tuple.tuple[0].clone(),
//                                 },
//                                 Value {
//                                     value: tuple.tuple[1].clone(),
//                                 },
//                             );
//                         } else {
//                             eprintln!("Expected tuple of length 2, got {:?}", tuple.tuple.len());
//                         }
//                     } else if let Ok(tuple) = item.downcast::<PyTuple>() {
//                         if tuple.len() == 2 {
//                             let key = tuple.get_item(0)?;
//                             let value = tuple.get_item(1)?;
//                             dict.insert(
//                                 Key { key: key.into() },
//                                 Value {
//                                     value: value.into(),
//                                 },
//                             );
//                         } else {
//                             eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
//                         }
//                     } else if let Ok(list) = item.extract::<List>() {
//                         if list.list.len() == 2 {
//                             let key = list.list[0].clone();
//                             let value = list.list[1].clone();
//                             if key.bind(py).is_instance_of::<PyString>()
//                                 || key.bind(py).is_instance_of::<PyBytes>()
//                             {
//                                 dict.insert(
//                                     Key { key: key.into() },
//                                     Value {
//                                         value: value.into(),
//                                     },
//                                 );
//                             } else {
//                                 eprintln!(
//                                     "Expected key to be a string or bytes, got {:?}",
//                                     key.bind(py).get_type().name()
//                                 );
//                             }
//                         } else {
//                             eprintln!("Expected list of length 2, got {:?}", list.list.len());
//                         }
//                     } else if let Ok(list) = item.downcast::<PyList>() {
//                         if list.len() == 2 {
//                             let key = list.get_item(0)?;
//                             let value = list.get_item(1)?;
//                             if key.is_instance_of::<PyString>() || key.is_instance_of::<PyBytes>() {
//                                 dict.insert(
//                                     Key { key: key.into() },
//                                     Value {
//                                         value: value.into(),
//                                     },
//                                 );
//                             } else {
//                                 eprintln!(
//                                     "Expected key to be a string or bytes, got {:?}",
//                                     key.get_type().name()
//                                 );
//                             }
//                         } else {
//                             eprintln!("Expected list of length 2, got {:?}", list.len());
//                         }
//                     } else {
//                         eprintln!("Expected tuple, got {:?}", item.get_type().name());
//                     }
//                 }
//             } else if let Ok(iterable) = arg.downcast::<PyIterator>() {
//                 for item in iterable {
//                     if let Ok(item) = item {
//                         if let Ok(inner_iterable) = item.downcast::<PyIterator>() {
//                             let mut inner_iterable = inner_iterable.clone();

//                             // Safely grab the first and second elements from the inner iterator
//                             let key = inner_iterable.next().and_then(|res| res.ok());
//                             let value = inner_iterable.next().and_then(|res| res.ok());

//                             if let (Some(key), Some(value)) = (key, value) {
//                                 dict.insert(
//                                     Key { key: key.into() },
//                                     Value {
//                                         value: value.into(),
//                                     },
//                                 );
//                             } else {
//                                 eprintln!("Inner iterable did not have exactly two elements.");
//                             }
//                         }
//                     }
//                 }
//             } else {
//                 eprintln!(
//                     "Expected mapping or iterable, got {:?}",
//                     arg.get_type().name()
//                 );
//             }
//         } else if args.len() > 1 {
//             eprintln!("Expected 1 argument, got {}", args.len());
//         }

//         if let Some(kwargs) = kwargs {
//             for (key, value) in kwargs.iter() {
//                 dict.insert(
//                     Key { key: key.into() },
//                     Value {
//                         value: value.into(),
//                     },
//                 );
//             }
//         }

//         Ok(Dict { dict })
//     }

//     fn __setitem__(&mut self, py: Python, keys: List, values: List) {
//         let num_items = keys.list.len().min(values.list.len());
//         self.dict.reserve(num_items);

//         unsafe {
//             for (key, value) in keys.list.iter().zip(values.list.iter()) {
//                 let key_ptr = key.as_ptr();
//                 let value_ptr = value.as_ptr();
//                 self.dict.insert(
//                     Key {
//                         key: PyObject::from_borrowed_ptr(py, key_ptr),
//                     },
//                     Value {
//                         value: PyObject::from_borrowed_ptr(py, value_ptr),
//                     },
//                 );
//             }
//         }
//     }
// }

// #[pyclass]
// pub struct Dict2 {
//     dict: IndexMap<Key, Value>,
// }

// #[pymethods]
// impl Dict2 {
//     // #[new]
//     // fn new() -> Self {
//     //     Dict {
//     //         dict: IndexMap::new(),
//     //     }
//     // }

//     // fn __setitem__(&mut self, py: Python, keys: List, values: List) {
//     //     let num_items = keys.list.len().min(values.list.len());
//     //     self.dict.reserve(num_items);

//     //     unsafe {
//     //         for (key, value) in keys.list.iter().zip(values.list.iter()) {
//     //             let key_ptr = key.as_ptr();
//     //             let value_ptr = value.as_ptr();
//     //             self.dict.insert(
//     //                 Key {
//     //                     key: PyObject::from_borrowed_ptr(py, key_ptr),
//     //                 },
//     //                 PyObject::from_borrowed_ptr(py, value_ptr),
//     //             );
//     //         }
//     //     }
//     // }

//     // #[new]
//     // fn new(keys: List, values: List) -> Self {
//     //     let mut dict = IndexMap::new();
//     //     let num_items = keys.list.len().min(values.list.len());
//     //     dict.reserve(num_items);

//     //     for (key, value) in keys.list.iter().zip(values.list.iter()) {
//     //         dict.insert(
//     //             Key { key: key.clone() },
//     //             Value {
//     //                 value: value.clone(),
//     //             },
//     //         );
//     //     }

//     //     Dict2 { dict }
//     // }

//     #[new]
//     #[pyo3(signature = (*args, **kwargs))]
//     pub fn new(
//         py: Python,
//         args: &Bound<'_, PyTuple>,
//         kwargs: Option<&Bound<'_, PyDict>>,
//     ) -> PyResult<Self> {
//         let mut dict = IndexMap::new();

//         if args.len() == 1 {
//             let arg = args.get_item(0)?;

//             if let Ok(mapping) = arg.downcast::<Dict>() {
//                 dict = mapping.borrow().dict.clone();
//             } else if let Ok(mapping) = arg.downcast::<PyDict>() {
//                 dict.reserve(mapping.len());
//                 for (key, value) in mapping.iter() {
//                     dict.insert(
//                         Key { key: key.into() },
//                         Value {
//                             value: value.into(),
//                         },
//                     );
//                 }
//             } else if let Ok(list) = arg.downcast::<List>() {
//                 dict.reserve(list.borrow().list.len());
//                 for item in &list.borrow().list {
//                     if let Ok(tuple) = item.bind(py).downcast::<Tuple>() {
//                         if tuple.borrow().tuple.len() == 2 {
//                             dict.insert(
//                                 Key {
//                                     key: tuple.borrow().tuple[0].clone(),
//                                 },
//                                 Value {
//                                     value: tuple.borrow().tuple[1].clone(),
//                                 },
//                             );
//                         } else {
//                             eprintln!(
//                                 "Expected tuple of length 2, got {:?}",
//                                 tuple.borrow().tuple.len()
//                             );
//                         }
//                     } else if let Ok(tuple) = item.bind(py).downcast::<PyTuple>() {
//                         if tuple.len() == 2 {
//                             let key = tuple.get_item(0)?;
//                             let value = tuple.get_item(1)?;
//                             dict.insert(
//                                 Key { key: key.into() },
//                                 Value {
//                                     value: value.into(),
//                                 },
//                             );
//                         } else {
//                             eprintln!("Expected tuple of length 2, got {:?}", tuple.len());
//                         }
//                     }
//                 }
//             } else if let Ok(iterable) = arg.downcast::<PyIterator>() {
//                 for item in iterable {
//                     if let Ok(item) = item {
//                         if let Ok(inner_iterable) = item.downcast::<PyIterator>() {
//                             let mut inner_iterable = inner_iterable.clone();

//                             // Safely grab the first and second elements from the inner iterator
//                             let key = inner_iterable.next().and_then(|res| res.ok());
//                             let value = inner_iterable.next().and_then(|res| res.ok());

//                             if let (Some(key), Some(value)) = (key, value) {
//                                 dict.insert(
//                                     Key { key: key.into() },
//                                     Value {
//                                         value: value.into(),
//                                     },
//                                 );
//                             } else {
//                                 eprintln!("Inner iterable did not have exactly two elements.");
//                             }
//                         }
//                     }
//                 }
//             }
//         }

//         if let Some(kwargs) = kwargs {
//             for (key, value) in kwargs.iter() {
//                 dict.insert(
//                     Key { key: key.into() },
//                     Value {
//                         value: value.into(),
//                     },
//                 );
//             }
//         }

//         Ok(Dict2 { dict })
//     }

//     // fn __setitem__(&mut self, keys: List, values: List) {
//     //     let num_items = keys.list.len().min(values.list.len());
//     //     self.dict.reserve(num_items);

//     //     for (key, value) in keys.list.iter().zip(values.list.iter()) {
//     //         self.dict.insert(Key { key: key.clone() }, value.clone());
//     //     }
//     // }
// }

/// A minimal dict-like class in Rust
#[pyclass]
pub struct Dict {
    // We store key-value pairs in a `HashMap<PyObject, PyObject>` for simplicity.
    map: HashMap<Key, Value>,
}

#[pymethods]
impl Dict {
    /// Mimic Python's `dict()` constructor
    ///
    /// Usage:
    ///  - `Dict()` -> empty
    ///  - `Dict(mapping)` -> copy key-value pairs from `mapping`
    ///  - `Dict(iterable_of_pairs)` -> from each `(k, v)` in the iterable
    ///  - `Dict(**kwargs)` -> from the keyword arguments
    ///  - Combinations like `Dict(mapping, **kwargs)` also allowed
    #[new]
    #[pyo3(signature = (*args, **kwargs))]
    pub fn new(
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        // Python's dict() does not allow more than one positional argument
        if args.len() > 1 {
            return Err(PyErr::new::<PyTypeError, _>(
                "dict expected at most 1 arguments, got more",
            ));
        }

        // Start with an empty HashMap
        let mut map = HashMap::new();

        // If there is exactly one positional argument, process it
        if args.len() == 1 {
            let obj = args.get_item(0)?;
            update_with_obj(py, &mut map, &obj)?;
        }

        // Process **kwargs
        if let Some(kw_dict) = kwargs {
            for (k, v) in kw_dict.iter() {
                // Each key is typically a Python string in real usage, but we’ll just store
                // them as PyObject to remain flexible.
                let key_obj = k.unbind();
                let val_obj = v.unbind();
                map.insert(key_obj, val_obj);
            }
        }

        Ok(Self { map })
    }
}

// A helper function to mimic the `dict.update(...)` logic that Python’s dict
// uses internally. It checks if `obj` is a mapping (has a `.keys()` method).
// If so, it copies key-value pairs from there; otherwise, it treats `obj`
// as an iterable of `(key, value)` pairs.
fn update_with_obj(
    py: Python<'_>,
    map: &mut HashMap<PyObject, PyObject>,
    obj: &Bound<'_, PyAny>,
) -> PyResult<()> {
    // Does `obj` appear to be a mapping? (i.e., has a .keys attribute)
    if obj.hasattr("keys")? {
        // If so, call `obj.keys()` to get the keys
        let keys = obj.getattr("keys")?.call0()?;
        // Loop over each key in that sequence
        for key_item in keys.try_iter()? {
            let key_pyobj = key_item?.unbind();
            // For mapping objects, you typically do `value = obj[key]`
            let value_pyobj = obj.get_item(key_pyobj.as_ref(py))?.unbind();
            map.insert(key_pyobj, value_pyobj);
        }
    } else {
        // Not a mapping: treat `obj` as an iterable of (key, value) pairs
        for pair_item in obj.iter()? {
            let pair = pair_item?;
            // In Python, we expect something like: for (k, v) in iterable: ...
            let (k, v): (PyObject, PyObject) = pair.extract()?;
            map.insert(k, v);
        }
    }
    Ok(())
}

#[pymodule]
pub fn register_dict(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict>()?;
    // m.add_class::<Dict2>()?;
    Ok(())
}
