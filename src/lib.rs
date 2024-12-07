use std::borrow::{Borrow, BorrowMut};

use num_bigint::BigInt;
use pyo3::basic::CompareOp;
use pyo3::prelude::*;
use rug::{Float, Integer};
use pyo3::types::{
    PyAny, PyInt, 
    // PyList, 
    // PySequence, 
    // PyString
};
// use pyo3::exceptions::PyIndexError;
// use pyo3::basic::CompareOp; // for CompareOp::Eq
// use std::collections::hash_map::DefaultHasher;
// use std::hash::{Hash, Hasher};
// use rug::Integer;
// use num_bigint::BigInt;

#[derive(FromPyObject)]
#[pyclass]
struct int(BigInt);

#[pymethods]
impl int {
    #[new]
    fn new(value: i32) -> Self {
        int(Integer::from(value))
    }

    fn __repr__(&self) -> String {
        format!("int({})", self.0)
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    // fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
    //     if let Ok(other_instance) = other.downcast::<Self>() {
    //         Ok(self.0 == other_instance.borrow().0)
    //     } else if let Ok(other_int) = other.extract::<BigInt>() {
    //         Ok(self.0 == BigInt::from(other_int))
    //     } else {
    //         Ok(false)
    //     }
    // }

    fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => {
                if let Ok(other_instance) = other.downcast::<Self>() {
                    Ok(self.0 == other_instance.borrow().0)
                } else if let Ok(other_int) = other.extract::<BigInt>() {
                    Ok(self.0 == BigInt::from(other_int))
                } else {
                    Ok(false)
                }
            },
            CompareOp::Lt => {
                if let Ok(other_instance) = other.downcast::<Self>() {
                    Ok(self.0 < other_instance.borrow().0)
                } else if let Ok(other_int) = other.extract::<BigInt>() {
                    Ok(self.0 < BigInt::from(other_int))
                } else {
                    Ok(false)
                }
            },
            CompareOp::Le => {
                if let Ok(other_instance) = other.downcast::<Self>() {
                    Ok(self.0 <= other_instance.borrow().0)
                } else if let Ok(other_int) = other.extract::<BigInt>() {
                    Ok(self.0 <= BigInt::from(other_int))
                } else {
                    Ok(false)
                }
            },
            CompareOp::Ne => {
                if let Ok(other_instance) = other.downcast::<Self>() {
                    Ok(self.0 != other_instance.borrow().0)
                } else if let Ok(other_int) = other.extract::<BigInt>() {
                    Ok(self.0 != BigInt::from(other_int))
                } else {
                    Ok(false)
                }
            },
            CompareOp::Ge => {
                if let Ok(other_instance) = other.downcast::<Self>() {
                    Ok(self.0 >= other_instance.borrow().0)
                } else if let Ok(other_int) = other.extract::<BigInt>() {
                    Ok(self.0 >= BigInt::from(other_int))
                } else {
                    Ok(false)
                }
            },
            CompareOp::Gt => {
                if let Ok(other_instance) = other.downcast::<Self>() {
                    Ok(self.0 > other_instance.borrow().0)
                } else if let Ok(other_int) = other.extract::<BigInt>() {
                    Ok(self.0 > BigInt::from(other_int))
                } else {
                    Ok(false)
                }
            },
        }
    }
    
    // fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
    //     // Attempt to extract a reference to Self from the other object
    //     if let Ok(other_instance) = other.extract::<Self>() {
    //         // Compare the internal values for equality
    //         Ok(self.0 == other_instance.0)
    //     } else {
    //         // If extraction fails, the objects are not equal
    //         Ok(false)
    //     }
    // }

    // fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
    //     op.matches(self.0.cmp(&other.0))
    // }

    // fn __bool__(&self) -> bool {
    //     self.0 != 0
    // }
}

// #[pyclass]
// struct num(i32);

// #[pymethods]
// impl num {
//     #[new]
//     fn new(value: i32) -> Self {
//         Self(value)
//     }

//     fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
//         let class_name: Bound<'_, PyString> = slf.get_type().qualname()?;
//         Ok(format!("{}({})", class_name, slf.borrow().0))
//     }

//     fn __str__(&self) -> String {
//         self.0.to_string()
//     }

//     fn __hash__(&self) -> u64 {
//         let mut hasher = DefaultHasher::new();
//         self.0.hash(&mut hasher);
//         hasher.finish()
//     }

//     fn __richcmp__(&self, other: &Self, op: CompareOp) -> PyResult<bool> {
//         match op {
//             CompareOp::Lt => Ok(self.0 < other.0),
//             CompareOp::Le => Ok(self.0 <= other.0),
//             CompareOp::Eq => Ok(self.0 == other.0),
//             CompareOp::Ne => Ok(self.0 != other.0),
//             CompareOp::Ge => Ok(self.0 >= other.0),
//             CompareOp::Gt => Ok(self.0 > other.0),
//         }
//     }

//     // fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
//     //     op.matches(self.0.cmp(&other.0))
//     // }

//     fn __bool__(&self) -> bool {
//         self.0 != 0
//     }
// }


// #[derive(FromPyObject)]
// #[pyclass]
// struct list {
//     iterable: Vec<PyObject>,
// }


