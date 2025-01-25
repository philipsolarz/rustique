use std::os::raw::c_char;

use pyo3::{
    ffi,
    prelude::*,
    types::{PyDict, PyIterator, PyList, PySequence, PyTuple},
};

#[pyclass]
struct FilterIterator {
    func: Option<PyObject>,
    iter: PyObject,
}

#[pymethods]
impl FilterIterator {
    #[new]
    #[pyo3(signature = (iter, func=None))]
    fn new(iter: PyObject, func: Option<PyObject>) -> Self {
        FilterIterator { func, iter }
    }

    fn __iter__(slf: PyRef<Self>) -> PyResult<Py<FilterIterator>> {
        Ok(slf.into())
    }

    fn __next__(slf: PyRefMut<Self>) -> PyResult<Option<PyObject>> {
        let py = slf.py();

        loop {
            let item_ptr = unsafe { ffi::PyIter_Next(slf.iter.as_ptr()) };
            if item_ptr.is_null() {
                if PyErr::occurred(py) {
                    return Err(PyErr::fetch(py));
                } else {
                    return Ok(None);
                }
            }

            let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };

            match &slf.func {
                Some(func_obj) => {
                    let result = func_obj.call1(py, (item.clone(),))?;
                    if result.is_truthy(py)? {
                        return Ok(Some(item));
                    }
                }
                None => {
                    if item.is_truthy(py)? {
                        return Ok(Some(item));
                    }
                }
            }
        }
    }
}

#[pyclass]
struct MapIterator {
    func: PyObject,
    iterables: Vec<PyObject>,
}

#[pymethods]
impl MapIterator {
    #[new]
    fn new(func: PyObject, iterables: Vec<PyObject>) -> Self {
        MapIterator { func, iterables }
    }

    fn __iter__(slf: PyRef<Self>) -> PyResult<Py<MapIterator>> {
        Ok(slf.into())
    }

    fn __next__(slf: PyRefMut<Self>) -> PyResult<Option<PyObject>> {
        let py = slf.py();

        let mut next_items = Vec::new();
        for iterable in &slf.iterables {
            let item_ptr = unsafe { ffi::PyIter_Next(iterable.as_ptr()) };
            if item_ptr.is_null() {
                if PyErr::occurred(py) {
                    return Err(PyErr::fetch(py));
                } else {
                    return Ok(None);
                }
            }
            let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };
            next_items.push(item);
        }

        let args = PyTuple::new(py, next_items)?;
        let result = slf.func.call1(py, args)?;

        Ok(Some(result))
    }
}

#[pyclass]
struct ListIterator {
    iter: PyObject,
}

#[pymethods]
impl ListIterator {
    #[new]
    fn new(iter: PyObject) -> Self {
        ListIterator { iter }
    }

    fn __iter__(slf: PyRef<Self>) -> PyResult<Py<ListIterator>> {
        Ok(slf.into())
    }

    fn __next__(slf: PyRefMut<Self>) -> PyResult<Option<PyObject>> {
        let py = slf.py();

        // Get the next item from the iterator
        let item_ptr = unsafe { ffi::PyIter_Next(slf.iter.as_ptr()) };
        if item_ptr.is_null() {
            if PyErr::occurred(py) {
                return Err(PyErr::fetch(py));
            } else {
                return Ok(None); // Stop iteration when the iterable is exhausted
            }
        }

        let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };
        Ok(Some(item))
    }
}

#[pyclass(name = "Rustique")]
#[derive(Clone)]
pub struct Engine {
    original_builtins: Option<Py<PyDict>>,
}

#[pymethods]
impl Engine {
    #[new]
    pub fn new() -> Self {
        Engine {
            original_builtins: None,
        }
    }

    #[pyo3(signature = (iterable, func=None))]
    pub fn filter(
        &self,
        py: Python,
        iterable: &Bound<'_, PyAny>,
        func: Option<PyObject>,
    ) -> PyResult<PyObject> {
        let iter = PyIterator::from_object(iterable)?.into();
        let filter_iter = Py::new(py, FilterIterator { func, iter })?;
        Ok(filter_iter.to_object(py))
        // let iter = iterable.call_method0(py, "__iter__")?;
        // let filter_iter = Py::new(py, FilterIterator { func, iter })?;
        // Ok(filter_iter.to_object(py))
        // Ok(())
    }

    #[pyo3(signature = (func, iterable, *iterables))]
    pub fn map(
        &self,
        py: Python,
        func: PyObject,
        iterable: &Bound<'_, PyAny>,
        iterables: &Bound<'_, PyTuple>,
    ) -> PyResult<PyObject> {
        let mut all_iterables = Vec::new();

        let iter = PyIterator::from_object(iterable)?.into();
        all_iterables.push(iter);

        for iterable in iterables.iter() {
            let iter = PyIterator::from_object(&iterable)?.into();
            all_iterables.push(iter);
        }

        let map_iter = Py::new(py, MapIterator::new(func, all_iterables))?;
        Ok(map_iter.to_object(py))
    }

