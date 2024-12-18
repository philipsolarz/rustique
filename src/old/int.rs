use std::borrow::Borrow;

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use rug::Integer;
use pyo3::ffi;
use pyo3::types::{PyAny, PyFloat, PyInt};

#[pyclass]
pub struct int(Integer);

#[pymethods]
impl int {
    #[new]
    fn new(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(val) = obj.downcast::<Self>() {
            return Ok(Self(val.borrow().0.clone()));
        }
        if let Ok(val) = obj.extract::<i64>() {
            return Ok(Self(Integer::from(val)));
        }
        
        if let Ok(py_int) = obj.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = py_int.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let result = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(result));
            }
        }

        if let Ok(py_float) = obj.downcast::<PyFloat>() {
            let float_value = py_float.extract::<f64>()?;
            return Ok(Self(Integer::from(float_value as i64)));
            // if float_value.fract() == 0.0 {
            //     return Ok(Self(Integer::from(float_value as i64)));
            // } else {
            //     return Err(PyTypeError::new_err(
            //         "Cannot convert float with fractional part to Integer",
            //     ));
            // }
        }

        Err(PyTypeError::new_err(
            "Provided object cannot be converted to rug::Integer. Expected a Python int.",
        ))
    }

    fn __repr__(&self) -> String {
        format!("int({})", self.0)
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(self.0 == other.borrow().0);
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(self.0 == Integer::from(other));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(self.0 == other);
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(self.0 == Integer::from(float_value as i64));
            } else {
                return Ok(false);
            }
        }

        Ok(false)
    }

    fn __neq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(!self.__eq__(other)?)
    }

    fn __lt__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(self.0 < other.borrow().0);
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(self.0 < Integer::from(other));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(self.0 < other);
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(self.0 < Integer::from(float_value as i64));
            } else {
                return Ok(false);
            }
        }

        Ok(false)
    }

    fn __le__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.__lt__(other)? || self.__eq__(other)?)
    }

    fn __gt__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(!self.__le__(other)?)
    }

    fn __ge__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(!self.__lt__(other)?)
    }

    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(Self(self.0.clone() + other.borrow().0.clone()));
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(Self(self.0.clone() + Integer::from(other)));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(self.0.clone() + other));
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(Self(self.0.clone() + Integer::from(float_value as i64)));
            } else {
                return Err(PyTypeError::new_err(
                    "Cannot add float with fractional part to Integer",
                ));
            }
        }

        Err(PyTypeError::new_err(
            "Cannot add provided object to Integer. Expected a Python int.",
        ))
    }

    fn __sub__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(Self(self.0.clone() - other.borrow().0.clone()));
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(Self(self.0.clone() - Integer::from(other)));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(self.0.clone() - other));
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(Self(self.0.clone() - Integer::from(float_value as i64)));
            } else {
                return Err(PyTypeError::new_err(
                    "Cannot subtract float with fractional part from Integer",
                ));
            }
        }

        Err(PyTypeError::new_err(
            "Cannot subtract provided object from Integer. Expected a Python int.",
        ))
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(Self(self.0.clone() * other.borrow().0.clone()));
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(Self(self.0.clone() * Integer::from(other)));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(self.0.clone() * other));
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(Self(self.0.clone() * Integer::from(float_value as i64)));
            } else {
                return Err(PyTypeError::new_err(
                    "Cannot multiply Integer by float with fractional part",
                ));
            }
        }

        Err(PyTypeError::new_err(
            "Cannot multiply provided object with Integer. Expected a Python int.",
        ))
    }

    fn __truediv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(Self(self.0.clone() / other.borrow().0.clone()));
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(Self(self.0.clone() / Integer::from(other)));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(self.0.clone() / other));
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(Self(self.0.clone() / Integer::from(float_value as i64)));
            } else {
                return Err(PyTypeError::new_err(
                    "Cannot divide Integer by float with fractional part",
                ));
            }
        }

        Err(PyTypeError::new_err(
            "Cannot divide provided object by Integer. Expected a Python int.",
        ))
    }

    fn __floordiv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(Self(self.0.clone() / other.borrow().0.clone()));
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(Self(self.0.clone() / Integer::from(other)));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(self.0.clone() / other));
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(Self(self.0.clone() / Integer::from(float_value as i64)));
            } else {
                return Err(PyTypeError::new_err(
                    "Cannot divide Integer by float with fractional part",
                ));
            }
        }

        Err(PyTypeError::new_err(
            "Cannot divide provided object by Integer. Expected a Python int.",
        ))
    }

    fn __mod__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other) = other.downcast::<Self>() {
            return Ok(Self(self.0.clone() % other.borrow().0.clone()));
        }
        if let Ok(other) = other.extract::<i64>() {
            return Ok(Self(self.0.clone() % Integer::from(other)));
        }
        if let Ok(other) = other.downcast::<PyInt>() {
            unsafe {
                let py_long_ptr = other.as_ptr() as *mut ffi::PyLongObject;

                let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
                let num_bytes = ((num_bits + 7) / 8) as usize;
    
                let mut buffer = vec![0u8; num_bytes];
    
                let res = ffi::_PyLong_AsByteArray(
                    py_long_ptr as *mut _,
                    buffer.as_mut_ptr(),
                    num_bytes,
                    0,
                    1,
                );
    
                if res == -1 {
                    return Err(PyTypeError::new_err("Failed to extract integer bytes"));
                }
    
                let other = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

                return Ok(Self(self.0.clone() % other));
            }
        }
        if let Ok(other) = other.downcast::<PyFloat>() {
            let float_value = other.extract::<f64>()?;

            if float_value.fract() == 0.0 {
                return Ok(Self(self.0.clone() % Integer::from(float_value as i64)));
            } else {
                return Err(PyTypeError::new_err(
                    "Cannot modulo Integer by float with fractional part",
                ));
            }
        }

        Err(PyTypeError::new_err(
            "Cannot modulo provided object by Integer. Expected a Python int.",
        ))
    }


}



