use std::ops::Add;

use pyo3::prelude::*;
#[pyclass(name="i32")]
pub struct I32(i32);

#[pymethods]
impl I32 {
    #[new]
    fn new(i: i32) -> Self {
        I32(i)
    }
}

#[pyclass(name="i64")]
pub struct I64(i64);

#[pymethods]
impl I64 {
    #[new]
    fn new(i: i64) -> Self {
        I64(i)
    }
}