    #[pyo3(signature = (iterable))]
    pub fn list(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        // Convert input to an iterator
        let iter = PyIterator::from_object(iterable)?.into();

        // Create ListIterator and return as an iterator object
        let list_iter = Py::new(py, ListIterator { iter })?;
        Ok(list_iter.to_object(py))
    }

    // #[pyo3(signature = (iterable))]
    // pub fn tuple(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert input to an iterator
    //     let iter = PyIterator::from_object(iterable)?.into();

    //     // Create ListIterator and return as an iterator object
    //     let list_iter = Py::new(py, ListIterator { iter })?;
    //     Ok(list_iter.to_object(py))
    // }

    // #[pyo3(signature = (iterable))]
    // pub fn set(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert input to an iterator
    //     let iter = PyIterator::from_object(iterable)?.into();

    //     // Create ListIterator and return as an iterator object
    //     let list_iter = Py::new(py, ListIterator { iter })?;
    //     Ok(list_iter.to_object(py))
    // }

    // #[pyo3(signature = (iterable))]
    // pub fn enumerate(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert input to an iterator
    //     let iter = PyIterator::from_object(iterable)?.into();

    //     // Create ListIterator and return as an iterator object
    //     let list_iter = Py::new(py, ListIterator { iter })?;
    //     Ok(list_iter.to_object(py))
    // }

    // #[pyo3(signature = (iterable))]
    // pub fn all(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert input to an iterator
    //     let iter = PyIterator::from_object(iterable)?.into();

    //     // Create ListIterator and return as an iterator object
    //     let list_iter = Py::new(py, ListIterator { iter })?;
    //     Ok(list_iter.to_object(py))
    // }

    // #[pyo3(signature = (iterable))]
    // pub fn any(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert input to an iterator
    //     let iter = PyIterator::from_object(iterable)?.into();

    //     // Create ListIterator and return as an iterator object
    //     let list_iter = Py::new(py, ListIterator { iter })?;
    //     Ok(list_iter.to_object(py))
    // }

    // #[pyo3(signature = (iterable, func=None))]

    // #[pyo3(signature = (iterable))]
    // pub fn list(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert the input to an iterator
    //     let iterator = PyIterator::from_object(iterable)?;

    //     // Create a new empty list
    //     let py_list = PyList::empty(py);

    //     // Iterate over the elements and append to the list
    //     for item in iterator {
    //         let element = item?;
    //         py_list.append(element)?;
    //     }

    //     // Return the constructed list
    //     Ok(py_list.to_object(py))
    // }

    pub fn __enter__(&mut self, py: Python) -> PyResult<Self> {
        let builtins = py.import("builtins")?;

        let original_builtins = PyDict::new(py);
        for key in builtins.dict().keys() {
            let key_str: String = key.extract()?;
            if !key_str.starts_with("__") {
                original_builtins.set_item(&key_str, builtins.getattr(&key_str)?)?;
            }
        }

        let proxy_module = create_rustique_proxy_from_dict(py, &original_builtins)?;
        builtins.setattr("py", proxy_module)?;

        // We need a generic way to do this
        builtins.setattr("filter", Py::new(py, self.clone())?.getattr(py, "filter")?)?;
        builtins.setattr("map", Py::new(py, self.clone())?.getattr(py, "map")?)?;
        builtins.setattr("list", Py::new(py, self.clone())?.getattr(py, "list")?)?;

        self.original_builtins = Some(original_builtins.into());

        Ok(Self {
            original_builtins: self.original_builtins.clone(),
        })
    }

    pub fn __exit__(
        &mut self,
        py: Python,
        _exc_type: PyObject,
        _exc_value: PyObject,
        _traceback: PyObject,
    ) -> PyResult<bool> {
        if let Some(original_builtins) = &self.original_builtins {
            let original_builtins = original_builtins.bind(py);
            let builtins = py.import("builtins")?;

            for key in original_builtins.keys() {
                let key_str: String = key.extract()?;
                builtins.setattr(&key_str, original_builtins.get_item(&key_str)?)?;
            }

            builtins.delattr("py")?;
        }

        Ok(true)
    }
}

fn create_rustique_proxy_from_dict(
    py: Python,
    original_builtins: &Bound<'_, PyDict>,
) -> PyResult<Py<PyModule>> {
    let proxy_module = PyModule::new(py, "py_proxy")?;
    for key in original_builtins.keys() {
        let key_str: String = key.extract()?;
        let original_obj = original_builtins.get_item(&key_str)?;
        proxy_module.add(&key_str, original_obj)?;
    }
    Ok(proxy_module.into())
}

#[pymodule]
pub fn register_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    Ok(())
}
