use pyo3::prelude::*;

use crate::sequence::{Sequence, SequenceIterator};

#[pyclass]
pub struct TupleIterator {
    index: usize,
    length: usize,
    tuple: Vec<PyObject>,
}

#[pymethods]
impl TupleIterator {
    pub fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    pub fn __next__(mut slf: PyRefMut<'_, Self>) -> Option<PyObject> {
        let py = slf.py();
        if slf.index < slf.length {
            let item_ptr = slf.tuple[slf.index].as_ptr();
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

// #[derive(Clone)]
// #[pyclass]
// pub struct Tuple;

// #[pymethods]
// impl Tuple {
//     #[new]
//     pub fn new(elements: Vec<PyObject>) -> (Self, Sequence) {
//         (Tuple, Sequence { elements })
//     }

//     pub fn __repr__(&self, py: Python) -> String {
//         let reprs: Vec<String> = self
//             .elements
//             .iter()
//             .map(|obj| {
//                 obj.call_method0(py, "__repr__")
//                     .and_then(|repr_obj| repr_obj.extract::<String>(py))
//                     .unwrap_or_else(|_| "<error>".to_string())
//             })
//             .collect();
//         format!("Tuple([{}])", reprs.join(", "))
//     }

//     pub fn __iter__(
//         slf: PyRef<'_, Self>,
//         base: PyRef<'_, Sequence>,
//     ) -> PyResult<Py<SequenceIterator>> {
//         let py = slf.py();
//         let length = base.elements.len();
//         Py::new(
//             py,
//             SequenceIterator {
//                 index: 0,
//                 length,
//                 elements: base.elements.clone(),
//             },
//         )
//     }
// }

#[derive(Clone)]
#[pyclass]
pub struct Tuple {
    pub tuple: Vec<PyObject>,
}

#[pymethods]
impl Tuple {
    #[new]
    pub fn new(elements: Vec<PyObject>) -> Self {
        Tuple { tuple: elements }
    }

    pub fn __repr__(&self) -> String {
        Python::with_gil(|py| {
            let reprs: Vec<String> = self
                .tuple
                .iter()
                .map(|obj| {
                    obj.call_method0(py, "__repr__")
                        .and_then(|repr_obj| repr_obj.extract::<String>(py))
                        .unwrap_or_else(|_| "<error>".to_string())
                })
                .collect();

            format!("Tuple([{}])", reprs.join(", "))
        })
    }

    pub fn __iter__(slf: PyRef<'_, Self>) -> PyResult<Py<TupleIterator>> {
        let py = slf.py();
        let length = slf.tuple.len();
        Py::new(
            py,
            TupleIterator {
                index: 0,
                length,
                tuple: slf.tuple.clone(),
            },
        )
    }
}

#[pymodule]
pub fn register_tuple(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Tuple>()?;
    Ok(())
}
