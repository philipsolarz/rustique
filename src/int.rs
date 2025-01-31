use pyo3::exceptions::{PyTypeError, PyValueError, PyZeroDivisionError};
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyBytes, PyFloat, PyInt, PyString};
use pyo3::{ffi, prelude::*};
use rug::Integer;
use std::ops::{Div, Neg};
#[pyclass(name = "Int")]
pub struct Int {
    pub value: Integer,
    // base: isize,
}

#[pymethods]
impl Int {
    #[new]
    #[pyo3(signature = (val, base=10))]
    fn new(val: &Bound<'_, PyAny>, base: isize) -> PyResult<Self> {
        let value = convert_to_integer(val)?;
        Ok(Self { value })
    }

    fn __repr__(&self) -> String {
        self.value.to_string()
    }

    fn __str__(&self) -> String {
        self.value.to_string()
    }

    // fn __hash__(&self) -> u64 {
    //     self.value.mod_u(1 << 63) as u64
    // }

    fn __index__(&self, py: Python<'_>) -> PyResult<PyObject> {
        if self.value.is_zero() {
            return Ok(0i32.into_py(py));
        }

        let is_negative = if self.value.is_negative() { 1 } else { 0 };

        let digits: Vec<u8> = self.value.to_digits::<u8>(rug::integer::Order::Lsf);

        unsafe {
            let ptr = ffi::_PyLong_FromByteArray(
                digits.as_ptr() as *mut u8,
                digits.len(),
                1,
                is_negative,
            );
            if ptr.is_null() {
                Err(PyErr::fetch(py))
            } else {
                Ok(PyObject::from_owned_ptr(py, ptr))
            }
        }
    }

