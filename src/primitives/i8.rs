use pyo3::exceptions::{PyOverflowError, PyTypeError, PyZeroDivisionError};
use pyo3::prelude::*;
use pyo3::class::basic::CompareOp;
use pyo3::types::{PyBool, PyFloat, PyInt, PyString};

fn py_any_to_i8(obj: &Bound<'_, PyAny>) -> PyResult<i8> {
    if let Ok(i) = obj.extract::<i8>() {
        return Ok(i);
    }
    if let Ok(i) = obj.downcast::<I8>() {
        return Ok(i.borrow().0);
    }
    
    if let Ok(i) = obj.downcast::<PyInt>() {
        return Ok(i.extract::<i8>()?);
    }
    
    if let Ok(i) = obj.downcast::<PyFloat>() {
        return Ok(i.extract::<f64>()? as i8);
    }

    if let Ok(i) = obj.downcast::<PyBool>() {
        return Ok(i.extract::<bool>()? as i8);
    }

    if let Ok(i) = obj.downcast::<PyString>() {
        return Ok(i.extract::<String>()?.parse::<i8>()?);
    }

    Err(PyTypeError::new_err("Could not convert to i8"))
}

#[pyclass(name="i8")]
pub struct I8(i8);

#[pymethods]
impl I8 {
    #[new]
    pub fn new(#[pyo3(from_py_with = "py_any_to_i8")] value: i8) -> Self {
        I8(value)
    }

    pub fn __repr__(&self) -> String {
        format!("{}", self.0)
    }

    #[getter]
    pub fn value(&self) -> i8 {
        self.0
    }

    #[setter]
    pub fn set_value(&mut self, value: i8) {
        self.0 = value;
    }
    
