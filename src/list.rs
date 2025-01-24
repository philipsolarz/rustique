use pyo3::{
    exceptions::{PyIndexError, PyTypeError, PyValueError},
    ffi,
    prelude::*,
    types::{PyDict, PyInt, PyList, PySlice, PyTuple},
    IntoPyObjectExt,
};
use std::os::raw::c_char;

type Operation = Box<dyn Fn(Vec<PyObject>, Python) -> PyResult<Vec<PyObject>>>;
// type Operation = Box<dyn Fn(Vec<PyObject>, Python) -> PyResult<Vec<PyObject>> + Send>;
#[pyclass(subclass, unsendable)]
pub struct List {
    pub list: Vec<PyObject>,
    operations: Vec<Operation>,
}

#[pymethods]
impl List {
    #[new]
    #[pyo3(signature = (*args, **kwargs))]
    pub fn new(args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Self> {
        if let Some(kwargs) = kwargs {
            if !kwargs.is_empty() {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "list() takes no keyword arguments",
                ));
            }
        }
        if args.len() > 1 {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "list() takes at most 1 argument",
            ));
        }

        let list = if args.is_empty() {
            Vec::new()
        } else {
            let iterable = args.get_item(0)?;
            let py = iterable.py();

            unsafe {
                if ffi::PyList_CheckExact(iterable.as_ptr()) != 0 {
                    let list_ptr = iterable.as_ptr() as *mut ffi::PyListObject;
                    let length = ffi::PyList_Size(iterable.as_ptr());
                    if length < 0 {
                        return Err(PyErr::fetch(py));
                    }

                    let len_usize = length as usize;
                    let mut result = Vec::with_capacity(len_usize);

                    let items_ptr = (*list_ptr).ob_item;
                    let items_slice: &[*mut ffi::PyObject] =
                        std::slice::from_raw_parts(items_ptr, len_usize);
                    result.extend(items_slice.iter().map(|&item_ptr| {
                        ffi::Py_INCREF(item_ptr);
                        PyObject::from_owned_ptr(py, item_ptr)
                    }));

                    result
                } else if ffi::PyTuple_CheckExact(iterable.as_ptr()) != 0 {
                    let tuple_ptr = iterable.as_ptr() as *mut ffi::PyTupleObject;
                    let length = ffi::PyTuple_Size(iterable.as_ptr());
                    if length < 0 {
                        return Err(PyErr::fetch(py));
                    }

                    let len_usize = length as usize;
                    let mut result = Vec::with_capacity(len_usize);

                    let items_ptr = (*tuple_ptr).ob_item.as_ptr();
                    let items_slice: &[*mut ffi::PyObject] =
                        std::slice::from_raw_parts(items_ptr, len_usize);
                    result.extend(items_slice.iter().map(|&item_ptr| {
                        ffi::Py_INCREF(item_ptr);
                        PyObject::from_owned_ptr(py, item_ptr)
                    }));

                    result
                } else {
                    let error_msg: *const c_char = b"expected iterable\0".as_ptr() as *const c_char;
                    let seq_ptr = ffi::PySequence_Fast(iterable.as_ptr(), error_msg);
                    if seq_ptr.is_null() {
                        return Err(PyErr::fetch(py));
                    }

                    let len = ffi::PySequence_Size(seq_ptr);
                    if len < 0 {
                        ffi::Py_DECREF(seq_ptr);
                        return Err(PyErr::fetch(py));
                    }

                    let len_usize = len as usize;
                    let mut result = Vec::with_capacity(len_usize);

                    result.extend(
                        (0..len_usize)
                            .map(|i| {
                                let item_ptr =
                                    ffi::PySequence_GetItem(seq_ptr, i as ffi::Py_ssize_t);
                                if item_ptr.is_null() {
                                    Err(PyErr::fetch(py))
                                } else {
                                    Ok(PyObject::from_owned_ptr(py, item_ptr))
                                }
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    );

                    ffi::Py_DECREF(seq_ptr);
                    result
                }
            }
        };

        Ok(List {
            list,
            operations: Vec::new(),
        })
    }

    // ------------------------------------------------------------------------
    // Sequence protocol: __len__
    // ------------------------------------------------------------------------
    fn __len__(&self) -> usize {
        self.list.len()
    }

    // ------------------------------------------------------------------------
    // Sequence protocol: __getitem__
    //
    // ------------------------------------------------------------------------
    fn __getitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        if let Ok(i) = index.downcast::<PyInt>() {
            let i = i.extract::<isize>()?;
            let idx = self.to_valid_index(i)?;
            Ok(self.list[idx].clone())
        } else if let Ok(slice) = index.downcast::<PySlice>() {
            let slice_info = slice.indices(self.list.len() as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;
            let slice_len = slice_info.slicelength;

            if step == 0 {
                return Err(PyErr::new::<PyValueError, _>("slice step cannot be zero"));
            }

            if step == 1 {
                let range_start = start.max(0) as usize;
                let range_stop = stop.max(0) as usize;
                let slice_vec = self.list[range_start..range_stop].to_vec();
                let new_list = List {
                    list: slice_vec,
                    operations: Vec::new(),
                };
                return Ok(new_list.into_py_any(index.py())?);
            }

            let mut sliced = Vec::with_capacity(slice_len as usize);
            let mut i = start;
            if step > 1 {
                while i < stop {
                    sliced.push(self.list[i as usize].clone());
                    i += step;
                }
            } else {
                while i > stop {
                    sliced.push(self.list[i as usize].clone());
                    i += step;
                }
            }

            let new_list = List {
                list: sliced,
                operations: Vec::new(),
            };
            return Ok(new_list.into_py_any(index.py())?);
        } else {
            Err(PyErr::new::<PyTypeError, _>(
                "list indices must be integers or slices",
            ))
        }
    }

    // ------------------------------------------------------------------------
    // Sequence protocol: __setitem__
    //
    // ------------------------------------------------------------------------
    fn __setitem__(&mut self, index: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Ok(i) = index.extract::<isize>() {
            let idx = self.to_valid_index(i)?;
            let val = value.clone().unbind();
            self.list[idx] = val;
            return Ok(());
        }

        if let Ok(slice) = index.downcast::<PySlice>() {
            let slice_info = slice.indices(self.list.len() as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;
            let slice_len = slice_info.slicelength;

            let mut replacement = Vec::new();
            if let Ok(iter) = value.try_iter() {
                for item_result in iter {
                    let item = item_result?.unbind();
                    replacement.push(item);
                }
            } else {
                return Err(PyErr::new::<PyTypeError, _>("can only assign an iterable"));
            }

            if step == 1 {
                let start_usize = start as usize;
                let stop_usize = stop as usize;
                self.list.drain(start_usize..stop_usize);
                let mut idx = start_usize;
                for val in replacement {
                    self.list.insert(idx, val);
                    idx += 1;
                }
            } else {
                if replacement.len() != slice_len as usize {
                    return Err(PyErr::new::<PyValueError, _>(
                        "attempt to assign sequence of size X to extended slice of size Y",
                    ));
                }
                let mut i = start;
                let mut rep_idx = 0;
                if step > 0 {
                    while i < stop {
                        self.list[i as usize] = replacement[rep_idx].clone();
                        i += step;
                        rep_idx += 1;
                    }
                } else {
                    while i > stop {
                        self.list[i as usize] = replacement[rep_idx].clone();
                        i += step;
                        rep_idx += 1;
                    }
                }
            }

            return Ok(());
        }

        Err(PyErr::new::<PyTypeError, _>(
            "list indices must be integers or slices",
        ))
    }
    // ------------------------------------------------------------------------
    // Sequence protocol: __delitem__
    //
    // ------------------------------------------------------------------------
    fn __delitem__(&mut self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Ok(i) = index.extract::<isize>() {
            let idx = self.to_valid_index(i)?;
            self.list.remove(idx);
            return Ok(());
        }

        if let Ok(slice) = index.downcast::<PySlice>() {
            let len = self.list.len();
            let slice_info = slice.indices(len as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;

            if step == 1 {
                self.list.drain(start as usize..stop as usize);
            } else if step == -1 && start < stop {
                // A small edge case: when step == -1 and (start < stop),
                //  Python effectively produces an empty range.
                //  So there's nothing to delete.
                // We do nothing
            } else {
                let mut new_list = Vec::with_capacity(len);
                let mut idx: isize = 0;

                if step > 0 {
                    let mut slice_indices = Vec::new();
                    let mut i = start;
                    while i < stop {
                        slice_indices.push(i);
                        i += step;
                    }
                    let slice_set: std::collections::HashSet<isize> =
                        slice_indices.into_iter().collect();
                    for _elem in &self.list {
                        if !slice_set.contains(&idx) {
                            new_list.push(self.list[idx as usize].clone());
                        }
                        idx += 1;
                    }
                } else {
                    let mut slice_indices = Vec::new();
                    let mut i = start;
                    while i > stop {
                        slice_indices.push(i);
                        i += step;
                    }
                    let slice_set: std::collections::HashSet<isize> =
                        slice_indices.into_iter().collect();
                    for _elem in &self.list {
                        if !slice_set.contains(&idx) {
                            new_list.push(self.list[idx as usize].clone());
                        }
                        idx += 1;
                    }
                }

                self.list = new_list;
            }

            return Ok(());
        }

        Err(PyErr::new::<PyTypeError, _>(
            "list indices must be integers or slices",
        ))
    }

    // ------------------------------------------------------------------------
    // list.append(x)
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, x, /)")]
    pub fn append(&mut self, x: &Bound<'_, PyAny>) -> PyResult<()> {
        // self.list.push(x.unbind());
        self.list.push(x.clone().unbind());
        Ok(())
    }

    // ------------------------------------------------------------------------
    // list.extend(iterable)
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, iterable, /)")]
    pub fn extend(&mut self, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        let iter = iterable.try_iter()?;
        for item_result in iter {
            self.list.push(item_result?.unbind());
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // list.insert(i, x)
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, i, x, /)")]
    pub fn insert(&mut self, i: isize, x: &Bound<'_, PyAny>) -> PyResult<()> {
        // let val = x.unbind();
        let val = x.clone().unbind();
        let mut idx = i;
        // In Python, negative indices are treated as offset from the end
        if idx < 0 {
            idx += self.list.len() as isize;
            if idx < 0 {
                idx = 0;
            }
        }
        if idx > self.list.len() as isize {
            idx = self.list.len() as isize;
        }
        self.list.insert(idx as usize, val);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // list.remove(value)
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, value, /)")]
    pub fn remove(&mut self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        for (i, elem) in self.list.iter().enumerate() {
            if elem.bind(py).eq(value)? {
                self.list.remove(i);
                return Ok(());
            }
        }

        Err(PyErr::new::<PyValueError, _>("value not in list"))
    }

    // ------------------------------------------------------------------------
    // list.pop([index])
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, index=None, /)")]
    #[pyo3(signature = (index=None))]
    pub fn pop(&mut self, index: Option<isize>) -> PyResult<PyObject> {
        if self.list.is_empty() {
            return Err(PyErr::new::<PyIndexError, _>("pop from empty list"));
        }
        match index {
            Some(i) => {
                let idx = self.to_valid_index(i)?;
                Ok(self.list.remove(idx))
            }
            None => {
                // No index means pop the last item
                Ok(self.list.pop().unwrap())
            }
        }
    }

    // ------------------------------------------------------------------------
    // list.clear()
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, /)")]
    pub fn clear(&mut self) {
        self.list.clear();
    }

    // ------------------------------------------------------------------------
    // list.index(value, [start, [stop]])
    //
    // For simplicity, we omit `start` and `stop` parameters here.
    // You can easily extend this to accept them if desired.
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, value, start=0, stop=None, /)")]
    pub fn index(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        start: Option<isize>,
        stop: Option<isize>,
    ) -> PyResult<usize> {
        let len = self.list.len() as isize;

        let start = start.unwrap_or(0);
        let start = if start < 0 { start + len } else { start }.max(0).min(len);

        let stop = stop.unwrap_or(len);
        let stop = if stop < 0 { stop + len } else { stop }.max(0).min(len);

        for i in start..stop {
            if self.list[i as usize].bind(py).eq(value)? {
                return Ok(i as usize);
            }
        }

        Err(PyErr::new::<PyValueError, _>("value not in list"))
    }

    // ------------------------------------------------------------------------
    // list.count(value)
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, value, /)")]
    pub fn count(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        let mut count = 0 as usize;
        for elem in &self.list {
            if elem.bind(py).eq(value)? {
                count += 1;
            }
        }
        Ok(count)
    }

    // ------------------------------------------------------------------------
    // list.reverse()
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, /)")]
    pub fn reverse(&mut self) {
        self.list.reverse();
    }

    // ------------------------------------------------------------------------
    // list.copy()
    //
    // Creates a shallow copy. Each PyObject is reference-counted, so this is
    // the same behavior as Python's shallow copy.
    // ------------------------------------------------------------------------
    #[pyo3(text_signature = "($self, /)")]
    pub fn copy(&self) -> Self {
        List {
            list: self.list.clone(),
            operations: Vec::new(),
        }
    }

    // ------------------------------------------------------------------------
    // __iter__: return an iterator
    // ------------------------------------------------------------------------

    // fn __iter__(slf: PyRef<'_, List>) -> PyResult<ListIterator> {
    //     Ok(ListIterator {
    //         list: slf.list.clone(),
    //         index: 0,
    //     })
    // }

    fn __iter__(slf: PyRef<'_, List>) -> PyResult<ListIterator> {
        Ok(ListIterator {
            iter: slf.list.clone().into_iter(),
        })
    }

    // fn __iter__(slf: PyRef<'_, List>) -> PyResult<ListIterator2> {
    //     Ok(ListIterator2 {
    //         index: 0,
    //         length: slf.list.len(),
    //         list: slf.list.clone(),
    //     })
    // }

    // fn __iter__(slf: PyRef<Self>) -> ListIterator {
    //     ListIterator {
    //         index: 0,
    //         list_ref: slf.into(),
    //     }
    // }

    fn __contains__(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        for elem in &self.list {
            if elem.bind(py).eq(value)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn __eq__(&self, py: Python, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(list) = other.downcast::<List>() {
            if self.list.len() != list.borrow().list.len() {
                return Ok(false);
            }

            for (a, b) in self.list.iter().zip(list.borrow().list.iter()) {
                if !a.bind(py).eq(b.bind(py))? {
                    return Ok(false);
                }
            }

            Ok(true)
        } else if let Ok(list) = other.downcast::<PyList>() {
            if self.list.len() != list.len() {
                return Ok(false);
            }

            for (a, b) in self.list.iter().zip(list.iter()) {
                if !a.bind(py).eq(b)? {
                    return Ok(false);
                }
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ------------------------------------------------------------------------
    // __mul__(self, n): repeat the list n times
    // ------------------------------------------------------------------------
    fn __mul__(&self, n: isize) -> Self {
        if n <= 0 {
            // Return empty
            List {
                list: Vec::new(),
                operations: Vec::new(),
            }
        } else {
            // Repeat the list
            let times = n as usize;
            let mut repeated = Vec::with_capacity(self.list.len() * times);
            for _ in 0..times {
                repeated.extend(self.list.iter().cloned());
            }
            List {
                list: repeated,
                operations: Vec::new(),
            }
        }
    }

    // ------------------------------------------------------------------------
    // __rmul__(self, n): same as __mul__ (for n * list)
    // ------------------------------------------------------------------------
    fn __rmul__(&self, n: isize) -> Self {
        self.__mul__(n)
    }

    /// For demonstration, a no-op “iter()” method that simply allows us
    /// to chain subsequent `map`, `filter`, etc. calls.
    ///
    /// In Python, you might call: `list.iter().map(...).filter(...).collect()`.
    #[pyo3(text_signature = "($self)")]
    fn iter<'py>(mut slf: PyRefMut<'py, Self>) -> PyResult<PyRefMut<'py, Self>> {
        slf.operations.push(Box::new(|items, _py| Ok(items)));
        Ok(slf)
    }

    /// Applies a Python function to every item in the list (deferred).
    ///
    /// In Python: `list.map(lambda x: x + 1)`
    #[pyo3(text_signature = "($self, func)")]
    fn map<'py>(mut slf: PyRefMut<'py, Self>, func: PyObject) -> PyResult<PyRefMut<'py, Self>> {
        slf.operations.push(Box::new(move |items, py| {
            items
                .into_iter()
                .map(|item| func.call1(py, (item,)).map_err(|e| e.into()))
                .collect::<PyResult<Vec<_>>>()
        }));
        Ok(slf)
    }

    /// Filters items based on a provided Python callable that returns True/False (deferred).
    ///
    /// In Python: `list.filter(lambda x: x % 2 == 0)`
    #[pyo3(text_signature = "($self, predicate)")]
    fn filter<'py>(
        mut slf: PyRefMut<'py, Self>,
        predicate: PyObject,
    ) -> PyResult<PyRefMut<'py, Self>> {
        slf.operations.push(Box::new(move |items, py| {
            items
                .into_iter()
                .filter_map(|item| {
                    match predicate
                        .call1(py, (&item,))
                        .and_then(|res| res.is_truthy(py))
                    {
                        Ok(true) => Some(Ok(item)),    // Keep the item
                        Ok(false) => None,             // Filter out
                        Err(e) => Some(Err(e.into())), // Handle errors gracefully
                    }
                })
                .collect::<PyResult<Vec<_>>>()
        }));
        Ok(slf)
    }

    /// Materialize all the lazy transformations into a final `Vec<PyObject>`.
    /// In Python: `final_list = list.collect()`.
    #[pyo3(text_signature = "($self)")]
    fn collect(&mut self, py: Python) -> PyResult<Vec<PyObject>> {
        let mut result = std::mem::take(&mut self.list); // Move to avoid cloning
        for operation in self.operations.drain(..) {
            result = operation(result, py)?;
        }
        Ok(result)
    }
}

