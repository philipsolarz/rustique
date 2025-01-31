use num_bigint::BigInt;
use num_traits::sign::Signed;
use num_traits::ToPrimitive;
use pyo3::{
    exceptions::{PyOverflowError, PyTypeError, PyValueError, PyZeroDivisionError},
    ffi,
    prelude::*,
    types::{
        PyBool, PyComplex, PyDict, PyFloat, PyFrozenSet, PyInt, PyIterator, PyList, PyMapping,
        PySequence, PyString, PyTuple,
    },
};
use std::iter::Iterator;
use std::sync::{Arc, Mutex}; // Add this at the top of your file

use pyo3::prelude::*;
use pyo3::types::PyModule;

// #[pyclass]
// struct RangeIterator {
//     current: i64,
//     stop: i64,
//     step: i64,
// }

// #[pymethods]
// impl RangeIterator {
//     fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
//         slf
//     }

//     fn __next__(&mut self) -> PyResult<Option<i64>> {
//         // Check if current is beyond the stop based on step direction
//         if (self.step > 0 && self.current >= self.stop)
//             || (self.step < 0 && self.current <= self.stop)
//         {
//             return Ok(None);
//         }

//         let current = self.current;

//         // Compute next value, handle overflow by setting to stop
//         match self.current.checked_add(self.step) {
//             Some(next) => self.current = next,
//             None => self.current = self.stop, // Terminate on overflow
//         };

//         Ok(Some(current))
//     }
// }

// #[pyfunction]
// fn range(_py: Python<'_>, args: &PyTuple) -> PyResult<PyObject> {
//     let len = args.len();
//     if !(1..=3).contains(&len) {
//         return Err(PyTypeError::new_err(format!(
//             "range expected 1 to 3 arguments, got {}",
//             len
//         )));
//     }

//     // Parse start, stop, step from arguments
//     let (start, stop, step) = match len {
//         1 => {
//             let stop = args.get_item(0)?.extract::<i64>()?;
//             (0, stop, 1)
//         }
//         2 => {
//             let start = args.get_item(0)?.extract::<i64>()?;
//             let stop = args.get_item(1)?.extract::<i64>()?;
//             (start, stop, 1)
//         }
//         _ => {
//             let start = args.get_item(0)?.extract::<i64>()?;
//             let stop = args.get_item(1)?.extract::<i64>()?;
//             let step = args.get_item(2)?.extract::<i64>()?;
//             (start, stop, step)
//         }
//     };

//     if step == 0 {
//         return Err(PyValueError::new_err("range() arg 3 must not be zero"));
//     }

//     // Create the iterator
//     let iterator = RangeIterator {
//         current: start,
//         stop,
//         step,
//     };

//     // Convert to a Python object
//     Ok(iterator.into_py(_py))
// }

// Define a type for closures that take a Python object and return a result with a new Python object.
type Closure = Box<dyn Fn(PyObject) -> PyResult<PyObject> + Send + Sync>;
// type Closure = Box<dyn Fn(&Bound<'_, PyAny>) -> PyResult<PyObject> + Send + Sync>;

#[pyclass]
struct Engine2 {
    buffer: Vec<Closure>,
}

#[pymethods]
impl Engine2 {
    #[new]
    fn new() -> Self {
        Engine2 { buffer: Vec::new() }
    }

    fn range(mut slf: PyRefMut<Self>, n: i32) -> PyRefMut<Self> {
        slf.buffer.push(Box::new(move |_prev: PyObject| {
            Python::with_gil(|py| {
                let builtins = PyModule::import(py, "builtins")?;
                let range = builtins.getattr("range")?.call1((n,))?;
                Ok(range.to_object(py))
            })
        }));
        slf
    }

    fn list(mut slf: PyRefMut<Self>) -> PyRefMut<Self> {
        slf.buffer.push(Box::new(|prev: PyObject| {
            Python::with_gil(|py| {
                let builtins = PyModule::import(py, "builtins")?;
                let list_func = builtins.getattr("list")?;
                let list = list_func.call1((prev,))?;
                Ok(list.to_object(py))
            })
        }));
        slf
    }

    fn filter(mut slf: PyRefMut<Self>, func: PyObject) -> PyRefMut<Self> {
        slf.buffer.push(Box::new(move |prev: PyObject| {
            let func = func.clone();
            Python::with_gil(|py| {
                let builtins = PyModule::import(py, "builtins")?;
                let filter_func = builtins.getattr("filter")?;
                let filtered = filter_func.call1((func, prev))?;
                Ok(filtered.to_object(py))
            })
        }));
        slf
    }

    // Method c: Filter elements (uses previous result)
    fn flush(&mut self, py: Python) -> PyResult<PyObject> {
        let mut current = py.None();
        for closure in &self.buffer {
            current = closure(current)?;
        }
        Ok(current)
    }
}

// Base trait for Rust-side iterators
trait RustIteratorTrait: Send {
    fn next(&mut self, py: Python) -> PyResult<Option<PyObject>>;
}

// PyO3 wrapper for Rust-side iterators
#[pyclass(subclass)]
struct RustIterator {
    inner: Arc<Mutex<Box<dyn RustIteratorTrait>>>,
}

#[pymethods]
impl RustIterator {
    fn __next__(&self, py: Python) -> PyResult<Option<PyObject>> {
        let mut inner = self.inner.lock().unwrap();
        inner.next(py)
    }

    fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }
}

struct MapState {
    func: PyObject,
    iterables: Vec<Box<dyn RustIteratorTrait>>,
}