    pub fn __richcmp__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.0 == other),
            CompareOp::Ne => Ok(self.0 != other),
            CompareOp::Lt => Ok(self.0 < other),
            CompareOp::Le => Ok(self.0 <= other),
            CompareOp::Gt => Ok(self.0 > other),
            CompareOp::Ge => Ok(self.0 >= other),
        }
    }

    pub fn __add__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_add(other) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during addition")),
        }
    }

    pub fn checked_add(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_add(other) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during addition")),
        }
    }

    pub fn wrapping_add(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.wrapping_add(other)
    }

    pub fn overflowing_add(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> (i8, bool) {
        self.0.overflowing_add(other)
    }

    pub fn saturating_add(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.saturating_add(other)
    }

    pub fn __sub__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_sub(other) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during addition")),
        }
    }

    pub fn checked_sub(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_sub(other) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during addition")),
        }
    }

    pub fn wrapping_sub(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.wrapping_sub(other)
    }

    pub fn overflowing_sub(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> (i8, bool) {
        self.0.overflowing_sub(other)
    }

    pub fn saturating_sub(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.saturating_sub(other)
    }

    pub fn __mul__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_mul(other) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during addition")),
        }
    }

    pub fn checked_mul(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_mul(other) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during addition")),
        }
    }

    pub fn wrapping_mul(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.wrapping_mul(other)
    }

    pub fn overflowing_mul(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> (i8, bool) {
        self.0.overflowing_mul(other)
    }

    pub fn saturating_mul(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.saturating_mul(other)
    }

    pub fn __truediv__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<f64> {
        if other == 0 {
            return Err(PyZeroDivisionError::new_err("Division by zero"));
        }
        Ok(self.0 as f64 / other as f64)
    }
    
    pub fn checked_truediv(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> Option<f64> {
        if other == 0 {
            return None;
        }
        Some(self.0 as f64 / other as f64)
    }

    pub fn __floordiv__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<Self> {
        if other == 0 {
            return Err(PyZeroDivisionError::new_err("Division by zero"));
        }
        if self.0 == i8::MIN && other == -1 {
            return Err(PyOverflowError::new_err("Overflow occurred in floor division"));
        }
        Ok(I8(self.0 / other))
    }

    pub fn checked_floordiv(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> Option<i8> {
        self.0.checked_div(other)
    }

    pub fn wrapping_floordiv(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        if other == 0 {
            return Err(PyZeroDivisionError::new_err("Division by zero"));
        }
        Ok(self.0.wrapping_div(other))
    }

    pub fn overflowing_floordiv(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<(i8, bool)> {
        if other == 0 {
            return Err(PyZeroDivisionError::new_err("Division by zero"));
        }
        let (result, overflow) = self.0.overflowing_div(other);
        Ok((result, overflow))
    }

    pub fn __mod__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        if other == 0 {
            return Err(PyZeroDivisionError::new_err("Modulo by zero"));
        }
        Ok(self.0 % other)
    }

    pub fn checked_rem(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> Option<i8> {
        if other == 0 {
            None
        } else {
            Some(self.0 % other)
        }
    }

    pub fn wrapping_rem(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.wrapping_rem(other)
    }

    pub fn overflowing_rem(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> (i8, bool) {
        self.0.overflowing_rem(other)
    }

    pub fn __neg__(&self) -> PyResult<i8> {
        if self.0 == i8::MIN {
            return Err(PyOverflowError::new_err(
                "Overflow occurred during negation",
            ));
        }
        Ok(-self.0)
    }

    pub fn __pos__(&self) -> i8 {
        self.0
    }

    pub fn __abs__(&self) -> i8 {
        self.0.abs()
    }

    pub fn __and__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0 & other
    }

    pub fn __or__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0 | other
    }

    pub fn __xor__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0 ^ other
    }

    pub fn __lshift__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_shl(other as u32) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during left shift")),
        }
    }

    pub fn checked_shl(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_shl(other as u32) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during left shift")),
        }
    }

    pub fn wrapping_shl(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.wrapping_shl(other as u32)
    }

    pub fn overflowing_shl(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> (i8, bool) {
        self.0.overflowing_shl(other as u32)
    }

    pub fn __rshift__(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_shr(other as u32) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during right shift")),
        }
    }

    pub fn checked_shr(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> PyResult<i8> {
        match self.0.checked_shr(other as u32) {
            Some(result) => Ok(result),
            None => Err(PyOverflowError::new_err("Overflow occurred during right shift")),
        }
    }

    pub fn wrapping_shr(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> i8 {
        self.0.wrapping_shr(other as u32)
    }

    pub fn overflowing_shr(&self, #[pyo3(from_py_with = "py_any_to_i8")] other: i8) -> (i8, bool) {
        self.0.overflowing_shr(other as u32)
    }

    pub fn __invert__(&self) -> i8 {
        !self.0
    }

    pub fn count_ones(&self) -> u32 {
        self.0.count_ones()
    }

    pub fn leading_zeros(&self) -> u32 {
        self.0.leading_zeros()
    }

    pub fn trailing_zeros(&self) -> u32 {
        self.0.trailing_zeros()
    }

    pub fn rotate_left(&self, n: u32) -> i8 {
        self.0.rotate_left(n)
    }

    pub fn rotate_right(&self, n: u32) -> i8 {
        self.0.rotate_right(n)
    }

    pub fn swap_bytes(&self) -> i8 {
        self.0.swap_bytes()
    }

    pub fn to_be(&self) -> i8 {
        self.0.to_be()
    }

    pub fn to_le(&self) -> i8 {
        self.0.to_le()
    }

    #[staticmethod]
    pub fn from_be(#[pyo3(from_py_with = "py_any_to_i8")] be: i8) -> i8 {
        i8::from_be(be)
    }

    #[staticmethod]
    pub fn from_le(#[pyo3(from_py_with = "py_any_to_i8")] le: i8) -> i8 {
        i8::from_le(le)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.to_be_bytes().to_vec()
    }

    #[staticmethod]
    pub fn from_bytes(bytes: Vec<u8>) -> i8 {
        let mut array = [0; 1];
        let bytes_len = bytes.len();
        array.copy_from_slice(&bytes[0..bytes_len.min(1)]);
        i8::from_be_bytes(array)
    }

    pub fn bit_length(&self) -> u32 {
        self.0.leading_zeros() + self.0.count_ones()
    }

    #[staticmethod]
    pub fn zero() -> i8 {
        0
    }

    #[staticmethod]
    pub fn one() -> i8 {
        1
    }

    #[staticmethod]
    pub fn min_value() -> i8 {
        i8::MIN
    }

    #[staticmethod]
    pub fn max_value() -> i8 {
        i8::MAX
    }

    pub fn is_positive(&self) -> bool {
        self.0.is_positive()
    }

    pub fn is_negative(&self) -> bool {
        self.0.is_negative()
    }

}

pub fn register_i8(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<I8>()?;
    Ok(())
}
