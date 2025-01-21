use pyo3::{
    exceptions::{PyIndexError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyDict, PyIterator, PyList, PySlice, PyTuple},
};

// #[pyclass]
// pub struct ListIterator {
//     index: usize,
//     length: usize,
//     list: Vec<PyObject>,
// }

// #[pymethods]
// impl ListIterator {
//     pub fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
//         slf
//     }

//     pub fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyObject> {
//         let py = slf.py();
//         if slf.index < slf.length {
//             let item_ptr = slf.list[slf.index].as_ptr();
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

// #[derive(Clone)]
// #[pyclass]
// pub struct List {
//     pub list: Vec<PyObject>,
// }

// #[pymethods]
// impl List {
//     #[new]
//     pub fn new(iterable: Option<PyObject>, py: Python) -> PyResult<Self> {
//         let list = match iterable {
//             Some(obj) => {
//                 // Attempt to convert the object into an iterator
//                 let py_iter = PyIterator::from_object(&obj.bind(py))?;
//                 let mut elements = Vec::new();

//                 // Iterate through Python iterable and push elements into the Rust Vec
//                 for item in py_iter {
//                     elements.push(item?);
//                 }

//                 elements
//             }
//             None => Vec::new(),
//         };

//         Ok(List { list })
//     }
//     // #[new]
//     // pub fn new() -> Self {
//     //     List { list: Vec::new() }
//     // }

//     pub fn __repr__(&self) -> String {
//         Python::with_gil(|py| {
//             let reprs: Vec<String> = self
//                 .list
//                 .iter()
//                 .map(|obj| {
//                     obj.call_method0(py, "__repr__")
//                         .and_then(|repr_obj| repr_obj.extract::<String>(py))
//                         .unwrap_or_else(|_| "<error>".to_string())
//                 })
//                 .collect();

//             format!("List([{}])", reprs.join(", "))
//         })
//     }

//     pub fn append(&mut self, item: PyObject) {
//         self.list.push(item);
//     }

//     pub fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<ListIterator>> {
//         let py = slf.py();
//         let length = slf.list.len();
//         Py::new(
//             py,
//             ListIterator {
//                 index: 0,
//                 length,
//                 list: slf.list.clone(),
//             },
//         )
//     }
// }

#[pyclass]
pub struct List {
    pub list: Vec<PyObject>,
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
            let iter = iterable.try_iter()?;
            let mut elements = Vec::new();
            for item_result in iter {
                let item = item_result?.unbind();
                elements.push(item);
            }
            elements
        };

        Ok(List { list })
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
        // Check if `index` is an int or a slice
        if let Ok(i) = index.extract::<isize>() {
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
            let mut sliced = Vec::with_capacity(slice_len as usize);
            let mut i = start;
            if step > 0 {
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

            let new_list = List { list: sliced };
            let py_list = Py::new(index.py(), new_list)?;
            return Ok(py_list.into_any());
            // return Ok(Py::new(index.py(), new_list)?.into_ref(index.py()).unbind());
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
        // If it's an integer
        if let Ok(i) = index.extract::<isize>() {
            let idx = self.to_valid_index(i)?;
            let val = value.clone().unbind();
            self.list[idx] = val;
            return Ok(());
        }

        // Else, try to parse it as a slice
        if let Ok(slice) = index.downcast::<PySlice>() {
            // Get slice info
            let slice_info = slice.indices(self.list.len() as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;
            let slice_len = slice_info.slicelength;

            // Collect the replacement items into a Vec
            let mut replacement = Vec::new();
            if let Ok(iter) = value.try_iter() {
                for item_result in iter {
                    let item = item_result?.unbind();
                    replacement.push(item);
                }
            } else {
                // If the 'value' is not iterable, Python normally raises TypeError
                return Err(PyErr::new::<PyTypeError, _>("can only assign an iterable"));
            }

            if step == 1 {
                // Regular contiguous slice, e.g. a[start:stop] = replacement
                // 1) Remove the old slice
                // 2) Insert replacement

                let start_usize = start as usize;
                let stop_usize = stop as usize;
                // Remove the slice range
                self.list.drain(start_usize..stop_usize);
                // Insert the replacement
                // `splicing` approach:
                let mut idx = start_usize;
                for val in replacement {
                    self.list.insert(idx, val);
                    idx += 1;
                }
            } else {
                // Extended slice, e.g. a[start:stop:step] = replacement
                // The lengths must match exactly in Python, or ValueError
                if replacement.len() != slice_len as usize {
                    return Err(PyErr::new::<PyValueError, _>(
                        "attempt to assign sequence of size X to extended slice of size Y",
                    ));
                }
                // Assign item by item
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
        // If it's an integer
        if let Ok(i) = index.extract::<isize>() {
            let idx = self.to_valid_index(i)?;
            self.list.remove(idx);
            return Ok(());
        }

        // Else, try it as a slice
        if let Ok(slice) = index.downcast::<PySlice>() {
            let slice_info = slice.indices(self.list.len() as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;

            // If step == 1 (contiguous range), we can do a simple drain
            if step == 1 {
                self.list.drain(start as usize..stop as usize);
            } else if step == -1 && start < stop {
                // A small edge case: when step == -1 and (start < stop),
                //  Python effectively produces an empty range.
                //  So there's nothing to delete.
                // We do nothing
            } else {
                // For extended slices, let's build a new Vec excluding the slice
                // approach: skip the indices in the slice
                let len = self.list.len() as isize;
                let mut new_list = Vec::with_capacity(self.list.len());
                let mut idx: isize = 0;

                if step > 0 {
                    // gather slice indices in ascending order
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
                    // step < 0
                    // gather slice indices in descending order
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
        }
    }

    // ------------------------------------------------------------------------
    // __iter__: return an iterator
    // ------------------------------------------------------------------------
    fn __iter__(slf: PyRef<Self>) -> ListIterator {
        ListIterator {
            index: 0,
            list_ref: slf.into(),
        }
    }

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
            List { list: Vec::new() }
        } else {
            // Repeat the list
            let times = n as usize;
            let mut repeated = Vec::with_capacity(self.list.len() * times);
            for _ in 0..times {
                repeated.extend(self.list.iter().cloned());
            }
            List { list: repeated }
        }
    }

    // ------------------------------------------------------------------------
    // __rmul__(self, n): same as __mul__ (for n * list)
    // ------------------------------------------------------------------------
    fn __rmul__(&self, n: isize) -> Self {
        self.__mul__(n)
    }
}

// ------------------------------------------------------------------------
// A separate iterator class for `__iter__`
// ------------------------------------------------------------------------
#[pyclass]
pub struct ListIterator {
    index: usize,
    list_ref: Py<List>,
}

#[pymethods]
impl ListIterator {
    fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> Option<PyObject> {
        // let inner = self.list_ref.as_ref(py);
        let inner = self.list_ref.borrow(py);
        if self.index < inner.list.len() {
            let item = inner.list[self.index].clone();
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }
}

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