// #[pymethods]
// impl list {
//     #[new]
//     #[pyo3(signature = (iterable=None))]
//     fn new(iterable: Option<Vec<PyObject>>) -> Self {
//         list {
//             iterable: iterable.unwrap_or_default(),
//         }
//     }

    // fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
    //     let a = other.downcast::<list>()?;
    //     // let b = self.__eq__(a).is_ok();
    //     let b = self.iterable.iter().eq(other.iterable.iter());

    //     Ok(true)
        
    // }

    // fn __eq__(&self, other: PyObject) -> bool {
    //     Python::with_gil(|py| {
    //         // Attempt to downcast the other object to a `list`
    //         if let Ok(list_bound) = other.downcast_bound::<list>(py) {
    //             // Unbind to get the owned Py<list>
    //             let list_abc: Py<list> = list_bound.clone().unbind();
    
    //             // Extract the Rust `list` instance
    //             let other_list = list_abc.borrow(py);
    
                
    //             // Compare the `iterable` of both lists
    //             self.iterable == other_list.iterable
    //         } else {
    //             false // Return false if the other object is not a `list`
    //         }
    //     })
    // }
// }
            
            // if let Ok(other) = other.extract::<list>(py) {
            //     return self.iterable == other.iterable;
            // }

            // let class_bound = class.downcast_bound::<Class>(py)?;

            // Alternatively you can get a `PyRefMut` directly
            // let class_ref: PyRefMut<'_, Class> = class.extract(py)?;
            // assert_eq!(class_ref.i, 1);

//             if other.downcast_bound::<list>(py).is_ok() {

//                 return true;
//             }

//             if other.downcast_bound::<PyList>(py).is_ok() {
//                 return true;
//             }

//             if other.downcast_bound::<PySequence>(py).is_ok() {
//                 return true;
//             }

//             false
//         })
//     }
// }


// #[pyclass]
// struct List {
//     elements: Vec<PyObject>,
// }



// #[pymethods]
// impl List {
//     #[new]
//     #[pyo3(signature = (elements=None))]
//     fn new(py: Python, elements: Option<&Bound<PyAny>>) -> PyResult<Self> {
//         let mut vec = Vec::new();

//         if let Some(iterable) = elements {
//             if let Ok(seq) = iterable.downcast::<PySequence>() {
//                 for item in seq.try_iter()? {
//                     vec.push(item?.into_pyobject(py)?.unbind());
//                 }
//             } else {
//                 for item in iterable.try_iter()? {
//                     vec.push(item?.into_pyobject(py)?.unbind());
//                 }
//             }
//         }

//         Ok(List { elements: vec })
//     }
// }
    // /// Appends a new element to the list.
    // fn append(&mut self, element: PyObject) {
    //     self.elements.push(element);
    // }

    // fn __len__(&self) -> usize {
    //     self.elements.len()
    // }


    // fn __getitem__(&self, index: isize, py: Python) -> PyResult<PyObject> {
    //     let idx = if index < 0 {
    //         self.elements.len().checked_sub((-index) as usize)
    //     } else {
    //         Some(index as usize)
    //     }.ok_or_else(|| PyIndexError::new_err("Index out of range"))?;

    //     self.elements.get(idx)
    //         .cloned()
    //         .ok_or_else(|| PyIndexError::new_err("Index out of range"))
    // }

    // fn __setitem__(&mut self, index: isize, value: PyObject) -> PyResult<()> {
    //     let idx = if index < 0 {
    //         self.elements.len().checked_sub((-index) as usize)
    //     } else {
    //         Some(index as usize)
    //     }.ok_or_else(|| PyIndexError::new_err("Index out of range"))?;

    //     if idx < self.elements.len() {
    //         self.elements[idx] = value;
    //         Ok(())
    //     } else {
    //         Err(PyIndexError::new_err("Index out of range"))
    //     }
    // }

    // fn __delitem__(&mut self, index: isize) -> PyResult<()> {
    //     let idx = if index < 0 {
    //         self.elements.len().checked_sub((-index) as usize)
    //     } else {
    //         Some(index as usize)
    //     }.ok_or_else(|| PyIndexError::new_err("Index out of range"))?;

    //     if idx < self.elements.len() {
    //         self.elements.remove(idx);
    //         Ok(())
    //     } else {
    //         Err(PyIndexError::new_err("Index out of range"))
    //     }
    // }

    // fn __contains__(&self, py: Python, element: PyObject) -> PyResult<bool> {
    //     for obj in &self.elements {
    //         if obj.downcast_bound::<PyAny>(py)?.rich_compare(element.downcast_bound::<PyAny>(py)?, CompareOp::Eq)?.is_truthy()? {
    //             return Ok(true);
    //         }
    //     }
    //     Ok(false)
    // }

    // fn __str__(&self, py: Python) -> PyResult<String> {
    //     let mut s = String::from("[");
    //     for (i, obj) in self.elements.iter().enumerate() {
    //         if i > 0 {
    //             s.push_str(", ");
    //         }
    //         let obj_ref = obj.as_ref(py); // Obtain a &PyAny reference
    //         let obj_str = obj_ref.repr()?.to_str()?; // Call repr() and convert to &str
    //         s.push_str(obj_str);
    //     }
    //     s.push(']');
    //     Ok(s)
    // }
    

    // fn __repr__(&self, py: Python) -> PyResult<String> {
    //     Ok(format!("List({})", self.__str__(py)?))
    // }

    /// Concatenates two lists.
    // fn __add__(&self, other: &List) -> List {
    //     let mut new_elements = self.elements.clone();
    //     new_elements.extend_from_slice(&other.elements);
    //     List {
    //         elements: new_elements,
    //     }
    // }

    // fn __mul__(&self, times: isize) -> List {
    //     let mut new_elements = Vec::new();
    //     for _ in 0..times.max(0) {
    //         new_elements.extend(self.elements.iter().cloned());
    //     }
    //     List {
    //         elements: new_elements,
    //     }
    // }
// }



#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<int>()?;
    // m.add_class::<num>()?;
    // m.add_class::<list>()?;
    Ok(())
}