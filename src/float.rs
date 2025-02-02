use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError, PyZeroDivisionError};
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyFloat, PyInt, PyString, PyTuple, PyType};
use pyo3::{prelude::*, PyObject};
use rug::Integer;
use std::str::FromStr;

#[pyclass(name = "Float")]
pub struct Float {
    pub value: f64,
}

#[pymethods]
impl Float {
    #[new]
    #[pyo3(signature = (val = None))]
    fn new(val: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let value = match val {
            None => 0.0,
            Some(obj) => {
                if obj.is_none() {
                    0.0
                } else {
                    convert_to_float(obj)?
                }
            }
        };
        Ok(Self { value })
    }

    fn __repr__(&self) -> String {
        let s = self.value.to_string();
        if self.value.fract() == 0.0 && !s.contains('e') && !s.contains('E') {
            format!("{}.0", s)
        } else {
            s
        }
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }

    fn __bool__(&self) -> bool {
        self.value != 0.0
    }

    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        Ok(Py::new(
            py,
            Self {
                value: self.value + other_val,
            },
        )?)
    }

    fn __sub__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        Ok(Py::new(
            py,
            Self {
                value: self.value - other_val,
            },
        )?)
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        Ok(Py::new(
            py,
            Self {
                value: self.value * other_val,
            },
        )?)
    }

    fn __truediv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        if other_val == 0.0 {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        Ok(Py::new(
            py,
            Self {
                value: self.value / other_val,
            },
        )?)
    }

    fn __floordiv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        if other_val == 0.0 {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        let result = (self.value / other_val).floor();
        Ok(Py::new(py, Self { value: result })?)
    }

    fn __radd__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        self.__add__(other)
    }

    fn __rsub__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        Ok(Py::new(
            py,
            Self {
                value: other_val - self.value,
            },
        )?)
    }

    fn __rmul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        self.__mul__(other)
    }

    fn __rtruediv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        if self.value == 0.0 {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        Ok(Py::new(
            py,
            Self {
                value: other_val / self.value,
            },
        )?)
    }

    fn __rfloordiv__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<Self>> {
        let py = other.py();
        let other_val = convert_to_float(other)?;
        if self.value == 0.0 {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        let result = (other_val / self.value).floor();
        Ok(Py::new(py, Self { value: result })?)
    }

    fn __iadd__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let other_val = convert_to_float(other)?;
        self.value += other_val;
        Ok(())
    }

    fn __isub__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let other_val = convert_to_float(other)?;
        self.value -= other_val;
        Ok(())
    }

    fn __imul__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let other_val = convert_to_float(other)?;
        self.value *= other_val;
        Ok(())
    }

    fn __itruediv__(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        let other_val = convert_to_float(other)?;
        if other_val == 0.0 {
            return Err(PyZeroDivisionError::new_err("division by zero"));
        }
        self.value /= other_val;
        Ok(())
    }

    fn __neg__(&self) -> Self {
        Self { value: -self.value }
    }

    fn __abs__(&self) -> Self {
        Self {
            value: self.value.abs(),
        }
    }

    fn __pos__(&self) -> Self {
        Self { value: self.value }
    }

    fn __pow__(
        &self,
        exponent: &Bound<'_, PyAny>,
        modulus: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<Self>> {
        if modulus.is_some() {
            return Err(PyTypeError::new_err(
                "pow() 3rd argument not allowed unless all arguments are integers",
            ));
        }
        let py = exponent.py();
        let exponent_val = convert_to_float(exponent)?;
        let result = self.value.powf(exponent_val);
        Ok(Py::new(py, Self { value: result })?)
    }

    fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
        let other_val = convert_to_float(other)?;
        Ok(match op {
            CompareOp::Eq => self.value == other_val,
            CompareOp::Ne => self.value != other_val,
            CompareOp::Lt => self.value < other_val,
            CompareOp::Le => self.value <= other_val,
            CompareOp::Gt => self.value > other_val,
            CompareOp::Ge => self.value >= other_val,
        })
    }

    fn __round__(&self, py: Python<'_>, ndigits: Option<i32>) -> PyResult<PyObject> {
        let ndigits = ndigits.unwrap_or(0);
        let factor = 10.0f64.powi(ndigits);
        let rounded = (self.value * factor).round() / factor;
        if ndigits <= 0 {
            let int_value = Integer::from_f64(rounded)
                .ok_or_else(|| PyValueError::new_err("rounded value out of range"))?;
            Ok(Py::new(py, crate::int::Int { value: int_value })?.into_py(py))
        } else {
            Ok(Py::new(py, Self { value: rounded })?.into_py(py))
        }
    }

    fn __trunc__(&self, py: Python<'_>) -> PyResult<PyObject> {
        let truncated = self.value.trunc();
        let int_value = Integer::from_f64(truncated)
            .ok_or_else(|| PyValueError::new_err("truncated value out of range"))?;
        Ok(Py::new(py, crate::int::Int { value: int_value })?.into_py(py))
    }

    fn __floor__(&self, py: Python<'_>) -> PyResult<PyObject> {
        let floored = self.value.floor();
        let int_value = Integer::from_f64(floored)
            .ok_or_else(|| PyValueError::new_err("floored value out of range"))?;
        Ok(Py::new(py, crate::int::Int { value: int_value })?.into_py(py))
    }

    fn __ceil__(&self, py: Python<'_>) -> PyResult<PyObject> {
        let ceiled = self.value.ceil();
        let int_value = Integer::from_f64(ceiled)
            .ok_or_else(|| PyValueError::new_err("ceiled value out of range"))?;
        Ok(Py::new(py, crate::int::Int { value: int_value })?.into_py(py))
    }

    fn is_integer(&self) -> bool {
        self.value.is_finite() && self.value.fract() == 0.0
    }

    fn as_integer_ratio(&self, py: Python<'_>) -> PyResult<PyObject> {
        if self.value.is_nan() {
            return Err(PyValueError::new_err("cannot convert NaN to integer ratio"));
        }
        if self.value.is_infinite() {
            return Err(PyOverflowError::new_err(
                "cannot convert Infinity to integer ratio",
            ));
        }

        let bits = self.value.to_bits();
        let sign_bit = (bits >> 63) as u64;
        let sign = if sign_bit == 1 { -1 } else { 1 };

        let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
        let mantissa_bits = bits & 0x000f_ffff_ffff_ffff;

        if self.value == 0.0 {
            let numerator = 0;
            let denominator = 1;
            let numerator_int = Py::new(
                py,
                crate::int::Int {
                    value: Integer::from(numerator),
                },
            )?;
            let denominator_int = Py::new(
                py,
                crate::int::Int {
                    value: Integer::from(denominator),
                },
            )?;
            let tuple = PyTuple::new_bound(py, &[numerator_int, denominator_int]);
            return Ok(tuple.into_py(py));
        }

        let (numerator, denominator) = if exponent_bits == 0 {
            // Denormal number
            let numerator = Integer::from(mantissa_bits) * sign;
            let denominator = Integer::from(1) << 1074;
            (numerator, denominator)
        } else {
            // Normal number
            let m = Integer::from(mantissa_bits) + (Integer::from(1) << 52);
            let exponent = exponent_bits - 1023;
            let adjusted_exponent = exponent - 52;

            if adjusted_exponent >= 0 {
                let numerator = m << adjusted_exponent as u32;
                (numerator * sign, Integer::from(1))
            } else {
                let denominator = Integer::from(1) << (-adjusted_exponent) as u32;
                (m * sign, denominator)
            }
        };

        let numerator_int = Py::new(py, crate::int::Int { value: numerator })?;
        let denominator_int = Py::new(py, crate::int::Int { value: denominator })?;
        let tuple = PyTuple::new_bound(py, &[numerator_int, denominator_int]);
        Ok(tuple.into_py(py))
    }

    fn hex(&self) -> String {
        let value = self.value;
        if value.is_nan() {
            "nan".to_string()
        } else if value.is_infinite() {
            if value.is_sign_negative() {
                "-inf".to_string()
            } else {
                "inf".to_string()
            }
        } else if value == 0.0 {
            let sign = if value.is_sign_negative() { "-" } else { "" };
            format!("{}0x0.0p+0", sign)
        } else {
            let bits = value.to_bits();
            let sign_bit = (bits >> 63) & 1;
            let exponent_bits = ((bits >> 52) & 0x7ff) as u16;
            let mantissa_bits = bits & 0x0fffffffffffff;

            let sign_str = if sign_bit != 0 { "-" } else { "" };

            let (mantissa_part, exponent_value) = if exponent_bits == 0 {
                // Subnormal number
                (format!("0.{:013x}", mantissa_bits), -1022)
            } else {
                // Normal number
                (
                    format!("1.{:013x}", mantissa_bits),
                    exponent_bits as i32 - 1023,
                )
            };

            let exponent_str = if exponent_value >= 0 {
                format!("+{}", exponent_value)
            } else {
                exponent_value.to_string()
            };

            format!("{}0x{}p{}", sign_str, mantissa_part, exponent_str)
        }
    }

    #[classmethod]
    fn fromhex(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        let trimmed = s.trim();

        if trimmed.is_empty() {
            return Err(PyValueError::new_err(
                "invalid hexadecimal floating-point string",
            ));
        }

        let mut chars = trimmed.chars().peekable();

        // Parse sign
        let sign = match chars.peek() {
            Some('+') => {
                chars.next();
                1.0
            }
            Some('-') => {
                chars.next();
                -1.0
            }
            _ => 1.0,
        };

        // Check for '0x' or '0X' prefix
        if chars.next() != Some('0') {
            return Err(PyValueError::new_err(
                "invalid hexadecimal floating-point string",
            ));
        }
        match chars.next() {
            Some('x') | Some('X') => (),
            _ => {
                return Err(PyValueError::new_err(
                    "invalid hexadecimal floating-point string",
                ))
            }
        }

        // Split into significand and exponent parts
        let remaining: String = chars.collect();
        let parts: Vec<&str> = remaining.splitn(2, |c| c == 'p' || c == 'P').collect();

        if parts.len() != 2 {
            return Err(PyValueError::new_err(
                "invalid hexadecimal floating-point string",
            ));
        }

        let significand_str = parts[0];
        let exponent_str = parts[1];

        // Parse exponent part
        let exponent: i32 = exponent_str.parse().map_err(|_| {
            PyValueError::new_err("invalid hexadecimal floating-point string (exponent part)")
        })?;

        // Split significand into integer and fractional parts
        let mut split_significand = significand_str.splitn(2, '.');
        let integer_part_str = split_significand.next().unwrap();
        let fractional_part_str = split_significand.next().unwrap_or("");

        // Ensure at least one digit is present in significand
        if integer_part_str.is_empty() && fractional_part_str.is_empty() {
            return Err(PyValueError::new_err(
                "invalid hexadecimal floating-point string (empty significand)",
            ));
        }

        // Parse integer part of significand
        let mut integer_value = 0.0;
        for c in integer_part_str.chars() {
            let digit = c.to_digit(16).ok_or_else(|| {
                PyValueError::new_err("invalid hexadecimal floating-point string (invalid digit)")
            })?;
            integer_value = integer_value * 16.0 + digit as f64;
        }

        // Parse fractional part of significand
        let mut fractional_value = 0.0;
        for (i, c) in fractional_part_str.chars().enumerate() {
            let digit = c.to_digit(16).ok_or_else(|| {
                PyValueError::new_err("invalid hexadecimal floating-point string (invalid digit)")
            })?;
            fractional_value += digit as f64 * 16.0f64.powi(-(i as i32 + 1));
        }

        let total_significand = integer_value + fractional_value;

        // Calculate the final value
        let value = sign * total_significand * 2.0f64.powi(exponent);

        Ok(Self { value })
    }

    fn conjugate(&self) -> Self {
        Self { value: self.value }
    }
}