impl RustIteratorTrait for MapState {
    fn next(&mut self, py: Python) -> PyResult<Option<PyObject>> {
        // Collect next items from all iterables
        let mut args = Vec::new();
        for iter in &mut self.iterables {
            match iter.next(py)? {
                Some(item) => args.push(item),
                None => return Ok(None), // Stop if any iterable is exhausted
            }
        }

        // Call Python function (still crosses boundary here)
        let result = self.func.call1(py, PyTuple::new(py, &args)?)?;
        Ok(Some(result))
    }
}

struct PyIteratorWrapper {
    py_iter_obj: PyObject,
}

impl RustIteratorTrait for PyIteratorWrapper {
    fn next(&mut self, py: Python) -> PyResult<Option<PyObject>> {
        let iter = self.py_iter_obj.bind(py);
        let mut py_iter = PyIterator::from_object(iter)?;

        match py_iter.next() {
            Some(Ok(item)) => Ok(Some(item.unbind().into_py(py))),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

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

    fn __next__(mut slf: PyRefMut<Self>) -> PyResult<Option<PyObject>> {
        let py = slf.py();
        let mut args = Vec::new();

        // Iterate over each stored Python iterator
        for iter_obj in &slf.iterables {
            let iter = iter_obj.bind(py);
            let mut py_iter = PyIterator::from_object(iter)?; // Safe wrapper

            match py_iter.next() {
                Some(item) => args.push(item?), // Propagate errors
                None => return Ok(None),        // StopIteration (normal exit)
            }
        }

        // Call func with collected arguments
        let result = slf.func.call1(py, PyTuple::new(py, args)?)?;
        Ok(Some(result))
    }

    // fn __next__(slf: PyRefMut<Self>) -> PyResult<Option<PyObject>> {
    //     let py = slf.py();

    //     let mut next_items = Vec::new();
    //     for iterable in &slf.iterables {
    //         let item_ptr = unsafe { ffi::PyIter_Next(iterable.as_ptr()) };
    //         if item_ptr.is_null() {
    //             if PyErr::occurred(py) {
    //                 return Err(PyErr::fetch(py));
    //             } else {
    //                 return Ok(None);
    //             }
    //         }
    //         let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };
    //         next_items.push(item);
    //     }

    //     let args = PyTuple::new(py, next_items)?;
    //     let result = slf.func.call1(py, args)?;

    //     Ok(Some(result))
    // }
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

#[pyclass]
struct ZipIterator {
    iterables: Vec<PyObject>, // Each one is already an iterator
    strict: bool,
}

#[pymethods]
impl ZipIterator {
    #[new]
    fn new(iterables: Vec<PyObject>, strict: bool) -> Self {
        ZipIterator { iterables, strict }
    }

    fn __iter__(slf: PyRef<Self>) -> PyResult<Py<ZipIterator>> {
        Ok(slf.into())
    }

    fn __next__(mut slf: PyRefMut<Self>, py: Python) -> PyResult<Option<PyObject>> {
        let mut result_elems = Vec::with_capacity(slf.iterables.len());

        for (i, iter_obj) in slf.iterables.iter().enumerate() {
            let item_ptr = unsafe { ffi::PyIter_Next(iter_obj.as_ptr()) };
            if item_ptr.is_null() {
                // If an error occurred in retrieving the item, raise it
                if PyErr::occurred(py) {
                    return Err(PyErr::fetch(py));
                }

                // If no item is found, we have an exhausted iterator
                if slf.strict {
                    // For strict=True, raise ValueError if lengths mismatch
                    let msg = format!("zip() argument {} is shorter than argument 1", i + 1);
                    return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(msg));
                } else {
                    // For default behavior, end iteration
                    return Ok(None);
                }
            }

            // Otherwise, we got an item
            let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };
            result_elems.push(item);
        }

        // Return a tuple of these items
        let tuple = PyTuple::new(py, result_elems)?;
        Ok(Some(tuple.to_object(py)))
    }
}

// #[pyclass]
// struct EnumerateIterator {
//     iter: PyObject, // a PyObject that is an iterator
//     index: i64,     // current index
// }

// #[pymethods]
// impl EnumerateIterator {
//     #[new]
//     fn new(py: Python, iterable: &Bound<'_, PyAny>, start: i64) -> PyResult<Self> {
//         // Make sure `iterable` is an iterator
//         let _ = PyIterator::from_object(&iterable)?;
//         Ok(EnumerateIterator {
//             iter: iterable,
//             index: start,
//         })
//     }

//     fn __iter__(slf: PyRef<Self>) -> PyResult<Py<EnumerateIterator>> {
//         Ok(slf.into())
//     }

//     fn __next__(mut slf: PyRefMut<Self>, py: Python) -> PyResult<Option<PyObject>> {
//         let item_ptr = unsafe { ffi::PyIter_Next(slf.iter.as_ptr()) };
//         if item_ptr.is_null() {
//             if PyErr::occurred(py) {
//                 return Err(PyErr::fetch(py));
//             } else {
//                 return Ok(None); // Exhausted
//             }
//         }
//         let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };

//         let result_tuple = PyTuple::new(py, &[slf.index.to_object(py), item])?;
//         slf.index += 1;
//         Ok(Some(result_tuple.to_object(py)))
//     }
// }

// #[pyclass]
// struct RangeIterator {
//     current: i64,
//     stop: i64,
//     step: i64,
//     done: bool,
// }

// #[pymethods]
// impl RangeIterator {
//     #[new]
//     #[pyo3(signature = (start, stop=None, step=None))]
//     fn new(start: i64, stop: Option<i64>, step: Option<i64>) -> PyResult<Self> {
//         // If only one arg, treat that as "stop" with start=0
//         let (start, stop, step) = match (stop, step) {
//             (None, None) => (0, start, 1),         // range(stop)
//             (Some(s), None) => (start, s, 1),      // range(start, stop)
//             (Some(s), Some(st)) => (start, s, st), // range(start, stop, step)
//         };

//         if step == 0 {
//             return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
//                 "range() arg 3 must not be zero",
//             ));
//         }

//         // No checking for negative step boundary in this snippet
//         Ok(RangeIterator {
//             current: start,
//             stop,
//             step,
//             done: false,
//         })
//     }

//     fn __iter__(slf: PyRef<Self>) -> PyResult<Py<RangeIterator>> {
//         Ok(slf.into())
//     }

//     fn __next__(mut slf: PyRefMut<Self>) -> PyResult<Option<i64>> {
//         if slf.done {
//             return Ok(None);
//         }

//         // For positive steps, stop if current >= stop
//         // For negative steps, stop if current <= stop
//         // Python does not include stop itself
//         let finished = if slf.step > 0 {
//             slf.current >= slf.stop
//         } else {
//             slf.current <= slf.stop
//         };
//         if finished {
//             slf.done = true;
//             return Ok(None);
//         }

//         let val = slf.current;
//         slf.current = slf.current.saturating_add(slf.step); // to avoid overflow
//         Ok(Some(val))
//     }
// }

#[pyclass]
struct DictIterator {
    iter: PyObject, // Stores the Python iterable
}

#[pymethods]
impl DictIterator {
    #[new]
    fn new(iter: PyObject) -> Self {
        DictIterator { iter }
    }