    fn __bool__(&self) -> bool {
        !self.value.is_zero()
    }

    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: &self.value + value,
            },
        )
    }

    fn __sub__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: &self.value - value,
            },
        )
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: &self.value * value,
            },
        )
    }

    fn __floordiv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        if value == 0 {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        Py::new(
            py,
            Self {
                value: self.value.clone().div(value),
            },
        )
    }

    fn __truediv__(&self, other: &Bound<'_, PyAny>) -> PyResult<f64> {
        let value = convert_to_integer(other)?;
        Ok(self.value.to_f64() / value.to_f64())
    }

    fn __radd__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        self.__add__(other)
    }

    fn __rsub__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: value - &self.value,
            },
        )
    }

    fn __rmul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        self.__mul__(other)
    }

    fn __rtruediv__(&self, other: &Bound<'_, PyAny>) -> PyResult<f64> {
        let value = convert_to_integer(other)?;
        Ok(value.to_f64() / self.value.to_f64())
    }

    fn __rfloordiv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        if self.value.is_zero() {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        Py::new(
            py,
            Self {
                value: value.div(self.value.clone()),
            },
        )
    }

    fn __iadd__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let value = convert_to_integer(other)?;
        self.value += value;
        Ok(())
    }

    fn __isub__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let value = convert_to_integer(other)?;
        self.value -= value;
        Ok(())
    }

    fn __imul__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let value = convert_to_integer(other)?;
        self.value *= value;
        Ok(())
    }

    fn __idiv__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let value = convert_to_integer(other)?;
        self.value /= value;
        Ok(())
    }

    fn __neg__(&self) -> Self {
        Self {
            value: self.value.clone().neg(),
        }
    }

    fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
        let other_val = convert_to_integer(other)?;
        Ok(match op {
            CompareOp::Eq => self.value == other_val,
            CompareOp::Ne => self.value != other_val,
            CompareOp::Lt => self.value < other_val,
            CompareOp::Le => self.value <= other_val,
            CompareOp::Gt => self.value > other_val,
            CompareOp::Ge => self.value >= other_val,
        })
    }

    fn __and__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: &self.value & value,
            },
        )
    }

    fn __or__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: &self.value | value,
            },
        )
    }

    fn __xor__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        Py::new(
            py,
            Self {
                value: &self.value ^ value,
            },
        )
    }

    fn __lshift__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let shift_amount = convert_to_integer(other)?;
        if shift_amount.is_negative() {
            return Err(PyValueError::new_err("negative shift count"));
        }
        let result = &self.value << shift_amount.to_usize_wrapping();
        Py::new(
            py,
            Self {
                value: result.into(),
            },
        )
    }

    fn __rshift__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let shift_amount = convert_to_integer(other)?;
        if shift_amount.is_negative() {
            return Err(PyValueError::new_err("negative shift count"));
        }
        let result = &self.value >> shift_amount.to_usize_wrapping();
        Py::new(
            py,
            Self {
                value: result.into(),
            },
        )
    }

    fn __mod__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        if value.is_zero() {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        Py::new(
            py,
            Self {
                value: &self.value % value,
            },
        )
    }

    fn __divmod__(&self, other: &Bound<'_, PyAny>) -> PyResult<(Py<Self>, Py<Self>)> {
        let py = other.py();
        let value = convert_to_integer(other)?;
        if value.is_zero() {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        let (quotient, remainder) = self.value.clone().div_rem(value);
        Ok((
            Py::new(py, Self { value: quotient })?,
            Py::new(py, Self { value: remainder })?,
        ))
    }

    fn __invert__(&self) -> Self {
        Self {
            value: !self.value.clone(),
        }
    }

    fn __abs__(&self) -> Self {
        Self {
            value: self.value.clone().abs(),
        }
    }

    fn __pos__(&self) -> Self {
        Self {
            value: self.value.clone(),
        }
    }

    fn __pow__(
        &self,
        exponent: &Bound<'_, PyAny>,
        modulus: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<Self>> {
        let py = exponent.py();
        let exp_val = convert_to_integer(exponent)?;

        if let Some(m) = modulus {
            let m_val = convert_to_integer(m)?;
            if m_val.is_zero() {
                return Err(PyZeroDivisionError::new_err(
                    "pow() 3rd argument cannot be 0",
                ));
            }

            if exp_val.is_negative() {
                return Err(PyValueError::new_err(
                    "pow() exponent must not be negative with modulus",
                ));
            }

            let result = self
                .value
                .clone()
                .pow_mod(&exp_val, &m_val)
                .map_err(|e| PyValueError::new_err(format!("modular exponent error: {e}")))?;
            return Py::new(py, Self { value: result });
        }

        // if exp_val.is_negative() {
        //     // We do float math, though for extremely large exponents this might
        //     // become 0.0. Python does the same (overflow to 0.0 or Inf).
        //     let base_f = self.value.to_f64();
        //     let e_f = exp_val.to_f64(); // negative
        //     let result = base_f.powf(e_f);
        //     return Ok(result.into_py(py));
        // }

        if exp_val.is_negative() {
            return Err(PyValueError::new_err(
                "negative exponent not supported (use custom Float if desired).",
            ));
        }

        let result = big_pow(&self.value, &exp_val);
        Py::new(py, Self { value: result })
    }

    fn to_bytes(&self, py: Python<'_>, order: &str) -> PyResult<PyObject> {
        let order = match order {
            "big" => rug::integer::Order::Msf,
            "little" => rug::integer::Order::Lsf,
            _ => return Err(PyValueError::new_err("order must be 'big' or 'little'")),
        };

        let digits: Vec<u8> = self.value.to_digits::<u8>(order);
        let bytes = PyBytes::new(py, &digits);
        Ok(bytes.to_object(py))
    }

    fn bit_length(&self) -> usize {
        self.value.significant_bits().try_into().unwrap()
    }
}

fn big_pow(base: &Integer, exponent: &Integer) -> Integer {
    let mut e = exponent.clone();
    let mut result = Integer::from(1);
    let mut cur = base.clone();
    while !e.is_zero() {
        if e.is_odd() {
            result *= &cur;
        }
        e >>= 1;
        cur.square_mut();
    }
    result
}

fn convert_to_integer(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
    if let Ok(int) = obj.downcast::<Int>() {
        return Ok(int.borrow().value.clone());
    }

    if let Ok(py_int) = obj.downcast::<PyInt>() {
        unsafe {
            let ptr = py_int.as_ptr();
            let num_bits = ffi::_PyLong_NumBits(ptr) as isize;
            if num_bits == -1 {
                return Err(PyErr::fetch(obj.py()));
            }

            let num_bytes = ((num_bits as usize) + 7) / 8;
            let mut buffer = vec![0u8; num_bytes];

            let res = ffi::_PyLong_AsByteArray(
                ptr as *mut ffi::PyLongObject,
                buffer.as_mut_ptr(),
                num_bytes,
                1,
                1,
            );

            if res == -1 {
                return Err(PyErr::fetch(obj.py()));
            }

            return Ok(Integer::from_digits(&buffer, rug::integer::Order::Lsf));
        }
    }

    if let Ok(py_float) = obj.downcast::<PyFloat>() {
        let val = py_float.extract::<f64>()?;

        if val.is_nan() {
            return Err(PyValueError::new_err("Cannot convert NaN to integer"));
        }
        if val.is_infinite() {
            return Err(PyValueError::new_err("Cannot convert infinity to integer"));
        }

        if val != val.trunc() {
            return Err(PyValueError::new_err(
                "Cannot convert float with fractional part to integer",
            ));
        }

        return Integer::from_f64(val.trunc())
            .ok_or_else(|| PyValueError::new_err("Float value out of range"));
    }

    if let Ok(rust_float) = obj.downcast::<crate::float::Float>() {
        let val = rust_float.borrow().value;
        if val.is_nan() {
            return Err(PyValueError::new_err("Cannot convert NaN to integer"));
        }
        if val.is_infinite() {
            return Err(PyValueError::new_err("Cannot convert infinity to integer"));
        }

        if val != val.trunc() {
            return Err(PyValueError::new_err(
                "Cannot convert float with fractional part to integer",
            ));
        }

        return Integer::from_f64(val.trunc())
            .ok_or_else(|| PyValueError::new_err("Float value out of range"));
    }

    if let Ok(py_str) = obj.downcast::<PyString>() {
        let s = py_str.to_str()?;
        return Ok(Integer::from_str_radix(s, 10)
            .map_err(|_| PyValueError::new_err("Invalid integer string"))?);
    }

    if let Ok(int_method) = obj.getattr("__int__") {
        let result = int_method.call0()?;
        return convert_to_integer(&result);
    }

    Err(PyTypeError::new_err(format!(
        "Unsupported type for integer conversion: {}",
        obj.get_type().name()?
    )))
}

#[pymodule]
pub fn register_int(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let version = m.py().version_info();
    if version.major != 3 || version.minor < 8 {
        return Err(PyErr::new::<PyValueError, _>(
            "This module requires Python 3.8+",
        ));
    }
    m.add_class::<Int>()?;
    Ok(())
}
