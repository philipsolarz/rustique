use indexmap::IndexMap;
use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::types::{PyBytes, PyDict, PyIterator, PyString, PyTuple, PyType};
use pyo3::{prelude::*, types::PyList};
use pyo3::{PyClass, PyObject};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

pub trait PySized {
    fn __len__(&self) -> PyResult<usize>;
}

pub trait PyContainer {
    fn __contains__(&self, item: PyObject) -> PyResult<bool>;
}

pub trait PyIterable {
    fn __iter__(&self) -> PyResult<PyObject>;
}

pub trait PyMappingView: PySized {}
pub trait PyCollection: PySized + PyContainer + PyIterable {}

pub trait PyMapping: PyCollection {
    fn __getitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<PyObject>;

    fn get(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<PyObject>,
    ) -> PyResult<PyObject> {
        match self.__getitem__(py, key) {
            Ok(val) => Ok(val),
            Err(err) => {
                if err.is_instance_of::<PyKeyError>(py) {
                    match default {
                        Some(obj) => Ok(obj),
                        None => Ok(py.None()),
                    }
                } else {
                    Err(err)
                }
            }
        }
    }

    fn __contains__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        match self.__getitem__(py, key) {
            Ok(_value) => Ok(true),
            Err(err) => {
                if err.is_instance_of::<PyKeyError>(py) {
                    Ok(false)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn keys(&self, py: Python<'_>) -> PyResult<KeysView>;

    fn values(&self, py: Python<'_>) -> PyResult<ValuesView>;

    fn items(&self, py: Python<'_>) -> PyResult<ItemsView>;

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;
}

pub trait PySet: PyCollection {
    fn __le__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    fn __lt__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    fn __ge__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    fn __gt__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    fn __ne__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    fn _from_iterable(py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized;

    fn __and__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized;

    fn __rand__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        self.__and__(py, other)
    }

    fn isdisjoint(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool>;

    // fn __or__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized;

    // fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     self.__or__(py, other)
    // }

    fn __sub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized;

    // fn __rsub__(&self, py: Python<'_>, other: &PyAny) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     let other_as_self = Self::_from_iterable(py, other)?;
    //     other_as_self.__sub__(py, &PyAny::from(self))
    // }

    // fn __xor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized;

    // fn __rxor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     self.__xor__(py, other)
    // }

    // fn _hash(&self, py: Python<'_>) -> PyResult<u64>;
}

#[derive(Debug, Clone)]
pub struct Key {
    key: PyObject,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        Python::with_gil(|py| {
            let result = self.key.call_method1(py, "__eq__", (&other.key,))?;
            result.extract::<bool>(py)
        })
        .unwrap_or(false)
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Python::with_gil(|py| match self.key.call_method0(py, "__hash__") {
            Ok(hash) => {
                let hash = hash.extract::<isize>(py).unwrap();
                hash.hash(state);
            }
            Err(err) => {
                eprintln!("Failed to compute hash for Key: {:?}", err);
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct Value {
    value: PyObject,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        Python::with_gil(|py| {
            let result = self.value.call_method1(py, "__eq__", (&other.value,))?;
            result.extract::<bool>(py)
        })
        .unwrap_or(false)
    }
}

impl Eq for Value {}
pub trait PyKeysView: PyMappingView + PySet {}

#[derive(Clone)]
#[pyclass]
pub struct KeysView {
    keys: Vec<Key>,
}

impl PySized for KeysView {
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.keys.len())
    }
}

impl PyContainer for KeysView {
    fn __contains__(&self, item: PyObject) -> PyResult<bool> {
        let candidate = Key { key: item };
        Ok(self.keys.contains(&candidate))
    }
}

impl PyIterable for KeysView {
    fn __iter__(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let keys: Vec<PyObject> = self.keys.iter().map(|key| key.key.clone_ref(py)).collect();
            Ok(PyList::new(py, &keys)?.to_object(py))
        })
    }
}

impl PyMappingView for KeysView {}

impl PyCollection for KeysView {}

// impl<'py> IntoPy<PyObject> for KeysView {
//     fn into_py(self, py: Python<'py>) -> PyObject {
//         Py::new(py, self).unwrap().to_object(py)
//     }
// }

impl PySet for KeysView {
    fn __le__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<KeysView>() {
            for k in &self.keys {
                if !other_view.keys.contains(k) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    fn __lt__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let le = self.__le__(py, other)?;
        if !le {
            return Ok(false);
        }
        let self_len = self.__len__()?;
        if let Ok(other_view) = other.extract::<KeysView>() {
            let other_len = other_view.__len__()?;
            Ok(self_len < other_len)
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    fn __ge__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<KeysView>() {
            for k in &other_view.keys {
                if !self.keys.contains(k) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    fn __gt__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let ge = self.__ge__(py, other)?;
        if !ge {
            return Ok(false);
        }
        let self_len = self.__len__()?;
        if let Ok(other_view) = other.extract::<KeysView>() {
            let other_len = other_view.__len__()?;
            Ok(self_len > other_len)
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<KeysView>() {
            if self.keys.len() != other_view.keys.len() {
                return Ok(false);
            }

            for k in &self.keys {
                if !other_view.keys.contains(k) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    fn __ne__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let eq = self.__eq__(py, other)?;
        Ok(!eq)
    }

    fn _from_iterable(py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        let iter = iterable.try_iter()?;
        let mut keys = Vec::new();
        for obj in iter {
            let obj = obj?;
            keys.push(Key {
                key: obj.to_object(py),
            });
        }
        Ok(KeysView { keys })
    }

    fn __and__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        let mut new_keys = Vec::new();
        if let Ok(other_view) = other.extract::<KeysView>() {
            for k in &self.keys {
                if other_view.keys.contains(k) {
                    new_keys.push(k.clone());
                }
            }
            Ok(KeysView { keys: new_keys })
        } else {
            return Err(PyTypeError::new_err("Expected a KeysView"));
        }
    }

    fn __rand__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        self.__and__(py, other)
    }

    fn isdisjoint(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<KeysView>() {
            for k in &self.keys {
                if other_view.keys.contains(k) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    // fn __or__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     if let Ok(other_view) = other.extract::<KeysView>() {
    //         let mut set = HashSet::new();
    //         set.extend(self.keys.iter().cloned());
    //         set.extend(other_view.keys.iter().cloned());
    //         let new_keys = set.into_iter().collect();
    //         Ok(KeysView { keys: new_keys })
    //     } else {
    //         Err(PyTypeError::new_err("Expected a KeysView"))
    //     }
    // }

    // fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     self.__or__(py, other)
    // }

    fn __sub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        if let Ok(other_view) = other.extract::<KeysView>() {
            let mut new_keys = Vec::new();
            for k in &self.keys {
                if !other_view.keys.contains(k) {
                    new_keys.push(k.clone());
                }
            }
            Ok(KeysView { keys: new_keys })
        } else {
            Err(PyTypeError::new_err("Expected a KeysView"))
        }
    }

    // fn __xor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     let left_minus_right = self.__sub__(py, other)?;
    //     // let right_minus_left = self.__rsub__(py, other)?;
    //     let right_minus_left = if let Ok(other_view) = other.extract::<KeysView>() {
    //         other_view.__sub__(py, &PyAny::from(self))?
    //     } else {
    //         return Err(PyTypeError::new_err("Expected a KeysView"));
    //     };
    //     left_minus_right.__or__(py, &right_minus_left.into_py(py).into_ref(py))
    // }

    // fn _hash(&self, py: Python<'_>) -> PyResult<u64> {
    //     let mut hasher = std::collections::hash_map::DefaultHasher::new();
    //     for k in &self.keys {
    //         k.hash(&mut hasher);
    //     }
    //     Ok(hasher.finish())
    // }
}
impl PyKeysView for KeysView {}
pub trait PyItemsView: PyMappingView + PySet {}

#[derive(Clone)]
#[pyclass]
pub struct ItemsView {
    items: Vec<(Key, Value)>,
}

impl PySized for ItemsView {
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.items.len())
    }
}

impl PyContainer for ItemsView {
    fn __contains__(&self, item: PyObject) -> PyResult<bool> {
        let candidate = Value { value: item };
        Ok(self.items.iter().any(|(_, value)| value == &candidate))
    }
}

impl PyIterable for ItemsView {
    fn __iter__(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let items: Vec<PyObject> = self
                .items
                .iter()
                .map(|(key, value)| {
                    let key = key.key.clone_ref(py);
                    let value = value.value.clone_ref(py);
                    PyTuple::new(py, &[key, value]).unwrap().to_object(py)
                })
                .collect();
            Ok(PyList::new(py, &items)?.to_object(py))
        })
    }
}

impl PyMappingView for ItemsView {}

impl PyCollection for ItemsView {}

// impl<'py> IntoPy<PyObject> for ItemsView {
//     fn into_py(self, py: Python<'py>) -> PyObject {
//         Py::new(py, self).unwrap().to_object(py)
//     }
// }

impl PySet for ItemsView {
    fn __le__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<ItemsView>() {
            for (key, value) in &self.items {
                if !other_view.items.contains(&(key.clone(), value.clone())) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    fn __lt__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let le = self.__le__(py, other)?;
        if !le {
            return Ok(false);
        }
        let self_len = self.__len__()?;
        if let Ok(other_view) = other.extract::<ItemsView>() {
            let other_len = other_view.__len__()?;
            Ok(self_len < other_len)
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    fn __ge__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<ItemsView>() {
            for (key, value) in &other_view.items {
                if !self.items.contains(&(key.clone(), value.clone())) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    fn __gt__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let ge = self.__ge__(py, other)?;
        if !ge {
            return Ok(false);
        }
        let self_len = self.__len__()?;
        if let Ok(other_view) = other.extract::<ItemsView>() {
            let other_len = other_view.__len__()?;
            Ok(self_len > other_len)
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<ItemsView>() {
            if self.items.len() != other_view.items.len() {
                return Ok(false);
            }

            for (key, value) in &self.items {
                if !other_view.items.contains(&(key.clone(), value.clone())) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    fn __ne__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let eq = self.__eq__(py, other)?;
        Ok(!eq)
    }

    fn _from_iterable(py: Python<'_>, iterable: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        let iter = iterable.try_iter()?;
        let mut items = Vec::new();
        for obj in iter {
            let obj = obj?;
            let key = obj.get_item(0)?;
            let value = obj.get_item(1)?;
            items.push((
                Key {
                    key: key.to_object(py),
                },
                Value {
                    value: value.to_object(py),
                },
            ));
        }
        Ok(ItemsView { items })
    }

    fn __and__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        let mut new_items = Vec::new();
        if let Ok(other_view) = other.extract::<ItemsView>() {
            for (key, value) in &self.items {
                if other_view.items.contains(&(key.clone(), value.clone())) {
                    new_items.push((key.clone(), value.clone()));
                }
            }
            Ok(ItemsView { items: new_items })
        } else {
            return Err(PyTypeError::new_err("Expected an ItemsView"));
        }
    }

    fn __rand__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        self.__and__(py, other)
    }

    fn isdisjoint(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_view) = other.extract::<ItemsView>() {
            for (key, value) in &self.items {
                if other_view.items.contains(&(key.clone(), value.clone())) {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    // fn __or__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     if let Ok(other_view) = other.extract::<ItemsView>() {
    //         let mut set = HashSet::new();
    //         set.extend(self.items.iter().cloned());
    //         set.extend(other_view.items.iter().cloned());
    //         let new_items = set.into_iter().collect();
    //         Ok(ItemsView { items: new_items })
    //     } else {
    //         Err(PyTypeError::new_err("Expected an ItemsView"))
    //     }
    // }

    // fn __ror__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     self.__or__(py, other)
    // }

    fn __sub__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    where
        Self: Sized,
    {
        if let Ok(other_view) = other.extract::<ItemsView>() {
            let mut new_items = Vec::new();
            for (key, value) in &self.items {
                if !other_view.items.contains(&(key.clone(), value.clone())) {
                    new_items.push((key.clone(), value.clone()));
                }
            }
            Ok(ItemsView { items: new_items })
        } else {
            Err(PyTypeError::new_err("Expected an ItemsView"))
        }
    }

    // fn __xor__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Self>
    // where
    //     Self: Sized,
    // {
    //     let left_minus_right = self.__sub__(py, other)?;
    //     // let right_minus_left = self.__rsub__(py, other)?;
    //     let right_minus_left = if let Ok(other_view) = other.extract::<ItemsView>() {
    //         other_view.__sub__(py, &PyAny::from(self))?
    //     } else {
    //         return Err(PyTypeError::new_err("Expected an ItemsView"));
    //     };
    //     left_minus_right.__or__(py, &right_minus_left.into_py(py).into_ref(py))
    // }

    // fn _hash(&self, py: Python<'_>) -> PyResult<u64> {
    //     let mut hasher = std::collections::hash_map::DefaultHasher::new();
    //     for (key, value) in &self.items {
    //         key.hash(&mut hasher);
    //         value.hash(&mut hasher);
    //     }
    //     Ok(hasher.finish())
    // }
}

pub trait PyValuesView: PyMappingView + PyCollection {}

#[derive(Clone)]
#[pyclass]
pub struct ValuesView {
    values: Vec<Value>,
}

impl PySized for ValuesView {
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.values.len())
    }
}

impl PyContainer for ValuesView {
    fn __contains__(&self, item: PyObject) -> PyResult<bool> {
        let candidate = Value { value: item };
        Ok(self.values.contains(&candidate))
    }
}

impl PyIterable for ValuesView {
    fn __iter__(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let values: Vec<PyObject> = self
                .values
                .iter()
                .map(|value| value.value.clone_ref(py))
                .collect();
            Ok(PyList::new(py, &values)?.to_object(py))
        })
    }
}

impl PyMappingView for ValuesView {}

impl PyCollection for ValuesView {}

// impl<'py> IntoPy<PyObject> for ValuesView {
//     fn into_py(self, py: Python<'py>) -> PyObject {
//         Py::new(py, self).unwrap().to_object(py)
//     }
// }

pub trait PyMutableMapping: PyMapping {
    fn __setitem__(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()>;

    fn __delitem__(&mut self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<()>;

    fn clear(&mut self, py: Python<'_>) -> PyResult<()>;

    fn pop(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<PyObject>,
    ) -> PyResult<PyObject>;

    fn popitem(&mut self, py: Python<'_>) -> PyResult<(PyObject, PyObject)>;

    fn setdefault(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<PyObject>,
    ) -> PyResult<PyObject>;

    fn update(&mut self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()>;
}

pub trait _PyDict: PyMutableMapping {}

#[derive(Debug, Clone)]
#[pyclass(subclass)]
pub struct Dict {
    dict: IndexMap<Key, Value>,
}

impl PySized for Dict {
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.dict.len())
    }
}
impl PyContainer for Dict {
    fn __contains__(&self, item: PyObject) -> PyResult<bool> {
        let candidate = Key { key: item };
        Ok(self.dict.contains_key(&candidate))
    }
}

impl PyIterable for Dict {
    fn __iter__(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let keys: Vec<PyObject> = self
                .dict
                .iter()
                .map(|(key, _)| key.key.clone_ref(py))
                .collect();
            Ok(PyList::new(py, &keys)?.to_object(py))
        })
    }
}

impl PyCollection for Dict {}
impl PyMapping for Dict {
    fn __getitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let k = key.to_object(py);
        match self.dict.get(&Key { key: k }) {
            Some(value) => Ok(value.value.clone()),
            None => Err(PyKeyError::new_err("Key not found")),
        }
    }

    fn __contains__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        let k = key.to_object(py);
        Ok(self.dict.contains_key(&Key { key: k }))
    }

    fn get(
        &self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<PyObject>,
    ) -> PyResult<PyObject> {
        let k = key.to_object(py);
        match self.dict.get(&Key { key: k }) {
            Some(value) => Ok(value.value.clone()),
            None => match default {
                Some(obj) => Ok(obj),
                None => Ok(py.None()),
            },
        }
    }
    fn keys(&self, py: Python<'_>) -> PyResult<KeysView> {
        let keys = self
            .dict
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<Key>>();
        Ok(KeysView { keys })
    }

    fn items(&self, py: Python<'_>) -> PyResult<ItemsView> {
        let items = self
            .dict
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<(Key, Value)>>();
        Ok(ItemsView { items })
    }

    fn values(&self, py: Python<'_>) -> PyResult<ValuesView> {
        let values = self
            .dict
            .iter()
            .map(|(_, value)| value.clone())
            .collect::<Vec<Value>>();
        Ok(ValuesView { values })
    }
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_dict) = other.extract::<PyRef<Dict>>() {
            if self.dict.len() != other_dict.dict.len() {
                return Ok(false);
            }
            for (k, v) in &self.dict {
                match other_dict.dict.get(k) {
                    Some(other_v) => {
                        if other_v != v {
                            return Ok(false);
                        }
                    }
                    None => return Ok(false),
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl PyMutableMapping for Dict {
    fn __setitem__(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let k = key.to_object(py);
        let v = value.to_object(py);
        self.dict.insert(Key { key: k }, Value { value: v });
        Ok(())
    }

    fn __delitem__(&mut self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<()> {
        let k = key.to_object(py);
        self.dict.swap_remove(&Key { key: k });
        Ok(())
    }
    fn clear(&mut self, _py: Python<'_>) -> PyResult<()> {
        self.dict.clear();
        Ok(())
    }

    fn pop(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<PyObject>,
    ) -> PyResult<PyObject> {
        let k = key.to_object(py);
        match self.dict.swap_remove(&Key { key: k }) {
            Some(value) => Ok(value.value),
            None => match default {
                Some(obj) => Ok(obj),
                None => Err(PyKeyError::new_err("Key not found")),
            },
        }
    }
    fn popitem(&mut self, _py: Python<'_>) -> PyResult<(PyObject, PyObject)> {
        match self.dict.pop() {
            Some((k, v)) => Ok((k.key, v.value)),
            None => Err(PyKeyError::new_err("Dict is empty")),
        }
    }

    fn setdefault(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        default: Option<PyObject>,
    ) -> PyResult<PyObject> {
        let k = key.to_object(py);
        match self.dict.get(&Key {
            key: k.clone_ref(py),
        }) {
            Some(value) => Ok(value.value.clone()),
            None => {
                let default = default.unwrap_or_else(|| Python::with_gil(|py| py.None().into()));
                self.dict.insert(
                    Key { key: k },
                    Value {
                        value: default.clone(),
                    },
                );
                Ok(default)
            }
        }
    }

    fn update(&mut self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Ok(other_dict) = other.extract::<PyRef<Dict>>() {
            for (k, v) in &other_dict.dict {
                self.dict.insert(k.clone(), v.clone());
            }
            Ok(())
        } else {
            Err(PyTypeError::new_err("Expected a Dict"))
        }
    }
}
impl _PyDict for Dict {}

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
                    dict.insert(
                        Key { key: key.into() },
                        Value {
                            value: value.into(),
                        },
                    );
                }
            } else if let Ok(iterable) = arg.try_iter() {
                for item in iterable {
                    let item = item?;

                    if let Ok(tuple) = item.downcast::<PyTuple>() {
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
                            return Err(PyValueError::new_err(
                                "Dict must be initialized with a sequence of 2-tuples",
                            ));
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
                dict.insert(
                    Key { key: key.into() },
                    Value {
                        value: value.clone(),
                    },
                );
            }
        } else {
            return Err(PyValueError::new_err("Expected an iterable"));
        }

        Ok(Dict { dict })
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
                        .key
                        .call_method0(py, "__repr__")?
                        .extract::<String>(py)?;
                    let value_repr = value
                        .value
                        .call_method0(py, "__repr__")?
                        .extract::<String>(py)?;
                    Ok(format!("{}: {}", key_repr, value_repr))
                })
                .collect();

            entries.map(|entries| format!("{{{}}}", entries.join(", ")))
        })
    }

    pub fn __len__(&self) -> PyResult<usize> {
        Ok(self.dict.len())
    }

    pub fn keys(&self) -> PyResult<KeysView> {
        Python::with_gil(|py| PyMapping::keys(self, py))
    }

    pub fn values(&self) -> PyResult<ValuesView> {
        Python::with_gil(|py| PyMapping::values(self, py))
    }

    pub fn items(&self) -> PyResult<ItemsView> {
        Python::with_gil(|py| PyMapping::items(self, py))
    }

    pub fn __getitem__(&self, key: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        Python::with_gil(|py| PyMapping::__getitem__(self, py, key))
    }

    pub fn __contains__(&self, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        Python::with_gil(|py| PyMapping::__contains__(self, py, key))
    }

    pub fn get(&self, key: &Bound<'_, PyAny>, default: Option<PyObject>) -> PyResult<PyObject> {
        Python::with_gil(|py| PyMapping::get(self, py, key, default))
    }

    // pub fn __setitem__(&mut self, key: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) {
    //     Python::with_gil(|py| PyMutableMapping::__setitem__(self, py, key, value))
    // }

    // pub fn __delitem__(&mut self, key: PyObject) {
    //     PyMutableMapping::__delitem__(self, key)
    // }

    // pub fn clear(&mut self) {
    //     PyMutableMapping::clear(self)
    // }

    // pub fn pop(&mut self, key: PyObject, default: Option<PyObject>) -> Option<PyObject> {
    //     PyMutableMapping::pop(self, key, default)
    // }

    // pub fn popitem(&mut self) -> Option<(PyObject, PyObject)> {
    //     PyMutableMapping::popitem(self)
    // }

    // pub fn setdefault(&mut self, key: PyObject, default: Option<PyObject>) -> PyObject {
    //     PyMutableMapping::setdefault(self, key, default)
    // }

    // pub fn update(&mut self, other: PyObject) {
    //     PyMutableMapping::update(self, other)
    // }

    // pub fn __eq__(&self, other: PyObject) -> bool {
    //     PyMapping::__eq__(self, &other)
    // }
}

#[pymodule]
pub fn register_collections(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dict>()?;
    m.add_class::<KeysView>()?;
    m.add_class::<ValuesView>()?;
    m.add_class::<ItemsView>()?;
    Ok(())
}