// ------------------------------------------------------------------------
// A separate iterator class for `__iter__`
// ------------------------------------------------------------------------

// #[pyclass]
// pub struct ListIterator {
//     list: Vec<PyObject>,
//     index: usize,
// }

// #[pymethods]
// impl ListIterator {
//     #[new]
//     fn new(list: Vec<PyObject>) -> Self {
//         Self { list, index: 0 }
//     }

//     fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
//         slf
//     }

//     fn __next__(&mut self) -> Option<PyObject> {
//         if self.index < self.list.len() {
//             let item = self.list[self.index].clone();
//             self.index += 1;
//             Some(item)
//         } else {
//             None
//         }
//     }
// }

#[pyclass]
pub struct ListIterator {
    iter: std::vec::IntoIter<PyObject>,
}

#[pymethods]
impl ListIterator {
    fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    fn __next__(&mut self) -> Option<PyObject> {
        self.iter.next()
    }
}

// #[pyclass]
// pub struct ListIterator {
//     index: usize,
//     list_ref: Py<List>,
// }

// #[pymethods]
// impl ListIterator {
//     fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
//         slf
//     }

//     fn __next__(&mut self, py: Python<'_>) -> Option<PyObject> {
//         // let inner = self.list_ref.as_ref(py);
//         let inner = self.list_ref.borrow(py);
//         if self.index < inner.list.len() {
//             let item = inner.list[self.index].clone();
//             self.index += 1;
//             Some(item)
//         } else {
//             None
//         }
//     }
// }

// ------------------------------------------------------------------------
// Helper methods (not exposed to Python) for index validation
// ------------------------------------------------------------------------
impl List {
    /// Convert a possibly negative index into a valid, in-bounds index,
    /// or raise PyIndexError if out of range.
    fn to_valid_index(&self, i: isize) -> PyResult<usize> {
        let len = self.list.len() as isize;
        let mut idx = i;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 || idx >= len {
            return Err(PyErr::new::<PyIndexError, _>("list index out of range"));
        }
        Ok(idx as usize)
    }
}

#[pymodule]
pub fn register_list(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<List>()?;
    Ok(())
}
