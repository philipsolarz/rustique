use pyo3::exceptions::{PyIndexError, PyTypeError, PyValueError};
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyInt, PySlice, PyString};
use pyo3::{prelude::*, PyResult};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[pyclass(name = "Str")]
struct Str {
    value: String,
}

#[pymethods]
impl Str {
    #[new]
    fn new(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            value: convert_to_string(obj)?,
        })
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        let py_str = PyString::new(py, &self.value);
        py_str.repr().unwrap().to_str().unwrap().to_owned()
    }

    fn __str__(&self) -> &str {
        &self.value
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.value.hash(&mut hasher);
        hasher.finish()
    }

    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        let other_str = check_str_type(other)?;
        Ok(Self {
            value: format!("{}{}", self.value, other_str),
        })
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        let count = convert_to_count(other)?;
        Ok(Self {
            value: self.value.repeat(count),
        })
    }

    fn __rmul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        self.__mul__(other)
    }

    fn __len__(&self) -> usize {
        self.value.chars().count()
    }

    fn __getitem__(&self, idx: &Bound<'_, PyAny>) -> PyResult<Self> {
        let len_chars = self.value.chars().count();

        if let Ok(slice) = idx.downcast::<PySlice>() {
            let slice_info = slice.indices(len_chars as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;
            // let slice_len = slice_info.slicelength;

            if step != 1 {
                return Err(PyValueError::new_err("slice step not implemented"));
            }
            let chars: Vec<char> = self.value.chars().collect();
            let result = chars[start as usize..stop as usize]
                .iter()
                .collect::<String>();
            Ok(Self { value: result })
        } else {
            let idx = convert_to_index(idx, len_chars)?;
            let ch = self
                .value
                .chars()
                .nth(idx)
                .ok_or_else(|| PyIndexError::new_err("string index out of range"))?;
            Ok(Self {
                value: ch.to_string(),
            })
        }
    }

    fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
        let other_str = check_str_type(other)?;
        Ok(match op {
            CompareOp::Eq => self.value == other_str,
            CompareOp::Ne => self.value != other_str,
            CompareOp::Lt => self.value < other_str,
            CompareOp::Le => self.value <= other_str,
            CompareOp::Gt => self.value > other_str,
            CompareOp::Ge => self.value >= other_str,
        })
    }

    fn upper(&self) -> Self {
        Self {
            value: self.value.to_uppercase(),
        }
    }

    fn lower(&self) -> Self {
        Self {
            value: self.value.to_lowercase(),
        }
    }

    fn strip(&self) -> Self {
        Self {
            value: self.value.trim().to_string(),
        }
    }

    fn split(&self, py: Python<'_>) -> PyResult<Vec<Py<Self>>> {
        self.value
            .split_whitespace()
            .map(|s| {
                Py::new(
                    py,
                    Self {
                        value: s.to_string(),
                    },
                )
            })
            .collect()
    }

    fn startswith(&self, prefix: &Bound<'_, PyAny>) -> PyResult<bool> {
        let prefix_str = check_str_type(prefix)?;
        Ok(self.value.starts_with(&prefix_str))
    }

    fn endswith(&self, suffix: &Bound<'_, PyAny>) -> PyResult<bool> {
        let suffix_str = check_str_type(suffix)?;
        Ok(self.value.ends_with(&suffix_str))
    }

    fn replace(&self, old: &Bound<'_, PyAny>, new: &Bound<'_, PyAny>) -> PyResult<Self> {
        let old_str = check_str_type(old)?;
        let new_str = check_str_type(new)?;
        Ok(Self {
            value: self.value.replace(&old_str, &new_str),
        })
    }

    fn join(&self, iterable: &Bound<'_, PyAny>) -> PyResult<Self> {
        let elements = iterable
            .try_iter()?
            .map(|item| Ok(check_str_type(&item?)?))
            .collect::<PyResult<Vec<String>>>()?;
        Ok(Self {
            value: elements.join(&self.value),
        })
    }

    fn encode(&self, encoding: &str, _errors: &str) -> PyResult<Vec<u8>> {
        if encoding.to_lowercase() != "utf-8" {
            return Err(PyValueError::new_err("Only UTF-8 encoding is supported"));
        }
        Ok(self.value.as_bytes().to_vec())
    }
}

fn convert_to_string(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(obj.to_string())
}

fn check_str_type(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(s) = obj.downcast::<Str>() {
        Ok(s.borrow().value.clone())
    } else if let Ok(s) = obj.downcast::<PyString>() {
        Ok(s.to_str()?.to_owned())
    } else {
        let type_name = obj.get_type().name()?;
        Err(PyTypeError::new_err(format!(
            "Expected string, got {}",
            type_name
        )))
    }
}

fn convert_to_count(obj: &Bound<'_, PyAny>) -> PyResult<usize> {
    let count = if let Ok(py_int) = obj.downcast::<PyInt>() {
        py_int.extract::<isize>()?
    } else {
        let type_name = obj.get_type().name()?;
        return Err(PyTypeError::new_err(format!(
            "can't multiply sequence by non-int of type '{}'",
            type_name
        )));
    };
    Ok(if count < 0 { 0 } else { count as usize })
}

fn convert_to_index(idx: &Bound<'_, PyAny>, len: usize) -> PyResult<usize> {
    let index = if let Ok(py_int) = idx.downcast::<PyInt>() {
        py_int.extract::<isize>()?
    } else {
        let type_name = idx.get_type().name()?;
        return Err(PyTypeError::new_err(format!(
            "string indices must be integers, not {}",
            type_name
        )));
    };

    let adjusted = if index < 0 {
        index + len as isize
    } else {
        index
    };

    if adjusted < 0 || adjusted >= len as isize {
        Err(PyIndexError::new_err("string index out of range"))
    } else {
        Ok(adjusted as usize)
    }
}

#[pymodule]
pub fn register_str(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Str>()?;
    Ok(())
}