fn convert_to_float(obj: &Bound<'_, PyAny>) -> PyResult<f64> {
    if let Ok(float_rust) = obj.downcast::<Float>() {
        return Ok(float_rust.borrow().value);
    }

    if let Ok(py_float) = obj.downcast::<PyFloat>() {
        return Ok(py_float.extract::<f64>()?);
    }

    if let Ok(py_int) = obj.downcast::<PyInt>() {
        return Ok(py_int.extract::<i64>()? as f64);
    }

    if let Ok(int_rust) = obj.downcast::<crate::int::Int>() {
        return Ok(int_rust.borrow().value.to_f64());
    }

    if let Ok(py_str) = obj.downcast::<PyString>() {
        let s = py_str.to_str()?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(PyValueError::new_err("Invalid float string"));
        }
        let val: f64 = trimmed
            .parse()
            .map_err(|_| PyValueError::new_err("Invalid float string"))?;
        return Ok(val);
    }

    if let Ok(float_method) = obj.getattr("__float__") {
        let result = float_method.call0()?;
        return convert_to_float(&result);
    }

    if let Ok(index_method) = obj.getattr("__index__") {
        let result = index_method.call0()?;
        if let Ok(int_rust) = result.downcast::<crate::int::Int>() {
            return Ok(int_rust.borrow().value.to_f64());
        } else if let Ok(py_int) = result.downcast::<PyInt>() {
            return Ok(py_int.extract::<i64>()? as f64);
        }
    }

    Err(PyTypeError::new_err(format!(
        "Unsupported type for float conversion: {}",
        obj.get_type().name()?
    )))
}

#[pymodule]
pub fn register_float(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Float>()?;
    Ok(())
}