    fn __iter__(slf: PyRef<Self>) -> PyResult<Py<DictIterator>> {
        Ok(slf.into())
    }

    fn __next__(slf: PyRefMut<Self>, py: Python) -> PyResult<Option<(PyObject, PyObject)>> {
        let item_ptr = unsafe { pyo3::ffi::PyIter_Next(slf.iter.as_ptr()) };
        if item_ptr.is_null() {
            if PyErr::occurred(py) {
                return Err(PyErr::fetch(py));
            } else {
                return Ok(None); // Exhausted
            }
        }

        let item = unsafe { PyObject::from_owned_ptr(py, item_ptr) };

        // Expecting key-value pairs (e.g., tuples of length 2)
        if let Ok(pair) = item.downcast_bound::<PyTuple>(py) {
            if pair.len() != 2 {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "ValueError: dict() argument must contain iterable of key-value pairs",
                ));
            }
            let key = pair.get_item(0)?;
            let value = pair.get_item(1)?;
            return Ok(Some((key.to_object(py), value.to_object(py))));
        }

        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "TypeError: dict() argument must contain iterable of key-value pairs",
        ))
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
        builtins.setattr("all", Py::new(py, self.clone())?.getattr(py, "all")?)?;
        builtins.setattr("any", Py::new(py, self.clone())?.getattr(py, "any")?)?;
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

    pub fn abs(&self, x: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        if let Ok(int) = x.downcast::<PyInt>() {
            if let Ok(i) = int.extract::<i32>() {
                let abs_val = i.abs();
                return Ok(abs_val.to_object(x.py()));
            } else if let Ok(i) = int.extract::<i64>() {
                let abs_val = i.abs();
                return Ok(abs_val.to_object(x.py()));
            } else {
                let bigint: BigInt = int.extract::<BigInt>()?;
                let abs_val = bigint.abs();
                return Ok(abs_val.to_object(x.py()));
            }
        } else if let Ok(float) = x.downcast::<PyFloat>() {
            let abs_val = float.extract::<f64>()?.abs();
            return Ok(abs_val.to_object(x.py()));
        } else if let Ok(complex) = x.downcast::<PyComplex>() {
            let real = complex.getattr("real")?.extract::<f64>()?;
            let imag = complex.getattr("imag")?.extract::<f64>()?;
            let abs_val = (real.powi(2) + imag.powi(2)).sqrt();
            return Ok(abs_val.to_object(x.py()));
        } else if let Ok(abs_func) = x.getattr("__abs__") {
            let abs_val: PyObject = abs_func.call0()?.extract()?;
            return Ok(abs_val);
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "TypeError: bad operand type for abs()",
            ));
        }
    }

    // pub fn aiter(&self, x: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     todo!()
    // }

    #[pyo3(signature = (iterable))]
    pub fn all(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let iterator = PyIterator::from_object(iterable)?;

        for item in iterator {
            let obj = item?;
            if !obj.is_truthy()? {
                return Ok(false.to_object(py)); // Return False if any element is falsy
            }
        }

        Ok(true.to_object(py)) // Return True if all elements are truthy or iterable is empty
    }

    // pub fn anext(&self, x: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     todo!()
    // }

    #[pyo3(signature = (iterable))]
    pub fn any(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let iterator = PyIterator::from_object(iterable)?;

        for item in iterator {
            let obj = item?;
            if obj.is_truthy()? {
                return Ok(true.to_object(py)); // Return True if any element is truthy
            }
        }

        Ok(false.to_object(py)) // Return False if no truthy elements are found
    }

    // pub fn ascii(&self, x: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     todo!()
    // }

    pub fn bin(&self, py: Python, x: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        if let Ok(num) = x.extract::<i64>() {
            let bin_str = format!("0b{:b}", num);
            return Ok(PyString::new(py, &bin_str).to_object(py));
        }

        if let Ok(index_func) = x.getattr("__index__") {
            let index_value: i64 = index_func.call0()?.extract()?;
            let bin_str = format!("0b{:b}", index_value);
            return Ok(PyString::new(py, &bin_str).to_object(py));
        }

        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "TypeError: bin() argument must be an integer or implement __index__()",
        ))
    }

    #[pyo3(signature = (x=None))]
    pub fn bool(&self, py: Python, x: Option<PyObject>) -> PyResult<PyObject> {
        match x {
            Some(value) => {
                if value.is_truthy(py)? {
                    Ok(true.to_object(py))
                } else {
                    Ok(false.to_object(py))
                }
            }
            None => Ok(false.to_object(py)),
        }
    }

    // pub fn breakpoint(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn bytearray(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn bytes(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn callable(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn chr(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn classmethod(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn compile(&self) -> PyResult<()> {
    //     todo!()
    // }

    // #[pyo3(signature = (real=0, imag=0))]
    pub fn complex(
        &self,
        py: Python,
        real: Option<&Bound<'_, PyAny>>,
        imag: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        match (real, imag) {
            // Case 1: No arguments provided, return 0j
            (None, None) => {
                let zero = 0.0;
                return Ok(PyComplex::from_doubles(py, zero, zero).to_object(py));
            }

            // Case 2: Only the real part provided
            (Some(real_part), None) => {
                if let Ok(s) = real_part.downcast::<PyString>() {
                    let s = s.to_str()?.trim().trim_matches(&['(', ')'][..]); // Strip parentheses and spaces
                    let complex_val = s.parse::<num_complex::Complex64>().map_err(|_| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(
                            "ValueError: invalid complex string",
                        )
                    })?;
                    return Ok(
                        PyComplex::from_doubles(py, complex_val.re, complex_val.im).to_object(py)
                    );
                }

                // Handle if real_part is a number
                if let Ok(num) = real_part.extract::<f64>() {
                    return Ok(PyComplex::from_doubles(py, num, 0.0).to_object(py));
                }

                // Handle custom objects implementing __complex__, __float__, or __index__
                if let Ok(complex_func) = real_part.getattr("__complex__") {
                    return Ok(complex_func.call0()?.to_object(py));
                }
                if let Ok(float_func) = real_part.getattr("__float__") {
                    let real_val = float_func.call0()?.extract::<f64>()?;
                    return Ok(PyComplex::from_doubles(py, real_val, 0.0).to_object(py));
                }
                if let Ok(index_func) = real_part.getattr("__index__") {
                    let real_val = index_func.call0()?.extract::<i64>()? as f64;
                    return Ok(PyComplex::from_doubles(py, real_val, 0.0).to_object(py));
                }

                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "TypeError: complex() argument must be a string, a number, or an object implementing __complex__(), __float__(), or __index__()",
                ));
            }

            // Case 3: Only the imaginary part provided
            (None, Some(imag_part)) => {
                let imag_value = imag_part.extract::<f64>().unwrap_or(0.0);
                return Ok(PyComplex::from_doubles(py, 0.0, imag_value).to_object(py));
            }

            // Case 4: Two arguments provided (real, imag)
            (Some(real_part), Some(imag_part)) => {
                let real_value = if let Ok(r) = real_part.extract::<f64>() {
                    r
                } else if let Ok(c) = real_part.extract::<num_complex::Complex64>() {
                    c.re
                } else {
                    return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        "TypeError: real part must be a number",
                    ));
                };

                let imag_value = if let Ok(i) = imag_part.extract::<f64>() {
                    i
                } else if let Ok(c) = imag_part.extract::<num_complex::Complex64>() {
                    c.im
                } else {
                    return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        "TypeError: imaginary part must be a number",
                    ));
                };

                return Ok(PyComplex::from_doubles(py, real_value, imag_value).to_object(py));
            }
        }
    }

    // pub fn delattr(&self) -> PyResult<()> {
    //     todo!()
    // }

    #[pyo3(signature = (*args, **kwargs))]
    pub fn dict(
        &self,
        py: Python,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PyObject> {
        let new_dict = PyDict::new(py);

        // Case 1: No arguments, return an empty dictionary
        if args.is_empty() && kwargs.is_none() {
            return Ok(new_dict.to_object(py));
        }

        // Case 2: If kwargs exist, add them to the dictionary
        if let Some(kw) = kwargs {
            new_dict.update(kw.as_mapping())?;
        }

        // Case 3: Handle positional arguments
        if !args.is_empty() {
            if args.len() > 1 {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "TypeError: dict expected at most 1 argument, got multiple",
                ));
            }

            let first_arg = args.get_item(0)?;

            // If it's a mapping, update directly
            if let Ok(mapping) = first_arg.downcast::<PyDict>() {
                new_dict.update(mapping.as_mapping())?;
            }
            // Handle generic mapping-like objects with __getitem__ and keys()
            else if first_arg.hasattr("__getitem__")? && first_arg.hasattr("keys")? {
                let keys = first_arg.getattr("keys")?.call0()?;
                for key_result in keys.try_iter()? {
                    let key = key_result?;
                    let value = first_arg.get_item(&key)?;
                    new_dict.set_item(&key, value)?;
                }
            }
            // If it's an iterable of pairs, use the iterator approach
            else if first_arg.is_instance_of::<pyo3::types::PyList>()
                || first_arg.is_instance_of::<pyo3::types::PyTuple>()
                || first_arg.hasattr("__iter__")?
            {
                let iter = first_arg.call_method0("__iter__")?;
                let dict_iter = Py::new(py, DictIterator::new(iter.into()))?;

                // Iterate through the DictIterator and populate the dictionary
                let iterator = dict_iter.bind(py);
                for item in iterator.try_iter()? {
                    let (key, value): (PyObject, PyObject) = item?.extract()?;
                    new_dict.set_item(key, value)?;
                }
            } else {
                return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "TypeError: dict() argument must be a mapping or iterable of key-value pairs",
                ));
            }
        }

        Ok(new_dict.to_object(py))
    }

    // pub fn dir(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn divmod(
    //     &self,
    //     py: Python,
    //     a: &Bound<'_, PyAny>,
    //     b: &Bound<'_, PyAny>,
    // ) -> PyResult<PyObject> {
    //     // Handle integer values with arbitrary precision
    //     if let (Ok(a_int), Ok(b_int)) = (a.extract::<i64>(), b.extract::<i64>()) {
    //         if b_int == 0 {
    //             return Err(PyErr::new::<PyZeroDivisionError, _>(
    //                 "ZeroDivisionError: division by zero",
    //             ));
    //         }
    //         let quotient = a_int / b_int;
    //         let remainder = a_int % b_int;
    //         return Ok(PyTuple::new(py, &[quotient, remainder]).to_object(py));
    //     }

    //     // Handle big integers using num-bigint
    //     if let (Ok(a_big), Ok(b_big)) = (a.extract::<BigInt>(), b.extract::<BigInt>()) {
    //         if b_big.is_zero() {
    //             return Err(PyErr::new::<PyZeroDivisionError, _>(
    //                 "ZeroDivisionError: division by zero",
    //             ));
    //         }
    //         let (quotient, remainder) = a_big.div_rem(&b_big);
    //         return Ok(
    //             PyTuple::new(py, &[quotient.to_object(py), remainder.to_object(py)]).to_object(py),
    //         );
    //     }

    //     // Handle floating-point numbers
    //     if let (Ok(a_float), Ok(b_float)) = (a.extract::<f64>(), b.extract::<f64>()) {
    //         if b_float == 0.0 {
    //             return Err(PyErr::new::<PyZeroDivisionError, _>(
    //                 "ZeroDivisionError: float division by zero",
    //             ));
    //         }
    //         let quotient = (a_float / b_float).floor();
    //         let remainder = a_float % b_float;

    //         // Ensure remainder sign consistency with Python's divmod
    //         let adjusted_remainder = if remainder == 0.0 || (remainder > 0.0) == (b_float > 0.0) {
    //             remainder
    //         } else {
    //             remainder + b_float
    //         };

    //         let quotient_int = quotient as i64;
    //         return Ok(PyTuple::new(py, &[quotient_int, adjusted_remainder]).to_object(py));
    //     }

    //     // Handle mixed types (int and float)
    //     if let Ok(a_int) = a.extract::<i64>() {
    //         if let Ok(b_float) = b.extract::<f64>() {
    //             if b_float == 0.0 {
    //                 return Err(PyErr::new::<PyZeroDivisionError, _>(
    //                     "ZeroDivisionError: float division by zero",
    //                 ));
    //             }
    //             let a_float = a_int as f64;
    //             let quotient = (a_float / b_float).floor();
    //             let remainder = a_float % b_float;
    //             let quotient_int = quotient as i64;
    //             return Ok(PyTuple::new(py, &[quotient_int, remainder]).to_object(py));
    //         }
    //     }

    //     if let Ok(a_float) = a.extract::<f64>() {
    //         if let Ok(b_int) = b.extract::<i64>() {
    //             if b_int == 0 {
    //                 return Err(PyErr::new::<PyZeroDivisionError, _>(
    //                     "ZeroDivisionError: float division by zero",
    //                 ));
    //             }
    //             let b_float = b_int as f64;
    //             let quotient = (a_float / b_float).floor();
    //             let remainder = a_float % b_float;
    //             let quotient_int = quotient as i64;
    //             return Ok(PyTuple::new(py, &[quotient_int, remainder]).to_object(py));
    //         }
    //     }

    //     // Fallback: check if the objects have __floordiv__ and __mod__ methods
    //     if let Ok(floordiv) = a.call_method1("__floordiv__", (b,)) {
    //         if let Ok(modulo) = a.call_method1("__mod__", (b,)) {
    //             return Ok(PyTuple::new(py, &[floordiv, modulo]).to_object(py));
    //         }
    //     }

    //     // If no valid operations, raise TypeError
    //     Err(PyErr::new::<PyTypeError, _>(
    //         "TypeError: divmod() requires numeric arguments",
    //     ))
    // }

    // #[pyo3(signature = (iterable, start=0))]
    // pub fn enumerate(
    //     &self,
    //     py: Python,
    //     iterable: &Bound<'_, PyAny>,
    //     start: i64,
    // ) -> PyResult<PyObject> {
    //     let enum_obj = Py::new(py, EnumerateIterator::new(py, iterable, start)?)?;
    //     Ok(enum_obj.to_object(py))
    // }

    // pub fn eval(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn exec(&self) -> PyResult<()> {
    //     todo!()
    // }

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

    #[pyo3(signature = (value=None))]
    pub fn float(&self, py: Python, value: Option<&Bound<'_, PyAny>>) -> PyResult<PyObject> {
        match value {
            // Case 1: No arguments provided, return 0.0
            None => {
                return Ok(0.0f64.to_object(py));
            }

            Some(val) => {
                // Case 2: Handle if value is already a float
                if let Ok(float_val) = val.extract::<f64>() {
                    return Ok(float_val.to_object(py));
                }

                // Case 3: Handle if value is an integer (including arbitrary precision)
                if let Ok(int_val) = val.extract::<i64>() {
                    return Ok((int_val as f64).to_object(py));
                }
                if let Ok(bigint_val) = val.extract::<num_bigint::BigInt>() {
                    let float_val = bigint_val.to_f64().ok_or_else(|| {
                        PyErr::new::<PyOverflowError, _>(
                            "OverflowError: integer is too large to convert to float",
                        )
                    })?;
                    return Ok(float_val.to_object(py));
                }

                // Case 4: Handle string input (convert to float)
                if let Ok(string_val) = val.downcast::<PyString>() {
                    let s = string_val.to_str()?.trim(); // Remove surrounding whitespace

                    // Attempt to parse the string into a float
                    match s.to_lowercase().as_str() {
                        "inf" | "+inf" | "infinity" | "+infinity" => {
                            return Ok(f64::INFINITY.to_object(py));
                        }
                        "-inf" | "-infinity" => {
                            return Ok(f64::NEG_INFINITY.to_object(py));
                        }
                        "nan" => {
                            return Ok(f64::NAN.to_object(py));
                        }
                        _ => {
                            // Handle numeric string parsing
                            match s.parse::<f64>() {
                                Ok(parsed_val) => return Ok(parsed_val.to_object(py)),
                                Err(_) => {
                                    return Err(PyErr::new::<PyValueError, _>(
                                        "ValueError: could not convert string to float",
                                    ));
                                }
                            }
                        }
                    }
                }

                // Case 5: Handle custom objects implementing __float__()
                if let Ok(float_func) = val.getattr("__float__") {
                    let float_result = float_func.call0()?.extract::<f64>()?;
                    return Ok(float_result.to_object(py));
                }

                // Case 6: Handle custom objects implementing __index__()
                if let Ok(index_func) = val.getattr("__index__") {
                    let index_value: i64 = index_func.call0()?.extract()?;
                    return Ok((index_value as f64).to_object(py));
                }

                // If none of the above worked, return TypeError
                Err(PyErr::new::<PyTypeError, _>(
                    "TypeError: float() argument must be a string, a number, or an object implementing __float__() or __index__()",
                ))
            }
        }
    }

    // pub fn format(&self) -> PyResult<()> {
    //     todo!()
    // }

    #[pyo3(signature = (iterable=None))]
    pub fn frozenset(&self, py: Python, iterable: Option<&Bound<'_, PyAny>>) -> PyResult<PyObject> {
        match iterable {
            // Case 1: No arguments, return an empty frozenset
            None => {
                let empty_set = PyFrozenSet::empty(py)?;
                return Ok(empty_set.to_object(py));
            }

            Some(iter) => {
                // Case 2: Convert an iterable to frozenset
                let py_iter = PyIterator::from_object(iter)?;
                // Collect elements into a list first (since PyFrozenSet expects a collection)
                let elements: Vec<PyObject> = py_iter
                    .map(|item| item.and_then(|i| Ok(i.to_object(py))))
                    .collect::<PyResult<_>>()?;

                // Create a frozenset from the collected elements
                let py_frozenset = PyFrozenSet::new(py, &elements)?;
                return Ok(py_frozenset.to_object(py));
            }
        }
    }

    // pub fn getattr(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn globals(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn hasattr(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn hash(&self, py: Python, obj: &Bound<'_, PyAny>) -> PyResult<i64> {
    //     // Case 2: Handle specific types manually for performance (int, float, str, tuple)
    //     if let Ok(i) = obj.extract::<i64>() {
    //         return Ok(i);
    //     }

    //     if let Ok(f) = obj.extract::<f64>() {
    //         return Ok(f.to_bits() as i64);
    //     }

    //     if let Ok(s) = obj.extract::<String>() {
    //         return Ok(calculate_hash_for_string(&s));
    //     }

    //     // Case 1: Check if the object has a __hash__ method
    //     if obj.hasattr("__hash__")? {
    //         let hash_value: i64 = obj.call_method0("__hash__")?.extract()?;
    //         return Ok(hash_value);
    //     }

    //     // If none of the above, raise TypeError
    //     Err(PyErr::new::<PyTypeError, _>("TypeError: unhashable type"))
    // }

    // pub fn help(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn hex(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn id(&self) -> PyResult<()> {
    //     todo!()
    // }

    // pub fn input(&self) -> PyResult<()> {
    //     todo!()
    // }

    pub fn int(&self) -> PyResult<()> {
        todo!()
    }

    pub fn isinstance(&self) -> PyResult<()> {
        todo!()
    }

    pub fn issubclass(&self) -> PyResult<()> {
        todo!()
    }

    pub fn iter(&self) -> PyResult<()> {
        todo!()
    }

    pub fn len(&self) -> PyResult<()> {
        todo!()
    }

    #[pyo3(signature = (iterable))]
    pub fn list(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<Py<PyList>> {
        // Check if input is a Rust-side iterator
        if let Ok(rust_iter) = iterable.downcast::<RustIterator>() {
            // Process entirely in Rust
            let mut elements = Vec::new();
            let asdf = rust_iter.borrow_mut();
            let mut inner = asdf.inner.lock().unwrap();
            while let Some(item) = inner.next(py)? {
                elements.push(item);
            }
            Ok(PyList::new(py, elements)?.into())
        } else {
            // Fallback for Python iterators
            let mut iter = iterable.try_iter()?;
            let py_list = PyList::empty(py);
            while let Some(item) = iter.next() {
                py_list.append(item?)?;
            }
            Ok(py_list.into())
        }
    }

    // #[pyo3(signature = (iterable))]
    // pub fn list(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<List> {
    //     let iter = iterable.try_iter()?;
    // }

    // #[pyo3(signature = (iterable))]
    // pub fn list(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Get an iterator from the input
    //     let iter = iterable.try_iter()?;

    //     // Create a new Python list
    //     let py_list = PyList::empty(py);

    //     // Iterate and collect elements into the list
    //     for item in iter {
    //         let item = item?;
    //         py_list.append(item)?;
    //     }

    //     // Return the constructed list
    //     Ok(py_list.into())
    // }

    // #[pyo3(signature = (iterable))]
    // pub fn list(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    //     // Convert input to an iterator
    //     let iter = PyIterator::from_object(iterable)?.into();

    //     // Create ListIterator and return as an iterator object
    //     let list_iter = Py::new(py, ListIterator { iter })?;
    //     Ok(list_iter.to_object(py))
    // }

    pub fn locals(&self) -> PyResult<()> {
        todo!()
    }

    // #[pyo3(signature = (func, iterable, *iterables))]
    // pub fn map(
    //     &self,
    //     py: Python,
    //     func: PyObject,
    //     iterable: &Bound<'_, PyAny>,
    //     iterables: &Bound<'_, PyTuple>,
    // ) -> PyResult<PyObject> {
    //     // Convert all inputs to Rust-side iterators
    //     let mut rust_iters = Vec::new();

    //     // Helper to wrap Python iterators into RustIteratorTrait
    //     fn to_rust_iter(obj: &Bound<'_, PyAny>) -> PyResult<Box<dyn RustIteratorTrait>> {
    //         let py_iter = obj.try_iter()?.to_object(obj.py());
    //         Ok(Box::new(PyIteratorWrapper {
    //             py_iter_obj: py_iter,
    //         }))
    //     }

    //     rust_iters.push(to_rust_iter(iterable)?);
    //     for item in iterables.iter() {
    //         rust_iters.push(to_rust_iter(&item)?);
    //     }

    //     // Create the Rust-side map iterator
    //     let map_state = MapState {
    //         func: func.clone_ref(py),
    //         iterables: rust_iters,
    //     };

    //     // Wrap it for Python
    //     let rust_iter = RustIterator {
    //         inner: Arc::new(Mutex::new(Box::new(map_state))),
    //     };

    //     Ok(rust_iter.into_py(py))
    // }

    #[pyo3(signature = (func, iterable, *iterables))]
    pub fn map(
        &self,
        py: Python,
        func: PyObject,
        iterable: &Bound<'_, PyAny>,
        iterables: &Bound<'_, PyTuple>,
    ) -> PyResult<PyObject> {
        // Convert all inputs to iterators upfront
        let mut iters = vec![iterable.try_iter()?.into()]; // First iterable

        for item in iterables.iter() {
            iters.push(item.try_iter()?.into()); // Subsequent iterables
        }

        // Create MapIterator with these iterators
        let map_iter = Py::new(py, MapIterator::new(func, iters))?;
        Ok(map_iter.to_object(py))
    }

    // #[pyo3(signature = (func, iterable, *iterables))]
    // pub fn map(
    //     &self,
    //     py: Python,
    //     func: PyObject,
    //     iterable: &Bound<'_, PyAny>,
    //     iterables: &Bound<'_, PyTuple>,
    // ) -> PyResult<PyObject> {
    //     let mut all_iterables = Vec::new();

    //     let iter = PyIterator::from_object(iterable)?.into();
    //     all_iterables.push(iter);

    //     for iterable in iterables.iter() {
    //         let iter = PyIterator::from_object(&iterable)?.into();
    //         all_iterables.push(iter);
    //     }

    //     let map_iter = Py::new(py, MapIterator::new(func, all_iterables))?;
    //     Ok(map_iter.to_object(py))
    // }

    pub fn max(&self) -> PyResult<()> {
        todo!()
    }

    pub fn memoryview(&self) -> PyResult<()> {
        todo!()
    }

    pub fn min(&self) -> PyResult<()> {
        todo!()
    }

    pub fn next(&self) -> PyResult<()> {
        todo!()
    }

    pub fn object(&self) -> PyResult<()> {
        todo!()
    }

    pub fn oct(&self) -> PyResult<()> {
        todo!()
    }

    pub fn open(&self) -> PyResult<()> {
        todo!()
    }

    pub fn ord(&self) -> PyResult<()> {
        todo!()
    }

    pub fn pow(&self) -> PyResult<()> {
        todo!()
    }

    pub fn print(&self) -> PyResult<()> {
        todo!()
    }

    pub fn property(&self) -> PyResult<()> {
        todo!()
    }

    // #[pyo3(signature = (*args))]
    // pub fn range(&self, py: Python, args: &Bound<'_, PyTuple>) -> PyResult<PyObject> {
    //     // We can do quick parsing based on argument count
    //     let (start, stop, step) = match args.len() {
    //         1 => {
    //             // range(stop)
    //             let stop_val: i64 = args.get_item(0)?.extract()?;
    //             (stop_val, None, None)
    //         }
    //         2 => {
    //             // range(start, stop)
    //             let start_val: i64 = args.get_item(0)?.extract()?;
    //             let stop_val: i64 = args.get_item(1)?.extract()?;
    //             (start_val, Some(stop_val), None)
    //         }
    //         3 => {
    //             // range(start, stop, step)
    //             let start_val: i64 = args.get_item(0)?.extract()?;
    //             let stop_val: i64 = args.get_item(1)?.extract()?;
    //             let step_val: i64 = args.get_item(2)?.extract()?;
    //             (start_val, Some(stop_val), Some(step_val))
    //         }
    //         _ => {
    //             return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
    //                 "range expected at most 3 arguments",
    //             ));
    //         }
    //     };

    //     let iter_obj = Py::new(py, RangeIterator::new(start, stop, step)?)?;
    //     Ok(iter_obj.to_object(py))
    // }

    pub fn repr(&self) -> PyResult<()> {
        todo!()
    }

    pub fn reversed(&self) -> PyResult<()> {
        todo!()
    }

    pub fn round(&self) -> PyResult<()> {
        todo!()
    }

    pub fn set(&self) -> PyResult<()> {
        todo!()
    }

    pub fn setattr(&self) -> PyResult<()> {
        todo!()
    }

    pub fn slice(&self) -> PyResult<()> {
        todo!()
    }

    // #[pyo3(signature = (iterable, *, key=None, reverse=false))]
    // pub fn sorted(
    //     &self,
    //     py: Python,
    //     iterable: &Bound<'_, PyAny>,
    //     key: Option<PyObject>,
    //     reverse: bool,
    // ) -> PyResult<PyObject> {
    //     let iter = PyIterator::from_object(iterable)?;

    //     // Collect items into a Rust Vec
    //     let mut items: Vec<PyObject> = Vec::new();
    //     for item in iter {
    //         items.push(item?.into());
    //     }

    //     if let Some(key_fn) = key {
    //         // If a key function is provided, do a "decorate-sort-undecorate":
    //         // 1. Map each item to (key(item), item).
    //         // 2. Sort by the key-part.
    //         // 3. Strip away the key-part afterwards.
    //         let mut decorated = Vec::with_capacity(items.len());
    //         for obj in items.into_iter() {
    //             let key_value = key_fn.call1(py, (obj.clone(),))?;
    //             decorated.push((key_value, obj));
    //         }

    //         // Sort in-place by key
    //         decorated.sort_by(|a, b| {
    //             // Compare a.0 (key_value) and b.0
    //             // Use Python’s rich comparison if needed
    //             let cmp_result = a.0.compare(&b.0);
    //             cmp_result.unwrap_or(std::cmp::Ordering::Equal)
    //         });

    //         if reverse {
    //             decorated.reverse();
    //         }

    //         // Undecorate
    //         items = decorated.into_iter().map(|(_, obj)| obj).collect();
    //     } else {
    //         // Sort directly (compare the objects themselves)
    //         items.sort_by(|a, b| a.compare(b).unwrap_or(std::cmp::Ordering::Equal));

    //         if reverse {
    //             items.reverse();
    //         }
    //     }

    //     // Return a new list
    //     Ok(PyList::new(py, &items).to_object(py))
    // }

    pub fn staticmethod(&self) -> PyResult<()> {
        todo!()
    }

    pub fn str(&self) -> PyResult<()> {
        todo!()
    }

    #[pyo3(signature = (iterable, start=0))]
    pub fn sum(&self, py: Python, iterable: &Bound<'_, PyAny>, start: i64) -> PyResult<PyObject> {
        // Turn iterable into an iterator
        let iter = PyIterator::from_object(iterable)?;

        // Accumulate values in a i64 for simplicity; real-world code
        // might handle floats or arbitrary numeric types.
        let mut total = start;

        for item in iter {
            let obj = item?;
            // For generality, consider extracting float, or call __add__ if dynamic
            let val: i64 = obj.extract()?;
            total = total.checked_add(val).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyOverflowError, _>("Integer overflow in sum()")
            })?;
        }

        Ok(total.to_object(py))
    }

    // pub fn super(&self) -> PyResult<()> {
    //     todo!()
    // }

    #[pyo3(signature = (iterable))]
    pub fn tuple(&self, py: Python, iterable: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let iter = PyIterator::from_object(iterable)?;
        let mut items = Vec::new();

        for item in iter {
            items.push(item?);
        }

        // Return a real PyTuple
        let py_tuple = PyTuple::new(py, items)?;
        Ok(py_tuple.to_object(py))
    }

    // pub fn type(&self) -> PyResult<()> {
    //     todo!()
    // }

    pub fn vars(&self) -> PyResult<()> {
        todo!()
    }

    #[pyo3(signature = (*iterables, strict=false))]
    pub fn zip(
        &self,
        py: Python,
        iterables: &Bound<'_, PyTuple>,
        strict: bool,
    ) -> PyResult<PyObject> {
        // Convert each argument to an actual iterator
        let mut iters = Vec::new();

        for obj in iterables.iter() {
            let iter = PyIterator::from_object(&obj)?;
            iters.push(iter.into());
        }

        let zip_iter = Py::new(py, ZipIterator::new(iters, strict))?;
        Ok(zip_iter.to_object(py))
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

fn calculate_hash_for_string(s: &str) -> i64 {
    let mut hash: i64 = 0;
    for byte in s.as_bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(*byte as i64);
    }
    hash
}

#[pymodule]
pub fn register_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    m.add_class::<Engine2>()?;
    m.add_class::<RustIterator>()?;
    // m.add_class::<PyIteratorWrapper>()?;
    // m.add_class::<MapState>()?;

    Ok(())
}
