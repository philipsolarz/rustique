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
    // #[pyo3(signature = (val = 0.0))]
    fn new(val: &Bound<'_, PyAny>) -> PyResult<Self> {
        let value = convert_to_float(val)?;
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
        self.value.fract() == 0.0
    }

    #[classmethod]
    fn fromhex(_cls: &Bound<'_, PyType>, s: &str) -> PyResult<Self> {
        let value = f64::from_str(s)
            .map_err(|_| PyValueError::new_err("invalid hexadecimal floating-point string"))?;
        Ok(Self { value })
    }

    // fn hex(&self) -> String {
    //     // format!("{:a}", self.value)
    // }

    fn as_integer_ratio(&self, py: Python<'_>) -> PyResult<PyObject> {
        if self.value.is_nan() {
            return Err(PyValueError::new_err("cannot convert NaN to integer ratio"));
        }
        if self.value.is_infinite() {
            return Err(PyOverflowError::new_err(
                "cannot convert Infinity to integer ratio",
            ));
        }
        let py_float = PyFloat::new(py, self.value);
        let ratio = py_float.call_method0("as_integer_ratio")?;
        Ok(ratio.into_py(py))
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
        let val: f64 = s
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
